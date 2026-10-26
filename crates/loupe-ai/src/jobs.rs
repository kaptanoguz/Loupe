//! AI iş işleyicisi: `face_detect`, `clip_embed`, `cluster_faces`, `cluster_events`.
//!
//! Girdi orijinal dosya değil, içerik adresli `md` küçük resmidir (768 px,
//! yönlendirmesi düzeltilmiş): RAW/HEIC çözümü bir kez yapılır, yüz kutuları
//! ekranda görünen görüntüyle aynı koordinattadır ve disk takılı olmasa da
//! AI çalışır. Çıkarım veritabanı kilidi dışında yapılır; sonuçlar tek
//! `BEGIN IMMEDIATE` transaction'ında yazılır.

use std::sync::{Arc, Mutex};

use image::RgbImage;
use loupe_core::{MediaId, now_ms};
use loupe_db::Pool;
use loupe_db::repo::media;
use loupe_jobs::{Job, JobContext, JobError, JobHandler, JobKind};
use loupe_media::{ThumbCache, ThumbSize};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::AiError;
use crate::clip;
use crate::engine::{ClipImageEncoder, ClipTextEncoder, FaceDetector, FaceEmbedder};
use crate::events::{EventDetector, EventReport};
use crate::faces::FaceClusterer;
use crate::mock::DeterministicMockEngine;
use crate::model::{ModelId, register};

impl From<AiError> for JobError {
    fn from(e: AiError) -> Self {
        match e {
            AiError::Db(d) => Self::transient("DB", d.to_string()),
            AiError::Media(m) => Self::from(m),
            AiError::ModelLoad { .. } => Self::transient("MODEL_LOAD", e.to_string()),
            AiError::Poisoned => Self::transient("POISONED", e.to_string()),
            AiError::Inference { .. } => Self::permanent("INFERENCE", e.to_string()),
            AiError::Dimension { .. } => Self::permanent("DIMENSION", e.to_string()),
            AiError::InvalidInput(_) => Self::permanent("INVALID_INPUT", e.to_string()),
            AiError::NotFound { .. } => Self::permanent("NOT_FOUND", e.to_string()),
            AiError::Io(_) | AiError::Download { .. } | AiError::Cancelled => Self::transient("IO", e.to_string()),
            AiError::Verify { .. } => Self::permanent("VERIFY", e.to_string()),
        }
    }
}

/// Yazar bağlantısında `BEGIN IMMEDIATE` transaction'ı açar; `f` hata dönerse geri alınır.
pub fn write_tx<T>(pool: &Pool, f: impl FnOnce(&Connection) -> Result<T, AiError>) -> Result<T, AiError> {
    pool.write(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx);
        if out.is_ok() {
            tx.commit()?;
        }
        Ok(out)
    })?
}

/// Takılabilir modeller. Gerçek ONNX modelleri `ort` özelliğiyle
/// (`crate::onnx`), testler [`DeterministicMockEngine`] ile kurulur; her biri
/// [`crate::ManagedModel`] ile sarılıp boşta boşaltılabilir.
#[derive(Clone)]
pub struct AiEngine {
    pub detector: Arc<dyn FaceDetector>,
    pub embedder: Arc<dyn FaceEmbedder>,
    pub clip_image: Arc<dyn ClipImageEncoder>,
    pub clip_text: Arc<dyn ClipTextEncoder>,
}

impl std::fmt::Debug for AiEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiEngine")
            .field("detector", &self.detector.info().name)
            .field("embedder", &self.embedder.info().name)
            .field("clip_image", &self.clip_image.info().name)
            .field("clip_text", &self.clip_text.info().name)
            .finish()
    }
}

impl From<DeterministicMockEngine> for AiEngine {
    fn from(m: DeterministicMockEngine) -> Self {
        let clip = Arc::new(m.clip);
        Self {
            detector: Arc::new(m.detector),
            embedder: Arc::new(m.embedder),
            clip_image: Arc::clone(&clip) as Arc<dyn ClipImageEncoder>,
            clip_text: clip,
        }
    }
}

