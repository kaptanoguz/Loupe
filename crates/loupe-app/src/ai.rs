//! AI işçileri: yüz tespiti (FaceDetect), CLIP vektörleri (ClipEmbed), yüz ve
//! etkinlik kümeleme.
//!
//! Mod açılışta bir kez seçilir ([`AiMode::from_env`]):
//! - `onnx`: `ort` özelliğiyle derlenmiş ve `$XDG_DATA_HOME/loupe/models/`
//!   altında dört model ile tokenizer var (Ayarlar → Yapay Zekâ ya da
//!   `make models` indirir; ONNX Runtime da aynı dizine gelir).
//! - `mock`: `LOUPE_AI=mock` (gösterim ve uçtan uca testler; deterministik).
//! - `off`: model yok. Etkinlik kümelemesi (model gerektirmez) yine çalışır;
//!   yüz ve CLIP işleri sessizce atlanır ve modeller gelince yeniden kuyruğa
//!   konur (`media_ml_state` satırı yazılmadığı için).
//!
//! Kısma ve duraklatma işçi havuzunun [`loupe_jobs::Throttle`]'ıdır; seçim
//! `settings` tablosunda (`resources.mode`, `resources.paused`) kalıcıdır.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use loupe_ai::AiEngine;
use loupe_ai::jobs::{AI_JOB_KINDS, AiJobHandler, write_tx};
use loupe_ai::{DeterministicMockEngine, EventDetector};
use loupe_core::now_ms;
use loupe_db::Pool;
use loupe_jobs::{Job, JobContext, JobError, JobHandler, JobKind, NewJob, Priority, ResourceMode};
use loupe_media::ThumbCache;
use serde::Serialize;

use crate::{AppError, AppResult};

