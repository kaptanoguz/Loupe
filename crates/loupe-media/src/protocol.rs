//! `thumb://` özel şeması için istek işleyicisi.
//!
//! Tauri'ye doğrudan takılır:
//!
//! ```ignore
//! let proto = ThumbProtocol::new(ThumbCache::new(root));
//! builder.register_uri_scheme_protocol("thumb", move |_ctx, req| proto.handle(&req))
//! ```
//!
//! Yanıt gövdesi önbellekteki WebP/AVIF baytlarının kendisidir: JSON, base64
//! ya da IPC serileştirmesi yoktur. İçerik adresli olduğu için yanıt
//! `immutable` işaretlenir; tarayıcı aynı URI'yi bir daha sormaz.
//!
//! Kabul edilen biçimler (platforma göre WebView farklı yazar):
//! - `thumb://sm/<blake3>`              (kanonik)
//! - `thumb://localhost/sm/<blake3>`    (Linux/macOS WebKit)
//! - `http://thumb.localhost/sm/<blake3>` (Windows WebView2)

use std::borrow::Cow;
use std::sync::Arc;

use http::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HeaderValue, IF_NONE_MATCH,
};
use http::{Method, Request, Response, StatusCode, Uri};
use loupe_core::Blake3Hash;

use crate::thumb::{ThumbCache, ThumbSize};

pub const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";

type MissHook = Arc<dyn Fn(Blake3Hash, ThumbSize) + Send + Sync>;

/// Önbellekte olmayan küçük resim istendiğinde çağrılır (ör. `Immediate`
/// öncelikli üretim işi kuyruğa konur). İstek 404 ile hemen döner.
#[derive(Clone)]
pub struct ThumbProtocol {
    cache: ThumbCache,
    on_miss: Option<MissHook>,
}

impl std::fmt::Debug for ThumbProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbProtocol").field("cache", &self.cache).field("on_miss", &self.on_miss.is_some()).finish()
    }
}

impl ThumbProtocol {
    #[must_use]
    pub fn new(cache: ThumbCache) -> Self {
        Self { cache, on_miss: None }
    }

    #[must_use]
    pub fn with_miss_hook(mut self, hook: impl Fn(Blake3Hash, ThumbSize) + Send + Sync + 'static) -> Self {
        self.on_miss = Some(Arc::new(hook));
        self
    }

    /// URI'den boyut sınıfı ve içerik özeti.
    #[must_use]
    pub fn parse(uri: &Uri) -> Option<(ThumbSize, Blake3Hash)> {
        let host = uri.host().unwrap_or("");
        let mut segs = uri.path().split('/').filter(|s| !s.is_empty());
        let size = match (uri.scheme_str(), host) {
            (Some("thumb"), "localhost") | (Some("http" | "https"), "thumb.localhost") => segs.next()?,
            (Some("thumb"), size) => size,
            _ => return None,
        };
        let hash = segs.next()?;
        if segs.next().is_some() {
            return None;
        }
        let hash = hash.strip_suffix(".webp").or_else(|| hash.strip_suffix(".avif")).unwrap_or(hash);
        if hash.len() != 64 {
            return None;
        }
        Some((size.parse().ok()?, Blake3Hash::from_hex(hash).ok()?))
    }

    /// Kanonik URI (arayüz `<img src>` için).
    #[must_use]
    pub fn uri_for(size: ThumbSize, hash: &Blake3Hash) -> String {
        format!("thumb://{}/{}", size.as_str(), hash.to_hex())
    }

    pub fn handle<B>(&self, req: &Request<B>) -> Response<Cow<'static, [u8]>> {
        let head = match *req.method() {
            Method::GET => false,
            Method::HEAD => true,
            _ => return status(StatusCode::METHOD_NOT_ALLOWED),
        };
        let Some((size, hash)) = Self::parse(req.uri()) else {
            return status(StatusCode::BAD_REQUEST);
        };
        let etag = format!("\"{}-{}\"", hash.to_hex(), size.as_str());
        let Some((path, format)) = self.cache.lookup(&hash, size) else {
            if let Some(hook) = &self.on_miss {
                hook(hash, size);
            }
            return status(StatusCode::NOT_FOUND);
        };
        if req.headers().get(IF_NONE_MATCH).is_some_and(|v| v.as_bytes() == etag.as_bytes()) {
            let mut resp = status(StatusCode::NOT_MODIFIED);
            set(&mut resp, ETAG, &etag);
            resp.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static(CACHE_IMMUTABLE));
            return resp;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            // Okuma ile arama arasında LRU temizliği silmiş olabilir.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return status(StatusCode::NOT_FOUND),
            Err(_) => return status(StatusCode::INTERNAL_SERVER_ERROR),
        };
        let len = bytes.len();
        let mut resp = Response::new(if head { Cow::Borrowed(&[][..]) } else { Cow::Owned(bytes) });
        let h = resp.headers_mut();
        h.insert(CONTENT_TYPE, HeaderValue::from_static(format.mime()));
        h.insert(CACHE_CONTROL, HeaderValue::from_static(CACHE_IMMUTABLE));
        h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        h.insert(CONTENT_LENGTH, HeaderValue::from(len));
        set(&mut resp, ETAG, &etag);
        resp
    }
}

fn status(code: StatusCode) -> Response<Cow<'static, [u8]>> {
    let mut resp = Response::new(Cow::Borrowed(&[][..]));
    *resp.status_mut() = code;
    if code != StatusCode::NOT_MODIFIED {
        resp.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    resp
}

fn set(resp: &mut Response<Cow<'static, [u8]>>, name: http::header::HeaderName, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        resp.headers_mut().insert(name, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_platform_forms() {
        let hex = "ab".repeat(32);
        let want = Some((ThumbSize::Sm, Blake3Hash([0xab; 32])));
        for uri in [
            format!("thumb://sm/{hex}"),
            format!("thumb://localhost/sm/{hex}"),
            format!("http://thumb.localhost/sm/{hex}"),
            format!("thumb://sm/{hex}.webp?v=1"),
        ] {
            assert_eq!(ThumbProtocol::parse(&uri.parse().unwrap()), want, "{uri}");
        }
        for bad in [
            format!("thumb://xl/{hex}"),
            "thumb://sm/abc".to_owned(),
            format!("thumb://sm/{hex}/extra"),
            format!("https://example.com/sm/{hex}"),
            format!("thumb://sm/{}", "zz".repeat(32)),
        ] {
            assert_eq!(ThumbProtocol::parse(&bad.parse().unwrap()), None, "{bad}");
        }
        assert_eq!(ThumbProtocol::uri_for(ThumbSize::Lg, &Blake3Hash([0xab; 32])), format!("thumb://lg/{hex}"));
    }
}