/// `ml_models` satır kimlikleri (ilk işte kaydedilir).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelIds {
    pub detector: ModelId,
    pub embedder: ModelId,
    pub clip_image: ModelId,
    pub clip_text: ModelId,
}

impl AiEngine {
    /// Modelleri kaydeder ve her görevin etkin modeli yapar.
    pub fn register(&self, conn: &Connection) -> Result<ModelIds, AiError> {
        Ok(ModelIds {
            detector: register(conn, self.detector.info())?,
            embedder: register(conn, self.embedder.info())?,
            clip_image: register(conn, self.clip_image.info())?,
            clip_text: register(conn, self.clip_text.info())?,
        })
    }
}

pub struct AiJobHandler {
    pool: Arc<Pool>,
    thumbs: ThumbCache,
    engine: AiEngine,
    faces: FaceClusterer,
    events: EventDetector,
    ids: Mutex<Option<ModelIds>>,
}

impl std::fmt::Debug for AiJobHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiJobHandler").field("engine", &self.engine).finish_non_exhaustive()
    }
}

/// Bu işleyicinin çalıştırdığı iş türleri ([`loupe_jobs::JobRouter::route`] için).
pub const AI_JOB_KINDS: [JobKind; 4] =
    [JobKind::FaceDetect, JobKind::ClipEmbed, JobKind::ClusterFaces, JobKind::ClusterEvents];

impl AiJobHandler {
    #[must_use]
    pub fn new(pool: Arc<Pool>, thumbs: ThumbCache, engine: AiEngine) -> Self {
        Self {
            pool,
            thumbs,
            engine,
            faces: FaceClusterer::default(),
            events: EventDetector::default(),
            ids: Mutex::new(None),
        }
    }

    #[must_use]
    pub const fn with_clusterer(mut self, faces: FaceClusterer) -> Self {
        self.faces = faces;
        self
    }

    #[must_use]
    pub const fn with_event_detector(mut self, events: EventDetector) -> Self {
        self.events = events;
        self
    }

    #[must_use]
    pub const fn engine(&self) -> &AiEngine {
        &self.engine
    }

    pub fn model_ids(&self) -> Result<ModelIds, AiError> {
        let mut guard = self.ids.lock().map_err(|_| AiError::Poisoned)?;
        if let Some(ids) = *guard {
            return Ok(ids);
        }
        let ids = write_tx(&self.pool, |c| self.engine.register(c))?;
        *guard = Some(ids);
        Ok(ids)
    }

    /// Küçük resmi önbellekten çözer (`md`, yoksa `lg`, o da yoksa `sm`).
    /// `None`: medya katalogda yok ya da çöpte.
    fn load_image(&self, id: MediaId) -> Result<Option<RgbImage>, JobError> {
        let Some(m) = self.pool.read(|c| media::get(c, id))? else { return Ok(None) };
        if m.soft_deleted_at.is_some() {
            return Ok(None);
        }
        let found =
            [ThumbSize::Md, ThumbSize::Lg, ThumbSize::Sm].into_iter().find_map(|s| self.thumbs.lookup(&m.blake3, s));
        let Some((path, _)) = found else {
            return Err(JobError::transient("NO_THUMB", format!("medya {} için küçük resim henüz yok", id.0)));
        };
        let img = image::open(&path).map_err(|e| JobError::permanent("DECODE", format!("{}: {e}", path.display())))?;
        Ok(Some(img.to_rgb8()))
    }

    fn media_id(job: &Job) -> Result<MediaId, JobError> {
        job.media_id.ok_or_else(|| JobError::permanent("NO_MEDIA", "iş bir medyaya bağlı değil"))
    }

