use loupe_core::{LibraryId, now_ms};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::DbError;
use crate::convert::{bool_col, enum_col, to_i64};
use crate::models::{Library, LibraryPresence, NewLibrary};

const COLUMNS: &str = "id, uuid, name, kind, volume_uuid, volume_label, network_source, fs_type, rel_root,
    last_mount_path, supports_hardlink, supports_noreplace, supports_inotify, is_managed, layout_template,
    default_import_mode, is_primary, state, state_reason, total_bytes, free_bytes, last_seen_at,
    created_at, updated_at";

fn from_row(r: &Row<'_>) -> rusqlite::Result<Library> {
    Ok(Library {
        id: LibraryId(r.get(0)?),
        uuid: r.get(1)?,
        name: r.get(2)?,
        kind: enum_col(r, 3)?,
        volume_uuid: r.get(4)?,
        volume_label: r.get(5)?,
        network_source: r.get(6)?,
        fs_type: r.get(7)?,
        rel_root: r.get(8)?,
        last_mount_path: r.get(9)?,
        supports_hardlink: bool_col(r, 10)?,
        supports_noreplace: bool_col(r, 11)?,
        supports_inotify: bool_col(r, 12)?,
        is_managed: bool_col(r, 13)?,
        layout_template: r.get(14)?,
        default_import_mode: enum_col(r, 15)?,
        is_primary: bool_col(r, 16)?,
        state: enum_col(r, 17)?,
        state_reason: r.get(18)?,
        total_bytes: r.get(19)?,
        free_bytes: r.get(20)?,
        last_seen_at: r.get(21)?,
        created_at: r.get(22)?,
        updated_at: r.get(23)?,
    })
}

pub fn insert(conn: &Connection, lib: &NewLibrary) -> Result<LibraryId, DbError> {
    let now = now_ms();
    let id = conn.query_row(
        "INSERT INTO libraries (uuid, name, kind, volume_uuid, volume_label, network_source, fs_type, rel_root,
             last_mount_path, supports_hardlink, supports_noreplace, supports_inotify, is_managed,
             default_import_mode, is_primary, last_seen_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16, ?16)
         RETURNING id",
        params![
            lib.uuid,
            lib.name,
            lib.kind.as_str(),
            lib.volume_uuid,
            lib.volume_label,
            lib.network_source,
            lib.fs_type,
            lib.rel_root,
            lib.last_mount_path,
            lib.supports_hardlink,
            lib.supports_noreplace,
            lib.supports_inotify,
            lib.is_managed,
            lib.default_import_mode.as_str(),
            lib.is_primary,
            now,
        ],
        |r| r.get(0),
    )?;
    Ok(LibraryId(id))
}

pub fn get(conn: &Connection, id: LibraryId) -> Result<Option<Library>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM libraries WHERE id = ?1");
    Ok(conn.prepare_cached(&sql)?.query_row([id.0], from_row).optional()?)
}

pub fn require(conn: &Connection, id: LibraryId) -> Result<Library, DbError> {
    get(conn, id)?.ok_or(DbError::NotFound { entity: "kütüphane", id: id.0 })
}

pub fn find_by_uuid(conn: &Connection, uuid: &[u8; 16]) -> Result<Option<Library>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM libraries WHERE uuid = ?1");
    Ok(conn.prepare_cached(&sql)?.query_row([uuid], from_row).optional()?)
}

/// Bir diskteki kütüphaneler: disk yeniden takıldığında hangilerinin geri geldiğini bulmak için.
pub fn find_by_volume(conn: &Connection, volume_uuid: &str) -> Result<Vec<Library>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM libraries WHERE volume_uuid = ?1 ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([volume_uuid], from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn list(conn: &Connection) -> Result<Vec<Library>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM libraries ORDER BY is_primary DESC, name");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn rename(conn: &Connection, id: LibraryId, name: &str) -> Result<(), DbError> {
    let n =
        conn.execute("UPDATE libraries SET name = ?2, updated_at = ?3 WHERE id = ?1", params![id.0, name, now_ms()])?;
    if n == 0 { Err(DbError::NotFound { entity: "kütüphane", id: id.0 }) } else { Ok(()) }
}

