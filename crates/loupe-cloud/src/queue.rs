//! Yedekleme kuyruğu ve durum makinesi (RFC-001B §3.5).
//!
//! Kullanıcının "cloud_queue" dediği tablo şemada `sync_queue`'dur (migrasyon
//! 0002); medya başına gerçek durum `sync_status`'tadır. Bir durum geçişi ve
//! onun kuyruk değişikliği her zaman aynı transaction'da yazılır:
//!
//! | Olay | `sync_status.state` | `sync_queue` |
//! |---|---|---|
//! | kuyruğa al | `queued` | `upload` / `pending` |
//! | kira alındı | `uploading` | `running` (+1 deneme) |
//! | yüklendi + doğrulandı | `synced` | satır silinir |
//! | geçici hata | `queued` | `failed`, `run_after` = backoff |
//! | kalıcı hata / 12 deneme | `error` | `dead` |

use loupe_core::MediaId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::backoff::MAX_ATTEMPTS;
use crate::{CloudError, ErrorClass};

/// `Pool::write` içinde `BEGIN IMMEDIATE`; kapanış hata dönerse geri alınır.
pub fn write_tx<T>(
    pool: &loupe_db::Pool,
    f: impl FnOnce(&Connection) -> Result<T, CloudError>,
) -> Result<T, CloudError> {
    pool.write(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx);
        if out.is_ok() {
            tx.commit()?;
        }
        Ok(out)
    })?
}

/// Medyayı hedefe yüklemek üzere kuyruğa alır (yeni medya, değişen içerik ya
/// da kullanıcının "yeniden dene"si). Çalışan iş varsa ona dokunulmaz.
pub fn enqueue_upload(
    conn: &Connection,
    target_id: i64,
    media_id: MediaId,
    priority: u8,
    now_ms: i64,
) -> Result<(), CloudError> {
    let blake3: Vec<u8> = conn.query_row("SELECT blake3 FROM media_files WHERE id = ?1", [media_id.0], |r| r.get(0))?;
    conn.execute(
        "INSERT INTO sync_status (media_id, target_id, state, local_checksum, updated_at)
         VALUES (?1, ?2, 'queued', ?3, ?4)
         ON CONFLICT (media_id, target_id) DO UPDATE SET
           state = 'queued', local_checksum = excluded.local_checksum, conflict_status = 'none',
           last_error_code = NULL, updated_at = excluded.updated_at
         WHERE sync_status.state <> 'uploading'",
        params![media_id.0, target_id, blake3, now_ms],
    )?;
    conn.execute(
        "INSERT INTO sync_queue (target_id, media_id, op, priority, run_after, created_at, updated_at)
         VALUES (?1, ?2, 'upload', ?3, ?4, ?4, ?4)
         ON CONFLICT (target_id, media_id, op) DO UPDATE SET
           state = 'pending', attempts = 0, priority = MIN(sync_queue.priority, excluded.priority),
           run_after = excluded.run_after, last_error_code = NULL, last_error_msg = NULL,
           bytes_done = 0, updated_at = excluded.updated_at
         WHERE sync_queue.state <> 'running'",
        params![target_id, media_id.0, i64::from(priority.min(9)), now_ms],
    )?;
    Ok(())
}

