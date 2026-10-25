use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("{op} başarısız ({path}): {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("EXIF okunamadı ({path}): {reason}")]
    Exif { path: PathBuf, reason: String },

    #[error("görüntü çözülemedi ({path}): {reason}")]
    Decode { path: PathBuf, reason: String },

    #[error("görüntü kodlanamadı: {0}")]
    Encode(String),

    #[error("bu arka uç {format} biçimini desteklemiyor: {path}")]
    Unsupported { path: PathBuf, format: String },

    #[error("RAW dosyasında gömülü önizleme yok: {0}")]
    NoEmbeddedPreview(PathBuf),

    #[error("sidecar bozuk ({path}): {reason}")]
    Sidecar { path: PathBuf, reason: String },

    #[error("GeoNames verisi bozuk ({path}:{line}): {reason}")]
    GeoData { path: PathBuf, line: usize, reason: String },

    #[error("geçersiz koordinat: {lat}, {lon}")]
    InvalidCoordinate { lat: f64, lon: f64 },

    #[error("dış araç başarısız ({tool}): {reason}")]
    Tool { tool: &'static str, reason: String },
}

impl MediaError {
    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { op, path, source }
    }

    /// Yeniden denemenin anlamsız olduğu, dosyanın kendisinden kaynaklanan hata
    /// mı? (İş kuyruğu bunu doğrudan dead-letter'a gönderir.)
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        matches!(self, Self::Decode { .. } | Self::Unsupported { .. } | Self::NoEmbeddedPreview(_) | Self::Exif { .. })
    }
}
