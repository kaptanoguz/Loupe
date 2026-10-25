//! `thumb://orig/<blake3>`: Loupe'un 1:1 yakınlaştırması için tam çözünürlük.
//!
//! - JPEG/PNG/WebP, EXIF yönü 1 (ya da yok): dosyanın kendi baytları, yeniden
//!   kodlama yok.
//! - Yönü 2–8 olan JPEG: çözülür, döndürülür, JPEG q92 olarak
//!   `cache/full/<hh>/<hash>-o<yön>.jpg` altına bir kez yazılır. Yanıt EXIF
//!   taşımadığı için WebView ikinci kez döndürmez.
//! - RAW/DNG: gömülü tam boy önizleme (demosaic yok). Yönü 1 değilse aynı
//!   önbellekten döndürülmüş hali.
//! - HEIC, AVIF, JXL, TIFF ve video: 415; arayüz `lg` küçük resminde kalır.
//!
//! İçerik adresli olduğu için yanıt `immutable` işaretlenir.
//!
//! Kabul edilen biçimler `thumb://` ile aynıdır: `thumb://orig/<hash>`,
//! `thumb://localhost/orig/<hash>`, `http://thumb.localhost/orig/<hash>`
//! (`full` eş anlamlı).

use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use http::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HeaderValue, IF_NONE_MATCH,
};
use http::{Method, Request, Response, StatusCode, Uri};
use image::metadata::Orientation;
use loupe_core::{Blake3Hash, MediaFileType};
use loupe_db::Pool;
use loupe_media::protocol::CACHE_IMMUTABLE;

use crate::library::Libraries;
use crate::{AppError, AppResult};

/// Döndürülmüş kopyaların JPEG kalitesi.
pub const JPEG_QUALITY: u8 = 92;

/// Tam çözünürlük kaynağı hazırlanırken verilen karar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullSource {
    /// Dosya olduğu gibi gönderilir.
    File { path: PathBuf, mime: &'static str },
    /// Bellekte hazırlanmış (RAW önizlemesi, yönü 1).
    Bytes { bytes: Vec<u8>, mime: &'static str },
}

#[derive(Debug, Clone)]
pub struct FullRes {
    pool: Arc<Pool>,
    libraries: Arc<Libraries>,
    dir: PathBuf,
}

/// `thumb://orig/<hash>` biçiminde mi? Değilse `None` (küçük resim isteğidir).
#[must_use]
pub fn parse(uri: &Uri) -> Option<Blake3Hash> {
    let host = uri.host().unwrap_or("");
    let mut segs = uri.path().split('/').filter(|s| !s.is_empty());
    let kind = match (uri.scheme_str(), host) {
        (Some("thumb"), "localhost") | (Some("http" | "https"), "thumb.localhost") => segs.next()?,
        (Some("thumb"), k) => k,
        _ => return None,
    };
    if !matches!(kind, "orig" | "full") {
        return None;
    }
    let hash = segs.next()?;
    if segs.next().is_some() || hash.len() != 64 {
        return None;
    }
    Blake3Hash::from_hex(hash).ok()
}

#[must_use]
pub fn uri_for(hash: &Blake3Hash) -> String {
    format!("thumb://orig/{}", hash.to_hex())
}

struct Row {
    file_type: MediaFileType,
    mime: String,
    orientation: Option<u8>,
    path: Option<PathBuf>,
}

fn response(code: StatusCode) -> Response<Cow<'static, [u8]>> {
    let mut resp = Response::new(Cow::Borrowed(&[][..]));
    *resp.status_mut() = code;
    if code != StatusCode::NOT_MODIFIED {
        resp.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    resp
}

fn mime_of(t: MediaFileType) -> Option<&'static str> {
    match t {
        MediaFileType::Jpeg => Some("image/jpeg"),
        MediaFileType::Png => Some("image/png"),
        MediaFileType::Webp => Some("image/webp"),
        _ => None,
    }
}

impl FullRes {
    #[must_use]
    pub fn new(pool: Arc<Pool>, libraries: Arc<Libraries>, dir: PathBuf) -> Self {
        Self { pool, libraries, dir }
    }

    fn cache_path(&self, hash: &Blake3Hash, orientation: u8) -> PathBuf {
        let hex = hash.to_hex();
        self.dir.join(&hex[..2]).join(format!("{hex}-o{orientation}.jpg"))
    }

    fn row(&self, hash: &Blake3Hash) -> AppResult<Option<Row>> {
        let Some(id) = self.pool.read(|c| loupe_db::repo::media::find_by_blake3(c, hash))? else { return Ok(None) };
        let Some(m) = self.pool.read(|c| loupe_db::repo::media::get(c, id))? else { return Ok(None) };
        let orientation: Option<i64> = self
            .pool
            .read(|c| Ok(c.query_row("SELECT orientation FROM media_files WHERE id = ?1", [id.0], |r| r.get(0))?))?;
        Ok(Some(Row {
            file_type: m.file_type,
            mime: m.mime.clone(),
            orientation: orientation.and_then(|o| u8::try_from(o).ok()),
            path: self.libraries.media_path(&m)?,
        }))
    }

