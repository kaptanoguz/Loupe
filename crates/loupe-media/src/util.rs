//! Küçük yardımcılar: takvim hesabı ve atomik dosya yazma.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use crate::MediaError;

/// Proleptik Gregoryen tarih → 1970-01-01'den gün sayısı (Howard Hinnant).
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Yerel takvim zamanı → epoch ms (saat farkı uygulanmadan).
pub(crate) fn civil_to_ms(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32, ms: u32) -> i64 {
    days_from_civil(y, mo, d) * 86_400_000
        + i64::from(h) * 3_600_000
        + i64::from(mi) * 60_000
        + i64::from(s) * 1000
        + i64::from(ms)
}

/// Geçici dosya → fsync → rename → dizin fsync. Okuyucu yarım dosya görmez.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), MediaError> {
    let dir = path.parent().ok_or_else(|| MediaError::Io {
        op: "atomik yazma",
        path: path.to_path_buf(),
        source: std::io::Error::from(std::io::ErrorKind::InvalidInput),
    })?;
    fs::create_dir_all(dir).map_err(MediaError::io("dizin oluşturma", dir))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("dosya");
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let res = (|| {
        let mut f = File::create(&tmp).map_err(MediaError::io("geçici dosya", &tmp))?;
        f.write_all(bytes).map_err(MediaError::io("yazma", &tmp))?;
        f.sync_all().map_err(MediaError::io("fsync", &tmp))?;
        drop(f);
        fs::rename(&tmp, path).map_err(MediaError::io("rename", path))?;
        fsync_dir(dir)
    })();
    if res.is_err() {
        let _ignored = fs::remove_file(&tmp);
    }
    res
}

/// Windows'ta dizin `File` olarak açılamaz (os error 5); NTFS üst veriyi kendi
/// günlüğüyle korur, bu yüzden yalnız Unix'te fsync edilir.
fn fsync_dir(dir: &Path) -> Result<(), MediaError> {
    #[cfg(unix)]
    {
        File::open(dir).and_then(|d| d.sync_all()).map_err(MediaError::io("dizin fsync", dir))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(civil_to_ms(2025, 10, 24, 22, 30, 0, 0), 1_761_345_000_000);
    }
}
