//! Loupe medya hattı.
//!
//! - [`metadata`]: JPEG, PNG, WebP, HEIC ve TIFF tabanlı RAW (CR2, NEF, ARW, DNG)
//!   dosyalarından kamera, pozlama, çekim zamanı (saat dilimiyle) ve GPS.
//! - [`geo`]: koordinat biçimleme (`41° 00' 49.8" N`) ve çevrimdışı GeoNames
//!   ters coğrafi kodlama (R-ağacı, birim küre üzerinde en yakın yerleşim).
//! - [`sidecar`]: XMP ve `.loupe.json` sidecar okuma/yazma; orijinal dosyaya dokunmaz.
//! - [`raw`]: RAW içindeki gömülü JPEG önizlemeyi çıkarır.
//! - [`thumb`]: yönlendirmesi düzeltilmiş, içerik adresli küçük resimler
//!   (`sm` 256, `md` 768, `lg` 1920 px) ve [`thumb::ThumbnailBackend`] soyutlaması.
//! - [`protocol`]: Tauri `thumb://` özel şeması için saf Rust istek işleyicisi.

pub mod error;
pub mod geo;
pub mod metadata;
pub mod protocol;
pub mod raw;
pub mod sidecar;
pub mod thumb;
mod util;

pub use error::MediaError;
pub use geo::{GeoIndex, Place, format_dms};
pub use metadata::{MediaMetadata, read_metadata};
pub use protocol::ThumbProtocol;
pub use sidecar::{Sidecar, SidecarData};
pub use thumb::{ThumbCache, ThumbSize, ThumbnailBackend, Thumbnailer};

pub type Result<T, E = MediaError> = std::result::Result<T, E>;
