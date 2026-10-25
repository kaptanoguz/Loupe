use loupe_core::{
    Blake3Hash, CommitOutcome, ImportSessionId, LibraryId, MediaId, NewMediaFile, PHash, RelPath, VerifyState, now_ms,
};
use rusqlite::{Connection, OptionalExtension, Row, Statement, params};

use crate::DbError;
use crate::convert::{bits_from_i64, bits_to_i64, bool_col, enum_col, from_i64, hash_col, rel_path_col, to_i64};
use crate::models::{MediaFile, UserFieldsUpdate};

const COLUMNS: &str = "id, library_id, rel_path, original_filename, placement, import_session_id, file_type,
    raw_format, mime, size_bytes, mtime_ns, inode, device_id, blake3, phash, width, height, taken_at_utc,
    tz_offset_min, taken_at_source, rating, color_label, is_favorite, flag, soft_deleted_at, verify_state,
    last_verified_at, missing_since, imported_at, updated_at";

/// Tam kopya engeli `blake3 UNIQUE` kısıtındadır. Çakışma hedefi yalnızca `blake3`:
/// aynı yola farklı içerik yazmak (`UNIQUE(library_id, rel_path)`) gerçek bir hatadır
/// ve sessizce yutulmaz. Import anında dosya tam okunup doğrulandığı için
/// `last_verified_at = imported_at`.
pub(crate) const INSERT_SQL: &str = "INSERT INTO media_files (
        library_id, rel_path, original_filename, placement, import_session_id, file_type, raw_format, mime,
        size_bytes, mtime_ns, inode, device_id, blake3, phash, width, height, taken_at_utc, tz_offset_min,
        taken_at_source, last_verified_at, imported_at, updated_at)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?20, ?20)
    ON CONFLICT (blake3) DO NOTHING
    RETURNING id";

pub(crate) const ID_BY_BLAKE3_SQL: &str = "SELECT id FROM media_files WHERE blake3 = ?1";

fn from_row(r: &Row<'_>) -> rusqlite::Result<MediaFile> {
    Ok(MediaFile {
        id: MediaId(r.get(0)?),
        library_id: LibraryId(r.get(1)?),
        rel_path: rel_path_col(r, 2)?,
        original_filename: r.get(3)?,
        placement: enum_col(r, 4)?,
        import_session_id: r.get::<_, Option<i64>>(5)?.map(ImportSessionId),
        file_type: enum_col(r, 6)?,
        raw_format: r.get(7)?,
        mime: r.get(8)?,
        size_bytes: from_i64(r.get(9)?),
        mtime_ns: r.get(10)?,
        inode: r.get::<_, Option<i64>>(11)?.map(bits_from_i64),
        device_id: r.get::<_, Option<i64>>(12)?.map(bits_from_i64),
        blake3: hash_col(r, 13)?,
        phash: r.get::<_, Option<i64>>(14)?.map(PHash::from_db),
        width: r.get::<_, Option<i64>>(15)?.and_then(|v| u32::try_from(v).ok()),
        height: r.get::<_, Option<i64>>(16)?.and_then(|v| u32::try_from(v).ok()),
        taken_at_utc: r.get(17)?,
        tz_offset_min: r.get::<_, Option<i64>>(18)?.and_then(|v| i32::try_from(v).ok()),
        taken_at_source: enum_col(r, 19)?,
        rating: u8::try_from(r.get::<_, i64>(20)?).unwrap_or(0),
        color_label: match r.get::<_, Option<String>>(21)? {
            Some(_) => Some(enum_col(r, 21)?),
            None => None,
        },
        is_favorite: bool_col(r, 22)?,
        flag: enum_col(r, 23)?,
        soft_deleted_at: r.get(24)?,
        verify_state: enum_col(r, 25)?,
        last_verified_at: r.get(26)?,
        missing_since: r.get(27)?,
        imported_at: r.get(28)?,
        updated_at: r.get(29)?,
    })
}

