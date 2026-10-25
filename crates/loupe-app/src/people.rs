//! Kişiler görünümü: yüz kümeleri, adlandırma, birleştirme, yanlış yüzü
//! taşıma ya da ayırma. Asıl kümeleme mantığı `loupe_ai::faces`'tedir; burada
//! arayüzün ihtiyacı olan özetler ve tek transaction'lık komutlar vardır.
//!
//! Kapaklar kare/portre kırpılır (arayüz `bbox` ile `lg` küçük resimden
//! keser); daire yoktur.

use loupe_ai::FaceClusterer;
use loupe_ai::faces::{ClusterId, FaceId};
use loupe_ai::jobs::write_tx;
use loupe_db::Pool;
use rusqlite::Connection;
use serde::Serialize;

use crate::{AppError, AppResult};

/// Normalize yüz kutusu (EXIF yönü uygulanmış görüntüde, 0..1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BBoxDto {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceRef {
    pub face_id: i64,
    pub media_id: i64,
    /// Kapak küçük resmi: `thumb://lg/{hash}`.
    pub hash: String,
    /// Küçük resmin en-boy oranı (kırpma hesabı için).
    pub aspect: f32,
    pub bbox: BBoxDto,
}

/// Kişiler ızgarasında tek kart: adlandırılmış kişi ya da isimsiz küme.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonCard {
    /// Kişi kartında kişi kimliği, isimsiz kümede `null`.
    pub person_id: Option<i64>,
    /// Kişinin en büyük kümesi (birleştirme ve yüz listesi bunun üzerinden).
    pub cluster_id: Option<i64>,
    pub name: Option<String>,
    pub face_count: u64,
    pub photo_count: u64,
    pub cover: Option<FaceRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeopleDto {
    pub named: Vec<PersonCard>,
    /// İsimsiz kümeler, büyükten küçüğe (en çok 200).
    pub unnamed: Vec<PersonCard>,
    pub ignored: u64,
}

const HEX: &str = "lower(hex(m.blake3))";
const ASPECT: &str = "CASE WHEN m.width > 0 AND m.height > 0 THEN
    CASE WHEN m.orientation BETWEEN 5 AND 8 THEN CAST(m.height AS REAL) / m.width
         ELSE CAST(m.width AS REAL) / m.height END ELSE 1.5 END";

fn face_ref(conn: &Connection, face: Option<i64>) -> AppResult<Option<FaceRef>> {
    let Some(face) = face else { return Ok(None) };
    let sql = format!(
        "SELECT f.id, f.media_id, {HEX}, {ASPECT}, f.bbox_x, f.bbox_y, f.bbox_w, f.bbox_h
           FROM faces f JOIN media_files m ON m.id = f.media_id WHERE f.id = ?1"
    );
    use rusqlite::OptionalExtension;
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([face], |r| {
            Ok(FaceRef {
                face_id: r.get(0)?,
                media_id: r.get(1)?,
                hash: r.get(2)?,
                aspect: r.get::<_, f64>(3)? as f32,
                bbox: BBoxDto { x: r.get(4)?, y: r.get(5)?, w: r.get(6)?, h: r.get(7)? },
            })
        })
        .optional()?)
}

/// En iyi yüz: kapak yoksa kalitesi en yüksek olan.
fn best_face(conn: &Connection, cover: Option<i64>, where_sql: &str, id: i64) -> AppResult<Option<i64>> {
    use rusqlite::OptionalExtension;
    if cover.is_some() {
        return Ok(cover);
    }
    Ok(conn
        .prepare_cached(&format!(
            "SELECT f.id FROM faces f LEFT JOIN face_clusters c ON c.id = f.cluster_id
              WHERE f.is_rejected = 0 AND {where_sql} ORDER BY f.quality DESC, f.id LIMIT 1"
        ))?
        .query_row([id], |r| r.get(0))
        .optional()?)
}

pub fn list(pool: &Pool) -> AppResult<PeopleDto> {
    pool.read(|c| list_in(c).map_err(to_db)).map_err(AppError::from)
}