/// Disk bağlandı / çıkarıldı / salt okunur oldu. Çevrimiçiyse `last_seen_at` güncellenir.
pub fn update_presence(conn: &Connection, id: LibraryId, p: &LibraryPresence) -> Result<(), DbError> {
    let now = now_ms();
    let total = p.total_bytes.map(|v| to_i64("total_bytes", v)).transpose()?;
    let free = p.free_bytes.map(|v| to_i64("free_bytes", v)).transpose()?;
    let n = conn.execute(
        "UPDATE libraries
            SET state = ?2, state_reason = ?3,
                last_mount_path = COALESCE(?4, last_mount_path),
                total_bytes = COALESCE(?5, total_bytes), free_bytes = COALESCE(?6, free_bytes),
                last_seen_at = CASE WHEN ?2 = 'online' THEN ?7 ELSE last_seen_at END,
                updated_at = ?7
          WHERE id = ?1",
        params![id.0, p.state.as_str(), p.state_reason, p.last_mount_path, total, free, now],
    )?;
    if n == 0 { Err(DbError::NotFound { entity: "kütüphane", id: id.0 }) } else { Ok(()) }
}

pub fn update_capabilities(
    conn: &Connection,
    id: LibraryId,
    hardlink: bool,
    noreplace: bool,
    inotify: bool,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE libraries SET supports_hardlink = ?2, supports_noreplace = ?3, supports_inotify = ?4, updated_at = ?5
          WHERE id = ?1",
        params![id.0, hardlink, noreplace, inotify, now_ms()],
    )?;
    Ok(())
}

/// Kütüphanenin en çok `limit` medya kaydını kataloğdan siler (diskteki
/// dosyalara dokunulmaz). Yüzler, vektörler, küçük resim satırları ve işler
/// şemayla gider; bulut durum satırları (RESTRICT) önce silinir. Silinen
/// medya sayısını döner; 0 ise kütüphane boşalmıştır.
pub fn remove_media_batch(conn: &mut Connection, id: LibraryId, limit: u32) -> Result<usize, DbError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let batch = "SELECT id FROM media_files WHERE library_id = ?1 LIMIT ?2";
    tx.execute(&format!("DELETE FROM sync_status WHERE media_id IN ({batch})"), params![id.0, limit])?;
    let n = tx.execute(&format!("DELETE FROM media_files WHERE id IN ({batch})"), params![id.0, limit])?;
    tx.commit()?;
    Ok(n)
}

/// Medyası boşaltılmış kütüphaneyi siler; boş kalan isimsiz yüz kümelerini
/// temizler ve birincil kütüphane gittiyse yönetilen ilk kütüphaneyi birincil yapar.
pub fn remove_emptied(conn: &mut Connection, id: LibraryId) -> Result<bool, DbError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let removed = tx.execute("DELETE FROM libraries WHERE id = ?1", [id.0])? > 0;
    tx.execute("DELETE FROM face_clusters WHERE face_count <= 0 AND person_id IS NULL AND is_confirmed = 0", [])?;
    tx.execute(
        "UPDATE libraries SET is_primary = 1, updated_at = ?1
          WHERE id = (SELECT id FROM libraries WHERE is_managed = 1 ORDER BY id LIMIT 1)
            AND NOT EXISTS (SELECT 1 FROM libraries WHERE is_primary = 1)",
        [now_ms()],
    )?;
    tx.commit()?;
    Ok(removed)
}

/// Kütüphaneyi siler. İçinde medya kaydı varsa şema (ON DELETE RESTRICT) reddeder.
pub fn delete(conn: &Connection, id: LibraryId) -> Result<bool, DbError> {
    Ok(conn.execute("DELETE FROM libraries WHERE id = ?1", [id.0])? > 0)
}
