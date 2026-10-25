//! Verify Library kayıtları: çalıştırma, bulgu ve sayaçlar.

use loupe_core::{
    Blake3Hash, LibraryId, MediaId, RelPath, VerifyEntry, VerifyOutcome, VerifyRunId, VerifyState, VerifyTrigger,
    now_ms,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::DbError;
use crate::convert::{enum_col, from_i64, hash_col, rel_path_col, to_i64};
use crate::models::{VerifyFinding, VerifyRun};
use crate::repo::media;

const RUN_COLUMNS: &str = "id, library_id, trigger, state, cursor_media_id, total, checked, ok, missing, mismatched,
    unreadable, bytes_read, started_at, finished_at";

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<VerifyRun> {
    Ok(VerifyRun {
        id: VerifyRunId(r.get(0)?),
        library_id: LibraryId(r.get(1)?),
        trigger: enum_col(r, 2)?,
        state: r.get(3)?,
        cursor_media_id: r.get::<_, Option<i64>>(4)?.map(MediaId),
        total: from_i64(r.get(5)?),
        checked: from_i64(r.get(6)?),
        ok: from_i64(r.get(7)?),
        missing: from_i64(r.get(8)?),
        mismatched: from_i64(r.get(9)?),
        unreadable: from_i64(r.get(10)?),
        bytes_read: from_i64(r.get(11)?),
        started_at: r.get(12)?,
        finished_at: r.get(13)?,
    })
}

/// Yeni çalıştırma başlatır. Kütüphane başına tek etkin çalıştırma vardır
/// (`verify_runs_one_active`); varsa onu döndürür, böylece devam edilir.
pub fn start_run(conn: &Connection, library: LibraryId, trigger: VerifyTrigger) -> Result<VerifyRunId, DbError> {
    let active: Option<i64> = conn
        .query_row(
            "SELECT id FROM verify_runs WHERE library_id = ?1 AND state IN ('running','paused')",
            [library.0],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = active {
        conn.execute("UPDATE verify_runs SET state = 'running' WHERE id = ?1", [id])?;
        return Ok(VerifyRunId(id));
    }
    let id = conn.query_row(
        "INSERT INTO verify_runs (library_id, trigger, total, started_at)
         VALUES (?1, ?2, (SELECT count(*) FROM media_files WHERE library_id = ?1 AND soft_deleted_at IS NULL), ?3)
         RETURNING id",
        params![library.0, trigger.as_str(), now_ms()],
        |r| r.get(0),
    )?;
    Ok(VerifyRunId(id))
}

pub fn get_run(conn: &Connection, id: VerifyRunId) -> Result<Option<VerifyRun>, DbError> {
    let sql = format!("SELECT {RUN_COLUMNS} FROM verify_runs WHERE id = ?1");
    Ok(conn.prepare_cached(&sql)?.query_row([id.0], run_from_row).optional()?)
}

/// Doğrulanacak sonraki kayıtlar (id sırası; `cursor_media_id` ile devam).
pub fn entries_after(
    conn: &Connection,
    library: LibraryId,
    after: Option<MediaId>,
    limit: usize,
) -> Result<Vec<VerifyEntry>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, rel_path, blake3, size_bytes FROM media_files
          WHERE library_id = ?1 AND id > ?2 AND soft_deleted_at IS NULL
          ORDER BY id LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![library.0, after.map_or(0, |m| m.0), i64::try_from(limit).unwrap_or(i64::MAX)], |r| {
            Ok(VerifyEntry {
                media_id: MediaId(r.get(0)?),
                rel_path: rel_path_col(r, 1)?,
                blake3: hash_col(r, 2)?,
                size_bytes: from_i64(r.get(3)?),
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Tek dosyanın sonucunu yazar: medya durumu, varsa bulgu, çalıştırma sayaçları.
/// Çağıran tarafından tek transaction içinde çalıştırılmalıdır.
pub fn record(
    conn: &Connection,
    run: VerifyRunId,
    entry: &VerifyEntry,
    outcome: &VerifyOutcome,
    bytes_read: u64,
    quarantine: Option<&RelPath>,
) -> Result<(), DbError> {
    let now = now_ms();
    let (state, actual): (VerifyState, Option<&Blake3Hash>) = match outcome {
        VerifyOutcome::Ok => (VerifyState::Ok, None),
        VerifyOutcome::Missing => (VerifyState::Missing, None),
        VerifyOutcome::Mismatch { actual } => (VerifyState::Mismatch, Some(actual)),
        VerifyOutcome::Unreadable { .. } => (VerifyState::Unreadable, None),
    };
    media::set_verify_state(conn, entry.media_id, state)?;
    if outcome.is_problem() {
        conn.prepare_cached(
            "INSERT INTO verify_findings (run_id, media_id, kind, actual_blake3, quarantine_rel, found_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![
            run.0,
            entry.media_id.0,
            state.as_str(),
            actual.map(Blake3Hash::as_bytes),
            quarantine.map(RelPath::as_str),
            now
        ])?;
    }
    conn.prepare_cached(
        "UPDATE verify_runs
            SET checked = checked + 1,
                ok = ok + (?2 = 'ok'), missing = missing + (?2 = 'missing'),
                mismatched = mismatched + (?2 = 'mismatch'), unreadable = unreadable + (?2 = 'unreadable'),
                bytes_read = bytes_read + ?3, cursor_media_id = ?4
          WHERE id = ?1",
    )?
    .execute(params![run.0, state.as_str(), to_i64("bytes_read", bytes_read)?, entry.media_id.0])?;
    Ok(())
}

pub fn finish_run(conn: &Connection, run: VerifyRunId, state: &str) -> Result<(), DbError> {
    conn.execute("UPDATE verify_runs SET state = ?2, finished_at = ?3 WHERE id = ?1", params![run.0, state, now_ms()])?;
    Ok(())
}

pub fn open_findings(conn: &Connection, library: LibraryId) -> Result<Vec<VerifyFinding>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT f.id, f.run_id, f.media_id, f.kind, f.actual_blake3, f.quarantine_rel, f.found_at
           FROM verify_findings f JOIN verify_runs r ON r.id = f.run_id
          WHERE r.library_id = ?1 AND f.resolution = 'open'
          ORDER BY f.id",
    )?;
    let rows = stmt
        .query_map([library.0], |r| {
            Ok(VerifyFinding {
                id: r.get(0)?,
                run_id: VerifyRunId(r.get(1)?),
                media_id: MediaId(r.get(2)?),
                kind: enum_col(r, 3)?,
                actual_blake3: match r.get::<_, Option<Vec<u8>>>(4)? {
                    Some(_) => Some(hash_col(r, 4)?),
                    None => None,
                },
                quarantine_rel: r.get(5)?,
                found_at: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}
