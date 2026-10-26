//! Tahribatsız basit düzenleme: 90° döndürme, düzleştirme, kırpma ve üç ton
//! ayarı (parlaklık, kontrast, sıcaklık). Orijinal dosyaya asla yazılmaz;
//! tarif katalogda (`versions` + `edits`) ve `.loupe.json`'da durur, görüntü
//! her istekte tariften yeniden üretilir.
//!
//! Sıra (arayüzdeki canvas önizlemesi `apps/desktop/src/lib/edit/recipe.ts`
//! ile birebir aynı):
//! 1. EXIF yönü uygulanmış kaynak `quarter` kez saat yönünde 90° döndürülür → W×H "çerçeve".
//! 2. Düzleştirme: görüntü merkez etrafında `straighten` derece döner ve köşelerde
//!    boşluk kalmayacak kadar büyütülür ([`cover_scale`]); çerçeve W×H kalır.
//! 3. Kırpma: çerçeve içinde 0–1 normalleştirilmiş dikdörtgen.
//! 4. Ton: kanal başına 256 girişli tablo ([`luts`]): beyaz dengesi kazancı →
//!    parlaklık (gama) → kontrast (0.5 etrafında eğim).

use std::io::Cursor;
use std::path::Path;

use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, Rgb, RgbImage};
use loupe_core::MediaFileType;
use serde::{Deserialize, Serialize};

use crate::MediaError;

/// Tarif biçiminin sürümü (`edits.engine_version`): formül değişirse artar,
/// eski düzenlemeler eski formülle aynı görünmeye devam eder.
pub const ENGINE_VERSION: i64 = 1;

/// Düzleştirme sınırı (derece).
pub const MAX_STRAIGHTEN: f32 = 45.0;
/// Kırpma dikdörtgeninin en küçük kenarı (çerçevenin oranı).
pub const MIN_CROP: f32 = 0.01;

/// Çerçeve içinde normalleştirilmiş kırpma (sol üst köşe ve boyut, 0–1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Crop {
    pub const FULL: Self = Self { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    fn is_full(&self) -> bool {
        self.x <= 1e-4 && self.y <= 1e-4 && self.w >= 1.0 - 1e-4 && self.h >= 1.0 - 1e-4
    }
}

/// Bir fotoğrafın düzenleme tarifi. Varsayılan değer = düzenlenmemiş.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Recipe {
    /// Saat yönünde 90° dönüş sayısı (0–3).
    pub quarter: u8,
    /// Derece, −45…45; artı saat yönü.
    pub straighten: f32,
    pub crop: Option<Crop>,
    /// −100…100
    pub brightness: f32,
    /// −100…100
    pub contrast: f32,
    /// −100…100; artı sıcak (sarı), eksi soğuk (mavi).
    pub temperature: f32,
}

/// Tek tıkla iyileştirmenin önerdiği ton değerleri.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AutoTone {
    pub brightness: f32,
    pub contrast: f32,
    pub temperature: f32,
}

fn finite(name: &str, v: f32) -> Result<f32, String> {
    if v.is_finite() { Ok(v) } else { Err(format!("{name} sayı değil")) }
}

impl Recipe {
    /// Sınırları uygular, tam çerçeve kırpmayı ve sıfıra yakın değerleri atar.
    /// Sayı olmayan değer ya da çerçeve dışına taşan kırpma hatadır.
    pub fn normalized(self) -> Result<Self, String> {
        let slider = |name: &str, v: f32| finite(name, v).map(|v| round2(v.clamp(-100.0, 100.0)));
        let crop = match self.crop {
            None => None,
            Some(c) => {
                let (x, y, w, h) =
                    (finite("crop.x", c.x)?, finite("crop.y", c.y)?, finite("crop.w", c.w)?, finite("crop.h", c.h)?);
                if w < MIN_CROP || h < MIN_CROP || x < -1e-4 || y < -1e-4 || x + w > 1.0 + 1e-4 || y + h > 1.0 + 1e-4 {
                    return Err(format!("kırpma çerçevenin dışında: {x}, {y}, {w}×{h}"));
                }
                let x = x.clamp(0.0, 1.0 - MIN_CROP);
                let y = y.clamp(0.0, 1.0 - MIN_CROP);
                let c = Crop { x, y, w: w.min(1.0 - x), h: h.min(1.0 - y) };
                (!c.is_full()).then_some(c)
            }
        };
        let straighten = round2(finite("straighten", self.straighten)?.clamp(-MAX_STRAIGHTEN, MAX_STRAIGHTEN));
        Ok(Self {
            quarter: self.quarter % 4,
            straighten: if straighten.abs() < 0.01 { 0.0 } else { straighten },
            crop,
            brightness: slider("brightness", self.brightness)?,
            contrast: slider("contrast", self.contrast)?,
            temperature: slider("temperature", self.temperature)?,
        })
    }

    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    #[must_use]
    pub fn has_tone(&self) -> bool {
        self.brightness != 0.0 || self.contrast != 0.0 || self.temperature != 0.0
    }

