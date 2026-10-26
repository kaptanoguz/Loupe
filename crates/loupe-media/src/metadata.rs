//! EXIF / metadata çıkarıcı.
//!
//! `kamadak-exif` (saf Rust) JPEG, PNG, WebP, HEIF/HEIC ve TIFF kabını okur;
//! CR2, NEF, ARW ve DNG TIFF tabanlı olduğu için aynı yoldan açılır.
//!
//! Çekim zamanı: `DateTimeOriginal` yerel saattir. Saat farkı sırasıyla
//! `OffsetTimeOriginal` → `OffsetTime` → GPS UTC zaman damgasıyla farktan
//! (15 dakikaya yuvarlanmış) bulunur; hiçbiri yoksa `tz_offset_min = None` ve
//! yerel saat UTC kabul edilir (arayüz "saat dilimi bilinmiyor" gösterir).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use exif::{Exif, In, Tag, Value};
use loupe_core::{MediaFileType, TakenAtSource, classify};

use crate::MediaError;
use crate::util::civil_to_ms;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TakenAt {
    pub utc_ms: i64,
    pub tz_offset_min: Option<i32>,
    pub source: TakenAtSource,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsPosition {
    /// Ondalık derece, kuzey pozitif.
    pub lat: f64,
    /// Ondalık derece, doğu pozitif.
    pub lon: f64,
    /// Deniz seviyesine göre metre (deniz altı negatif).
    pub alt_m: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaMetadata {
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub focal_length_mm: Option<f64>,
    pub aperture_f: Option<f64>,
    pub exposure_s: Option<f64>,
    pub iso: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// EXIF Orientation 1–8.
    pub orientation: Option<u8>,
    pub taken_at: Option<TakenAt>,
    pub gps: Option<GpsPosition>,
    pub software: Option<String>,
    /// Video süresi.
    pub duration_ms: Option<i64>,
}

impl MediaMetadata {
    /// Arayüz için enstantane: `1/250 s`, `0.5 s`, `2 s`.
    #[must_use]
    pub fn exposure_display(&self) -> Option<String> {
        let s = self.exposure_s?;
        if s <= 0.0 {
            return None;
        }
        if s < 0.5 {
            Some(format!("1/{} s", (1.0 / s).round()))
        } else if (s - s.round()).abs() < 1e-9 {
            Some(format!("{} s", s.round()))
        } else {
            Some(format!("{s:.1} s"))
        }
    }
}

/// Dosyadan metadata okur. EXIF içermeyen dosya hata değildir: boş metadata
/// (ve çözülebilen biçimlerde başlıktan okunan boyutlar) döner.
pub fn read_metadata(path: &Path) -> Result<MediaMetadata, MediaError> {
    if classify(path).file_type == MediaFileType::Video {
        return crate::video::probe(path);
    }
    let file = File::open(path).map_err(MediaError::io("okuma için açma", path))?;
    let mut reader = BufReader::new(file);
    let parsed = exif::Reader::new()
        .continue_on_error(true)
        .read_from_container(&mut reader)
        .or_else(|e| e.distill_partial_result(|_errors| {}));
    let mut meta = match parsed {
        Ok(ex) => from_exif(&ex),
        Err(exif::Error::NotFound(_)) => MediaMetadata::default(),
        Err(exif::Error::InvalidFormat(reason)) if !is_exif_container(path) => {
            return Err(MediaError::Unsupported { path: path.to_path_buf(), format: reason.to_owned() });
        }
        Err(e) => return Err(MediaError::Exif { path: path.to_path_buf(), reason: e.to_string() }),
    };
    if meta.width.is_none()
        && matches!(classify(path).file_type, MediaFileType::Jpeg | MediaFileType::Png | MediaFileType::Webp)
        && let Ok((w, h)) = image::image_dimensions(path)
    {
        meta.width = Some(w);
        meta.height = Some(h);
    }
    Ok(meta)
}

fn is_exif_container(path: &Path) -> bool {
    !matches!(classify(path).file_type, MediaFileType::Other | MediaFileType::Video)
}

fn ascii(ex: &Exif, tag: Tag) -> Option<String> {
    let f = ex.get_field(tag, In::PRIMARY)?;
    match &f.value {
        Value::Ascii(parts) => {
            let s = String::from_utf8_lossy(parts.first()?)
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_owned();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    }
}

fn rational(ex: &Exif, tag: Tag, ifd: In) -> Option<f64> {
    match &ex.get_field(tag, ifd)?.value {
        Value::Rational(v) => v.first().filter(|r| r.denom != 0).map(exif::Rational::to_f64),
        Value::SRational(v) => v.first().filter(|r| r.denom != 0).map(exif::SRational::to_f64),
        _ => None,
    }
}

fn uint(ex: &Exif, tag: Tag) -> Option<u32> {
    ex.get_field(tag, In::PRIMARY)?.value.get_uint(0)
}

fn rationals3(ex: &Exif, tag: Tag) -> Option<[f64; 3]> {
    match &ex.get_field(tag, In::PRIMARY)?.value {
        Value::Rational(v) if v.len() >= 3 && v.iter().take(3).all(|r| r.denom != 0) => {
            Some([v[0].to_f64(), v[1].to_f64(), v[2].to_f64()])
        }
        _ => None,
    }
}

#[must_use]
pub fn from_exif(ex: &Exif) -> MediaMetadata {
    let gps = gps_position(ex);
    MediaMetadata {
        camera_make: ascii(ex, Tag::Make),
        camera_model: ascii(ex, Tag::Model),
        lens_model: ascii(ex, Tag::LensModel),
        focal_length_mm: rational(ex, Tag::FocalLength, In::PRIMARY),
        aperture_f: rational(ex, Tag::FNumber, In::PRIMARY),
        exposure_s: rational(ex, Tag::ExposureTime, In::PRIMARY),
        iso: uint(ex, Tag::PhotographicSensitivity),
        width: uint(ex, Tag::PixelXDimension).or_else(|| uint(ex, Tag::ImageWidth)),
        height: uint(ex, Tag::PixelYDimension).or_else(|| uint(ex, Tag::ImageLength)),
        orientation: uint(ex, Tag::Orientation).and_then(|o| u8::try_from(o).ok()).filter(|o| (1..=8).contains(o)),
        taken_at: taken_at(ex),
        gps,
        software: ascii(ex, Tag::Software),
        duration_ms: None,
    }
}

/// `YYYY:MM:DD HH:MM:SS` → (yıl, ay, gün, saat, dakika, saniye)
fn parse_exif_datetime(s: &str) -> Option<(i64, u32, u32, u32, u32, u32)> {
    let s = s.trim();
    let (date, time) = s.split_once([' ', 'T'])?;
    let mut d = date.split([':', '-']);
    let y = d.next()?.parse().ok()?;
    let mo = d.next()?.parse().ok()?;
    let da = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let h = t.next()?.parse().ok()?;
    let mi = t.next()?.parse().ok()?;
    let se = t.next().and_then(|x| x.get(..2)).unwrap_or("0").parse().ok()?;
    let valid = (1..=12).contains(&mo) && (1..=31).contains(&da) && h < 24 && mi < 60 && se < 61 && y > 1800;
    valid.then_some((y, mo, da, h, mi, se))
}

/// `+03:00`, `-05:30`, `Z` → dakika
fn parse_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    if s == "Z" {
        return Some(0);
    }
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let (h, m) = rest.split_once(':').unwrap_or((rest.get(..2)?, rest.get(2..).unwrap_or("0")));
    let minutes = h.parse::<i32>().ok()? * 60 + m.parse::<i32>().ok()?;
    (minutes <= 14 * 60).then_some(sign * minutes)
}

fn taken_at(ex: &Exif) -> Option<TakenAt> {
    let raw = ascii(ex, Tag::DateTimeOriginal).or_else(|| ascii(ex, Tag::DateTime))?;
    let (y, mo, d, h, mi, s) = parse_exif_datetime(&raw)?;
    let millis = ascii(ex, Tag::SubSecTimeOriginal)
        .and_then(|ss| format!("{ss:0<3}").get(..3).and_then(|x| x.parse::<u32>().ok()))
        .unwrap_or(0);
    let local_ms = civil_to_ms(y, mo, d, h, mi, s, millis);

    let explicit =
        ascii(ex, Tag::OffsetTimeOriginal).or_else(|| ascii(ex, Tag::OffsetTime)).and_then(|o| parse_offset(&o));
    let offset = explicit.or_else(|| gps_utc_ms(ex).and_then(|utc| offset_from_gps(local_ms, utc)));
    let utc_ms = local_ms - i64::from(offset.unwrap_or(0)) * 60_000;
    Some(TakenAt { utc_ms, tz_offset_min: offset, source: TakenAtSource::Exif })
}

/// GPSDateStamp + GPSTimeStamp → UTC epoch ms.
fn gps_utc_ms(ex: &Exif) -> Option<i64> {
    let date = ascii(ex, Tag::GPSDateStamp)?;
    let [h, m, s] = match &ex.get_field(Tag::GPSTimeStamp, In::PRIMARY)?.value {
        Value::Rational(v) if v.len() >= 3 && v.iter().take(3).all(|r| r.denom != 0) => {
            [v[0].to_f64(), v[1].to_f64(), v[2].to_f64()]
        }
        _ => return None,
    };
    let mut d = date.split([':', '-']);
    let (y, mo, da) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let secs = h * 3600.0 + m * 60.0 + s;
    Some(civil_to_ms(y, mo, da, 0, 0, 0, 0) + (secs * 1000.0).round() as i64)
}

/// Yerel saat ile GPS UTC farkı; 15 dakikaya yuvarlanır, ±14 saat dışı reddedilir.
fn offset_from_gps(local_ms: i64, utc_ms: i64) -> Option<i32> {
    let diff_min = (local_ms - utc_ms) as f64 / 60_000.0;
    let rounded = (diff_min / 15.0).round() * 15.0;
    ((rounded.abs() <= 14.0 * 60.0) && (diff_min - rounded).abs() <= 5.0).then_some(rounded as i32)
}

fn gps_position(ex: &Exif) -> Option<GpsPosition> {
    let lat = rationals3(ex, Tag::GPSLatitude)?;
    let lon = rationals3(ex, Tag::GPSLongitude)?;
    let lat_ref = ascii(ex, Tag::GPSLatitudeRef).unwrap_or_else(|| "N".into());
    let lon_ref = ascii(ex, Tag::GPSLongitudeRef).unwrap_or_else(|| "E".into());
    let to_dec = |v: [f64; 3]| v[0] + v[1] / 60.0 + v[2] / 3600.0;
    let mut la = to_dec(lat);
    let mut lo = to_dec(lon);
    if lat_ref.eq_ignore_ascii_case("S") {
        la = -la;
    }
    if lon_ref.eq_ignore_ascii_case("W") {
        lo = -lo;
    }
    if !(-90.0..=90.0).contains(&la) || !(-180.0..=180.0).contains(&lo) || (la == 0.0 && lo == 0.0) {
        return None; // 0,0 "Null Island": GPS kilidi olmayan cihazların yazdığı boş değer
    }
    let alt = rational(ex, Tag::GPSAltitude, In::PRIMARY).map(|a| {
        let below = ex.get_field(Tag::GPSAltitudeRef, In::PRIMARY).and_then(|f| f.value.get_uint(0)) == Some(1);
        if below { -a } else { a }
    });
    Some(GpsPosition { lat: la, lon: lo, alt_m: alt })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_and_offset_parsing() {
        assert_eq!(parse_exif_datetime("2025:10:24 22:30:05"), Some((2025, 10, 24, 22, 30, 5)));
        assert_eq!(parse_exif_datetime("0000:00:00 00:00:00"), None);
        assert_eq!(parse_offset("+03:00"), Some(180));
        assert_eq!(parse_offset("-05:30"), Some(-330));
        assert_eq!(parse_offset("+15:00"), None);
        assert_eq!(offset_from_gps(10_800_000 + 4_000, 0), Some(180));
        assert_eq!(offset_from_gps(10_440_000, 0), None, "15 dakikaya oturmayan fark saat ayarsızlığıdır");
    }

    #[test]
    fn exposure_text() {
        let m = |s| MediaMetadata { exposure_s: Some(s), ..Default::default() };
        assert_eq!(m(0.004).exposure_display().as_deref(), Some("1/250 s"));
        assert_eq!(m(2.0).exposure_display().as_deref(), Some("2 s"));
        assert_eq!(m(0.8).exposure_display().as_deref(), Some("0.8 s"));
    }
}
