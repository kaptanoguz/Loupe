//! `devices`, `device_assets` ve `lan_transfers` (migrasyon 0002) üzerindeki
//! işlemler. Kullanıcının "paired_devices" dediği tablo şemada `devices`'tır.

use loupe_core::{Blake3Hash, LibraryId, MediaId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::conflict::{ConflictPolicy, UploadKind};
use crate::{IssuedClientCert, LanError};

/// `Pool::write` içinde `BEGIN IMMEDIATE`; kapanış hata dönerse geri alınır.
pub fn write_tx<T>(pool: &loupe_db::Pool, f: impl FnOnce(&Connection) -> Result<T, LanError>) -> Result<T, LanError> {
    pool.write(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx);
        if out.is_ok() {
            tx.commit()?;
        }
        Ok(out)
    })?
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: i64,
    pub name: String,
    pub platform: String,
    pub conflict_policy: ConflictPolicy,
    pub target_library_id: Option<LibraryId>,
    pub revoked: bool,
}

pub fn insert_device(
    conn: &Connection,
    name: &str,
    platform: &str,
    cert: &IssuedClientCert,
    library: LibraryId,
    now_ms: i64,
) -> Result<i64, LanError> {
    conn.execute(
        "INSERT INTO devices (name, platform, cert_sha256, spki_sha256, cert_serial, cert_not_after,
                              target_library_id, paired_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            name,
            platform,
            cert.cert_sha256.as_slice(),
            cert.spki_sha256.as_slice(),
            cert.serial,
            cert.not_after_ms,
            library.0,
            now_ms
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn device(conn: &Connection, id: i64) -> Result<Option<Device>, LanError> {
    let row = conn
        .query_row(
            "SELECT id, name, platform, conflict_policy, target_library_id, revoked_at FROM devices WHERE id = ?1",
            [id],
            |r| {
                let device = Device {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    platform: r.get(2)?,
                    conflict_policy: ConflictPolicy::AskUser,
                    target_library_id: r.get::<_, Option<i64>>(4)?.map(LibraryId),
                    revoked: r.get::<_, Option<i64>>(5)?.is_some(),
                };
                Ok((device, r.get::<_, String>(3)?))
            },
        )
        .optional()?;
    row.map(|(mut d, policy)| {
        d.conflict_policy = ConflictPolicy::from_db(&policy)?;
        Ok(d)
    })
    .transpose()
}

/// Etkin cihazların sertifika parmak izleri (açılışta [`crate::DeviceRegistry`]'ye).
pub fn active_device_certs(conn: &Connection, now_ms: i64) -> Result<Vec<([u8; 32], i64)>, LanError> {
    let mut stmt = conn
        .prepare("SELECT cert_sha256, id FROM devices WHERE revoked_at IS NULL AND cert_not_after > ?1 ORDER BY id")?;
    let rows = stmt.query_map([now_ms], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (sha, id) = row?;
        if let Ok(a) = <[u8; 32]>::try_from(sha.as_slice()) {
            out.push((a, id));
        }
    }
    Ok(out)
}

pub fn set_conflict_policy(conn: &Connection, device_id: i64, policy: ConflictPolicy) -> Result<(), LanError> {
    conn.execute("UPDATE devices SET conflict_policy = ?2 WHERE id = ?1", params![device_id, policy.as_db()])?;
    Ok(())
}

pub fn revoke_device(conn: &Connection, device_id: i64, reason: &str, now_ms: i64) -> Result<(), LanError> {
    conn.execute(
        "UPDATE devices SET revoked_at = ?2, revoke_reason = ?3 WHERE id = ?1 AND revoked_at IS NULL",
        params![device_id, now_ms, reason],
    )?;
    Ok(())
}

pub fn touch_device(conn: &Connection, device_id: i64, addr: &str, now_ms: i64) -> Result<(), LanError> {
    conn.execute(
        "UPDATE devices SET last_seen_at = ?2, last_address = ?3 WHERE id = ?1",
        params![device_id, now_ms, addr],
    )?;
    Ok(())
}

/// `device_assets` satırını yazar ya da günceller.
pub fn upsert_device_asset(
    conn: &Connection,
    device_id: i64,
    device_asset_id: &str,
    blake3: &Blake3Hash,
    media_id: Option<MediaId>,
    client_modified_at: i64,
    now_ms: i64,
) -> Result<(), LanError> {
    conn.execute(
        "INSERT INTO device_assets (device_id, client_asset_id, blake3, media_id, client_modified_at, synced_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (device_id, client_asset_id) DO UPDATE SET
           blake3 = excluded.blake3,
           media_id = COALESCE(excluded.media_id, device_assets.media_id),
           client_modified_at = excluded.client_modified_at,
           synced_at = excluded.synced_at",
        params![
            device_id,
            device_asset_id,
            blake3.as_bytes().as_slice(),
            media_id.map(|m| m.0),
            client_modified_at,
            now_ms
        ],
    )?;
    Ok(())
}

/// Yükleme oturumu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub upload_id: String,
    pub device_id: i64,
    pub library_id: LibraryId,
    pub device_asset_id: String,
    pub filename: String,
    pub expected_blake3: Blake3Hash,
    pub expected_size: u64,
    pub offset: u64,
    /// `<upload_id>/<dosya adı>`, LAN staging dizinine göre.
    pub staging_name: String,
    pub client_taken_at: Option<i64>,
    pub client_modified_at: i64,
    pub state: String,
    pub kind: UploadKind,
    pub result_media_id: Option<MediaId>,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewTransfer<'a> {
    pub upload_id: &'a str,
    pub device_id: i64,
    pub library_id: LibraryId,
    pub device_asset_id: &'a str,
    pub filename: &'a str,
    pub expected_blake3: &'a Blake3Hash,
    pub expected_size: u64,
    pub staging_name: &'a str,
    pub client_taken_at: Option<i64>,
    pub client_modified_at: i64,
    pub kind: UploadKind,
    pub now_ms: i64,
    pub expires_at: i64,
}

fn to_i64(v: u64) -> Result<i64, LanError> {
    i64::try_from(v).map_err(|_| LanError::TooLarge)
}

pub fn insert_transfer(conn: &Connection, t: &NewTransfer<'_>) -> Result<(), LanError> {
    conn.execute(
        "INSERT INTO lan_transfers (upload_id, device_id, library_id, client_asset_id, filename, expected_blake3,
                                    expected_size, staging_name, client_taken_at, client_modified_at, resolution,
                                    replaces_media_id, created_at, updated_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
        params![
            t.upload_id,
            t.device_id,
            t.library_id.0,
            t.device_asset_id,
            t.filename,
            t.expected_blake3.as_bytes().as_slice(),
            to_i64(t.expected_size)?,
            t.staging_name,
            t.client_taken_at,
            t.client_modified_at,
            t.kind.resolution_db(),
            t.kind.previous().map(|m| m.0),
            t.now_ms,
            t.expires_at,
        ],
    )?;
    Ok(())
}

const TRANSFER_COLS: &str = "upload_id, device_id, library_id, client_asset_id, filename, expected_blake3,
    expected_size, offset_bytes, staging_name, client_taken_at, client_modified_at, state, resolution,
    replaces_media_id, result_media_id, expires_at";

fn transfer_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Transfer> {
    let blake: Vec<u8> = r.get(5)?;
    let blake = Blake3Hash::from_slice(&blake)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Blob, Box::new(e)))?;
    let size: i64 = r.get(6)?;
    let offset: i64 = r.get(7)?;
    let resolution: Option<String> = r.get(12)?;
    let replaces: Option<i64> = r.get(13)?;
    let kind = match (resolution.as_deref(), replaces) {
        (Some("keep_both"), Some(p)) => UploadKind::KeepBoth { previous: MediaId(p) },
        (Some("keep_newest"), Some(p)) => UploadKind::ReplaceWithNewer { previous: MediaId(p) },
        _ => UploadKind::New,
    };
    Ok(Transfer {
        upload_id: r.get(0)?,
        device_id: r.get(1)?,
        library_id: LibraryId(r.get(2)?),
        device_asset_id: r.get(3)?,
        filename: r.get(4)?,
        expected_blake3: blake,
        expected_size: u64::try_from(size).unwrap_or(0),
        offset: u64::try_from(offset).unwrap_or(0),
        staging_name: r.get(8)?,
        client_taken_at: r.get(9)?,
        client_modified_at: r.get(10)?,
        state: r.get(11)?,
        kind,
        result_media_id: r.get::<_, Option<i64>>(14)?.map(MediaId),
        expires_at: r.get(15)?,
    })
}

