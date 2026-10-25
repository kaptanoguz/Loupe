use std::io;
use std::path::PathBuf;

use loupe_core::{Blake3Hash, CoreError, MediaId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("{op} başarısız ({path}): {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("kütüphane başka bir işlem tarafından kilitli: {0}")]
    LibraryLocked(PathBuf),

    #[error("yetersiz alan: {needed} bayt gerekli, {available} bayt boş ({path})")]
    InsufficientSpace { path: PathBuf, needed: u64, available: u64 },

    #[error("hardlink farklı dosya sistemleri arasında yapılamaz: {src} → {dst}")]
    CrossDevice { src: PathBuf, dst: PathBuf },

    #[error("bu dosya sistemi hardlink desteklemiyor: {0}")]
    HardlinkUnsupported(PathBuf),

    #[error("doğrulama başarısız, yazılan dosya bozuk ({path}): beklenen {expected}, okunan {actual}")]
    VerifyMismatch { path: PathBuf, expected: Blake3Hash, actual: Blake3Hash },

    #[error("kaynak dosya import sırasında değişti: {0}")]
    SourceChanged(PathBuf),

    #[error("hedef zaten var: {0}")]
    TargetExists(PathBuf),

    #[error("aynı içerik katalogda zaten var (medya {existing}): {path}")]
    Duplicate { path: PathBuf, existing: MediaId },

    #[error("kaynak kütüphanenin içinde: {0}")]
    SourceInsideLibrary(PathBuf),

    #[error("normal dosya değil: {0}")]
    NotAFile(PathBuf),

    #[error("{0} için mount noktası bulunamadı")]
    MountNotFound(PathBuf),

    #[error("kütüphane diski bağlı değil (disk {volume})")]
    VolumeNotMounted { volume: String },

    #[error("kütüphane işareti yok: {0} (yeni kütüphane sanılmaz, kullanıcıya sorulur)")]
    MarkerMissing(PathBuf),

    #[error("kütüphane işareti farklı: {path} beklenen {expected}, bulunan {found}")]
    MarkerMismatch { path: PathBuf, expected: String, found: String },

    #[error("kütüphane işareti okunamadı ({path}): {reason}")]
    InvalidMarker { path: PathBuf, reason: String },

    #[error("mountinfo satırı çözülemedi: {0:?}")]
    MountInfoParse(String),

    #[error("dosya izleyici: {0}")]
    Watch(#[from] notify_debouncer_full::notify::Error),

    #[error("katalog: {0}")]
    Catalog(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error(transparent)]
    Core(#[from] CoreError),
}

impl StorageError {
    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { op, path, source }
    }

    pub(crate) fn catalog<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Catalog(Box::new(e))
    }
}