    /// Düzenlenmiş görüntünün en/boy oranı; `base` EXIF yönü uygulanmış orijinalinki.
    #[must_use]
    pub fn aspect(&self, base: f32) -> f32 {
        let a = if self.quarter % 2 == 1 { 1.0 / base } else { base };
        let a = match self.crop {
            Some(c) => a * c.w / c.h,
            None => a,
        };
        a.clamp(0.1, 10.0)
    }

    /// Katalog satırları (`edits.op`, `edits.params`), uygulama sırasıyla.
    #[must_use]
    pub fn to_ops(&self) -> Vec<(&'static str, serde_json::Value)> {
        let mut ops = Vec::new();
        if self.quarter != 0 {
            ops.push(("rotate", serde_json::json!({ "quarter": self.quarter })));
        }
        if self.straighten != 0.0 {
            ops.push(("straighten", serde_json::json!({ "angle": self.straighten })));
        }
        if let Some(c) = self.crop {
            ops.push(("crop", serde_json::json!({ "x": c.x, "y": c.y, "w": c.w, "h": c.h })));
        }
        if self.temperature != 0.0 {
            ops.push(("white_balance", serde_json::json!({ "temperature": self.temperature })));
        }
        if self.brightness != 0.0 || self.contrast != 0.0 {
            ops.push(("exposure", serde_json::json!({ "brightness": self.brightness, "contrast": self.contrast })));
        }
        ops
    }

    /// [`Recipe::to_ops`]'un tersi. Bilinmeyen işlemler (ileride gelecek
    /// araçlar ya da başka sürüm) yok sayılır.
    #[must_use]
    pub fn from_ops<'a>(ops: impl IntoIterator<Item = (&'a str, &'a serde_json::Value)>) -> Self {
        let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
        let mut r = Self::default();
        for (op, p) in ops {
            match op {
                "rotate" => r.quarter = (num(p, "quarter").round().rem_euclid(4.0)) as u8,
                "straighten" => r.straighten = num(p, "angle"),
                "crop" => r.crop = Some(Crop { x: num(p, "x"), y: num(p, "y"), w: num(p, "w"), h: num(p, "h") }),
                "white_balance" => r.temperature = num(p, "temperature"),
                "exposure" => {
                    r.brightness = num(p, "brightness");
                    r.contrast = num(p, "contrast");
                }
                _ => {}
            }
        }
        r
    }

    /// `.loupe.json` içindeki `edits` dizisi (başka araçlar da okuyabilsin diye açık işlem listesi).
    #[must_use]
    pub fn to_sidecar(&self) -> Option<serde_json::Value> {
        if self.is_identity() {
            return None;
        }
        let ops: Vec<serde_json::Value> = self
            .to_ops()
            .into_iter()
            .map(|(op, mut p)| {
                if let Some(o) = p.as_object_mut() {
                    o.insert("op".into(), op.into());
                }
                p
            })
            .collect();
        Some(serde_json::json!({ "engine": ENGINE_VERSION, "ops": ops }))
    }
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// Düzleştirilen W×H görüntünün W×H çerçeveyi boşluksuz kaplaması için büyütme.
#[must_use]
pub fn cover_scale(w: f32, h: f32, degrees: f32) -> f32 {
    let t = degrees.abs().to_radians();
    let (s, c) = t.sin_cos();
    if w <= 0.0 || h <= 0.0 {
        return 1.0;
    }
    (c + s * (h / w)).max(c + s * (w / h))
}