fn list_in(c: &Connection) -> AppResult<PeopleDto> {
    let mut named = Vec::new();
    let rows: Vec<(i64, String, Option<i64>, i64)> = c
        .prepare_cached(
            "SELECT id, name, cover_face_id, face_count FROM persons
              WHERE is_hidden = 0 AND face_count > 0 ORDER BY face_count DESC, name COLLATE NOCASE",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    for (id, name, cover, faces) in rows {
        let cluster: Option<i64> = c
            .prepare_cached("SELECT id FROM face_clusters WHERE person_id = ?1 ORDER BY face_count DESC, id LIMIT 1")?
            .query_row([id], |r| r.get(0))
            .ok();
        let photos: i64 = c
            .prepare_cached("SELECT count(DISTINCT media_id) FROM v_face_person WHERE person_id = ?1")?
            .query_row([id], |r| r.get(0))?;
        let cover = best_face(c, cover, "COALESCE(f.person_override_id, c.person_id) = ?1", id)?;
        named.push(PersonCard {
            person_id: Some(id),
            cluster_id: cluster,
            name: Some(name),
            face_count: u64::try_from(faces).unwrap_or(0),
            photo_count: u64::try_from(photos).unwrap_or(0),
            cover: face_ref(c, cover)?,
        });
    }
    let mut unnamed = Vec::new();
    let rows: Vec<(i64, Option<i64>, i64)> = c
        .prepare_cached(
            "SELECT id, cover_face_id, face_count FROM face_clusters
              WHERE person_id IS NULL AND is_ignored = 0 AND face_count > 0
              ORDER BY face_count DESC, id LIMIT 200",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    for (id, cover, faces) in rows {
        let photos: i64 = c
            .prepare_cached("SELECT count(DISTINCT media_id) FROM faces WHERE cluster_id = ?1 AND is_rejected = 0")?
            .query_row([id], |r| r.get(0))?;
        let cover = best_face(c, cover, "f.cluster_id = ?1", id)?;
        unnamed.push(PersonCard {
            person_id: None,
            cluster_id: Some(id),
            name: None,
            face_count: u64::try_from(faces).unwrap_or(0),
            photo_count: u64::try_from(photos).unwrap_or(0),
            cover: face_ref(c, cover)?,
        });
    }
    let ignored: i64 = c.query_row("SELECT count(*) FROM face_clusters WHERE is_ignored = 1", [], |r| r.get(0))?;
    Ok(PeopleDto { named, unnamed, ignored: u64::try_from(ignored).unwrap_or(0) })
}

/// Kişinin ya da kümenin yüzleri (taşıma/ayırma ekranı).
pub fn faces(pool: &Pool, person: Option<i64>, cluster: Option<i64>, limit: u32) -> AppResult<Vec<FaceRef>> {
    let (where_sql, id) = match (person, cluster) {
        (Some(p), _) => ("COALESCE(f.person_override_id, c.person_id) = ?1", p),
        (None, Some(k)) => ("f.cluster_id = ?1", k),
        (None, None) => return Err(AppError::Invalid("kişi ya da küme gerekli".into())),
    };
    let sql = format!(
        "SELECT f.id, f.media_id, {HEX}, {ASPECT}, f.bbox_x, f.bbox_y, f.bbox_w, f.bbox_h
           FROM faces f JOIN media_files m ON m.id = f.media_id LEFT JOIN face_clusters c ON c.id = f.cluster_id
          WHERE f.is_rejected = 0 AND m.soft_deleted_at IS NULL AND {where_sql}
          ORDER BY m.taken_at_utc DESC, f.id LIMIT ?2"
    );
    Ok(pool.read(|c| {
        Ok(c.prepare_cached(&sql)?
            .query_map(rusqlite::params![id, limit.clamp(1, 2_000)], |r| {
                Ok(FaceRef {
                    face_id: r.get(0)?,
                    media_id: r.get(1)?,
                    hash: r.get(2)?,
                    aspect: r.get::<_, f64>(3)? as f32,
                    bbox: BBoxDto { x: r.get(4)?, y: r.get(5)?, w: r.get(6)?, h: r.get(7)? },
                })
            })?
            .collect::<Result<_, _>>()?)
    })?)
}

/// Kümeyi adlandırır (aynı adlı kişi varsa ona bağlar). Kişi kimliği döner.
pub fn name_cluster(pool: &Pool, cluster: i64, name: &str) -> AppResult<i64> {
    let fc = FaceClusterer::default();
    Ok(write_tx(pool, |c| fc.name_cluster(c, ClusterId(cluster), name))?.0)
}

/// Kişiyi yeniden adlandırır. Aynı adda başka kişi varsa ikisi birleşir.
pub fn rename_person(pool: &Pool, person: i64, name: &str) -> AppResult<i64> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(AppError::Invalid("kişi adı 1–120 karakter olmalı".into()));
    }
    Ok(write_tx(pool, |c| {
        use rusqlite::OptionalExtension;
        let other: Option<i64> = c
            .prepare_cached("SELECT id FROM persons WHERE name = ?1 COLLATE NOCASE AND id <> ?2 ORDER BY id LIMIT 1")?
            .query_row(rusqlite::params![name, person], |r| r.get(0))
            .optional()?;
        let target = match other {
            Some(o) => {
                c.execute("UPDATE face_clusters SET person_id = ?1 WHERE person_id = ?2", [o, person])?;
                c.execute("UPDATE faces SET person_override_id = ?1 WHERE person_override_id = ?2", [o, person])?;
                c.execute("DELETE FROM persons WHERE id = ?1", [person])?;
                o
            }
            None => {
                let n = c.execute(
                    "UPDATE persons SET name = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![person, name, loupe_core::now_ms()],
                )?;
                if n == 0 {
                    return Err(loupe_ai::AiError::NotFound { entity: "kişi", id: person });
                }
                person
            }
        };
        loupe_ai::faces::refresh_person_counts(c)?;
        Ok(target)
    })?)
}

