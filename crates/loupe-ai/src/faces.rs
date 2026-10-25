//! Yüz kaydı ve artımlı kişi kümeleme.
//!
//! **Küme merkezi:** `vec_face_centroids` kümedeki yüz vektörlerinin
//! *toplamını* tutar (normalize edilmemiş). Kosinüs ölçeği önemsemediği için
//! toplamın yönü ortalamanın yönüdür; ama toplam ekleme (+v), çıkarma (−v) ve
//! birleştirmede (a + b) tamamen kesindir, kayan ortalama hatası birikmez.
//!
//! **Atama:** yeni yüz en yakın `knn` merkezle karşılaştırılır; kosinüs
//! benzerliği `threshold` üstündeyse o kümeye (ve kümenin kişisine) katılır,
//! değilse kendi kümesini açar. Kalitesi düşük yüzler kümelemeye girmez
//! (`faces_unclustered` indeksinde bekler).
//!
//! **Kullanıcı düzeltmesi:** [`FaceClusterer::merge`], [`FaceClusterer::detach_face`],
//! [`FaceClusterer::move_face`] ve [`FaceClusterer::name_cluster`]. Otomatik
//! kümeleme yalnızca kümesiz yüzlere dokunur; kullanıcının ayırdığı ya da
//! taşıdığı yüz geri çekilmez.

use loupe_core::{MediaId, now_ms};
use rusqlite::{Connection, OptionalExtension, params};

use crate::AiError;
use crate::embedding::{Embedding, from_blob, to_blob};
use crate::engine::DetectedFace;
use crate::model::ModelId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClusterId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PersonId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterConfig {
    /// Aynı kişi sayılmak için merkezle en düşük kosinüs benzerliği.
    /// ArcFace için 0.45–0.55 tipik; yüksek değer daha çok küçük küme demektir.
    pub threshold: f32,
    /// Bu kalitenin altındaki yüzler kümelemeye girmez.
    pub min_quality: f32,
    /// Bu tespit güveninin altındaki yüzler hiç kaydedilmez.
    pub min_score: f32,
    /// Atamada bakılan en yakın merkez sayısı.
    pub knn: usize,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self { threshold: 0.5, min_quality: 0.3, min_score: 0.5, knn: 8 }
    }
}

/// Kümelemenin bir yüz için verdiği karar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assignment {
    Joined(ClusterId),
    Created(ClusterId),
    /// Kalite eşiğin altında: yüz kayıtlı ama kümesiz.
    Skipped,
}