    /// Yüzleri bulur, vektörler, kaydeder ve kümelere atar. Aynı gömme
    /// modeliyle işlenmiş medya atlanır.
    pub fn detect_faces(&self, id: MediaId) -> Result<usize, JobError> {
        let ids = self.model_ids()?;
        let done: Option<Option<i64>> = self.pool.read(|c| {
            Ok(c.prepare_cached("SELECT faces_model_id FROM media_ml_state WHERE media_id = ?1")?
                .query_row([id.0], |r| r.get(0))
                .optional()?)
        })?;
        if done.flatten() == Some(ids.embedder.0) {
            return Ok(0);
        }
        let Some(img) = self.load_image(id)? else { return Ok(0) };
        let detected = self.engine.detector.detect(&img)?;
        let mut faces = Vec::with_capacity(detected.len());
        for f in detected {
            let e = self.engine.embedder.embed(&img, &f)?;
            faces.push((f, e));
        }
        let n =
            write_tx(&self.pool, |c| {
                let inserted = self.faces.insert_faces(c, id, &faces, ids.detector, ids.embedder)?;
                for &f in &inserted {
                    self.faces.assign(c, f)?;
                }
                crate::faces::refresh_person_counts(c)?;
                c.prepare_cached(
                "INSERT INTO media_ml_state (media_id, faces_model_id, faces_at, face_count) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (media_id) DO UPDATE SET faces_model_id = excluded.faces_model_id,
                   faces_at = excluded.faces_at, face_count = excluded.face_count",
            )?
            .execute(params![id.0, ids.embedder.0, now_ms(), i64::try_from(inserted.len()).unwrap_or(i64::MAX)])?;
                Ok(inserted.len())
            })?;
        Ok(n)
    }

    /// Görüntü vektörünü hesaplayıp `vec_media_clip`'e yazar.
    pub fn embed_clip(&self, id: MediaId) -> Result<bool, JobError> {
        let ids = self.model_ids()?;
        let done: Option<Option<i64>> = self.pool.read(|c| {
            Ok(c.prepare_cached("SELECT clip_model_id FROM media_ml_state WHERE media_id = ?1")?
                .query_row([id.0], |r| r.get(0))
                .optional()?)
        })?;
        if done.flatten() == Some(ids.clip_image.0) {
            return Ok(false);
        }
        let Some(img) = self.load_image(id)? else { return Ok(false) };
        let e = self.engine.clip_image.encode_image(&img)?;
        write_tx(&self.pool, |c| {
            clip::upsert(c, id, &e)?;
            c.prepare_cached(
                "INSERT INTO media_ml_state (media_id, clip_model_id, clip_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (media_id) DO UPDATE SET clip_model_id = excluded.clip_model_id, clip_at = excluded.clip_at",
            )?
            .execute(params![id.0, ids.clip_image.0, now_ms()])?;
            Ok(())
        })?;
        Ok(true)
    }

    pub fn cluster_faces(&self) -> Result<usize, JobError> {
        Ok(write_tx(&self.pool, |c| self.faces.cluster_pending(c))?)
    }

    pub fn cluster_events(&self) -> Result<EventReport, JobError> {
        Ok(write_tx(&self.pool, |c| self.events.recluster(c))?)
    }
}

impl JobHandler for AiJobHandler {
    fn run(&self, job: &Job, ctx: &JobContext<'_>) -> Result<(), JobError> {
        if ctx.is_cancelled() {
            return Err(JobError::transient("CANCELLED", "havuz kapanıyor"));
        }
        match job.kind {
            JobKind::FaceDetect => self.detect_faces(Self::media_id(job)?).map(|_| ()),
            JobKind::ClipEmbed => self.embed_clip(Self::media_id(job)?).map(|_| ()),
            JobKind::ClusterFaces => self.cluster_faces().map(|_| ()),
            JobKind::ClusterEvents => self.cluster_events().map(|_| ()),
            other => Err(JobError::permanent("UNSUPPORTED_KIND", format!("{other} bu işleyicide değil"))),
        }
    }
}