pub fn transfer(conn: &Connection, upload_id: &str) -> Result<Option<Transfer>, LanError> {
    Ok(conn
        .query_row(
            &format!("SELECT {TRANSFER_COLS} FROM lan_transfers WHERE upload_id = ?1"),
            [upload_id],
            transfer_row,
        )
        .optional()?)
}

/// Bu cihaz + varlık için alıcı/doğrulayıcı durumdaki oturum.
pub fn active_transfer(conn: &Connection, device_id: i64, device_asset_id: &str) -> Result<Option<Transfer>, LanError> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {TRANSFER_COLS} FROM lan_transfers
                  WHERE device_id = ?1 AND client_asset_id = ?2 AND state IN ('receiving','verifying')"
            ),
            params![device_id, device_asset_id],
            transfer_row,
        )
        .optional()?)
}

pub fn active_transfer_count(conn: &Connection, device_id: i64) -> Result<usize, LanError> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM lan_transfers WHERE device_id = ?1 AND state IN ('receiving','verifying')",
        [device_id],
        |r| r.get(0),
    )?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// Yalnızca fsync edilmiş baytlar yazılır (RFC-001B §2.6).
pub fn set_offset(conn: &Connection, upload_id: &str, offset: u64, now_ms: i64) -> Result<(), LanError> {
    conn.execute(
        "UPDATE lan_transfers SET offset_bytes = ?2, updated_at = ?3 WHERE upload_id = ?1",
        params![upload_id, to_i64(offset)?, now_ms],
    )?;
    Ok(())
}