/// `from` kümelerini `into` kümesine katar (hepsi tek transaction).
pub fn merge(pool: &Pool, into: i64, from: &[i64]) -> AppResult<()> {
    if from.is_empty() || from.len() > 500 {
        return Err(AppError::Invalid("birleştirilecek 1–500 küme seçilmeli".into()));
    }
    let fc = FaceClusterer::default();
    write_tx(pool, |c| {
        for &f in from {
            fc.merge(c, ClusterId(into), ClusterId(f))?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Kişileri birleştirir: `from` kişilerinin bütün kümeleri ve elle atanmış
/// yüzleri `into` kişisine geçer, `from` kişileri silinir.
pub fn merge_persons(pool: &Pool, into: i64, from: &[i64]) -> AppResult<()> {
    if from.is_empty() || from.len() > 500 {
        return Err(AppError::Invalid("birleştirilecek 1–500 kişi seçilmeli".into()));
    }
    write_tx(pool, |c| {
        let exists: i64 = c.query_row("SELECT count(*) FROM persons WHERE id = ?1", [into], |r| r.get(0))?;
        if exists == 0 {
            return Err(loupe_ai::AiError::NotFound { entity: "kişi", id: into });
        }
        for &f in from.iter().filter(|&&f| f != into) {
            c.execute("UPDATE face_clusters SET person_id = ?1, is_confirmed = 1 WHERE person_id = ?2", [into, f])?;
            c.execute("UPDATE faces SET person_override_id = ?1 WHERE person_override_id = ?2", [into, f])?;
            c.execute("DELETE FROM persons WHERE id = ?1", [f])?;
        }
        loupe_ai::faces::refresh_person_counts(c)?;
        Ok(())
    })?;
    Ok(())
}

/// Yanlış kümelenmiş yüzü başka kümeye taşır.
pub fn move_face(pool: &Pool, face: i64, to_cluster: i64) -> AppResult<()> {
    let fc = FaceClusterer::default();
    write_tx(pool, |c| fc.move_face(c, FaceId(face), ClusterId(to_cluster)))?;
    Ok(())
}

/// Yüzü kişiye atar: kişinin en büyük kümesine taşınır.
pub fn assign_face(pool: &Pool, face: i64, person: i64) -> AppResult<()> {
    let fc = FaceClusterer::default();
    write_tx(pool, |c| {
        use rusqlite::OptionalExtension;
        let cluster: Option<i64> = c
            .prepare_cached("SELECT id FROM face_clusters WHERE person_id = ?1 ORDER BY face_count DESC, id LIMIT 1")?
            .query_row([person], |r| r.get(0))
            .optional()?;
        match cluster {
            Some(k) => fc.move_face(c, FaceId(face), ClusterId(k)),
            None => {
                let k = fc.detach_face(c, FaceId(face))?;
                c.execute("UPDATE face_clusters SET person_id = ?1 WHERE id = ?2", [person, k.0])?;
                loupe_ai::faces::refresh_person_counts(c)
            }
        }
    })?;
    Ok(())
}

/// Yüzü kümesinden ayırır (kendi onaylı kümesine). Yeni küme kimliği döner.
pub fn detach_face(pool: &Pool, face: i64) -> AppResult<i64> {
    let fc = FaceClusterer::default();
    Ok(write_tx(pool, |c| fc.detach_face(c, FaceId(face)))?.0)
}

/// "Bu bir yüz değil": yüz kümelerden ve kişilerden düşer.
pub fn reject_face(pool: &Pool, face: i64) -> AppResult<()> {
    let fc = FaceClusterer::default();
    write_tx(pool, |c| {
        // Önce kendi kümesine ayrılır (eski kümenin merkezi düzelir), sonra o küme silinir.
        let own = fc.detach_face(c, FaceId(face))?;
        c.execute("UPDATE faces SET is_rejected = 1, cluster_id = NULL WHERE id = ?1", [face])?;
        c.execute("DELETE FROM vec_face_centroids WHERE cluster_id = ?1", [own.0])?;
        c.execute("DELETE FROM face_clusters WHERE id = ?1", [own.0])?;
        loupe_ai::faces::refresh_person_counts(c)
    })?;
    Ok(())
}

/// İsimsiz kümeyi gizler (yabancılar).
pub fn ignore_cluster(pool: &Pool, cluster: i64, ignored: bool) -> AppResult<()> {
    let n = pool.write(|c| {
        Ok(c.execute(
            "UPDATE face_clusters SET is_ignored = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![cluster, ignored, loupe_core::now_ms()],
        )?)
    })?;
    if n == 0 {
        return Err(AppError::NotFound { entity: "yüz kümesi", id: cluster });
    }
    Ok(())
}

fn to_db(e: AppError) -> loupe_db::DbError {
    match e {
        AppError::Db(d) => d,
        other => loupe_db::DbError::Validation(other.to_string()),
    }
}
