//! Küçük resim motoru.
//!
//! - Boyut sınıfları: `sm` 256, `md` 768, `lg` 1920 px uzun kenar; kaynak
//!   küçükse büyütülmez.
//! - EXIF yönlendirmesi uygulanır; çıktı her zaman "dik" görüntüdür.
//! - Önbellek içerik adreslidir: `<kök>/<blake3[0:2]>/<blake3>_<sınıf>.<webp|avif>`.
//!   Aynı içerik iki yerde dursa da tek küçük resmi vardır; dosya taşınsa da
//!   küçük resim geçerli kalır. Yazım atomiktir (geçici dosya + rename).
//! - Ağır C bağımlılıkları [`ThumbnailBackend`] arkasındadır. Varsayılan
//!   [`ImageBackend`] saf Rust `image` çözücüsü + RAW gömülü önizleme
//!   çıkarıcısıdır; `vips` özelliği [`VipsBackend`]'i (HEIC, AVIF, büyük
//!   dosyalarda shrink-on-load) ekler.

use std::fmt;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use loupe_core::{Blake3Hash, MediaFileType, classify};

use crate::MediaError;
use crate::util::write_atomic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ThumbSize {
    Sm,
    Md,
    Lg,
}

impl ThumbSize {
    /// Büyükten küçüğe: tek çözümle sırayla küçültülür.
    pub const ALL: [Self; 3] = [Self::Lg, Self::Md, Self::Sm];

    #[must_use]
    pub const fn long_edge(self) -> u32 {
        match self {
            Self::Sm => 256,
            Self::Md => 768,
            Self::Lg => 1920,
        }
    }

    /// `thumbnails.size_class` ve URI'deki ad.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sm => "sm",
            Self::Md => "md",
            Self::Lg => "lg",
        }
    }

    const fn webp_quality(self) -> f32 {
        match self {
            Self::Sm => 72.0,
            Self::Md => 78.0,
            Self::Lg => 82.0,
        }
    }
}

impl FromStr for ThumbSize {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "sm" => Ok(Self::Sm),
            "md" => Ok(Self::Md),
            "lg" => Ok(Self::Lg),
            _ => Err(()),
        }
    }
}

impl fmt::Display for ThumbSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThumbFormat {
    Webp,
    Avif,
}

impl ThumbFormat {
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Avif => "avif",
        }
    }

    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
        }
    }
}

/// `thumbnails.source_kind` değerleri.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    EmbeddedPreview,
    Decode,
    /// Videodan `ffmpeg` ile çıkarılan kare.
    VideoFrame,
}

impl SourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmbeddedPreview => "embedded_preview",
            Self::Decode => "decode",
            Self::VideoFrame => "video_frame",
        }
    }
}

/// Arka ucun ürettiği tek küçük resim.
#[derive(Debug, Clone)]
pub struct EncodedThumb {
    pub size: ThumbSize,
    pub format: ThumbFormat,
    pub width: u32,
    pub height: u32,
    pub source_kind: SourceKind,
    pub bytes: Vec<u8>,
}

/// Küçük resim üretim arka ucu. Uygulamalar iş parçacıkları arasında paylaşılır.
pub trait ThumbnailBackend: Send + Sync + fmt::Debug {
    fn name(&self) -> &'static str;

    /// `sizes` için küçük resimleri üretir. `orientation` EXIF değeridir (1–8);
    /// `None` ise arka uç dosyanın kendisinden okumayı dener.
    fn render(
        &self,
        source: &Path,
        sizes: &[ThumbSize],
        orientation: Option<u8>,
        format: ThumbFormat,
    ) -> Result<Vec<EncodedThumb>, MediaError>;
}

// ─────────────────────────────── ImageBackend ───────────────────────────────

/// Saf Rust arka uç: JPEG, PNG, WebP ve TIFF tabanlı RAW (gömülü önizleme).
#[derive(Debug, Default, Clone, Copy)]
pub struct ImageBackend;

impl ImageBackend {
    fn decode(source: &Path) -> Result<(DynamicImage, Option<Orientation>, SourceKind), MediaError> {
        let kind = classify(source);
        let decode_err =
            |e: image::ImageError| MediaError::Decode { path: source.to_path_buf(), reason: e.to_string() };
        match kind.file_type {
            MediaFileType::Raw | MediaFileType::Dng => {
                let jpeg = crate::raw::extract_preview(source)?;
                let img = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).map_err(decode_err)?;
                Ok((img, None, SourceKind::EmbeddedPreview))
            }
            MediaFileType::Jpeg | MediaFileType::Png | MediaFileType::Webp => {
                let reader = ImageReader::open(source)
                    .map_err(MediaError::io("görüntü açma", source))?
                    .with_guessed_format()
                    .map_err(MediaError::io("biçim koklama", source))?;
                let mut decoder = reader.into_decoder().map_err(decode_err)?;
                let orientation = decoder.orientation().ok();
                let img = DynamicImage::from_decoder(decoder).map_err(decode_err)?;
                Ok((img, orientation, SourceKind::Decode))
            }
            other => Err(MediaError::Unsupported { path: source.to_path_buf(), format: other.as_str().to_owned() }),
        }
    }
}