/// Ton ayarı için kanal başına tablo (R, G, B). Formül arayüzdekiyle aynıdır.
#[must_use]
pub fn luts(r: &Recipe) -> [[u8; 256]; 3] {
    let k = r.temperature / 100.0;
    let gains = [1.0 + 0.15 * k, 1.0, 1.0 - 0.15 * k];
    let gamma = 2f32.powf(-0.8 * r.brightness / 100.0);
    let slope = 2f32.powf(r.contrast / 100.0);
    let mut out = [[0u8; 256]; 3];
    for (ch, table) in out.iter_mut().enumerate() {
        for (i, v) in table.iter_mut().enumerate() {
            let x = (i as f32 / 255.0 * gains[ch]).min(1.0);
            let x = x.powf(gamma);
            let x = (0.5 + (x - 0.5) * slope).clamp(0.0, 1.0);
            *v = (x * 255.0).round() as u8;
        }
    }
    out
}

fn rotate_quarter(img: RgbImage, quarter: u8) -> RgbImage {
    match quarter % 4 {
        1 => image::imageops::rotate90(&img),
        2 => image::imageops::rotate180(&img),
        3 => image::imageops::rotate270(&img),
        _ => img,
    }
}

/// Tarifi uygular. `max_edge`: çıktının uzun kenarı en çok bu kadar olur
/// (küçük resimler); `None` tam çözünürlük. Büyütme yapılmaz.
#[must_use]
pub fn render(src: RgbImage, r: &Recipe, max_edge: Option<u32>) -> RgbImage {
    let mut frame = rotate_quarter(src, r.quarter);
    let crop = r.crop.unwrap_or(Crop::FULL);
    let (fw, fh) = (frame.width() as f32, frame.height() as f32);
    let (cw, ch) = ((crop.w * fw).max(1.0), (crop.h * fh).max(1.0));
    // Küçültme önce yapılır: hem hızlı hem kenar yumuşatmalı (çift doğrusal
    // örnekleme 2×'ten fazla küçültmede tırtıklı olurdu).
    let k = max_edge.map_or(1.0, |e| (e as f32 / cw.max(ch)).min(1.0));
    if k < 1.0 {
        let nw = ((fw * k).round() as u32).max(1);
        let nh = ((fh * k).round() as u32).max(1);
        frame = image::imageops::resize(&frame, nw, nh, FilterType::Triangle);
    }
    let (fw, fh) = (frame.width() as f32, frame.height() as f32);
    let ow = ((crop.w * fw).round() as u32).max(1);
    let oh = ((crop.h * fh).round() as u32).max(1);
    let lut = luts(r);
    let tone = r.has_tone();

    if r.straighten == 0.0 && r.crop.is_none() {
        if tone {
            for p in frame.pixels_mut() {
                *p = Rgb([lut[0][p[0] as usize], lut[1][p[1] as usize], lut[2][p[2] as usize]]);
            }
        }
        return frame;
    }

    let theta = r.straighten.to_radians();
    let (sin, cos) = theta.sin_cos();
    let s = cover_scale(fw, fh, r.straighten);
    let (cx, cy) = (fw / 2.0, fh / 2.0);
    let (x0, y0) = (crop.x * fw, crop.y * fh);
    let mut out = RgbImage::new(ow, oh);
    for (ox, oy, px) in out.enumerate_pixels_mut() {
        // Çıktı pikseli → çerçeve noktası → döndürmenin tersiyle kaynak noktası.
        let dx = x0 + ox as f32 + 0.5 - cx;
        let dy = y0 + oy as f32 + 0.5 - cy;
        let sx = (dx * cos + dy * sin) / s + cx - 0.5;
        let sy = (-dx * sin + dy * cos) / s + cy - 0.5;
        let v = bilinear(&frame, sx, sy);
        *px = if tone { Rgb([lut[0][v[0] as usize], lut[1][v[1] as usize], lut[2][v[2] as usize]]) } else { v };
    }
    out
}