/// `$data/models/` altında beklenen dosyalar.
pub const MODEL_FILES: [&str; 5] =
    ["scrfd_10g.onnx", "arcface_w600k_r50.onnx", "mclip_image.onnx", "mclip_text.onnx", "mclip_tokenizer.json"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiMode {
    Off {
        reason: String,
    },
    Mock,
    /// Model dizini.
    Onnx {
        dir: PathBuf,
    },
}

impl AiMode {
    /// `LOUPE_AI` (`off` | `mock` | `onnx`) yoksa model dizinine bakılır.
    #[must_use]
    pub fn from_env(data_dir: &Path) -> Self {
        let dir = data_dir.join("models");
        match std::env::var("LOUPE_AI").as_deref() {
            Ok("off") => Self::Off { reason: "LOUPE_AI=off".into() },
            Ok("mock") => Self::Mock,
            _ => Self::detect(&dir),
        }
    }

    /// ONNX desteği ve model dosyaları varsa `Onnx`, yoksa nedeniyle `Off`.
    #[must_use]
    pub fn detect(dir: &Path) -> Self {
        let missing: Vec<&str> = MODEL_FILES.iter().copied().filter(|f| !dir.join(f).is_file()).collect();
        if !cfg!(feature = "ort") {
            return Self::Off { reason: "bu derleme ONNX Runtime desteği olmadan yapıldı".into() };
        }
        if missing.is_empty() {
            Self::Onnx { dir: dir.to_path_buf() }
        } else {
            Self::Off { reason: format!("{} içinde model eksik: {}", dir.display(), missing.join(", ")) }
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Off { .. } => "off",
            Self::Mock => "mock",
            Self::Onnx { .. } => "onnx",
        }
    }
}

/// Açılmış AI motoru (modeller tembel yüklenir).
#[derive(Debug)]
pub struct AiRuntime {
    pub mode: AiMode,
    pub engine: Option<AiEngine>,
    /// Kurulum hatası (ör. ONNX Runtime bulunamadı): mod `off` gibi davranır.
    pub error: Option<String>,
    #[cfg(feature = "ort")]
    _reaper: Option<loupe_ai::IdleReaper>,
}

impl AiRuntime {
    #[must_use]
    pub fn open(mode: AiMode) -> Self {
        match &mode {
            AiMode::Off { .. } => Self::bare(mode, None, None),
            AiMode::Mock => {
                let engine = AiEngine::from(DeterministicMockEngine::default());
                Self::bare(mode, Some(engine), None)
            }
            AiMode::Onnx { dir } => Self::onnx(mode.clone(), dir),
        }
    }

    fn bare(mode: AiMode, engine: Option<AiEngine>, error: Option<String>) -> Self {
        Self {
            mode,
            engine,
            error,
            #[cfg(feature = "ort")]
            _reaper: None,
        }
    }

    #[cfg(feature = "ort")]
    fn onnx(mode: AiMode, dir: &Path) -> Self {
        use loupe_ai::onnx::{OnnxConfig, OnnxModelPaths, engine, init_runtime};
        let paths = OnnxModelPaths {
            scrfd: dir.join(MODEL_FILES[0]),
            arcface: dir.join(MODEL_FILES[1]),
            clip_image: dir.join(MODEL_FILES[2]),
            clip_text: dir.join(MODEL_FILES[3]),
            tokenizer: dir.join(MODEL_FILES[4]),
        };
        // `ORT_DYLIB_PATH` yoksa model yöneticisinin indirdiği kütüphane.
        let bundled = dir.join(crate::models::RUNTIME_FILE);
        let lib = (std::env::var_os("ORT_DYLIB_PATH").is_none() && bundled.is_file()).then_some(bundled.as_path());
        let built = init_runtime(lib).and_then(|()| {
            let reaper = loupe_ai::IdleReaper::start(std::time::Duration::from_secs(15))?;
            let e = engine(&paths, OnnxConfig::default(), Some(&reaper))?;
            Ok((e, reaper))
        });
        match built {
            Ok((e, reaper)) => Self { mode, engine: Some(e), error: None, _reaper: Some(reaper) },
            Err(e) => Self::bare(mode, None, Some(e.to_string())),
        }
    }

    #[cfg(not(feature = "ort"))]
    fn onnx(mode: AiMode, _dir: &Path) -> Self {
        Self::bare(mode, None, Some("ONNX Runtime desteği yok".into()))
    }

    /// Anlamsal arama ve yüz tanıma etkin mi.
    #[must_use]
    pub const fn active(&self) -> bool {
        self.engine.is_some()
    }

    /// İş yönlendiricisine takılacak işleyici.
    #[must_use]
    pub fn handler(&self, pool: Arc<Pool>, thumbs: ThumbCache) -> Arc<dyn JobHandler> {
        match &self.engine {
            Some(e) => Arc::new(AiJobHandler::new(pool, thumbs, e.clone())),
            None => Arc::new(EventsOnly { pool, events: EventDetector::default() }),
        }
    }
}

/// Model yokken: etkinlik kümelemesi çalışır, model isteyen işler atlanır.
#[derive(Debug)]
pub struct EventsOnly {
    pool: Arc<Pool>,
    events: EventDetector,
}

impl JobHandler for EventsOnly {
    fn run(&self, job: &Job, _ctx: &JobContext<'_>) -> Result<(), JobError> {
        match job.kind {
            JobKind::ClusterEvents => write_tx(&self.pool, |c| self.events.recluster(c))
                .map(|_| ())
                .map_err(|e| JobError::transient("EVENTS", e.to_string())),
            // `media_ml_state` yazılmaz: modeller gelince yeniden kuyruğa girer.
            JobKind::FaceDetect | JobKind::ClipEmbed | JobKind::ClusterFaces => Ok(()),
            other => Err(JobError::permanent("UNSUPPORTED_KIND", format!("{other} bu işleyicide değil"))),
        }
    }
}

/// AI işlerinin türleri (yönlendirici için).
#[must_use]
pub const fn job_kinds() -> [JobKind; 4] {
    AI_JOB_KINDS
}

/// Model etkinken: henüz işlenmemiş fotoğraflar için yüz ve CLIP işleri;
/// her durumda bir etkinlik kümelemesi. Kuyruğa konan iş sayısı döner.
pub fn backfill(pool: &Pool, queue: &loupe_jobs::JobQueue, models: bool) -> AppResult<usize> {
    let mut jobs = vec![NewJob::global(JobKind::ClusterEvents, Priority::Idle)];
    if models {
        let ids: Vec<i64> = pool.read(|c| {
            Ok(c.prepare(
                "SELECT m.id FROM media_files m LEFT JOIN media_ml_state s ON s.media_id = m.id
                  WHERE m.soft_deleted_at IS NULL AND m.file_type <> 'video'
                    AND (s.media_id IS NULL OR s.faces_model_id IS NULL OR s.clip_model_id IS NULL)
                  ORDER BY m.taken_at_utc DESC",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
        })?;
        for id in ids {
            let m = loupe_core::MediaId(id);
            jobs.push(NewJob::media(JobKind::FaceDetect, m, "", Priority::Background));
            jobs.push(NewJob::media(JobKind::ClipEmbed, m, "", Priority::Background));
        }
        jobs.push(NewJob::global(JobKind::ClusterFaces, Priority::Idle));
    }
    let n = jobs.len();
    queue.enqueue_batch(&jobs)?;
    Ok(n)
}

// ───────────────────────────── ayarlar ─────────────────────────────

pub const SETTING_MODE: &str = "resources.mode";
pub const SETTING_PAUSED: &str = "resources.paused";

#[must_use]
pub const fn mode_name(m: ResourceMode) -> &'static str {
    match m {
        ResourceMode::Quiet => "quiet",
        ResourceMode::Balanced => "balanced",
        ResourceMode::Full => "full",
    }
}

pub fn parse_mode(s: &str) -> AppResult<ResourceMode> {
    Ok(match s {
        "quiet" => ResourceMode::Quiet,
        "balanced" => ResourceMode::Balanced,
        "full" => ResourceMode::Full,
        other => return Err(AppError::Invalid(format!("bilinmeyen kaynak modu: {other}"))),
    })
}

/// `settings` tablosundan JSON değeri.
pub fn get_setting(pool: &Pool, key: &str) -> AppResult<Option<serde_json::Value>> {
    use rusqlite::OptionalExtension;
    let raw: Option<String> = pool.read(|c| {
        Ok(c.prepare_cached("SELECT value FROM settings WHERE key = ?1")?.query_row([key], |r| r.get(0)).optional()?)
    })?;
    raw.map(|s| serde_json::from_str(&s).map_err(|e| AppError::Invalid(format!("ayar {key}: {e}")))).transpose()
}

pub fn put_setting(pool: &Pool, key: &str, value: &serde_json::Value) -> AppResult<()> {
    let v = value.to_string();
    pool.write(|c| {
        c.prepare_cached(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )?
        .execute(rusqlite::params![key, v, now_ms()])?;
        Ok(())
    })?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiQueueDto {
    pub faces: u64,
    pub clip: u64,
    pub clustering: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatusDto {
    /// `off` | `mock` | `onnx`
    pub mode: &'static str,
    /// Neden kapalı (model eksik, ONNX Runtime yüklenemedi…).
    pub reason: Option<String>,
    /// CLIP ile anlamsal arama açık mı.
    pub semantic: bool,
    pub paused: bool,
    /// `quiet` | `balanced` | `full`
    pub resource_mode: &'static str,
    pub pending: AiQueueDto,
    /// Yüzü taranmış ve CLIP vektörü olan fotoğraflar.
    pub faces_done: u64,
    pub clip_done: u64,
    pub images: u64,
}

pub(crate) fn queue_counts(pool: &Pool) -> AppResult<(AiQueueDto, u64, u64, u64)> {
    Ok(pool.read(|c| {
        let n = |sql: &str| -> Result<u64, loupe_db::DbError> {
            let v: i64 = c.query_row(sql, [], |r| r.get(0))?;
            Ok(u64::try_from(v).unwrap_or(0))
        };
        let pending = "state IN ('pending','running','failed')";
        Ok((
            AiQueueDto {
                faces: n(&format!("SELECT count(*) FROM jobs WHERE kind = 'face_detect' AND {pending}"))?,
                clip: n(&format!("SELECT count(*) FROM jobs WHERE kind = 'clip_embed' AND {pending}"))?,
                clustering: n(&format!(
                    "SELECT count(*) FROM jobs WHERE kind IN ('cluster_faces','cluster_events') AND {pending}"
                ))?,
            },
            n("SELECT count(*) FROM media_ml_state WHERE faces_model_id IS NOT NULL")?,
            n("SELECT count(*) FROM media_ml_state WHERE clip_model_id IS NOT NULL")?,
            n("SELECT count(*) FROM media_files WHERE soft_deleted_at IS NULL AND file_type <> 'video'")?,
        ))
    })?)
}
