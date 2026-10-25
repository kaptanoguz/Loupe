//! Çok dilli CLIP vektörleri: `vec_media_clip` yazma, KNN ve aday kümesi
//! üzerinde kesin benzerlik. Görüntü ve metin (Türkçe/İngilizce) aynı 512
//! boyutlu uzaydadır (`loupe_db::CLIP_DIM`).

use loupe_core::MediaId;
use loupe_db::CLIP_DIM;
use rusqlite::{Connection, params_from_iter};

use crate::AiError;
use crate::embedding::{Embedding, dot, from_blob};

/// Medyanın görüntü vektörünü yazar (varsa değiştirir).
pub fn upsert(conn: &Connection, media: MediaId, e: &Embedding) -> Result<(), AiError> {
    if e.dim() != CLIP_DIM {
        return Err(AiError::Dimension { expected: CLIP_DIM, found: e.dim() });
    }
    // vec0 sanal tablosu UPSERT desteklemez.
    conn.prepare_cached("DELETE FROM vec_media_clip WHERE media_id = ?1")?.execute([media.0])?;
    conn.prepare_cached("INSERT INTO vec_media_clip (media_id, embedding) VALUES (?1, ?2)")?
        .execute(rusqlite::params![media.0, e.to_blob()])?;
    Ok(())
}

/// Tüm kütüphanede en yakın `k` medya (benzerlik = 1 − kosinüs uzaklığı), yakından uzağa.
pub fn knn(conn: &Connection, query: &Embedding, k: usize) -> Result<Vec<(MediaId, f32)>, AiError> {
    let k = i64::try_from(k.max(1)).unwrap_or(i64::MAX);
    Ok(conn
        .prepare_cached(
            "SELECT media_id, distance FROM vec_media_clip WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance",
        )?
        .query_map(rusqlite::params![query.to_blob(), k], |r| {
            #[allow(clippy::cast_possible_truncation)]
            Ok((MediaId(r.get(0)?), (1.0 - r.get::<_, f64>(1)?) as f32))
        })?
        .collect::<Result<_, _>>()?)
}

/// Yalnızca verilen adaylar için kesin kosinüs benzerliği (SQL süzgecinden
/// sonra: aday kümesi küçükken KNN'den hem hızlı hem kesindir). Vektörü
/// olmayan medya sonuçta yer almaz. Büyükten küçüğe sıralı.
pub fn similarities(
    conn: &Connection,
    query: &Embedding,
    candidates: &[MediaId],
) -> Result<Vec<(MediaId, f32)>, AiError> {
    let mut out = Vec::with_capacity(candidates.len());
    for chunk in candidates.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut stmt =
            conn.prepare(&format!("SELECT media_id, embedding FROM vec_media_clip WHERE media_id IN ({marks})"))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter().map(|m| m.0)), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (id, blob) = row?;
            let v = from_blob(&blob)?;
            if v.len() != query.dim() {
                return Err(AiError::Dimension { expected: query.dim(), found: v.len() });
            }
            out.push((MediaId(id), dot(query.as_slice(), &v)));
        }
    }
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.0.cmp(&b.0.0)));
    Ok(out)
}