fn bilinear(img: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: i64, y: i64| img.get_pixel(x as u32, y as u32).0;
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let mut o = [0u8; 3];
    for i in 0..3 {
        let top = f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * fx;
        let bot = f32::from(c[i]) + (f32::from(d[i]) - f32::from(c[i])) * fx;
        o[i] = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    Rgb(o)
}

/// Tek tıkla iyileştirme: uç değerlerden kontrast, ortancadan parlaklık,
/// orta tonların ortalamasından (gri dünya) yarım şiddette sıcaklık.
/// Gün batımının sıcaklığını tamamen silmemek için düzeltme yarımdır.
#[must_use]
pub fn auto_tone(img: &RgbImage) -> AutoTone {
    let mut hist = [0u64; 256];
    let (mut sr, mut sb, mut n) = (0f64, 0f64, 0f64);
    for p in img.pixels() {
        let [r, g, b] = p.0;
        let y = (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)).round() as usize;
        hist[y.min(255)] += 1;
        if (16..=240).contains(&y) {
            sr += f64::from(r);
            sb += f64::from(b);
            n += 1.0;
        }
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return AutoTone { brightness: 0.0, contrast: 0.0, temperature: 0.0 };
    }
    let pct = |q: f64| {
        let want = (total as f64 * q).ceil() as u64;
        let mut acc = 0;
        for (i, c) in hist.iter().enumerate() {
            acc += c;
            if acc >= want.max(1) {
                return i as f32 / 255.0;
            }
        }
        1.0
    };
    let (lo, mid, hi) = (pct(0.005), pct(0.5), pct(0.995));
    let range = (hi - lo).max(0.05);
    let contrast = (100.0 * (0.92 / range).clamp(1.0, 1.6).log2()).min(50.0);
    let m = mid.clamp(0.05, 0.95);
    let gamma = 0.47f32.ln() / m.ln();
    let brightness = (-100.0 / 0.8 * gamma.log2()).clamp(-40.0, 40.0);
    let temperature = if n > 0.0 && sr + sb > 0.0 {
        let k = (sb - sr) / (0.15 * (sr + sb));
        (50.0 * k as f32).clamp(-40.0, 40.0)
    } else {
        0.0
    };
    AutoTone { brightness: brightness.round(), contrast: contrast.round(), temperature: temperature.round() }
}

/// Dışa aktarma ve 1:1 için tam çözünürlüklü, yönü uygulanmış görüntü ve
/// varsa ICC renk profili. `orientation` katalogdaki EXIF yönüdür (RAW
/// önizlemeleri kendi EXIF'ini taşımaz). HEIC/AVIF/TIFF yalnızca `vips` ile.
pub fn decode_full(
    path: &Path,
    file_type: MediaFileType,
    orientation: Option<u8>,
) -> Result<(RgbImage, Option<Vec<u8>>), MediaError> {
    let bad = |e: image::ImageError| MediaError::Decode { path: path.to_path_buf(), reason: e.to_string() };
    let orient = orientation.and_then(Orientation::from_exif);
    match file_type {
        MediaFileType::Raw | MediaFileType::Dng => {
            let jpeg = crate::raw::extract_preview(path)?;
            let mut img = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).map_err(bad)?;
            img.apply_orientation(orient.unwrap_or(Orientation::NoTransforms));
            Ok((img.to_rgb8(), None))
        }
        MediaFileType::Jpeg | MediaFileType::Png | MediaFileType::Webp => {
            let reader = ImageReader::open(path)
                .map_err(MediaError::io("görüntü açma", path))?
                .with_guessed_format()
                .map_err(MediaError::io("biçim koklama", path))?;
            let mut decoder = reader.into_decoder().map_err(bad)?;
            let icc = decoder.icc_profile().ok().flatten();
            let embedded = decoder.orientation().ok();
            let mut img = DynamicImage::from_decoder(decoder).map_err(bad)?;
            img.apply_orientation(orient.or(embedded).unwrap_or(Orientation::NoTransforms));
            Ok((img.to_rgb8(), icc))
        }
        other => decode_with_vips(path, other),
    }
}

