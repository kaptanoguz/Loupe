//! Sağ denetçi paneli: tek medyanın EXIF, konum, dosya, hash ve yedekleme
//! durumu; küçük resimden ton dağılımı (histogram).

use loupe_core::{Blake3Hash, MediaId};
use loupe_media::{ThumbCache, ThumbSize, format_dms};
use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::library::Libraries;
use crate::{AppError, AppResult};

/// Histogram kutu sayısı (kanal başına).
pub const HISTOGRAM_BINS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Histogram {
    pub bins: usize,
    pub luma: Vec<u32>,
    pub red: Vec<u32>,
    pub green: Vec<u32>,
    pub blue: Vec<u32>,
    /// Tam siyah (0) ve tam beyaz (255) pikseller, binde: kırpılma uyarısı.
    pub shadows_clipped_permille: u32,
    pub highlights_clipped_permille: u32,
    /// Hangi küçük resimden hesaplandı (`sm` | `md`).
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpsDto {
    pub lat: f64,
    pub lon: f64,
    pub alt_m: Option<f64>,
    /// `38° 38' 34.2" N, 034° 49' 43.9" E`
    pub dms: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceDto {
    /// `Göreme, Nevşehir, Türkiye`
    pub display: String,
    pub locality: Option<String>,
    pub admin2: Option<String>,
    pub admin1: Option<String>,
    pub country: String,
    pub country_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTargetDto {
    pub target: String,
    pub provider: String,
    /// `local_only` | `queued` | `uploading` | `synced` | `conflict` | `error` | `remote_deleted`
    pub state: String,
    /// `none` | `age`
    pub encryption: String,
    pub key_fingerprint: Option<String>,
    pub remote_checksum_algo: Option<String>,
    pub uploaded_at: Option<i64>,
    pub last_verified_at: Option<i64>,
    pub last_error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectorDetail {
    pub id: i64,
    pub hash: String,
    pub file_name: String,
    pub original_filename: String,
    pub rel_path: String,
    /// Disk bağlıysa mutlak yol.
    pub path: Option<String>,
    pub library_name: String,
    pub library_online: bool,
    pub file_type: String,
    pub raw_format: Option<String>,
    pub mime: String,
    pub size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub megapixels: Option<f64>,
    pub taken_at_utc: i64,
    pub tz_offset_min: Option<i32>,
    /// `2026-10-24 14:32:10 +03:00` (saat dilimi bilinmiyorsa UTC ve `Z`).
    pub taken_local: String,
    /// `exif` | `gps` | `filename` | `mtime` | `user`
    pub taken_source: String,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    /// `35 mm`
    pub focal_length: Option<String>,
    /// `f/2.8`
    pub aperture: Option<String>,
    /// `1/500 s`
    pub exposure: Option<String>,
    pub iso: Option<u32>,
    pub gps: Option<GpsDto>,
    pub place: Option<PlaceDto>,
    pub rating: u8,
    pub label: Option<String>,
    pub favorite: bool,
    pub flag: String,
    pub sidecar_dirty: bool,
    pub verify_state: String,
    pub last_verified_at: i64,
    pub imported_at: i64,
    pub placement: String,
    pub sync: Vec<SyncTargetDto>,
    pub histogram: Option<Histogram>,
}

fn trim_num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

/// `f/2.8`, `f/11`
#[must_use]
pub fn format_aperture(f: f64) -> Option<String> {
    (f.is_finite() && f > 0.0).then(|| format!("f/{}", trim_num(f, 1)))
}

/// `1/500 s`, `0.5 s`, `2 s`, `1/3 s`
#[must_use]
pub fn format_exposure(s: f64) -> Option<String> {
    let meta = loupe_media::MediaMetadata { exposure_s: Some(s), ..Default::default() };
    meta.exposure_display()
}

/// `2026-10-24 14:32:10 +03:00`
#[must_use]
pub fn format_local(utc_ms: i64, tz_offset_min: Option<i32>) -> String {
    let off = tz_offset_min.unwrap_or(0);
    let local_s = (utc_ms + i64::from(off) * 60_000).div_euclid(1000);
    let (y, mo, d) = loupe_storage::layout::civil_from_days(local_s.div_euclid(86_400));
    let sod = local_s.rem_euclid(86_400);
    let tz = match tz_offset_min {
        None => "Z".to_owned(),
        Some(o) => format!("{}{:02}:{:02}", if o < 0 { '-' } else { '+' }, o.abs() / 60, o.abs() % 60),
    };
    format!("{y:04}-{mo:02}-{d:02} {:02}:{:02}:{:02} {tz}", sod / 3600, sod % 3600 / 60, sod % 60)
}

/// Küçük resmin piksellerinden kanal ve parlaklık dağılımı.
pub fn histogram(cache: &ThumbCache, hash: &Blake3Hash) -> Option<Histogram> {
    let (path, size) =
        [ThumbSize::Sm, ThumbSize::Md].into_iter().find_map(|s| cache.lookup(hash, s).map(|(p, _)| (p, s)))?;
    let img = image::open(&path).ok()?.to_rgb8();
    let mut h = Histogram {
        bins: HISTOGRAM_BINS,
        luma: vec![0; HISTOGRAM_BINS],
        red: vec![0; HISTOGRAM_BINS],
        green: vec![0; HISTOGRAM_BINS],
        blue: vec![0; HISTOGRAM_BINS],
        shadows_clipped_permille: 0,
        highlights_clipped_permille: 0,
        source: size.as_str(),
    };
    let bin = |v: u8| usize::from(v) * HISTOGRAM_BINS / 256;
    let (mut dark, mut bright, mut total) = (0u64, 0u64, 0u64);
    for p in img.pixels() {
        let [r, g, b] = p.0;
        // Rec. 709 parlaklığı, tamsayı yaklaşımı.
        let y = ((u32::from(r) * 2126 + u32::from(g) * 7152 + u32::from(b) * 722) / 10_000) as u8;
        h.red[bin(r)] += 1;
        h.green[bin(g)] += 1;
        h.blue[bin(b)] += 1;
        h.luma[bin(y)] += 1;
        dark += u64::from(r == 0 && g == 0 && b == 0);
        bright += u64::from(r == 255 && g == 255 && b == 255);
        total += 1;
    }
    let permille = |n: u64| (n * 1000).checked_div(total).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
    h.shadows_clipped_permille = permille(dark);
    h.highlights_clipped_permille = permille(bright);
    Some(h)
}

pub fn detail(libs: &Libraries, pool: &loupe_db::Pool, cache: &ThumbCache, id: MediaId) -> AppResult<InspectorDetail> {
    let roots = libs.roots()?;
    let mut d = pool
        .read(|c| {
            let row = c
                .prepare_cached(
                    "SELECT m.blake3, m.rel_path, m.original_filename, m.file_type, m.raw_format, m.mime,
                            m.size_bytes, m.width, m.height, m.orientation, m.taken_at_utc, m.tz_offset_min,
                            m.taken_at_source, m.camera_make, m.camera_model, m.lens_model, m.focal_length_mm,
                            m.aperture_f, m.exposure_s, m.iso, m.gps_lat, m.gps_lon, m.gps_alt_m,
                            m.rating, m.color_label, m.is_favorite, m.flag, m.sidecar_dirty, m.verify_state,
                            m.last_verified_at, m.imported_at, m.placement, m.library_id, lb.name,
                            l.locality, l.admin2, l.admin1, l.country, l.country_code, l.user_label
                       FROM media_files m
                       JOIN libraries lb ON lb.id = m.library_id
                       LEFT JOIN locations l ON l.id = m.location_id
                      WHERE m.id = ?1",
                )?
                .query_row([id.0], |r| {
                    let blake: Vec<u8> = r.get(0)?;
                    let rel: String = r.get(1)?;
                    let (w, h): (Option<u32>, Option<u32>) = (r.get(7)?, r.get(8)?);
                    let orientation: Option<u8> = r.get(9)?;
                    let (w, h) = if matches!(orientation, Some(5..=8)) { (h, w) } else { (w, h) };
                    let lat: Option<f64> = r.get(20)?;
                    let lon: Option<f64> = r.get(21)?;
                    let country: Option<String> = r.get(37)?;
                    let place = match country {
                        Some(country) => {
                            let (loc, a2, a1): (Option<String>, Option<String>, Option<String>) =
                                (r.get(34)?, r.get(35)?, r.get(36)?);
                            let user: Option<String> = r.get(39)?;
                            let mut parts: Vec<String> = Vec::new();
                            for p in
                                [user.clone().or_else(|| loc.clone()), a2.clone(), a1.clone()].into_iter().flatten()
                            {
                                if !parts.contains(&p) {
                                    parts.push(p);
                                }
                            }
                            parts.push(country.clone());
                            Some(PlaceDto {
                                display: parts.join(", "),
                                locality: user.or(loc),
                                admin2: a2,
                                admin1: a1,
                                country,
                                country_code: r.get(38)?,
                            })
                        }
                        None => None,
                    };
                    let focal: Option<f64> = r.get(16)?;
                    let aperture: Option<f64> = r.get(17)?;
                    let exposure: Option<f64> = r.get(18)?;
                    let taken: i64 = r.get(10)?;
                    let tz: Option<i32> = r.get(11)?;
                    let library_id: i64 = r.get(32)?;
                    Ok((
                        InspectorDetail {
                            id: id.0,
                            hash: blake.iter().map(|b| format!("{b:02x}")).collect(),
                            file_name: rel.rsplit('/').next().unwrap_or(&rel).to_owned(),
                            original_filename: r.get(2)?,
                            rel_path: rel,
                            path: None,
                            library_name: r.get(33)?,
                            library_online: false,
                            file_type: r.get(3)?,
                            raw_format: r.get(4)?,
                            mime: r.get(5)?,
                            size_bytes: u64::try_from(r.get::<_, i64>(6)?).unwrap_or(0),
                            width: w,
                            height: h,
                            megapixels: w.zip(h).map(|(w, h)| (f64::from(w) * f64::from(h) / 1e5).round() / 10.0),
                            taken_at_utc: taken,
                            tz_offset_min: tz,
                            taken_local: format_local(taken, tz),
                            taken_source: r.get(12)?,
                            camera_make: r.get(13)?,
                            camera_model: r.get(14)?,
                            lens_model: r.get(15)?,
                            focal_length: focal.filter(|f| *f > 0.0).map(|f| format!("{} mm", trim_num(f, 1))),
                            aperture: aperture.and_then(format_aperture),
                            exposure: exposure.and_then(format_exposure),
                            iso: r.get(19)?,
                            gps: lat.zip(lon).map(|(lat, lon)| GpsDto {
                                lat,
                                lon,
                                alt_m: None,
                                dms: format_dms(lat, lon).unwrap_or_default(),
                            }),
                            place,
                            rating: r.get(23)?,
                            label: r.get(24)?,
                            favorite: r.get(25)?,
                            flag: r.get(26)?,
                            sidecar_dirty: r.get(27)?,
                            verify_state: r.get(28)?,
                            last_verified_at: r.get(29)?,
                            imported_at: r.get(30)?,
                            placement: r.get(31)?,
                            sync: Vec::new(),
                            histogram: None,
                        },
                        library_id,
                        r.get::<_, Option<f64>>(22)?,
                    ))
                })
                .optional()?;
            let Some((mut d, lib, alt)) = row else { return Ok(None) };
            if let Some(g) = d.gps.as_mut() {
                g.alt_m = alt;
            }
            d.sync = c
                .prepare_cached(
                    "SELECT t.name, t.provider, s.state, s.encryption, s.key_fingerprint, s.remote_checksum_algo,
                            s.uploaded_at, s.last_verified_at, s.last_error_code
                       FROM sync_status s JOIN cloud_targets t ON t.id = s.target_id
                      WHERE s.media_id = ?1 ORDER BY t.name",
                )?
                .query_map([id.0], |r| {
                    Ok(SyncTargetDto {
                        target: r.get(0)?,
                        provider: r.get(1)?,
                        state: r.get(2)?,
                        encryption: r.get(3)?,
                        key_fingerprint: r.get(4)?,
                        remote_checksum_algo: r.get(5)?,
                        uploaded_at: r.get(6)?,
                        last_verified_at: r.get(7)?,
                        last_error_code: r.get(8)?,
                    })
                })?
                .collect::<Result<_, _>>()?;
            Ok(Some((d, lib)))
        })?
        .map(|(mut d, lib)| {
            if let Some(root) = roots.get(&loupe_core::LibraryId(lib)) {
                d.library_online = true;
                d.path = Some(root.join(&d.rel_path).display().to_string());
            }
            d
        })
        .ok_or(AppError::NotFound { entity: "medya", id: id.0 })?;
    if let Ok(hash) = Blake3Hash::from_hex(&d.hash) {
        d.histogram = histogram(cache, &hash);
    }
    Ok(d)
}