impl ThumbnailBackend for ImageBackend {
    fn name(&self) -> &'static str {
        "image"
    }

    fn render(
        &self,
        source: &Path,
        sizes: &[ThumbSize],
        orientation: Option<u8>,
        format: ThumbFormat,
    ) -> Result<Vec<EncodedThumb>, MediaError> {
        if format != ThumbFormat::Webp {
            return Err(MediaError::Unsupported { path: source.to_path_buf(), format: "avif çıktı".into() });
        }
        let (img, embedded, source_kind) = Self::decode(source)?;
        if img.width() == 0 || img.height() == 0 {
            return Err(MediaError::Decode { path: source.to_path_buf(), reason: "boş görüntü".into() });
        }
        let orient = orientation.and_then(Orientation::from_exif).or(embedded).unwrap_or(Orientation::NoTransforms);

        let mut ordered: Vec<ThumbSize> = sizes.to_vec();
        ordered.sort_by(|a, b| b.cmp(a));
        ordered.dedup();

        // Büyükten küçüğe: her basamak bir öncekinden küçültülür (tek çözüm).
        let mut current = img;
        let mut out = Vec::with_capacity(ordered.len());
        for size in ordered {
            current = downscale(current, size.long_edge());
            let mut oriented = current.clone();
            oriented.apply_orientation(orient);
            let (bytes, width, height) = encode_webp(&oriented, size.webp_quality())?;
            out.push(EncodedThumb { size, format, width, height, source_kind, bytes });
        }
        Ok(out)
    }
}

/// Uzun kenarı `edge`'e indirir; zaten küçükse dokunmaz.
fn downscale(img: DynamicImage, edge: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= edge {
        return img;
    }
    let (nw, nh) = scaled(w, h, edge);
    img.resize_exact(nw, nh, FilterType::CatmullRom)
}

/// Oranı koruyarak uzun kenarı `edge` yapan boyut (en az 1 px).
#[must_use]
pub fn scaled(w: u32, h: u32, edge: u32) -> (u32, u32) {
    let (long, short) = if w >= h { (w, h) } else { (h, w) };
    if long <= edge {
        return (w, h);
    }
    let s = u32::try_from((u64::from(short) * u64::from(edge) + u64::from(long) / 2) / u64::from(long))
        .unwrap_or(edge)
        .max(1);
    if w >= h { (edge, s) } else { (s, edge) }
}

#[cfg(feature = "lossy-webp")]
fn encode_webp(img: &DynamicImage, quality: f32) -> Result<(Vec<u8>, u32, u32), MediaError> {
    let (w, h) = (img.width(), img.height());
    let mut config = webp::WebPConfig::new().map_err(|()| MediaError::Encode("libwebp yapılandırılamadı".into()))?;
    config.quality = quality;
    config.method = 4;
    let mem = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        webp::Encoder::from_rgba(rgba.as_raw(), w, h).encode_advanced(&config)
    } else {
        let rgb = img.to_rgb8();
        webp::Encoder::from_rgb(rgb.as_raw(), w, h).encode_advanced(&config)
    }
    .map_err(|e| MediaError::Encode(format!("WebP: {e:?}")))?;
    Ok((mem.to_vec(), w, h))
}

#[cfg(not(feature = "lossy-webp"))]
fn encode_webp(img: &DynamicImage, _quality: f32) -> Result<(Vec<u8>, u32, u32), MediaError> {
    use image::ExtendedColorType;
    use image::codecs::webp::WebPEncoder;
    let (w, h) = (img.width(), img.height());
    let mut buf = Cursor::new(Vec::new());
    let enc = WebPEncoder::new_lossless(&mut buf);
    if img.color().has_alpha() {
        enc.encode(img.to_rgba8().as_raw(), w, h, ExtendedColorType::Rgba8)
    } else {
        enc.encode(img.to_rgb8().as_raw(), w, h, ExtendedColorType::Rgb8)
    }
    .map_err(|e| MediaError::Encode(format!("WebP: {e}")))?;
    Ok((buf.into_inner(), w, h))
}

/// WebP başlığından boyut (doğrulama ve testler için).
pub fn webp_dimensions(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    let reader = ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::WebP);
    reader.into_dimensions().map_err(|e| MediaError::Decode { path: PathBuf::from("<webp>"), reason: e.to_string() })
}

// ──────────────────────────────── VipsBackend ────────────────────────────────

