//! Ayarlar → Yapay Zekâ → Modeller: model dosyalarının durumu ve arka planda
//! indirme (RFC-001G §3). İndirme ve doğrulama `loupe_ai::fetch`'tedir; burada
//! yalnızca durum tutulur ve arayüze verilir.
//!
//! Durum yoklaması her seferinde yüzlerce MB özetlemesin diye doğrulama
//! sonucu (boyut, değişiklik zamanı) anahtarıyla önbelleklenir.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use loupe_ai::fetch::{Fetcher, InstallReport, Progress, SourceFailure};
use loupe_ai::models::{FileState, FileStatus, Lock, Manifest, verify};
use serde::Serialize;

use crate::{AppError, AppResult};

/// Model dizinine indirilen ONNX Runtime (uygulama `ORT_DYLIB_PATH` yoksa bunu yükler).
pub const RUNTIME_FILE: &str = "libonnxruntime.so";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelJobDto {
    pub running: bool,
    pub source: Option<String>,
    /// `resolve` | `download` | `verify` | `extract` | `done`
    pub phase: Option<&'static str>,
    pub done: u64,
    pub total: u64,
    pub installed: Vec<String>,
    pub failed: Vec<SourceFailure>,
    /// Kaynak dışı hata (iptal, dizin yazılamıyor…).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsDto {
    pub dir: String,
    pub files: Vec<FileStatus>,
    /// Uygulamanın beklediği beş dosya doğrulandı.
    pub ready: bool,
    /// ONNX Runtime: model dizininde doğrulanmış ya da `ORT_DYLIB_PATH` verilmiş.
    pub runtime: bool,
    /// Bu derleme `ort` özelliğiyle mi yapıldı.
    pub onnx_build: bool,
    /// Modeller hazır ama çalışan motor kapalı: yeniden başlatınca ONNX açılır.
    pub restart_needed: bool,
    pub job: ModelJobDto,
}

type Cache = HashMap<String, (u64, Option<SystemTime>, Option<String>, FileState)>;

#[derive(Debug)]
pub struct Models {
    dir: PathBuf,
    manifest: Manifest,
    job: Arc<Mutex<ModelJobDto>>,
    cancel: Arc<AtomicBool>,
    cache: Mutex<Cache>,
}

impl Models {
    pub fn new(dir: PathBuf) -> AppResult<Self> {
        Ok(Self {
            dir,
            manifest: Manifest::builtin()?,
            job: Arc::new(Mutex::new(ModelJobDto::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            cache: Mutex::new(HashMap::new()),
        })
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Dosya durumları; değişmemiş dosya yeniden özetlenmez.
    pub fn status(&self, engine_active: bool) -> AppResult<ModelsDto> {
        let lock = Lock::load(&self.dir);
        let mut cache = self.cache.lock().map_err(|_| AppError::Poisoned)?;
        let mut files = Vec::with_capacity(self.manifest.files.len());
        for f in &self.manifest.files {
            let path = self.dir.join(&f.file);
            let exp = self.manifest.file_expected(f, &lock);
            let sha = exp.map(|e| loupe_ai::models::hex(&e.sha256));
            let meta = std::fs::metadata(&path).ok();
            let key = meta.as_ref().map(|m| (m.len(), m.modified().ok()));
            let state = match (key, cache.get(&f.file)) {
                (Some((len, mtime)), Some((l, t, s, st))) if *l == len && *t == mtime && *s == sha => *st,
                _ => {
                    let st = verify(&path, exp)?;
                    if let Some((len, mtime)) = key {
                        cache.insert(f.file.clone(), (len, mtime, sha.clone(), st));
                    }
                    st
                }
            };
            files.push(FileStatus {
                file: f.file.clone(),
                role: f.role.clone(),
                state,
                size: exp.map(|e| e.size),
                sha256: sha,
                license: self.manifest.source(&f.source).and_then(|s| s.license.clone()),
            });
        }
        let ok = |name: &str| files.iter().any(|s| s.file == name && s.state == FileState::Verified);
        let ready = crate::ai::MODEL_FILES.iter().all(|f| ok(f));
        let runtime = ok(RUNTIME_FILE) || std::env::var_os("ORT_DYLIB_PATH").is_some();
        let onnx_build = cfg!(feature = "ort");
        Ok(ModelsDto {
            dir: self.dir.display().to_string(),
            restart_needed: onnx_build && ready && runtime && !engine_active,
            files,
            ready,
            runtime,
            onnx_build,
            job: self.job.lock().map_err(|_| AppError::Poisoned)?.clone(),
        })
    }

    /// Eksik ve bozuk dosyaları arka planda indirir. Çalışıyorsa yenisi başlamaz.
    pub fn install(&self) -> AppResult<()> {
        self.install_with(Fetcher::new()?)
    }

    /// Testler: yerel sunucuya bağlı indirici ve manifest.
    pub fn install_with(&self, fetcher: Fetcher) -> AppResult<()> {
        {
            let mut j = self.job.lock().map_err(|_| AppError::Poisoned)?;
            if j.running {
                return Ok(());
            }
            *j = ModelJobDto { running: true, ..ModelJobDto::default() };
        }
        self.cancel.store(false, Ordering::Relaxed);
        let (job, cancel, manifest, dir) =
            (self.job.clone(), self.cancel.clone(), self.manifest.clone(), self.dir.clone());
        let pj = job.clone();
        let progress = Arc::new(move |p: Progress| {
            if let Ok(mut j) = pj.lock() {
                j.source = Some(p.source);
                j.phase = Some(p.phase);
                j.done = p.done;
                j.total = p.total;
            }
        });
        tokio::spawn(async move {
            let result = fetcher.install(&manifest, &dir, progress, cancel).await;
            if let Ok(mut j) = job.lock() {
                j.running = false;
                match result {
                    Ok(InstallReport { installed, failed, .. }) => {
                        j.installed = installed;
                        j.failed = failed;
                    }
                    Err(e) => j.error = Some(e.to_string()),
                }
            }
        });
        Ok(())
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Testler ve [`Models::install_with`]: farklı manifestle.
    #[must_use]
    pub fn with_manifest(mut self, manifest: Manifest) -> Self {
        self.manifest = manifest;
        self
    }
}