impl Assignment {
    #[must_use]
    pub const fn cluster(self) -> Option<ClusterId> {
        match self {
            Self::Joined(c) | Self::Created(c) => Some(c),
            Self::Skipped => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FaceClusterer {
    pub cfg: ClusterConfig,
}

fn landmarks_blob(face: &DetectedFace) -> Option<Vec<u8>> {
    face.landmarks.map(|lm| to_blob(&lm.iter().flatten().copied().collect::<Vec<_>>()))
}

fn centroid(conn: &Connection, c: ClusterId) -> Result<Option<Vec<f32>>, AiError> {
    let blob: Option<Vec<u8>> = conn
        .prepare_cached("SELECT embedding FROM vec_face_centroids WHERE cluster_id = ?1")?
        .query_row([c.0], |r| r.get(0))
        .optional()?;
    blob.map(|b| from_blob(&b)).transpose()
}

fn set_centroid(conn: &Connection, c: ClusterId, v: &[f32]) -> Result<(), AiError> {
    // vec0 sanal tablosu UPSERT desteklemez.
    conn.prepare_cached("DELETE FROM vec_face_centroids WHERE cluster_id = ?1")?.execute([c.0])?;
    conn.prepare_cached("INSERT INTO vec_face_centroids (cluster_id, embedding) VALUES (?1, ?2)")?
        .execute(params![c.0, to_blob(v)])?;
    Ok(())
}

/// Merkeze `k · v` ekler (k = +1 katılım, −1 ayrılma).
fn shift_centroid(conn: &Connection, c: ClusterId, v: &[f32], k: f32) -> Result<(), AiError> {
    let mut sum = centroid(conn, c)?.unwrap_or_else(|| vec![0.0; v.len()]);
    if sum.len() != v.len() {
        return Err(AiError::Dimension { expected: sum.len(), found: v.len() });
    }
    for (s, x) in sum.iter_mut().zip(v) {
        *s += k * x;
    }
    set_centroid(conn, c, &sum)
}

fn face_vector(conn: &Connection, f: FaceId) -> Result<Vec<f32>, AiError> {
    let blob: Vec<u8> = conn
        .prepare_cached("SELECT embedding FROM vec_faces WHERE face_id = ?1")?
        .query_row([f.0], |r| r.get(0))
        .optional()?
        .ok_or(AiError::NotFound { entity: "yüz vektörü", id: f.0 })?;
    from_blob(&blob)
}

fn face_cluster(conn: &Connection, f: FaceId) -> Result<Option<ClusterId>, AiError> {
    let row: Option<Option<i64>> =
        conn.prepare_cached("SELECT cluster_id FROM faces WHERE id = ?1")?.query_row([f.0], |r| r.get(0)).optional()?;
    row.map(|c| c.map(ClusterId)).ok_or(AiError::NotFound { entity: "yüz", id: f.0 })
}

fn create_cluster(
    conn: &Connection,
    model: ModelId,
    cover: FaceId,
    v: &[f32],
    confirmed: bool,
) -> Result<ClusterId, AiError> {
    let now = now_ms();
    let id: i64 = conn
        .prepare_cached(
            "INSERT INTO face_clusters (model_id, cover_face_id, is_confirmed, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
        )?
        .query_row(params![model.0, cover.0, confirmed, now], |r| r.get(0))?;
    let c = ClusterId(id);
    set_centroid(conn, c, v)?;
    Ok(c)
}

fn touch_cluster(conn: &Connection, c: ClusterId) -> Result<(), AiError> {
    conn.prepare_cached("UPDATE face_clusters SET updated_at = ?2 WHERE id = ?1")?.execute(params![c.0, now_ms()])?;
    Ok(())
}

/// Boşalan kümeyi siler (merkez tetikleyiciyle düşer).
fn drop_if_empty(conn: &Connection, c: ClusterId) -> Result<bool, AiError> {
    Ok(conn.prepare_cached("DELETE FROM face_clusters WHERE id = ?1 AND face_count = 0")?.execute([c.0])? > 0)
}

impl FaceClusterer {
    #[must_use]
    pub const fn new(cfg: ClusterConfig) -> Self {
        Self { cfg }
    }

    /// Bir medyanın yüzlerini kaydeder (kümesiz). Aynı medyanın başka gömme
    /// modeliyle üretilmiş eski yüzleri silinir; tespit güveni düşük olanlar
    /// atlanır. Eklenen yüzleri döndürür.
    pub fn insert_faces(
        &self,
        conn: &Connection,
        media: MediaId,
        faces: &[(DetectedFace, Embedding)],
        detector: ModelId,
        embedder: ModelId,
    ) -> Result<Vec<FaceId>, AiError> {
        // Eski modelin yüzleri: kümelerinden düşülür, sonra silinir.
        let stale: Vec<(i64, Option<i64>)> = conn
            .prepare_cached("SELECT id, cluster_id FROM faces WHERE media_id = ?1 AND embedder_model_id <> ?2")?
            .query_map(params![media.0, embedder.0], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (id, cluster) in stale {
            if let Some(c) = cluster {
                let v = face_vector(conn, FaceId(id))?;
                shift_centroid(conn, ClusterId(c), &v, -1.0)?;
            }
            conn.prepare_cached("DELETE FROM faces WHERE id = ?1")?.execute([id])?;
            if let Some(c) = cluster {
                drop_if_empty(conn, ClusterId(c))?;
            }
        }

        let now = now_ms();
        let mut ids = Vec::with_capacity(faces.len());
        for (f, emb) in faces {
            if f.score < self.cfg.min_score {
                continue;
            }
            let Some(b) = f.bbox.clamped() else { continue };
            let id: i64 = conn
                .prepare_cached(
                    "INSERT INTO faces (media_id, bbox_x, bbox_y, bbox_w, bbox_h, landmarks, det_score, quality,
                                        detector_model_id, embedder_model_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) RETURNING id",
                )?
                .query_row(
                    params![
                        media.0,
                        f64::from(b.x),
                        f64::from(b.y),
                        f64::from(b.w),
                        f64::from(b.h),
                        landmarks_blob(f),
                        f64::from(f.score.clamp(0.0, 1.0)),
                        f64::from(f.quality.clamp(0.0, 1.0)),
                        detector.0,
                        embedder.0,
                        now
                    ],
                    |r| r.get(0),
                )?;
            conn.prepare_cached("INSERT INTO vec_faces (face_id, embedding) VALUES (?1, ?2)")?
                .execute(params![id, emb.to_blob()])?;
            ids.push(FaceId(id));
        }
        Ok(ids)
    }

    /// Kümesiz bir yüzü en yakın kümeye katar ya da yeni küme açar.
    pub fn assign(&self, conn: &Connection, face: FaceId) -> Result<Assignment, AiError> {
        let (cluster, quality, rejected, model): (Option<i64>, f64, bool, i64) = conn
            .prepare_cached("SELECT cluster_id, quality, is_rejected, embedder_model_id FROM faces WHERE id = ?1")?
            .query_row([face.0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?
            .ok_or(AiError::NotFound { entity: "yüz", id: face.0 })?;
        if let Some(c) = cluster {
            return Ok(Assignment::Joined(ClusterId(c)));
        }
        if rejected || quality < f64::from(self.cfg.min_quality) {
            return Ok(Assignment::Skipped);
        }
        let v = face_vector(conn, face)?;
        let k = i64::try_from(self.cfg.knn.max(1)).unwrap_or(8);
        // KNN önce, model süzgeci sonra: sqlite-vec KNN'i alt sorguda ister.
        let best: Option<(i64, f64)> = conn
            .prepare_cached(
                "SELECT v.cluster_id, v.distance
                   FROM (SELECT cluster_id, distance FROM vec_face_centroids
                          WHERE embedding MATCH ?1 AND k = ?2) v
                   JOIN face_clusters c ON c.id = v.cluster_id
                  WHERE c.model_id = ?3
                  ORDER BY v.distance LIMIT 1",
            )?
            .query_row(params![to_blob(&v), k, model], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if let Some((c, distance)) = best
            && 1.0 - distance >= f64::from(self.cfg.threshold)
        {
            let c = ClusterId(c);
            conn.prepare_cached("UPDATE faces SET cluster_id = ?2 WHERE id = ?1")?.execute(params![face.0, c.0])?;
            shift_centroid(conn, c, &v, 1.0)?;
            touch_cluster(conn, c)?;
            return Ok(Assignment::Joined(c));
        }
        let c = create_cluster(conn, ModelId(model), face, &v, false)?;
        conn.prepare_cached("UPDATE faces SET cluster_id = ?2 WHERE id = ?1")?.execute(params![face.0, c.0])?;
        Ok(Assignment::Created(c))
    }

    /// Kümesiz, reddedilmemiş ve kaliteli tüm yüzleri (kaliteliden başlayarak) atar.
    /// İlk yüz merkezi kurar; net yüzle başlamak bulanık yüzlerin küme açmasını azaltır.
    pub fn cluster_pending(&self, conn: &Connection) -> Result<usize, AiError> {
        let pending: Vec<i64> = conn
            .prepare_cached(
                "SELECT id FROM faces WHERE cluster_id IS NULL AND is_rejected = 0 AND quality >= ?1
                  ORDER BY quality DESC, id",
            )?
            .query_map([f64::from(self.cfg.min_quality)], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut n = 0;
        for id in pending {
            if self.assign(conn, FaceId(id))?.cluster().is_some() {
                n += 1;
            }
        }
        refresh_person_counts(conn)?;
        Ok(n)
    }

    /// `from` kümesini `into` kümesine katar. `into` isimsizse `from`'un kişisi geçer.
    pub fn merge(&self, conn: &Connection, into: ClusterId, from: ClusterId) -> Result<(), AiError> {
        if into == from {
            return Ok(());
        }
        let person = |c: ClusterId| -> Result<Option<i64>, AiError> {
            conn.prepare_cached("SELECT person_id FROM face_clusters WHERE id = ?1")?
                .query_row([c.0], |r| r.get(0))
                .optional()?
                .ok_or(AiError::NotFound { entity: "yüz kümesi", id: c.0 })
        };
        let (pi, pf) = (person(into)?, person(from)?);
        let sum_from = centroid(conn, from)?;
        conn.prepare_cached("UPDATE faces SET cluster_id = ?1 WHERE cluster_id = ?2")?
            .execute(params![into.0, from.0])?;
        if let Some(sf) = sum_from {
            shift_centroid(conn, into, &sf, 1.0)?;
        }
        conn.prepare_cached(
            "UPDATE face_clusters SET person_id = ?2, is_confirmed = 1, updated_at = ?3 WHERE id = ?1",
        )?
        .execute(params![into.0, pi.or(pf), now_ms()])?;
        conn.prepare_cached("DELETE FROM face_clusters WHERE id = ?1")?.execute([from.0])?;
        refresh_person_counts(conn)?;
        Ok(())
    }

    /// Yanlış kümelenmiş yüzü ayırır: kendi (onaylı) kümesine taşınır, eski
    /// kümenin merkezinden düşülür. Kişi ataması kaldırılır.
    pub fn detach_face(&self, conn: &Connection, face: FaceId) -> Result<ClusterId, AiError> {
        let old = face_cluster(conn, face)?;
        let v = face_vector(conn, face)?;
        let model: i64 = conn
            .prepare_cached("SELECT embedder_model_id FROM faces WHERE id = ?1")?
            .query_row([face.0], |r| r.get(0))?;
        let c = create_cluster(conn, ModelId(model), face, &v, true)?;
        conn.prepare_cached("UPDATE faces SET cluster_id = ?2, person_override_id = NULL WHERE id = ?1")?
            .execute(params![face.0, c.0])?;
        if let Some(o) = old {
            shift_centroid(conn, o, &v, -1.0)?;
            if !drop_if_empty(conn, o)? {
                conn.prepare_cached(
                    "UPDATE face_clusters SET cover_face_id = NULL WHERE id = ?1 AND cover_face_id = ?2",
                )?
                .execute(params![o.0, face.0])?;
            }
        }
        refresh_person_counts(conn)?;
        Ok(c)
    }

    /// Yüzü başka bir kümeye taşır (merkezler güncellenir).
    pub fn move_face(&self, conn: &Connection, face: FaceId, to: ClusterId) -> Result<(), AiError> {
        let old = face_cluster(conn, face)?;
        if old == Some(to) {
            return Ok(());
        }
        let v = face_vector(conn, face)?;
        conn.prepare_cached("UPDATE faces SET cluster_id = ?2, person_override_id = NULL WHERE id = ?1")?
            .execute(params![face.0, to.0])?;
        shift_centroid(conn, to, &v, 1.0)?;
        conn.prepare_cached("UPDATE face_clusters SET is_confirmed = 1, updated_at = ?2 WHERE id = ?1")?
            .execute(params![to.0, now_ms()])?;
        if let Some(o) = old {
            shift_centroid(conn, o, &v, -1.0)?;
            drop_if_empty(conn, o)?;
        }
        refresh_person_counts(conn)?;
        Ok(())
    }

    /// Kümeyi adlandırır. Aynı adda (büyük/küçük harf duyarsız) kişi varsa ona
    /// bağlanır, yoksa yeni kişi açılır. Küme onaylı olur.
    pub fn name_cluster(&self, conn: &Connection, cluster: ClusterId, name: &str) -> Result<PersonId, AiError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(AiError::InvalidInput("kişi adı 1–120 karakter olmalı".into()));
        }
        let now = now_ms();
        let existing: Option<i64> = conn
            .prepare_cached("SELECT id FROM persons WHERE name = ?1 COLLATE NOCASE ORDER BY id LIMIT 1")?
            .query_row([name], |r| r.get(0))
            .optional()?;
        let person = match existing {
            Some(p) => p,
            None => conn
                .prepare_cached("INSERT INTO persons (name, created_at, updated_at) VALUES (?1, ?2, ?2) RETURNING id")?
                .query_row(params![name, now], |r| r.get(0))?,
        };
        let n = conn
            .prepare_cached("UPDATE face_clusters SET person_id = ?2, is_confirmed = 1, updated_at = ?3 WHERE id = ?1")?
            .execute(params![cluster.0, person, now])?;
        if n == 0 {
            return Err(AiError::NotFound { entity: "yüz kümesi", id: cluster.0 });
        }
        conn.prepare_cached(
            "UPDATE persons SET cover_face_id = COALESCE(cover_face_id,
                    (SELECT cover_face_id FROM face_clusters WHERE id = ?2)), updated_at = ?3
              WHERE id = ?1",
        )?
        .execute(params![person, cluster.0, now])?;
        refresh_person_counts(conn)?;
        Ok(PersonId(person))
    }

    /// Kümenin yüzleri (id sırasıyla).
    pub fn cluster_faces(&self, conn: &Connection, cluster: ClusterId) -> Result<Vec<FaceId>, AiError> {
        Ok(conn
            .prepare_cached("SELECT id FROM faces WHERE cluster_id = ?1 ORDER BY id")?
            .query_map([cluster.0], |r| Ok(FaceId(r.get(0)?)))?
            .collect::<Result<_, _>>()?)
    }

    /// Yüzün kümesi (kümesizse `None`).
    pub fn cluster_of(&self, conn: &Connection, face: FaceId) -> Result<Option<ClusterId>, AiError> {
        face_cluster(conn, face)
    }

    /// Kümenin merkez yönü (normalize).
    pub fn centroid(&self, conn: &Connection, cluster: ClusterId) -> Result<Option<Embedding>, AiError> {
        centroid(conn, cluster)?.map(Embedding::normalized).transpose()
    }
}

/// `persons.face_count` tetikleyiciyle değil, kümeleme işleminin sonunda toplu güncellenir.
pub fn refresh_person_counts(conn: &Connection) -> Result<(), AiError> {
    conn.prepare_cached(
        "UPDATE persons SET face_count = (SELECT count(*) FROM v_face_person v WHERE v.person_id = persons.id)
          WHERE face_count <> (SELECT count(*) FROM v_face_person v WHERE v.person_id = persons.id)",
    )?
    .execute([])?;
    Ok(())
}
