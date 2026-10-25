use loupe_core::{
    Blake3Hash, ImportItemId, ImportItemState, ImportSessionId, LibraryId, MediaId, SessionState, now_ms,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::DbError;
use crate::convert::{enum_col, from_i64, to_i64};
use crate::models::{ImportItem, ImportSession, NewImportSession};

const SESSION_COLUMNS: &str = "id, library_id, mode, source_root, source_volume, state, total_files, total_bytes,
    done_files, done_bytes, skipped_dup, failed_files, started_at, finished_at";

const ITEM_COLUMNS: &str = "id, session_id, source_path, size_bytes, source_mtime_ns, blake3, state, media_id, error_code, attempts, updated_at";

fn session_from_row(r: &Row<'_>) -> rusqlite::Result<ImportSession> {
    Ok(ImportSession {
        id: ImportSessionId(r.get(0)?),
        library_id: LibraryId(r.get(1)?),
        mode: enum_col(r, 2)?,
        source_root: r.get(3)?,
        source_volume: r.get(4)?,
        state: enum_col(r, 5)?,
        total_files: from_i64(r.get(6)?),
        total_bytes: from_i64(r.get(7)?),
        done_files: from_i64(r.get(8)?),
        done_bytes: from_i64(r.get(9)?),
        skipped_dup: from_i64(r.get(10)?),
        failed_files: from_i64(r.get(11)?),
        started_at: r.get(12)?,
        finished_at: r.get(13)?,
    })
}

fn item_from_row(r: &Row<'_>) -> rusqlite::Result<ImportItem> {
    let blake3 = match r.get::<_, Option<Vec<u8>>>(5)? {
        Some(b) => Some(
            Blake3Hash::from_slice(&b)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Blob, Box::new(e)))?,
        ),
        None => None,
    };
    Ok(ImportItem {
        id: ImportItemId(r.get(0)?),
        session_id: ImportSessionId(r.get(1)?),
        source_path: r.get(2)?,
        size_bytes: from_i64(r.get(3)?),
        source_mtime_ns: r.get(4)?,
        blake3,
        state: enum_col(r, 6)?,
        media_id: r.get::<_, Option<i64>>(7)?.map(MediaId),
        error_code: r.get(8)?,
        attempts: u32::try_from(r.get::<_, i64>(9)?).unwrap_or(u32::MAX),
        updated_at: r.get(10)?,
    })
}

pub fn create_session(conn: &Connection, s: &NewImportSession) -> Result<ImportSessionId, DbError> {
    let id = conn.query_row(
        "INSERT INTO import_sessions (library_id, mode, source_root, source_volume, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
        params![s.library_id.0, s.mode.as_str(), s.source_root, s.source_volume, now_ms()],
        |r| r.get(0),
    )?;
    Ok(ImportSessionId(id))
}

pub fn get_session(conn: &Connection, id: ImportSessionId) -> Result<Option<ImportSession>, DbError> {
    let sql = format!("SELECT {SESSION_COLUMNS} FROM import_sessions WHERE id = ?1");
    Ok(conn.prepare_cached(&sql)?.query_row([id.0], session_from_row).optional()?)
}

/// Yarım kalmış oturumlar (açılışta devam ettirmek için).
pub fn active_sessions(conn: &Connection) -> Result<Vec<ImportSession>, DbError> {
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM import_sessions WHERE state IN ('scanning','running','paused') ORDER BY id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], session_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Durum geçişi. Son durumlarda (`done`, `failed`, `cancelled`) `finished_at` yazılır.
pub fn set_session_state(conn: &Connection, id: ImportSessionId, state: SessionState) -> Result<(), DbError> {
    let terminal = matches!(state, SessionState::Done | SessionState::Failed | SessionState::Cancelled);
    let n = conn.execute(
        "UPDATE import_sessions
            SET state = ?2, finished_at = CASE WHEN ?3 THEN max(?4, started_at) ELSE NULL END
          WHERE id = ?1",
        params![id.0, state.as_str(), terminal, now_ms()],
    )?;
    if n == 0 { Err(DbError::NotFound { entity: "import oturumu", id: id.0 }) } else { Ok(()) }
}

pub fn set_session_totals(conn: &Connection, id: ImportSessionId, files: u64, bytes: u64) -> Result<(), DbError> {
    conn.execute(
        "UPDATE import_sessions SET total_files = ?2, total_bytes = ?3 WHERE id = ?1",
        params![id.0, to_i64("total_files", files)?, to_i64("total_bytes", bytes)?],
    )?;
    Ok(())
}

/// Sayaçlara artış ekler (mutlak değer yazmaz; eşzamanlı işçiler çakışmaz).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub done_files: u64,
    pub done_bytes: u64,
    pub skipped_dup: u64,
    pub failed_files: u64,
}

pub fn add_progress(conn: &Connection, id: ImportSessionId, p: Progress) -> Result<(), DbError> {
    conn.prepare_cached(
        "UPDATE import_sessions
            SET done_files = done_files + ?2, done_bytes = done_bytes + ?3,
                skipped_dup = skipped_dup + ?4, failed_files = failed_files + ?5
          WHERE id = ?1",
    )?
    .execute(params![
        id.0,
        to_i64("done_files", p.done_files)?,
        to_i64("done_bytes", p.done_bytes)?,
        to_i64("skipped_dup", p.skipped_dup)?,
        to_i64("failed_files", p.failed_files)?,
    ])?;
    Ok(())
}

pub fn get_item(conn: &Connection, id: ImportItemId) -> Result<Option<ImportItem>, DbError> {
    let sql = format!("SELECT {ITEM_COLUMNS} FROM import_items WHERE id = ?1");
    Ok(conn.prepare_cached(&sql)?.query_row([id.0], item_from_row).optional()?)
}

/// Öğe durum geçişi. `failed` her seferinde deneme sayısını artırır.
pub fn set_item_state(
    conn: &Connection,
    id: ImportItemId,
    state: ImportItemState,
    blake3: Option<&Blake3Hash>,
    media_id: Option<MediaId>,
    error_code: Option<&str>,
) -> Result<(), DbError> {
    let n = conn
        .prepare_cached(
            "UPDATE import_items
                SET state = ?2,
                    blake3 = COALESCE(?3, blake3),
                    media_id = COALESCE(?4, media_id),
                    error_code = ?5,
                    attempts = attempts + (?2 = 'failed'),
                    updated_at = ?6
              WHERE id = ?1",
        )?
        .execute(params![
            id.0,
            state.as_str(),
            blake3.map(Blake3Hash::as_bytes),
            media_id.map(|m| m.0),
            error_code,
            now_ms()
        ])?;
    if n == 0 { Err(DbError::NotFound { entity: "import öğesi", id: id.0 }) } else { Ok(()) }
}

/// Çökme sonrası devam: henüz bitmemiş öğeler (kısmi indeks `import_items_resume`).
pub fn resumable_items(conn: &Connection, session: ImportSessionId) -> Result<Vec<ImportItem>, DbError> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM import_items
          WHERE session_id = ?1 AND state NOT IN ('committed','source_removed','skipped_duplicate')
          ORDER BY id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([session.0], item_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn count_items(conn: &Connection, session: ImportSessionId, state: ImportItemState) -> Result<u64, DbError> {
    let n: i64 = conn
        .prepare_cached("SELECT count(*) FROM import_items WHERE session_id = ?1 AND state = ?2")?
        .query_row(params![session.0, state.as_str()], |r| r.get(0))?;
    Ok(from_i64(n))
}
