//! Video: `ffprobe` ile süre, boyut, çekim zamanı ve konum; `ffmpeg` ile
//! küçük resim karesi. İkisi de sistemde kurulu araçlardır (Debian/Mint
//! paketi `ffmpeg`); yoksa video küçük resmi `MediaError::Tool` ile atlanır ve
//! metadata boş döner.
//!
//! Çekim zamanı sırası: `com.apple.quicktime.creationdate` (yerel saat ve
//! saat farkıyla) → kapsayıcının `creation_time` değeri (UTC). Konum
//! ISO 6709 (`+41.0082+028.9784+045.000/`) biçimindeki `location` etiketlerinden.

use std::path::Path;
use std::process::{Command, Stdio};

use loupe_core::TakenAtSource;
use serde_json::Value;

use crate::MediaError;
use crate::metadata::{GpsPosition, MediaMetadata, TakenAt};
use crate::util::civil_to_ms;

/// Kare videonun bu saniyesinden alınır (ilk kare çoğu zaman siyah); daha kısa
/// videoda ilk kare.
const FRAME_AT_S: &str = "1";
/// Kare en çok bu uzun kenarla çıkarılır (`lg` küçük resim boyu).
const FRAME_EDGE: u32 = 1920;

/// `ffmpeg` ve `ffprobe` çalışıyor mu?
#[must_use]
pub fn available() -> bool {
    ["ffmpeg", "ffprobe"].iter().all(|t| {
        Command::new(t)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// `ffprobe` çıktısından video bilgisi.
pub fn probe(path: &Path) -> Result<MediaMetadata, MediaError> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| MediaError::Tool { tool: "ffprobe", reason: e.to_string() })?;
    if !out.status.success() {
        return Err(MediaError::Decode {
            path: path.to_path_buf(),
            reason: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    let json: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| MediaError::Tool { tool: "ffprobe", reason: e.to_string() })?;
    Ok(from_probe(&json))
}

/// `ffprobe -print_format json -show_format -show_streams` çıktısını yorumlar.
#[must_use]
pub fn from_probe(json: &Value) -> MediaMetadata {
    let format = &json["format"];
    let tags = &format["tags"];
    let video = json["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|st| st["codec_type"] == "video" && st["disposition"]["attached_pic"] != 1));
    let mut meta = MediaMetadata::default();
    if let Some(v) = video {
        let w = v["width"].as_u64().and_then(|n| u32::try_from(n).ok());
        let h = v["height"].as_u64().and_then(|n| u32::try_from(n).ok());
        // Telefon videoları yatay kaydedilip döndürme bilgisi taşır; ekranda görünen boyut saklanır.
        let rotated = rotation(v).is_some_and(|r| r.rem_euclid(180) == 90);
        (meta.width, meta.height) = if rotated { (h, w) } else { (w, h) };
    }
    meta.duration_ms = format["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d >= 0.0)
        .map(|d| (d * 1000.0).round() as i64);
    let tag = |k: &str| tag_str(tags, k).or_else(|| video.and_then(|v| tag_str(&v["tags"], k)));
    meta.taken_at = tag("com.apple.quicktime.creationdate")
        .and_then(|s| parse_time(&s))
        .or_else(|| tag("creation_time").and_then(|s| parse_time(&s)));
    meta.gps = tag("com.apple.quicktime.location.ISO6709").or_else(|| tag("location")).and_then(|s| parse_iso6709(&s));
    meta.camera_make = tag("com.apple.quicktime.make").or_else(|| tag("com.android.manufacturer"));
    meta.camera_model = tag("com.apple.quicktime.model").or_else(|| tag("com.android.model"));
    meta.software = tag("com.apple.quicktime.software").or_else(|| tag("encoder"));
    meta
}

fn tag_str(tags: &Value, key: &str) -> Option<String> {
    tags.as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn rotation(stream: &Value) -> Option<i64> {
    let side = stream["side_data_list"].as_array().and_then(|l| l.iter().find_map(|d| d["rotation"].as_i64()));
    side.or_else(|| tag_str(&stream["tags"], "rotate").and_then(|s| s.parse().ok()))
}

/// `2025-08-14T13:23:45+0300`, `2025-08-14T10:23:45.000000Z`, `2025-08-14 10:23:45`.
fn parse_time(s: &str) -> Option<TakenAt> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<u32>().ok();
    let y = i64::from(num(0, 4)?);
    let (mo, d, h, mi, sec) = (num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 || y < 1971 {
        return None;
    }
    let local = civil_to_ms(y, mo, d, h, mi, sec, 0);
    let rest = s.get(19..).unwrap_or("");
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let (utc_ms, tz) = match rest {
        "" | "Z" | "z" => (local, None),
        r => {
            let sign = match r.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let digits: String = r[1..].chars().filter(char::is_ascii_digit).collect();
            let hh: i32 = digits.get(0..2)?.parse().ok()?;
            let mm: i32 = digits.get(2..4).and_then(|m| m.parse().ok()).unwrap_or(0);
            let off = sign * (hh * 60 + mm);
            (local - i64::from(off) * 60_000, Some(off))
        }
    };
    // `creation_time` UTC'dir; saat farkı bilinmiyorsa `None` (arayüz "saat dilimi bilinmiyor").
    Some(TakenAt { utc_ms, tz_offset_min: tz, source: TakenAtSource::Exif })
}

/// `+41.0082+028.9784+045.000/` → enlem, boylam, yükseklik.
fn parse_iso6709(s: &str) -> Option<GpsPosition> {
    let s = s.trim().trim_end_matches('/');
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices().skip(1) {
        if c == '+' || c == '-' {
            parts.push(&s[start..i]);
            start = i;
        }
    }
    parts.push(&s[start..]);
    let lat: f64 = parts.first()?.parse().ok()?;
    let lon: f64 = parts.get(1)?.parse().ok()?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) || (lat == 0.0 && lon == 0.0) {
        return None;
    }
    let alt_m = parts.get(2).and_then(|a| a.parse().ok());
    Some(GpsPosition { lat, lon, alt_m })
}

/// Videodan tek kare çıkarır (JPEG, ekrana göre döndürülmüş, uzun kenar ≤ 1920).
pub fn extract_frame(path: &Path, out: &Path) -> Result<(), MediaError> {
    let scale = format!("scale='if(gt(iw,ih),min({FRAME_EDGE},iw),-2)':'if(gt(iw,ih),-2,min({FRAME_EDGE},ih))'");
    let mut last = String::new();
    for at in [FRAME_AT_S, "0"] {
        let o = Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y", "-ss", at, "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-vf", &scale, "-q:v", "3", "-f", "image2", "-c:v", "mjpeg"])
            .arg(out)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| MediaError::Tool { tool: "ffmpeg", reason: e.to_string() })?;
        if o.status.success() && out.metadata().is_ok_and(|m| m.len() > 0) {
            return Ok(());
        }
        last = String::from_utf8_lossy(&o.stderr).trim().to_owned();
    }
    Err(MediaError::Decode {
        path: path.to_path_buf(),
        reason: if last.is_empty() { "kare çıkmadı".into() } else { last },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn iphone_probe_gives_display_size_local_time_and_place() {
        let j = json!({
            "streams": [
                {"codec_type": "video", "width": 1920, "height": 1080, "side_data_list": [{"rotation": -90}]},
                {"codec_type": "audio"}
            ],
            "format": {"duration": "12.345", "tags": {
                "creation_time": "2025-08-14T10:23:45.000000Z",
                "com.apple.quicktime.creationdate": "2025-08-14T13:23:45+0300",
                "com.apple.quicktime.location.ISO6709": "+41.0082+028.9784+045.000/",
                "com.apple.quicktime.make": "Apple",
                "com.apple.quicktime.model": "iPhone 15 Pro"
            }}
        });
        let m = from_probe(&j);
        assert_eq!((m.width, m.height), (Some(1080), Some(1920)));
        assert_eq!(m.duration_ms, Some(12_345));
        let t = m.taken_at.unwrap();
        assert_eq!(t.tz_offset_min, Some(180));
        assert_eq!(t.utc_ms, civil_to_ms(2025, 8, 14, 10, 23, 45, 0));
        let g = m.gps.unwrap();
        assert!((g.lat - 41.0082).abs() < 1e-9 && (g.lon - 28.9784).abs() < 1e-9);
        assert_eq!(g.alt_m, Some(45.0));
        assert_eq!(m.camera_model.as_deref(), Some("iPhone 15 Pro"));
    }

    #[test]
    fn android_probe_uses_utc_creation_time() {
        let j = json!({
            "streams": [{"codec_type": "video", "width": 1280, "height": 720, "tags": {"rotate": "0"}}],
            "format": {"duration": "3.0", "tags": {"creation_time": "2024-01-02T03:04:05.000000Z", "location": "-33.8688+151.2093/"}}
        });
        let m = from_probe(&j);
        assert_eq!((m.width, m.height), (Some(1280), Some(720)));
        let t = m.taken_at.unwrap();
        assert_eq!((t.utc_ms, t.tz_offset_min), (civil_to_ms(2024, 1, 2, 3, 4, 5, 0), None));
        assert!((m.gps.unwrap().lat + 33.8688).abs() < 1e-9);
        assert!(parse_time("0000-00-00 00:00:00").is_none());
        assert!(parse_iso6709("+00.0000+000.0000/").is_none());
    }
}
