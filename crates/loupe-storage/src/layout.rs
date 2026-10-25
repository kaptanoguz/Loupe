//! Kütüphane içi yerleşim: `{yyyy}/{mm}/{dd}/<ad>` ve ad çakışmasında `_1` eki.

use std::fs::Metadata;
use std::path::Path;

use loupe_core::{RelPath, TakenAtSource};

/// Çekim zamanı: UTC epoch ms + (biliniyorsa) yerel saat farkı.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureTime {
    pub utc_ms: i64,
    pub tz_offset_min: Option<i32>,
    pub source: TakenAtSource,
}

impl CaptureTime {
    /// Klasör tarihi yerel saatle hesaplanır (gece 01:00 çekimi önceki güne düşmesin);
    /// saat farkı bilinmiyorsa UTC.
    #[must_use]
    pub fn local_date(&self) -> (i64, u32, u32) {
        let offset_ms = i64::from(self.tz_offset_min.unwrap_or(0)) * 60_000;
        civil_from_days((self.utc_ms + offset_ms).div_euclid(86_400_000))
    }
}

/// Çekim zamanını bulur. EXIF/GPS/dosya adı çözümleyicisi `loupe-meta` ile gelir;
/// Adım 2A'da varsayılan mtime'dır.
pub trait DateResolver: Send + Sync {
    fn resolve(&self, path: &Path, meta: &Metadata) -> CaptureTime;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MtimeResolver;

impl DateResolver for MtimeResolver {
    fn resolve(&self, _path: &Path, meta: &Metadata) -> CaptureTime {
        let utc_ms = meta.modified().map(loupe_core::time::system_time_ms).unwrap_or(0);
        CaptureTime { utc_ms, tz_offset_min: None, source: TakenAtSource::Mtime }
    }
}

/// Gün sayısından (1970-01-01'den) takvim tarihi. Howard Hinnant'ın algoritması;
/// proleptik Gregoryen, negatif günler dahil doğru.
#[must_use]
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, u32::try_from(m).unwrap_or(1), u32::try_from(d).unwrap_or(1))
}

/// `libraries.layout_template` → göreli dizin. Desteklenen: `{yyyy}`, `{mm}`, `{dd}`.
pub fn render_dir(template: &str, t: &CaptureTime) -> Result<RelPath, loupe_core::CoreError> {
    let (y, m, d) = t.local_date();
    let s = template
        .replace("{yyyy}", &format!("{y:04}"))
        .replace("{mm}", &format!("{m:02}"))
        .replace("{dd}", &format!("{d:02}"));
    RelPath::new(&s)
}

/// `n`. çakışma adayı: `IMG_1.JPG` → `IMG_1_1.JPG`, `IMG_1_2.JPG`…
#[must_use]
pub fn candidate_name(file_name: &str, n: u32) -> String {
    if n == 0 {
        return file_name.to_owned();
    }
    match file_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}_{n}.{ext}"),
        _ => format!("{file_name}_{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_385), (2025, 10, 24));
    }

    #[test]
    fn local_offset_changes_folder() {
        // 2025-10-24 22:30 UTC = 2025-10-25 01:30 İstanbul (+03:00)
        let utc_ms = 1_761_345_000_000;
        let t = CaptureTime { utc_ms, tz_offset_min: Some(180), source: TakenAtSource::Exif };
        let utc = CaptureTime { tz_offset_min: None, ..t };
        assert_eq!(render_dir("{yyyy}/{mm}/{dd}", &t).unwrap().as_str(), "2025/10/25");
        assert_eq!(render_dir("{yyyy}/{mm}/{dd}", &utc).unwrap().as_str(), "2025/10/24");
    }

    #[test]
    fn suffixes() {
        assert_eq!(candidate_name("IMG_1.JPG", 0), "IMG_1.JPG");
        assert_eq!(candidate_name("IMG_1.JPG", 2), "IMG_1_2.JPG");
        assert_eq!(candidate_name("README", 1), "README_1");
        assert_eq!(candidate_name(".hidden", 1), ".hidden_1");
    }
}