/// libvips arka ucu (`vipsthumbnail`). Yönlendirmeyi kendisi uygular,
/// JPEG'lerde shrink-on-load ile tam çözüm yapmaz; HEIC ve AVIF destekler.
#[cfg(feature = "vips")]
#[derive(Debug, Clone)]
pub struct VipsBackend {
    bin: PathBuf,
}

#[cfg(feature = "vips")]
impl VipsBackend {
    /// `vipsthumbnail` PATH'te çalışıyorsa arka ucu döndürür.
    #[must_use]
    pub fn detect() -> Option<Self> {
        let bin = PathBuf::from("vipsthumbnail");
        let ok =
            std::process::Command::new(&bin).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        ok.then_some(Self { bin })
    }

    fn header(path: &Path, field: &str) -> Result<u32, MediaError> {
        let out = std::process::Command::new("vipsheader")
            .args(["-f", field])
            .arg(path)
            .output()
            .map_err(|e| MediaError::Tool { tool: "vipsheader", reason: e.to_string() })?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .map_err(|_| MediaError::Tool { tool: "vipsheader", reason: format!("{field} okunamadı") })
    }
}

#[cfg(feature = "vips")]
impl ThumbnailBackend for VipsBackend {
    fn name(&self) -> &'static str {
        "vips"
    }

    fn render(
        &self,
        source: &Path,
        sizes: &[ThumbSize],
        _orientation: Option<u8>,
        format: ThumbFormat,
    ) -> Result<Vec<EncodedThumb>, MediaError> {
        let dir = std::env::temp_dir().join(format!("loupe-vips-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).map_err(MediaError::io("geçici dizin", &dir))?;
        let result = (|| {
            let mut out = Vec::new();
            for &size in sizes {
                let target = dir.join(format!("{}.{}", size.as_str(), format.extension()));
                let q = size.webp_quality();
                let o = std::process::Command::new(&self.bin)
                    .arg(source)
                    .arg("--size")
                    .arg(format!("{0}x{0}>", size.long_edge()))
                    .arg("-o")
                    .arg(format!("{}[Q={q},strip]", target.display()))
                    .output()
                    .map_err(|e| MediaError::Tool { tool: "vipsthumbnail", reason: e.to_string() })?;
                if !o.status.success() {
                    let reason = String::from_utf8_lossy(&o.stderr).trim().to_owned();
                    return Err(MediaError::Decode { path: source.to_path_buf(), reason });
                }
                let bytes = std::fs::read(&target).map_err(MediaError::io("vips çıktısı", &target))?;
                out.push(EncodedThumb {
                    size,
                    format,
                    width: Self::header(&target, "width")?,
                    height: Self::header(&target, "height")?,
                    source_kind: SourceKind::Decode,
                    bytes,
                });
            }
            Ok(out)
        })();
        let _ignored = std::fs::remove_dir_all(&dir);
        result
    }
}

/// Önce `primary`, desteklemediği dosyada `fallback`.
#[derive(Debug)]
pub struct FallbackBackend {
    pub primary: Box<dyn ThumbnailBackend>,
    pub fallback: Box<dyn ThumbnailBackend>,
}

impl ThumbnailBackend for FallbackBackend {
    fn name(&self) -> &'static str {
        self.primary.name()
    }

    fn render(
        &self,
        source: &Path,
        sizes: &[ThumbSize],
        orientation: Option<u8>,
        format: ThumbFormat,
    ) -> Result<Vec<EncodedThumb>, MediaError> {
        match self.primary.render(source, sizes, orientation, format) {
            Err(MediaError::Unsupported { .. } | MediaError::Tool { .. }) => {
                self.fallback.render(source, sizes, orientation, format)
            }
            other => other,
        }
    }
}

/// Videoyu bir kareye çevirip `inner` ile küçültür; diğer dosyalar doğrudan `inner`'a gider.
#[derive(Debug)]
pub struct VideoBackend {
    pub inner: Box<dyn ThumbnailBackend>,
}