/// Hazırlanmış INSERT ifadesiyle tek satır yazar. Toplu yazma aynı ifadeyi
/// transaction boyunca yeniden kullanır.
pub(crate) fn insert_with(
    insert: &mut Statement<'_>,
    lookup: &mut Statement<'_>,
    m: &NewMediaFile,
) -> Result<CommitOutcome, DbError> {
    let size = to_i64("size_bytes", m.size_bytes)?;
    let id: Option<i64> = insert
        .query_row(
            params![
                m.library_id.0,
                m.rel_path.as_str(),
                m.original_filename,
                m.placement.as_str(),
                m.import_session_id.map(|s| s.0),
                m.file_type.as_str(),
                m.raw_format,
                m.mime,
                size,
                m.mtime_ns,
                m.inode.map(bits_to_i64),
                m.device_id.map(bits_to_i64),
                m.blake3.as_bytes(),
                m.phash.map(PHash::to_db),
                m.width,
                m.height,
                m.taken_at_utc,
                m.tz_offset_min,
                m.taken_at_source.as_str(),
                m.imported_at,
            ],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => Ok(CommitOutcome::Inserted(MediaId(id))),
        None => {
            let existing: i64 = lookup.query_row([m.blake3.as_bytes()], |r| r.get(0))?;
            Ok(CommitOutcome::Duplicate(MediaId(existing)))
        }
    }
}

/// Tek satır ekler. Aynı BLAKE3 zaten varsa satır yazılmaz, mevcut kimlik döner.
pub fn insert(conn: &Connection, m: &NewMediaFile) -> Result<CommitOutcome, DbError> {
    let mut ins = conn.prepare_cached(INSERT_SQL)?;
    let mut look = conn.prepare_cached(ID_BY_BLAKE3_SQL)?;
    insert_with(&mut ins, &mut look, m)
}

pub fn get(conn: &Connection, id: MediaId) -> Result<Option<MediaFile>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM media_files WHERE id = ?1");
    Ok(conn.prepare_cached(&sql)?.query_row([id.0], from_row).optional()?)
}

pub fn find_by_blake3(conn: &Connection, hash: &Blake3Hash) -> Result<Option<MediaId>, DbError> {
    Ok(conn.prepare_cached(ID_BY_BLAKE3_SQL)?.query_row([hash.as_bytes()], |r| r.get(0)).optional()?.map(MediaId))
}

pub fn find_by_rel_path(conn: &Connection, library: LibraryId, rel: &RelPath) -> Result<Option<MediaFile>, DbError> {
    let sql = format!("SELECT {COLUMNS} FROM media_files WHERE library_id = ?1 AND rel_path = ?2");
    Ok(conn.prepare_cached(&sql)?.query_row(params![library.0, rel.as_str()], from_row).optional()?)
}

