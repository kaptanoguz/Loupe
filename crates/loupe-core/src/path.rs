use std::fmt;
use std::path::{Path, PathBuf};

use unicode_normalization::{UnicodeNormalization, is_nfc};

use crate::CoreError;

/// Kütüphane köküne göre göreli, doğrulanmış yol: `2026/10/24/DSC_0412.NEF`.
///
/// Kurallar: boş değil, `/` ile başlamaz, `.`/`..`/boş bileşen yok, NUL yok,
/// ayırıcı yalnızca `/`, NFC normalize (macOS'tan gelen NFD adlar aynı kayda düşer).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    pub fn new(raw: &str) -> Result<Self, CoreError> {
        let s: String = if is_nfc(raw) { raw.to_owned() } else { raw.nfc().collect() };
        let bad = |reason| CoreError::InvalidRelPath { path: raw.to_owned(), reason };
        if s.is_empty() {
            return Err(bad("boş"));
        }
        if s.starts_with('/') {
            return Err(bad("mutlak yol"));
        }
        if s.contains('\0') {
            return Err(bad("NUL karakteri"));
        }
        if s.contains('\\') {
            return Err(bad("ters bölü ayırıcı"));
        }
        for part in s.split('/') {
            match part {
                "" => return Err(bad("boş bileşen")),
                "." | ".." => return Err(bad("nokta bileşeni")),
                _ => {}
            }
        }
        Ok(Self(s))
    }

    /// `base` altındaki göreli yolu döndürür; `base` dışına çıkan yolu reddeder.
    pub fn from_path(base: &Path, full: &Path) -> Result<Self, CoreError> {
        let rel = full.strip_prefix(base).map_err(|_| CoreError::InvalidRelPath {
            path: full.display().to_string(),
            reason: "kök dizinin dışında",
        })?;
        let s = rel
            .to_str()
            .ok_or_else(|| CoreError::InvalidRelPath { path: full.display().to_string(), reason: "UTF-8 değil" })?;
        Self::new(s)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn to_path(&self, root: &Path) -> PathBuf {
        root.join(&self.0)
    }

    /// Son bileşen (dosya adı).
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Üst dizin; kökteyse `None`.
    #[must_use]
    pub fn parent(&self) -> Option<&str> {
        self.0.rsplit_once('/').map(|(dir, _)| dir)
    }

    pub fn join(&self, child: &str) -> Result<Self, CoreError> {
        Self::new(&format!("{}/{child}", self.0))
    }

    /// İlk bileşen `.loupe` veya `.loupe-tmp` ise uygulamanın kendi dizinidir.
    #[must_use]
    pub fn is_internal(&self) -> bool {
        let first = self.0.split('/').next().unwrap_or_default();
        first == ".loupe" || first == ".loupe-tmp"
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RelPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_components() {
        assert!(RelPath::new("2026/10/24/a.jpg").is_ok());
        for bad in ["", "/etc/passwd", "a/../b", "./a", "a//b", "a/", "a\\b", "a\0b"] {
            assert!(RelPath::new(bad).is_err(), "{bad:?} kabul edilmemeli");
        }
    }

    #[test]
    fn normalizes_to_nfc() {
        let nfd = "S\u{0327}ehir.jpg"; // S + birleşen çengel
        let p = RelPath::new(nfd).unwrap();
        assert_eq!(p.as_str(), "\u{015e}ehir.jpg");
    }

    #[test]
    fn parts() {
        let p = RelPath::new("2026/10/24/a.jpg").unwrap();
        assert_eq!(p.file_name(), "a.jpg");
        assert_eq!(p.parent(), Some("2026/10/24"));
        assert!(!p.is_internal());
        assert!(RelPath::new(".loupe-tmp/x.part").unwrap().is_internal());
        assert!(RelPath::from_path(Path::new("/lib"), Path::new("/other/a")).is_err());
    }
}
