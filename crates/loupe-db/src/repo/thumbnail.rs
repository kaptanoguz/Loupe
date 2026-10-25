//! `thumbnails`: içerik adresli küçük resim önbelleğinin envanteri.

use loupe_core::{Blake3Hash, MediaId, now_ms};
use rusqlite::{Connection, OptionalExtension, params};

use crate::DbError;
use crate::convert::{hash_col, to_i64};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailRow {
    pub media_id: MediaId,
    /// `sm` | `md` | `lg`
    pub size_class: String,
    /// `webp` | `avif` | `jpeg`
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
    pub source_blake3: Blake3Hash,
    /// `embedded_preview` | `demosaic` | `decode` | `video_frame`
    pub source_kind: String,
}

pub fn upsert(conn: &Connection, t: &ThumbnailRow) -> Result<(), DbError> {
    conn.prepare_cached(
        "INSERT INTO thumbnails (media_id, size_class, format, width, height, byte_size, source_blake3, source_kind, generated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (media_id, size_class) DO UPDATE SET
           format = excluded.format, width = excluded.width, height = excluded.height,
           byte_size = excluded.byte_size, source_blake3 = excluded.source_blake3,
           source_kind = excluded.source_kind, generated_at = excluded.generated_at",
    )?
    .execute(params![
        t.media_id.0,
        t.size_class,
        t.format,
        t.width,
        t.height,
        to_i64("byte_size", t.byte_size)?,
        t.source_blake3.as_bytes(),
        t.source_kind,
        now_ms()
    ])?;
    Ok(())
}

pub fn get(conn: &Connection, media: MediaId, size_class: &str) -> Result<Option<ThumbnailRow>, DbError> {
    Ok(conn
        .prepare_cached(
            "SELECT format, width, height, byte_size, source_blake3, source_kind
               FROM thumbnails WHERE media_id = ?1 AND size_class = ?2",
        )?
        .query_row(params![media.0, size_class], |r| {
            Ok(ThumbnailRow {
                media_id: media,
                size_class: size_class.to_owned(),
                format: r.get(0)?,
                width: u32::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                height: u32::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
                byte_size: u64::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                source_blake3: hash_col(r, 4)?,
                source_kind: r.get(5)?,
            })
        })
        .optional()?)
}

/// LRU tahliyesi için son erişim zamanı (thumb:// işleyicisi toplu çağırır).
pub fn touch(conn: &Connection, media: &[MediaId]) -> Result<usize, DbError> {
    let mut stmt = conn.prepare_cached("UPDATE thumbnails SET last_access_at = ?2 WHERE media_id = ?1")?;
    let now = now_ms();
    let mut n = 0;
    for m in media {
        n += stmt.execute(params![m.0, now])?;
    }
    Ok(n)
}