#[cfg(feature = "vips")]
fn decode_with_vips(path: &Path, _file_type: MediaFileType) -> Result<(RgbImage, Option<Vec<u8>>), MediaError> {
    // `vipsthumbnail` yönü kendisi uygular; ">" yalnızca küçültür, yani tam boy.
    let tmp = std::env::temp_dir().join(format!("loupe-edit-{}.png", uuid::Uuid::new_v4().simple()));
    let out = std::process::Command::new("vipsthumbnail")
        .arg(path)
        .args(["--size", "65000x65000>", "-o"])
        .arg(&tmp)
        .output()
        .map_err(|e| MediaError::Tool { tool: "vipsthumbnail", reason: e.to_string() });
    let result = out.and_then(|o| {
        if !o.status.success() {
            let reason = String::from_utf8_lossy(&o.stderr).trim().to_owned();
            return Err(MediaError::Decode { path: path.to_path_buf(), reason });
        }
        let img =
            image::open(&tmp).map_err(|e| MediaError::Decode { path: path.to_path_buf(), reason: e.to_string() })?;
        Ok((img.to_rgb8(), None))
    });
    let _ignored = std::fs::remove_file(&tmp);
    result
}

#[cfg(not(feature = "vips"))]
fn decode_with_vips(path: &Path, file_type: MediaFileType) -> Result<(RgbImage, Option<Vec<u8>>), MediaError> {
    Err(MediaError::Unsupported { path: path.to_path_buf(), format: file_type.as_str().to_owned() })
}

/// Dışa aktarılan JPEG'e taşınan EXIF alanları: çekim zamanı, kamera, lens,
/// pozlama, telif ve GPS. Yön her zaman 1 yazılır (görüntü zaten döndürüldü);
/// küçük resim, maker note ve boyut alanları taşınmaz.
const KEEP: &[exif::Tag] = &[
    exif::Tag::DateTime,
    exif::Tag::DateTimeOriginal,
    exif::Tag::DateTimeDigitized,
    exif::Tag::OffsetTime,
    exif::Tag::OffsetTimeOriginal,
    exif::Tag::OffsetTimeDigitized,
    exif::Tag::SubSecTimeOriginal,
    exif::Tag::Make,
    exif::Tag::Model,
    exif::Tag::LensMake,
    exif::Tag::LensModel,
    exif::Tag::ExposureTime,
    exif::Tag::FNumber,
    exif::Tag::PhotographicSensitivity,
    exif::Tag::ExposureBiasValue,
    exif::Tag::FocalLength,
    exif::Tag::FocalLengthIn35mmFilm,
    exif::Tag::Flash,
    exif::Tag::Artist,
    exif::Tag::Copyright,
];

/// Orijinalin EXIF'inden dışa aktarma için TIFF baytları (JPEG APP1 gövdesi).
/// EXIF yoksa ya da okunamazsa `None`: dışa aktarma EXIF'siz sürer.
#[must_use]
pub fn export_exif(original: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(original).ok()?;
    let ex = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)).ok()?;
    let mut fields: Vec<exif::Field> = ex
        .fields()
        .filter(|f| f.ifd_num == exif::In::PRIMARY)
        .filter(|f| f.tag.context() == exif::Context::Gps || KEEP.contains(&f.tag))
        .cloned()
        .collect();
    fields.push(exif::Field {
        tag: exif::Tag::Orientation,
        ifd_num: exif::In::PRIMARY,
        value: exif::Value::Short(vec![1]),
    });
    fields.push(exif::Field {
        tag: exif::Tag::Software,
        ifd_num: exif::In::PRIMARY,
        value: exif::Value::Ascii(vec![b"Loupe".to_vec()]),
    });
    let mut w = exif::experimental::Writer::new();
    for f in &fields {
        w.push_field(f);
    }
    let mut buf = Cursor::new(Vec::new());
    w.write(&mut buf, false).ok()?;
    Some(buf.into_inner())
}