impl ThumbnailBackend for VideoBackend {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn render(
        &self,
        source: &Path,
        sizes: &[ThumbSize],
        orientation: Option<u8>,
        format: ThumbFormat,
    ) -> Result<Vec<EncodedThumb>, MediaError> {
        if classify(source).file_type != MediaFileType::Video {
            return self.inner.render(source, sizes, orientation, format);
        }
        let dir = std::env::temp_dir().join(format!("loupe-frame-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).map_err(MediaError::io("geçici dizin", &dir))?;
        let frame = dir.join("frame.jpg");
        // Kare ffmpeg tarafından zaten döndürüldü: EXIF yönlendirmesi uygulanmaz.
        let result = crate::video::extract_frame(source, &frame).and_then(|()| {
            let mut out = self.inner.render(&frame, sizes, Some(1), format)?;
            for t in &mut out {
                t.source_kind = SourceKind::VideoFrame;
            }
            Ok(out)
        });
        let _ignored = std::fs::remove_dir_all(&dir);
        result
    }
}

/// Derleme özelliklerine göre en iyi arka uç: `vips` açık ve kuruluysa vips,
/// desteklemediği durumda saf Rust; değilse yalnızca saf Rust.
#[must_use]
pub fn default_backend() -> Box<dyn ThumbnailBackend> {
    #[cfg(feature = "vips")]
    if let Some(v) = VipsBackend::detect() {
        let inner = Box::new(FallbackBackend { primary: Box::new(v), fallback: Box::new(ImageBackend) });
        return Box::new(VideoBackend { inner });
    }
    Box::new(VideoBackend { inner: Box::new(ImageBackend) })
}

// ───────────────────────────────── Önbellek ─────────────────────────────────

/// İçerik adresli küçük resim önbelleği.
#[derive(Debug, Clone)]
pub struct ThumbCache {
    root: PathBuf,
}

impl ThumbCache {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$XDG_CACHE_HOME/loupe/thumbs` (yoksa `~/.cache/loupe/thumbs`).
    #[must_use]
    pub fn default_root() -> Option<PathBuf> {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|c| c.join("loupe").join("thumbs"))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn path(&self, hash: &Blake3Hash, size: ThumbSize, format: ThumbFormat) -> PathBuf {
        let hex = hash.to_hex();
        let shard = hex.get(..2).unwrap_or("00");
        self.root.join(shard).join(format!("{hex}_{}.{}", size.as_str(), format.extension()))
    }

    /// Önbellekte varsa yol ve biçim (önce WebP).
    #[must_use]
    pub fn lookup(&self, hash: &Blake3Hash, size: ThumbSize) -> Option<(PathBuf, ThumbFormat)> {
        [ThumbFormat::Webp, ThumbFormat::Avif]
            .into_iter()
            .map(|f| (self.path(hash, size, f), f))
            .find(|(p, _)| p.is_file())
    }

    pub fn put(&self, hash: &Blake3Hash, thumb: &EncodedThumb) -> Result<PathBuf, MediaError> {
        let path = self.path(hash, thumb.size, thumb.format);
        write_atomic(&path, &thumb.bytes)?;
        Ok(path)
    }
}

/// `thumbnails` satırı için gereken her şey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedThumb {
    pub size: ThumbSize,
    pub format: ThumbFormat,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
    pub source_kind: SourceKind,
    pub path: PathBuf,
}

/// Arka uç + önbellek.
#[derive(Debug)]
pub struct Thumbnailer {
    backend: Box<dyn ThumbnailBackend>,
    cache: ThumbCache,
    format: ThumbFormat,
}

impl Thumbnailer {
    #[must_use]
    pub fn new(cache: ThumbCache, backend: Box<dyn ThumbnailBackend>) -> Self {
        Self { backend, cache, format: ThumbFormat::Webp }
    }

    #[must_use]
    pub fn with_format(mut self, format: ThumbFormat) -> Self {
        self.format = format;
        self
    }

    #[must_use]
    pub fn cache(&self) -> &ThumbCache {
        &self.cache
    }

    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Eksik boyutları üretip önbelleğe yazar. Önbellekte olanlar yeniden
    /// üretilmez (içerik adresli: aynı özet = aynı görüntü).
    pub fn generate(
        &self,
        source: &Path,
        hash: &Blake3Hash,
        orientation: Option<u8>,
        sizes: &[ThumbSize],
    ) -> Result<Vec<GeneratedThumb>, MediaError> {
        let missing: Vec<ThumbSize> =
            sizes.iter().copied().filter(|s| !self.cache.path(hash, *s, self.format).is_file()).collect();
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        let rendered = self.backend.render(source, &missing, orientation, self.format)?;
        rendered
            .into_iter()
            .map(|t| {
                let path = self.cache.put(hash, &t)?;
                Ok(GeneratedThumb {
                    size: t.size,
                    format: t.format,
                    width: t.width,
                    height: t.height,
                    byte_size: t.bytes.len() as u64,
                    source_kind: t.source_kind,
                    path,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_keeps_aspect_and_never_upscales() {
        assert_eq!(scaled(6000, 4000, 256), (256, 171));
        assert_eq!(scaled(4000, 6000, 768), (512, 768));
        assert_eq!(scaled(100, 50, 1920), (100, 50));
        assert_eq!(scaled(10_000, 1, 256), (256, 1));
    }

    #[test]
    fn cache_path_is_sharded() {
        let c = ThumbCache::new("/c");
        let h = Blake3Hash([0xab; 32]);
        let p = c.path(&h, ThumbSize::Sm, ThumbFormat::Webp);
        assert_eq!(p, PathBuf::from(format!("/c/ab/{}_sm.webp", "ab".repeat(32))));
    }
}