/// İmleç tabanlı sayfa (id artan). Çöp kutusundakiler hariç.
pub fn list_page(
    conn: &Connection,
    library: LibraryId,
    after: Option<MediaId>,
    limit: usize,
) -> Result<Vec<MediaFile>, DbError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM media_files
          WHERE library_id = ?1 AND id > ?2 AND soft_deleted_at IS NULL
          ORDER BY id LIMIT ?3"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows =
        stmt.query_map(params![library.0, after.map_or(0, |m| m.0), limit], from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn count(conn: &Connection, library: LibraryId) -> Result<u64, DbError> {
    let n: i64 = conn
        .prepare_cached("SELECT count(*) FROM media_files WHERE library_id = ?1 AND soft_deleted_at IS NULL")?
        .query_row([library.0], |r| r.get(0))?;
    Ok(from_i64(n))
}

/// Puan, renk etiketi, favori, işaret. Şemadaki tetikleyici `sidecar_dirty`'yi kurar.
pub fn update_user_fields(conn: &Connection, id: MediaId, u: &UserFieldsUpdate) -> Result<(), DbError> {
    let label_set = u.color_label.is_some();
    let label = u.color_label.flatten().map(|l| l.as_str());
    let n = conn.execute(
        "UPDATE media_files
            SET rating      = COALESCE(?2, rating),
                color_label = CASE WHEN ?3 THEN ?4 ELSE color_label END,
                is_favorite = COALESCE(?5, is_favorite),
                flag        = COALESCE(?6, flag),
                updated_at  = ?7
          WHERE id = ?1",
        params![id.0, u.rating, label_set, label, u.is_favorite, u.flag.map(|f| f.as_str()), now_ms()],
    )?;
    if n == 0 { Err(DbError::NotFound { entity: "medya", id: id.0 }) } else { Ok(()) }
}

/// Dosya kütüphane içinde yer değiştirdi (izleyici rename tespiti veya düzen değişikliği).
pub fn update_location(conn: &Connection, id: MediaId, rel: &RelPath, mtime_ns: i64) -> Result<(), DbError> {
    conn.execute(
        "UPDATE media_files SET rel_path = ?2, mtime_ns = ?3, updated_at = ?4 WHERE id = ?1",
        params![id.0, rel.as_str(), mtime_ns, now_ms()],
    )?;
    Ok(())
}

pub fn soft_delete(conn: &Connection, id: MediaId) -> Result<(), DbError> {
    let now = now_ms();
    conn.execute(
        "UPDATE media_files SET soft_deleted_at = ?2, updated_at = ?2 WHERE id = ?1 AND soft_deleted_at IS NULL",
        params![id.0, now],
    )?;
    Ok(())
}

pub fn restore(conn: &Connection, id: MediaId) -> Result<(), DbError> {
    conn.execute(
        "UPDATE media_files SET soft_deleted_at = NULL, updated_at = ?2 WHERE id = ?1",
        params![id.0, now_ms()],
    )?;
    Ok(())
}

/// Kalıcı silme (çöp kutusu boşaltma). Bağımlı satırlar şema tarafından temizlenir.
pub fn delete(conn: &Connection, id: MediaId) -> Result<bool, DbError> {
    Ok(conn.execute("DELETE FROM media_files WHERE id = ?1", [id.0])? > 0)
}

/// Doğrulama sonucu. `ok` ve `mismatch` tam okuma demektir → `last_verified_at` güncellenir.
pub fn set_verify_state(conn: &Connection, id: MediaId, state: VerifyState) -> Result<(), DbError> {
    let now = now_ms();
    conn.prepare_cached(
        "UPDATE media_files
            SET verify_state     = ?2,
                missing_since    = CASE WHEN ?2 = 'missing' THEN COALESCE(missing_since, ?3) ELSE NULL END,
                last_verified_at = CASE WHEN ?2 IN ('ok','mismatch') THEN ?3 ELSE last_verified_at END,
                updated_at       = ?3
          WHERE id = ?1",
    )?
    .execute(params![id.0, state.as_str(), now])?;
    Ok(())
}

/// pHash bant indeksleriyle yakın kopya adayları (Hamming ≤ `max_distance`, en çok 3
/// için bant eşleşmesi tam kapsar). İfadeler şemadaki indekslerle birebir aynıdır.
pub fn near_duplicates(conn: &Connection, phash: PHash, max_distance: u32) -> Result<Vec<(MediaId, u32)>, DbError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, hamming(phash, ?1) AS d FROM media_files
          WHERE phash IS NOT NULL
            AND ((phash & 65535) = ?2 OR ((phash >> 16) & 65535) = ?3
              OR ((phash >> 32) & 65535) = ?4 OR ((phash >> 48) & 65535) = ?5)
            AND hamming(phash, ?1) <= ?6
          ORDER BY d, id",
    )?;
    let rows = stmt
        .query_map(
            params![phash.to_db(), phash.band(0), phash.band(1), phash.band(2), phash.band(3), max_distance],
            |r| Ok((MediaId(r.get(0)?), u32::try_from(r.get::<_, i64>(1)?).unwrap_or(u32::MAX))),
        )?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}