/// Hedef etkinleşti: hedefte `synced` olmayan, çöpte olmayan her medya kuyruğa.
pub fn enqueue_missing(conn: &Connection, target_id: i64, now_ms: i64) -> Result<usize, CloudError> {
    let ids: Vec<i64> = conn
        .prepare(
            "SELECT m.id FROM media_files m
              WHERE m.soft_deleted_at IS NULL
                AND NOT EXISTS (SELECT 1 FROM sync_status s
                                 WHERE s.media_id = m.id AND s.target_id = ?1 AND s.state = 'synced')
              ORDER BY m.is_favorite DESC, m.id",
        )?
        .query_map([target_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in &ids {
        enqueue_upload(conn, target_id, MediaId(*id), 5, now_ms)?;
    }
    Ok(ids.len())
}

/// Alınmış iş.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub queue_id: i64,
    pub target_id: i64,
    pub media_id: MediaId,
    /// Bu deneme dahil.
    pub attempt: u32,
}

/// Sıradaki hazır yükleme işini kiralar. Süresi geçen kiralar (çöken işçi)
/// önce geri alınır.
pub fn claim_next(
    conn: &Connection,
    target_id: i64,
    owner: &str,
    lease_ms: i64,
    now_ms: i64,
) -> Result<Option<Lease>, CloudError> {
    conn.execute(
        "UPDATE sync_queue SET state = 'failed', lease_owner = NULL, lease_until = NULL, updated_at = ?2
          WHERE target_id = ?1 AND state = 'running' AND lease_until < ?2",
        params![target_id, now_ms],
    )?;
    let next: Option<(i64, i64, i64)> = conn
        .query_row(
            "SELECT id, media_id, attempts FROM sync_queue
              WHERE target_id = ?1 AND op = 'upload' AND state IN ('pending','failed') AND run_after <= ?2
              ORDER BY priority, run_after, id LIMIT 1",
            params![target_id, now_ms],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((queue_id, media_id, attempts)) = next else { return Ok(None) };
    conn.execute(
        "UPDATE sync_queue SET state = 'running', lease_owner = ?2, lease_until = ?3, attempts = attempts + 1,
                               updated_at = ?4
          WHERE id = ?1",
        params![queue_id, owner, now_ms + lease_ms, now_ms],
    )?;
    conn.execute(
        "UPDATE sync_status SET state = 'uploading', updated_at = ?3 WHERE media_id = ?1 AND target_id = ?2",
        params![media_id, target_id, now_ms],
    )?;
    Ok(Some(Lease {
        queue_id,
        target_id,
        media_id: MediaId(media_id),
        attempt: u32::try_from(attempts + 1).unwrap_or(u32::MAX),
    }))
}

/// Yüklenmiş ve doğrulanmış nesnenin `sync_status`'a yazılacak alanları.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncedObject {
    pub remote_key: String,
    pub remote_size: u64,
    pub remote_checksum: String,
    /// `sha256` | `md5` | `etag` | `blake3`
    pub remote_checksum_algo: &'static str,
    /// Şifreliyse anahtar parmak izi.
    pub key_fingerprint: Option<String>,
}

pub fn mark_synced(conn: &Connection, lease: &Lease, obj: &SyncedObject, now_ms: i64) -> Result<(), CloudError> {
    conn.execute(
        "UPDATE sync_status SET
           state = 'synced', remote_key = ?3, remote_size = ?4, remote_checksum = ?5, remote_checksum_algo = ?6,
           encryption = ?7, key_fingerprint = ?8, uploaded_at = ?9, last_verified_at = ?9,
           conflict_status = 'none', last_error_code = NULL, updated_at = ?9
         WHERE media_id = ?1 AND target_id = ?2",
        params![
            lease.media_id.0,
            lease.target_id,
            obj.remote_key,
            i64::try_from(obj.remote_size).unwrap_or(i64::MAX),
            obj.remote_checksum,
            obj.remote_checksum_algo,
            if obj.key_fingerprint.is_some() { "age" } else { "none" },
            obj.key_fingerprint,
            now_ms
        ],
    )?;
    conn.execute("DELETE FROM sync_queue WHERE id = ?1", [lease.queue_id])?;
    Ok(())
}

/// Hata sonrası: yeniden denenebilir ve deneme hakkı varsa `retry_at`'te
/// tekrar; değilse `dead` + `error`. Dönen değer yeniden deneme zamanıdır.
pub fn mark_failed(
    conn: &Connection,
    lease: &Lease,
    class: ErrorClass,
    message: &str,
    retry_at: i64,
    now_ms: i64,
) -> Result<Option<i64>, CloudError> {
    let retry = class.retryable() && lease.attempt < MAX_ATTEMPTS;
    let msg: String = message.chars().take(500).collect();
    if retry {
        conn.execute(
            "UPDATE sync_queue SET state = 'failed', run_after = ?2, lease_owner = NULL, lease_until = NULL,
                                   last_error_code = ?3, last_error_msg = ?4, updated_at = ?5
              WHERE id = ?1",
            params![lease.queue_id, retry_at, class.code(), msg, now_ms],
        )?;
    } else {
        conn.execute(
            "UPDATE sync_queue SET state = 'dead', lease_owner = NULL, lease_until = NULL,
                                   last_error_code = ?2, last_error_msg = ?3, updated_at = ?4
              WHERE id = ?1",
            params![lease.queue_id, class.code(), msg, now_ms],
        )?;
    }
    conn.execute(
        "UPDATE sync_status SET state = ?3, last_error_code = ?4, updated_at = ?5 WHERE media_id = ?1 AND target_id = ?2",
        params![lease.media_id.0, lease.target_id, if retry { "queued" } else { "error" }, class.code(), now_ms],
    )?;
    Ok(retry.then_some(retry_at))
}

/// Arayüz ve testler için `sync_status` satırı.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStatusRow {
    pub state: String,
    pub local_checksum: Vec<u8>,
    pub remote_key: Option<String>,
    pub remote_size: Option<i64>,
    pub remote_checksum: Option<String>,
    pub remote_checksum_algo: Option<String>,
    pub encryption: String,
    pub key_fingerprint: Option<String>,
    pub uploaded_at: Option<i64>,
    pub last_verified_at: Option<i64>,
    pub last_error_code: Option<String>,
}

pub fn status(conn: &Connection, media_id: MediaId, target_id: i64) -> Result<Option<SyncStatusRow>, CloudError> {
    Ok(conn
        .query_row(
            "SELECT state, local_checksum, remote_key, remote_size, remote_checksum, remote_checksum_algo, encryption,
                    key_fingerprint, uploaded_at, last_verified_at, last_error_code
               FROM sync_status WHERE media_id = ?1 AND target_id = ?2",
            params![media_id.0, target_id],
            |r| {
                Ok(SyncStatusRow {
                    state: r.get(0)?,
                    local_checksum: r.get(1)?,
                    remote_key: r.get(2)?,
                    remote_size: r.get(3)?,
                    remote_checksum: r.get(4)?,
                    remote_checksum_algo: r.get(5)?,
                    encryption: r.get(6)?,
                    key_fingerprint: r.get(7)?,
                    uploaded_at: r.get(8)?,
                    last_verified_at: r.get(9)?,
                    last_error_code: r.get(10)?,
                })
            },
        )
        .optional()?)
}

/// Geri okuma doğrulamasından sonra.
pub fn mark_verified(conn: &Connection, media_id: MediaId, target_id: i64, now_ms: i64) -> Result<(), CloudError> {
    conn.execute(
        "UPDATE sync_status SET last_verified_at = ?3, updated_at = ?3
          WHERE media_id = ?1 AND target_id = ?2 AND state = 'synced'",
        params![media_id.0, target_id, now_ms],
    )?;
    Ok(())
}