/// JPEG kodlar; ICC profili ve EXIF verildiyse gömülür.
pub fn encode_jpeg(
    img: &RgbImage,
    quality: u8,
    icc: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::with_capacity((img.width() * img.height()) as usize / 4);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    if let Some(icc) = icc {
        enc.set_icc_profile(icc).map_err(|e| MediaError::Encode(e.to_string()))?;
    }
    if let Some(exif) = exif {
        enc.set_exif_metadata(exif).map_err(|e| MediaError::Encode(e.to_string()))?;
    }
    enc.write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| MediaError::Encode(format!("JPEG: {e}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quadrants(w: u32, h: u32) -> RgbImage {
        // Sol üst kırmızı, sağ üst yeşil, sol alt mavi, sağ alt beyaz.
        RgbImage::from_fn(w, h, |x, y| match (x < w / 2, y < h / 2) {
            (true, true) => Rgb([255, 0, 0]),
            (false, true) => Rgb([0, 255, 0]),
            (true, false) => Rgb([0, 0, 255]),
            (false, false) => Rgb([255, 255, 255]),
        })
    }

    #[test]
    fn identity_is_a_no_op() {
        let src = quadrants(40, 20);
        let out = render(src.clone(), &Recipe::default(), None);
        assert_eq!(out, src);
        assert!(Recipe::default().is_identity());
    }

    #[test]
    fn quarter_turn_moves_corners_clockwise() {
        let out = render(quadrants(40, 20), &Recipe { quarter: 1, ..Recipe::default() }, None);
        assert_eq!((out.width(), out.height()), (20, 40));
        // Saat yönünde: sol alttaki mavi sol üste gelir.
        assert_eq!(out.get_pixel(2, 2).0, [0, 0, 255]);
        assert_eq!(out.get_pixel(17, 2).0, [255, 0, 0]);
    }

    #[test]
    fn crop_selects_the_normalized_rectangle() {
        let r = Recipe { crop: Some(Crop { x: 0.5, y: 0.5, w: 0.5, h: 0.5 }), ..Recipe::default() };
        let out = render(quadrants(40, 20), &r, None);
        assert_eq!((out.width(), out.height()), (20, 10));
        assert!(out.pixels().all(|p| p.0 == [255, 255, 255]));
        assert!((r.aspect(2.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn straighten_covers_the_frame_without_empty_corners() {
        let src = RgbImage::from_pixel(60, 40, Rgb([200, 100, 50]));
        let out = render(src, &Recipe { straighten: 10.0, ..Recipe::default() }, None);
        assert_eq!((out.width(), out.height()), (60, 40));
        // Köşeler de kaynaktan gelir: siyah (boş) piksel yok.
        for (x, y) in [(0, 0), (59, 0), (0, 39), (59, 39)] {
            assert_eq!(out.get_pixel(x, y).0, [200, 100, 50], "{x},{y}");
        }
        assert!((cover_scale(60.0, 40.0, 0.0) - 1.0).abs() < 1e-6);
        assert!(cover_scale(60.0, 40.0, 10.0) > 1.2);
    }

    #[test]
    fn max_edge_downscales_the_result() {
        let r = Recipe { crop: Some(Crop { x: 0.0, y: 0.0, w: 0.5, h: 1.0 }), ..Recipe::default() };
        let out = render(quadrants(400, 200), &r, Some(50));
        assert_eq!((out.width(), out.height()), (50, 50));
    }

    #[test]
    fn tone_tables_match_the_interface_formula() {
        let l = luts(&Recipe::default());
        assert!((0..256).all(|i| l[0][i] == i as u8 && l[2][i] == i as u8));
        // recipe.test.ts aynı değerleri bekler.
        let l = luts(&Recipe { brightness: 50.0, contrast: 30.0, temperature: 40.0, ..Recipe::default() });
        assert_eq!([l[0][64], l[1][64], l[2][64]], [86, 81, 76]);
        assert_eq!([l[0][200], l[1][200], l[2][200]], [243, 232, 220]);
        let l = luts(&Recipe { brightness: -40.0, contrast: -50.0, temperature: -60.0, ..Recipe::default() });
        assert_eq!([l[0][128], l[1][128], l[2][128]], [105, 114, 122]);
    }

    #[test]
    fn auto_tone_brightens_dark_flat_images_and_cools_warm_casts() {
        let dark = RgbImage::from_fn(64, 64, |x, _| {
            let v = 30 + (x as u8);
            Rgb([v + 20, v, v.saturating_sub(10)])
        });
        let a = auto_tone(&dark);
        assert!(a.brightness > 10.0, "{a:?}");
        assert!(a.contrast > 20.0, "{a:?}");
        assert!(a.temperature < 0.0, "{a:?}");
        let flat = RgbImage::from_fn(64, 64, |x, y| {
            let v = ((x + y) * 2) as u8;
            Rgb([v, v, v])
        });
        let a = auto_tone(&flat);
        assert_eq!(a.temperature, 0.0);
    }

    #[test]
    fn ops_round_trip_and_normalize() {
        let r = Recipe {
            quarter: 3,
            straighten: -2.5,
            crop: Some(Crop { x: 0.1, y: 0.2, w: 0.5, h: 0.6 }),
            brightness: 12.0,
            contrast: 0.0,
            temperature: -8.0,
        };
        let ops = r.to_ops();
        assert_eq!(
            ops.iter().map(|o| o.0).collect::<Vec<_>>(),
            ["rotate", "straighten", "crop", "white_balance", "exposure"]
        );
        let back = Recipe::from_ops(ops.iter().map(|(o, p)| (*o, p)));
        assert_eq!(back, r);

        let n = Recipe { quarter: 6, straighten: 90.0, crop: Some(Crop::FULL), brightness: 300.0, ..Recipe::default() }
            .normalized()
            .unwrap();
        assert_eq!(n, Recipe { quarter: 2, straighten: 45.0, crop: None, brightness: 100.0, ..Recipe::default() });
        assert!(
            Recipe { crop: Some(Crop { x: 0.8, y: 0.0, w: 0.5, h: 1.0 }), ..Recipe::default() }.normalized().is_err()
        );
        assert!(Recipe { contrast: f32::NAN, ..Recipe::default() }.normalized().is_err());
        assert_eq!(Recipe::default().to_sidecar(), None);
        assert_eq!(r.to_sidecar().unwrap()["ops"][0]["op"], "rotate");
    }

    #[test]
    fn jpeg_carries_exif_with_upright_orientation() {
        let dir = tempfile::tempdir().unwrap();
        // Orijinal: yönü 6 ve çekim zamanı olan bir JPEG.
        let mut fields = vec![
            exif::Field { tag: exif::Tag::Orientation, ifd_num: exif::In::PRIMARY, value: exif::Value::Short(vec![6]) },
            exif::Field {
                tag: exif::Tag::DateTimeOriginal,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"2026:10:24 14:03:11".to_vec()]),
            },
            exif::Field {
                tag: exif::Tag::Make,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"FUJIFILM".to_vec()]),
            },
        ];
        fields.push(exif::Field {
            tag: exif::Tag::MakerNote,
            ifd_num: exif::In::PRIMARY,
            value: exif::Value::Undefined(vec![1, 2, 3], 0),
        });
        let mut w = exif::experimental::Writer::new();
        for f in &fields {
            w.push_field(f);
        }
        let mut tiff = Cursor::new(Vec::new());
        w.write(&mut tiff, false).unwrap();
        let src = quadrants(16, 8);
        let orig = dir.path().join("a.jpg");
        std::fs::write(&orig, encode_jpeg(&src, 90, None, Some(tiff.into_inner())).unwrap()).unwrap();

        let out = encode_jpeg(&src, 90, None, export_exif(&orig)).unwrap();
        let ex = exif::Reader::new().read_from_container(&mut Cursor::new(&out)).unwrap();
        let get = |t| ex.get_field(t, exif::In::PRIMARY).map(|f| f.display_value().to_string());
        assert_eq!(get(exif::Tag::Orientation).as_deref(), Some("row 0 at top and column 0 at left"));
        assert_eq!(get(exif::Tag::DateTimeOriginal).as_deref(), Some("2026-10-24 14:03:11"));
        assert_eq!(get(exif::Tag::Make).as_deref(), Some("\"FUJIFILM\""));
        assert_eq!(get(exif::Tag::MakerNote), None);
    }
}
