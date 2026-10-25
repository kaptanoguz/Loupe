//! Model yaşam döngüsü: tembel yükleme ve boşta boşaltma.
//!
//! ONNX oturumları yüzlerce MB RAM/VRAM tutar. [`ManagedModel`] modeli ilk
//! kullanımda yükler; `idle_timeout` boyunca kullanılmazsa oturumu düşürür
//! (`drop`). Çalışan bir iş modeli `Arc` ile tuttuğu sürece boşaltılmaz; iş
//! bitince bellek serbest kalır. [`IdleReaper`] arka plan iş parçacığı kayıtlı
//! modelleri düzenli aralıkla yoklar.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use image::RgbImage;

use crate::AiError;
use crate::embedding::Embedding;
use crate::engine::{ClipImageEncoder, ClipTextEncoder, DetectedFace, FaceDetector, FaceEmbedder};
use crate::model::ModelInfo;

type Loader<T> = Box<dyn Fn() -> Result<T, AiError> + Send + Sync>;

struct Slot<T> {
    model: Arc<T>,
    last_used: Instant,
}

pub struct ManagedModel<T> {
    info: ModelInfo,
    loader: Loader<T>,
    idle_timeout: Duration,
    slot: Mutex<Option<Slot<T>>>,
    loads: AtomicU64,
    unloads: AtomicU64,
}

impl<T: Send + Sync> std::fmt::Debug for ManagedModel<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedModel")
            .field("model", &self.info.name)
            .field("idle_timeout", &self.idle_timeout)
            .field("loaded", &self.is_loaded())
            .finish_non_exhaustive()
    }
}

impl<T: Send + Sync> ManagedModel<T> {
    pub fn new(
        info: ModelInfo,
        idle_timeout: Duration,
        loader: impl Fn() -> Result<T, AiError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            info,
            loader: Box::new(loader),
            idle_timeout,
            slot: Mutex::new(None),
            loads: AtomicU64::new(0),
            unloads: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub fn info(&self) -> &ModelInfo {
        &self.info
    }

    /// Modeli döndürür; yüklü değilse yükler. Yükleme kilit altında yapılır:
    /// aynı anda gelen iki iş modeli iki kez yüklemez.
    pub fn get(&self) -> Result<Arc<T>, AiError> {
        let mut slot = self.slot.lock().map_err(|_| AiError::Poisoned)?;
        if let Some(s) = slot.as_mut() {
            s.last_used = Instant::now();
            return Ok(Arc::clone(&s.model));
        }
        let model = Arc::new((self.loader)()?);
        self.loads.fetch_add(1, Ordering::SeqCst);
        *slot = Some(Slot { model: Arc::clone(&model), last_used: Instant::now() });
        Ok(model)
    }

    /// `now` itibarıyla boşta süresi dolduysa ve kimse kullanmıyorsa boşaltır.
    pub fn unload_if_idle(&self, now: Instant) -> bool {
        let Ok(mut slot) = self.slot.lock() else { return false };
        let idle = slot.as_ref().is_some_and(|s| {
            now.saturating_duration_since(s.last_used) >= self.idle_timeout && Arc::strong_count(&s.model) == 1
        });
        if idle {
            *slot = None; // oturum burada düşer
            self.unloads.fetch_add(1, Ordering::SeqCst);
        }
        idle
    }

    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.slot.lock().map(|s| s.is_some()).unwrap_or(false)
    }

    #[must_use]
    pub fn load_count(&self) -> u64 {
        self.loads.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn unload_count(&self) -> u64 {
        self.unloads.load(Ordering::SeqCst)
    }

    #[must_use]
    pub const fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }
}

// Yönetilen model, sardığı modelin trait'ini uygular: iş işleyicisi modelin
// yüklü olup olmadığını bilmez; her çağrı gerekirse yükler.
impl<T: FaceDetector> FaceDetector for ManagedModel<T> {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn detect(&self, image: &RgbImage) -> Result<Vec<DetectedFace>, AiError> {
        self.get()?.detect(image)
    }
}

impl<T: FaceEmbedder> FaceEmbedder for ManagedModel<T> {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn embed(&self, image: &RgbImage, face: &DetectedFace) -> Result<Embedding, AiError> {
        self.get()?.embed(image, face)
    }
}

impl<T: ClipImageEncoder> ClipImageEncoder for ManagedModel<T> {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn encode_image(&self, image: &RgbImage) -> Result<Embedding, AiError> {
        self.get()?.encode_image(image)
    }
}

impl<T: ClipTextEncoder> ClipTextEncoder for ManagedModel<T> {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn encode_text(&self, text: &str) -> Result<Embedding, AiError> {
        self.get()?.encode_text(text)
    }
}

/// Boşaltıcıya kaydedilebilen her model.
pub trait Unloadable: Send + Sync {
    fn unload_if_idle(&self, now: Instant) -> bool;
}

impl<T: Send + Sync> Unloadable for ManagedModel<T> {
    fn unload_if_idle(&self, now: Instant) -> bool {
        Self::unload_if_idle(self, now)
    }
}

struct ReaperShared {
    models: Mutex<Vec<Weak<dyn Unloadable>>>,
    stop: AtomicBool,
    wake: (Mutex<()>, Condvar),
}

/// Kayıtlı modelleri `interval` aralıkla yoklayıp boşta olanları düşüren iş parçacığı.
/// Düşürülünce durur.
pub struct IdleReaper {
    shared: Arc<ReaperShared>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for IdleReaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdleReaper").finish_non_exhaustive()
    }
}

impl IdleReaper {
    pub fn start(interval: Duration) -> Result<Self, AiError> {
        let shared = Arc::new(ReaperShared {
            models: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            wake: (Mutex::new(()), Condvar::new()),
        });
        let s = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("loupe-ai-reaper".into())
            .spawn(move || {
                while !s.stop.load(Ordering::SeqCst) {
                    if let Ok(guard) = s.wake.0.lock() {
                        let _ignored = s.wake.1.wait_timeout(guard, interval);
                    }
                    let now = Instant::now();
                    if let Ok(mut models) = s.models.lock() {
                        models.retain(|w| w.upgrade().map(|m| m.unload_if_idle(now)).is_some());
                    }
                }
            })
            .map_err(|e| AiError::ModelLoad { name: "reaper".into(), reason: e.to_string() })?;
        Ok(Self { shared, thread: Some(thread) })
    }

    /// Modeli izlemeye alır (zayıf referans: model düşürülünce kayıt da düşer).
    pub fn watch(&self, model: &Arc<dyn Unloadable>) {
        if let Ok(mut m) = self.shared.models.lock() {
            m.push(Arc::downgrade(model));
        }
    }
}

impl Drop for IdleReaper {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.wake.1.notify_all();
        if let Some(t) = self.thread.take() {
            let _ignored = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelTask;

    fn info() -> ModelInfo {
        ModelInfo {
            task: ModelTask::FaceDetect,
            name: "t".into(),
            version: "1".into(),
            dim: None,
            file_blake3: [0; 32],
        }
    }

    #[test]
    fn lazy_load_and_busy_model_is_not_unloaded() {
        let m = ManagedModel::new(info(), Duration::from_millis(0), || Ok(vec![0u8; 1024]));
        assert!(!m.is_loaded());
        let held = m.get().unwrap();
        assert!(m.is_loaded());
        assert!(!m.unload_if_idle(Instant::now()), "kullanımdaki model düşürülmez");
        drop(held);
        assert!(m.unload_if_idle(Instant::now()));
        assert!(!m.is_loaded());
        m.get().unwrap();
        assert_eq!((m.load_count(), m.unload_count()), (2, 1));
    }
}
