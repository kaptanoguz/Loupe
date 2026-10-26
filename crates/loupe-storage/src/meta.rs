//! Platformlar arası dosya kimliği ve değişiklik zamanı.
//!
//! Unix'te inode ve aygıt numarası gerçek değerlerdir. Windows'ta kararlı
//! Rust bunları vermez; ikisi de 0 döner ve değişiklik denetimi boyut ile
//! değişiklik zamanına dayanır.

use std::fs::Metadata;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Değişiklik zamanı, Unix epoch'tan nanosaniye.
#[must_use]
pub fn mtime_ns(m: &Metadata) -> i64 {
    #[cfg(unix)]
    {
        m.mtime().saturating_mul(1_000_000_000).saturating_add(m.mtime_nsec())
    }
    #[cfg(not(unix))]
    {
        m.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
    }
}

/// Inode numarası (Windows'ta 0).
#[must_use]
pub fn inode(m: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        m.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        0
    }
}

/// Aygıt numarası (Windows'ta 0).
#[must_use]
pub fn device(m: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        m.dev()
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        0
    }
}