    /// Tam çözünürlüğün kaynağı. `Ok(None)`: bu tür için 1:1 yok (415).
    pub fn source(&self, hash: &Blake3Hash) -> AppResult<Option<FullSource>> {
        let row = self.row(hash)?.ok_or(AppError::NotFound { entity: "medya", id: 0 })?;
        let raw = matches!(row.file_type, MediaFileType::Raw | MediaFileType::Dng);
        let direct = mime_of(row.file_type);
        if !raw && direct.is_none() {
            return Ok(None);
        }
        let orient = row.orientation.filter(|o| (2..=8).contains(o));
        if let Some(o) = orient {
            let cached = self.cache_path(hash, o);
            if cached.is_file() {
                return Ok(Some(FullSource::File { path: cached, mime: "image/jpeg" }));
            }
        }
        let path = row.path.ok_or_else(|| AppError::LibraryOffline(format!("{} kaynağı çevrimdışı", row.mime)))?;
        let bytes = if raw {
            loupe_media::raw::extract_preview(&path)?
        } else if orient.is_none() {
            return Ok(Some(FullSource::File { path, mime: direct.unwrap_or("application/octet-stream") }));
        } else {
            std::fs::read(&path).map_err(AppError::io(format!("{}", path.display())))?
        };
        let Some(o) = orient else { return Ok(Some(FullSource::Bytes { bytes, mime: "image/jpeg" })) };
        let rotated = rotate_to_jpeg(&bytes, o)?;
        let out = self.cache_path(hash, o);
        write_atomic(&out, &rotated)?;
        Ok(Some(FullSource::Bytes { bytes: rotated, mime: "image/jpeg" }))
    }

    pub fn handle<B>(&self, req: &Request<B>, hash: &Blake3Hash) -> Response<Cow<'static, [u8]>> {
        let head = match *req.method() {
            Method::GET => false,
            Method::HEAD => true,
            _ => return response(StatusCode::METHOD_NOT_ALLOWED),
        };
        let etag = format!("\"{}-orig\"", hash.to_hex());
        if req.headers().get(IF_NONE_MATCH).is_some_and(|v| v.as_bytes() == etag.as_bytes()) {
            let mut resp = response(StatusCode::NOT_MODIFIED);
            if let Ok(v) = HeaderValue::from_str(&etag) {
                resp.headers_mut().insert(ETAG, v);
            }
            resp.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static(CACHE_IMMUTABLE));
            return resp;
        }
        let (bytes, mime) = match self.source(hash) {
            Ok(Some(FullSource::File { path, mime })) => match std::fs::read(&path) {
                Ok(b) => (b, mime),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return response(StatusCode::NOT_FOUND),
                Err(_) => return response(StatusCode::INTERNAL_SERVER_ERROR),
            },
            Ok(Some(FullSource::Bytes { bytes, mime })) => (bytes, mime),
            Ok(None) => return response(StatusCode::UNSUPPORTED_MEDIA_TYPE),
            Err(AppError::NotFound { .. }) => return response(StatusCode::NOT_FOUND),
            Err(AppError::LibraryOffline(_)) => return response(StatusCode::SERVICE_UNAVAILABLE),
            Err(AppError::Media(_)) => return response(StatusCode::UNPROCESSABLE_ENTITY),
            Err(_) => return response(StatusCode::INTERNAL_SERVER_ERROR),
        };
        let len = bytes.len();
        let mut resp = Response::new(if head { Cow::Borrowed(&[][..]) } else { Cow::Owned(bytes) });
        let h = resp.headers_mut();
        h.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
        h.insert(CACHE_CONTROL, HeaderValue::from_static(CACHE_IMMUTABLE));
        h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        h.insert(CONTENT_LENGTH, HeaderValue::from(len));
        if let Ok(v) = HeaderValue::from_str(&etag) {
            h.insert(ETAG, v);
        }
        resp
    }
}

/// Çözer, EXIF yönünü uygular ve JPEG olarak yeniden kodlar.
pub fn rotate_to_jpeg(bytes: &[u8], exif_orientation: u8) -> AppResult<Vec<u8>> {
    let bad = |e: image::ImageError| AppError::Invalid(format!("görüntü çözülemedi: {e}"));
    // Yön katalogdan gelir (RAW önizlemeleri kendi EXIF'ini taşımaz).
    let mut img = image::load_from_memory(bytes).map_err(bad)?;
    let orient = Orientation::from_exif(exif_orientation).unwrap_or(Orientation::NoTransforms);
    img.apply_orientation(orient);
    let mut out = Vec::with_capacity(bytes.len());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&img.to_rgb8())
        .map_err(bad)?;
    Ok(out)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let dir = path.parent().ok_or_else(|| AppError::Invalid("önbellek yolu".into()))?;
    std::fs::create_dir_all(dir).map_err(AppError::io(format!("{}", dir.display())))?;
    let tmp = dir.join(format!(".{}.{}.tmp", std::process::id(), uuid::Uuid::new_v4().simple()));
    let mut f = std::fs::File::create(&tmp).map_err(AppError::io("tam boy önbellek"))?;
    f.write_all(bytes).map_err(AppError::io("tam boy önbellek yazma"))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(AppError::io("tam boy önbellek rename"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_orig_forms_only() {
        let hex = "cd".repeat(32);
        let want = Some(Blake3Hash([0xcd; 32]));
        for uri in [
            format!("thumb://orig/{hex}"),
            format!("thumb://full/{hex}"),
            format!("thumb://localhost/orig/{hex}"),
            format!("http://thumb.localhost/orig/{hex}"),
        ] {
            assert_eq!(parse(&uri.parse().unwrap()), want, "{uri}");
        }
        for bad in [format!("thumb://sm/{hex}"), format!("thumb://orig/{hex}/x"), "thumb://orig/ab".to_owned()] {
            assert_eq!(parse(&bad.parse().unwrap()), None, "{bad}");
        }
    }

    #[test]
    fn rotation_swaps_dimensions() {
        let img = image::RgbImage::from_pixel(40, 20, image::Rgb([200, 10, 10]));
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&img).unwrap();
        let out = rotate_to_jpeg(&jpg, 6).unwrap();
        let back = image::load_from_memory(&out).unwrap();
        assert_eq!((back.width(), back.height()), (20, 40));
    }
}
