//! Elle oluşturulan albümler: kullanıcının seçtiği fotoğraflar (akıllı albümün
//! aksine kurala bağlı değil). Albümden çıkarmak ya da albümü silmek yalnızca
//! kataloğu değiştirir; diskteki dosyalara dokunulmaz.

use loupe_core::now_ms;
use loupe_db::Pool;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::{AppError, AppResult};

/// Albüm adı en çok bu kadar karakter (şemadaki CHECK ile aynı).
pub const MAX_NAME: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDto {
    pub id: i64,
    pub name: String,
    pub media_count: i64,
}

fn clean_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("albüm adı boş olamaz".into()));
    }
    if name.chars().count() > MAX_NAME {
        return Err(AppError::Invalid(format!("albüm adı en çok {MAX_NAME} karakter olabilir")));
    }
    Ok(name.to_owned())
}

fn same_name(c: &Connection, name: &str, except: Option<i64>) -> AppResult<bool> {
    Ok(c.query_row(
        "SELECT 1 FROM albums WHERE kind = 'album' AND name = ?1 COLLATE NOCASE AND id IS NOT ?2",
        params![name, except],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}

fn get(c: &Connection, id: i64) -> AppResult<AlbumDto> {
    c.query_row("SELECT id, name, media_count FROM albums WHERE id = ?1 AND kind = 'album'", [id], |r| {
        Ok(AlbumDto { id: r.get(0)?, name: r.get(1)?, media_count: r.get(2)? })
    })
    .optional()?
    .ok_or(AppError::NotFound { entity: "albüm", id })
}

/// Albümler, ada göre.
pub fn list(pool: &Pool) -> AppResult<Vec<AlbumDto>> {
    Ok(pool.read(|c| {
        Ok(c.prepare_cached(
            "SELECT id, name, media_count FROM albums WHERE kind = 'album' ORDER BY name COLLATE NOCASE, id",
        )?
        .query_map([], |r| Ok(AlbumDto { id: r.get(0)?, name: r.get(1)?, media_count: r.get(2)? }))?
        .collect::<Result<_, _>>()?)
    })?)
}

/// Yeni albüm. Aynı adlı (büyük/küçük harf farksız) albüm varsa reddedilir.
pub fn create(pool: &Pool, name: &str) -> AppResult<AlbumDto> {
    let name = clean_name(name)?;
    write(pool, |c| {
        if same_name(c, &name, None)? {
            return Err(AppError::Invalid(format!("\"{name}\" adlı bir albüm zaten var")));
        }
        let now = now_ms();
        c.execute(
            "INSERT INTO albums (kind, name, created_at, updated_at) VALUES ('album', ?1, ?2, ?2)",
            params![name, now],
        )?;
        get(c, c.last_insert_rowid())
    })
}

pub fn rename(pool: &Pool, id: i64, name: &str) -> AppResult<AlbumDto> {
    let name = clean_name(name)?;
    write(pool, |c| {
        get(c, id)?;
        if same_name(c, &name, Some(id))? {
            return Err(AppError::Invalid(format!("\"{name}\" adlı bir albüm zaten var")));
        }
        c.execute("UPDATE albums SET name = ?2, updated_at = ?3 WHERE id = ?1", params![id, name, now_ms()])?;
        get(c, id)
    })
}

/// Albümü siler; fotoğraflar katalogda ve diskte kalır.
pub fn delete(pool: &Pool, id: i64) -> AppResult<()> {
    write(pool, |c| {
        get(c, id)?;
        c.execute("DELETE FROM albums WHERE id = ?1", [id])?;
        Ok(())
    })
}

/// Fotoğrafları albümün sonuna ekler; zaten albümde olanlar atlanır. Döner: eklenen sayısı.
pub fn add(pool: &Pool, id: i64, media: &[i64]) -> AppResult<u64> {
    write(pool, |c| {
        get(c, id)?;
        let tx = c.transaction()?;
        let mut pos: f64 =
            tx.query_row("SELECT COALESCE(max(position), 0) FROM album_media WHERE album_id = ?1", [id], |r| r.get(0))?;
        let now = now_ms();
        let mut added = 0;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO album_media (album_id, media_id, position, added_at)
                 SELECT ?1, id, ?3, ?4 FROM media_files WHERE id = ?2 AND soft_deleted_at IS NULL",
            )?;
            for &m in media {
                let n = ins.execute(params![id, m, pos + 1.0, now])?;
                if n > 0 {
                    pos += 1.0;
                    added += 1;
                }
            }
        }
        tx.execute("UPDATE albums SET updated_at = ?2 WHERE id = ?1", params![id, now])?;
        tx.commit()?;
        Ok(added)
    })
}

/// Fotoğrafları albümden çıkarır. Döner: çıkarılan sayısı.
pub fn remove(pool: &Pool, id: i64, media: &[i64]) -> AppResult<u64> {
    write(pool, |c| {
        get(c, id)?;
        let tx = c.transaction()?;
        let mut removed = 0;
        {
            let mut del = tx.prepare_cached("DELETE FROM album_media WHERE album_id = ?1 AND media_id = ?2")?;
            for &m in media {
                removed += del.execute(params![id, m])? as u64;
            }
        }
        tx.execute("UPDATE albums SET updated_at = ?2 WHERE id = ?1", params![id, now_ms()])?;
        tx.commit()?;
        Ok(removed)
    })
}

/// Uygulama hatası taşıyan yazma: `Pool::write` yalnızca `DbError` döndürür.
fn write<T>(pool: &Pool, f: impl FnOnce(&mut Connection) -> AppResult<T>) -> AppResult<T> {
    let mut out = None;
    let res = pool.write(|c| {
        out = Some(f(c));
        Ok(())
    });
    res?;
    out.unwrap_or_else(|| Err(AppError::Invalid("albüm işlemi çalışmadı".into())))
}