pub fn set_state(
    conn: &Connection,
    upload_id: &str,
    state: &str,
    error: Option<&str>,
    now_ms: i64,
) -> Result<(), LanError> {
    conn.execute(
        "UPDATE lan_transfers SET state = ?2, error_code = ?3, updated_at = ?4 WHERE upload_id = ?1",
        params![upload_id, state, error, now_ms],
    )?;
    Ok(())
}

/// Süresi dolan oturumlar `expired` olur; staging yolları döner (silinmek üzere).
pub fn expire_transfers(conn: &Connection, now_ms: i64) -> Result<Vec<String>, LanError> {
    let names: Vec<String> = conn
        .prepare(
            "SELECT staging_name FROM lan_transfers WHERE state IN ('receiving','verifying') AND expires_at <= ?1",
        )?
        .query_map([now_ms], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    conn.execute(
        "UPDATE lan_transfers SET state = 'expired', updated_at = ?1
          WHERE state IN ('receiving','verifying') AND expires_at <= ?1",
        [now_ms],
    )?;
    Ok(names)
}

/// Aktarım bitti: dosya katalogda (yeni ya da zaten var olan medya).
/// `inserted` yeni bir medya satırıysa çakışma çözümünün yan etkileri uygulanır:
/// - "yenisini tut": eski medya çöp kutusuna (`soft_deleted_at`), geçmiş
///   `lan_transfers.replaces_media_id`'de kalır;
/// - "ikisini de tut": iki medya aynı `near_duplicate_groups` grubuna alınır.
///
/// Medya satırı, `device_assets` ve oturum aynı transaction'da yazılır.
pub fn complete_transfer(
    conn: &Connection,
    t: &Transfer,
    media_id: MediaId,
    inserted: bool,
    now_ms: i64,
) -> Result<(), LanError> {
    if inserted {
        match t.kind {
            UploadKind::New => {}
            UploadKind::ReplaceWithNewer { previous } => {
                conn.execute(
                    "UPDATE media_files SET soft_deleted_at = ?2, updated_at = ?2
                      WHERE id = ?1 AND soft_deleted_at IS NULL",
                    params![previous.0, now_ms],
                )?;
            }
            UploadKind::KeepBoth { previous } => {
                let group: Option<i64> = conn
                    .query_row("SELECT near_dup_group_id FROM media_files WHERE id = ?1", [previous.0], |r| r.get(0))
                    .optional()?
                    .flatten();
                let group = match group {
                    Some(g) => g,
                    None => {
                        conn.execute("INSERT INTO near_duplicate_groups (created_at) VALUES (?1)", [now_ms])?;
                        conn.last_insert_rowid()
                    }
                };
                conn.execute(
                    "UPDATE media_files SET near_dup_group_id = ?1 WHERE id IN (?2, ?3)",
                    params![group, previous.0, media_id.0],
                )?;
                conn.execute(
                    "UPDATE near_duplicate_groups
                        SET member_count = (SELECT count(*) FROM media_files WHERE near_dup_group_id = ?1)
                      WHERE id = ?1",
                    [group],
                )?;
            }
        }
    }
    upsert_device_asset(
        conn,
        t.device_id,
        &t.device_asset_id,
        &t.expected_blake3,
        Some(media_id),
        t.client_modified_at,
        now_ms,
    )?;
    conn.execute(
        "UPDATE lan_transfers SET state = 'done', offset_bytes = expected_size, result_media_id = ?2, updated_at = ?3
          WHERE upload_id = ?1",
        params![t.upload_id, media_id.0, now_ms],
    )?;
    Ok(())
}
