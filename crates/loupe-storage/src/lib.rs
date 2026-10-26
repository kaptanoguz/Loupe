//! Loupe depolama katmanı (RFC-001A §3).
//!
//! - [`volume`]: klasörün bulunduğu diski (UUID, etiket, ağ kaynağı) bulur,
//!   `.loupe/library.json` işaretini okur/yazar ve disk başka bir yola
//!   takıldığında kütüphaneyi yeniden konumlandırır.
//! - [`hash`]: dosyayı tek geçişte okuyup BLAKE3 ve (görüntüyse) pHash üretir.
//! - [`import`]: Kopyala / Taşı / Hardlink. Önce `.loupe-tmp/` altına yazar,
//!   fsync eder, BLAKE3 doğrular, sonra `YYYY/MM/DD/` ağacına atomik taşır.
//! - [`watch`]: `notify` + debounce, olaylar bir kanala gider.
//! - [`verify`]: Verify Library (bit-rot taraması).
//!
//! Sıra her zaman: dosya → fsync → veritabanı → (taşı modunda) kaynağı silme.

pub mod error;
pub mod fsutil;
pub mod hash;
pub mod import;
pub mod layout;
pub mod meta;
pub mod verify;
pub mod volume;
pub mod watch;

pub use error::StorageError;
pub use hash::{FileDigest, HashOptions, hash_file};
pub use import::{ImportOutcome, Importer, ImporterConfig};
pub use layout::{CaptureTime, DateResolver, MtimeResolver};
pub use verify::{VerifyOptions, VerifyReport, verify_entry, verify_library};
pub use volume::{LibraryMarker, MountEntry, VolumeInfo, VolumeProbe};
pub use watch::{LibraryWatcher, WatchEvent};

/// Kütüphane kökündeki uygulama dizini (işaret, kilit, karantina).
pub const LOUPE_DIR: &str = ".loupe";
/// Atomik yazmaların geçici dizini. Hedefle aynı dosya sisteminde olmalı ki
/// son adım bir `rename` olsun.
pub const TMP_DIR: &str = ".loupe-tmp";
/// `.loupe/` altında LAN (tus) yüklemelerinin yarım dosyaları. Yeniden
/// başlatmada silinmez: telefon kaldığı bayttan devam eder.
pub const LAN_STAGING_DIR: &str = "lan-staging";

pub type Result<T, E = StorageError> = std::result::Result<T, E>;
