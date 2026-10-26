//! Kurulum gerektirmeyen telefon aktarımı (RFC-001G).
//!
//! Masaüstündeki QR artık `https://<LAN adresi>:<port>/m#t=<belirteç>`
//! taşır: telefon kamerası adresi tarayıcıda açar, sayfa ikilinin içinden
//! gelir ([`ASSETS`]). Tarayıcı istemci sertifikası sunamaz; bu yüzden
//! eşleşmede tek kullanımlık belirteç bir **oturum anahtarıyla** değiştirilir:
//!
//! | Uç nokta | Kimlik | Amaç |
//! |---|---|---|
//! | `GET /m`, `/m/*.js`, `/m/app.css`, `/m/icon.svg` | — | gömülü sayfa |
//! | `POST /m/session` | QR belirteci | oturum anahtarı + `devices` (`kind = 'web'`) |
//! | `GET /m/session` | oturum | bu tarayıcı hâlâ eşleşmiş mi |
//! | `POST /m/precheck` | oturum | ad + değişiklik zamanıyla "zaten aktarıldı" |
//!
//! tus uç noktaları (`/files…`) aynıdır; kimlik mTLS yerine
//! `Authorization: Bearer <anahtar>` ya da `loupe_s` çerezidir. Anahtarın
//! yalnızca alan ayrımlı SHA-256 parmak izi saklanır ve [`crate::DeviceRegistry`]'ye
//! girer: iptal, son görülme ve çakışma politikası uygulama cihazlarıyla aynıdır.

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;

use crate::identity::sha256;

/// Oturum çerezinin adı.
pub const SESSION_COOKIE: &str = "loupe_s";
/// Tarayıcı oturumunun ömrü: telefon her açılışta yeniden eşleşmesin.
pub const WEB_SESSION_TTL_MS: i64 = 365 * 24 * 3600 * 1000;
/// Bir ön kontrol isteğindeki en çok öğe.
pub const MAX_PRECHECK_ITEMS: usize = 5000;
const SESSION_DOMAIN: &[u8] = b"loupe-web-session-v1\0";
/// 32 bayt base64url = 43 karakter; daha uzun değer özetlenmeden reddedilir.
const MAX_KEY_LEN: usize = 64;

/// Gömülü durağan dosyalar: (yol, içerik türü, gövde).
pub const ASSETS: &[(&str, &str, &str)] = &[
    ("/m", "text/html; charset=utf-8", include_str!("../web/index.html")),
    ("/m/app.js", "text/javascript; charset=utf-8", include_str!("../web/app.js")),
    ("/m/core.js", "text/javascript; charset=utf-8", include_str!("../web/core.js")),
    ("/m/blake3.js", "text/javascript; charset=utf-8", include_str!("../web/blake3.js")),
    ("/m/app.css", "text/css; charset=utf-8", include_str!("../web/app.css")),
    ("/m/icon.svg", "image/svg+xml", include_str!("../web/icon.svg")),
];

/// Sayfa yalnızca kendi kaynaklarını yükler ve yalnızca bu sunucuyla konuşur.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
                       connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// Yeni oturum anahtarı (256 bit, base64url).
#[must_use]
pub fn new_session_key() -> String {
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

/// `devices.cert_sha256` olarak saklanan parmak izi. Alan ayrımı sayesinde bir
/// istemci sertifikasının özetiyle çakışamaz.
#[must_use]
pub fn session_fingerprint(key: &str) -> [u8; 32] {
    let mut buf = Vec::with_capacity(SESSION_DOMAIN.len() + key.len());
    buf.extend_from_slice(SESSION_DOMAIN);
    buf.extend_from_slice(key.as_bytes());
    sha256(&buf)
}

/// İstekteki oturum anahtarı: önce `Authorization: Bearer`, sonra çerez.
#[must_use]
pub fn session_key(h: &HeaderMap) -> Option<&str> {
    let bearer = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let cookie = || {
        h.get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|kv| kv.trim().split_once('='))
            .find(|(k, _)| *k == SESSION_COOKIE)
            .map(|(_, v)| v.trim())
    };
    bearer.or_else(cookie).filter(|k| !k.is_empty() && k.len() <= MAX_KEY_LEN)
}

/// `Set-Cookie`: yalnızca HTTPS, betikten okunmaz, başka siteden gönderilmez.
#[must_use]
pub fn session_cookie(key: &str) -> String {
    format!("{SESSION_COOKIE}={key}; Path=/; Max-Age={}; Secure; HttpOnly; SameSite=Strict", WEB_SESSION_TTL_MS / 1000)
}

/// Durağan dosya yanıtı; bilinmeyen yol `None`.
#[must_use]
pub fn asset(path: &str) -> Option<Response> {
    let (_, ctype, body) = ASSETS.iter().find(|(p, _, _)| *p == path)?;
    let mut res = (StatusCode::OK, *body).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
    // Sürüm güncellemesinde eski betik önbellekte kalmasın.
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    harden(h);
    Some(res)
}

/// Her web yanıtına eklenen güvenlik başlıkları.
pub fn harden(h: &mut HeaderMap) {
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
}

/// `POST /m/session` gövdesi.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct WebPairRequest {
    pub token: String,
    pub device_name: String,
    /// `android` | `ios` | `desktop`
    pub platform: String,
}

/// `POST /m/session` ve `GET /m/session` yanıtı.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct WebSession {
    pub device_id: i64,
    pub device_name: String,
    pub server_name: String,
    /// Yalnızca eşleşme yanıtında; sonra tarayıcıda kalır.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

/// `POST /m/precheck` öğesi.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PrecheckItem {
    pub device_asset_id: String,
    /// Dosyanın `lastModified` değeri (ms).
    pub modified_at: i64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct PrecheckResponse {
    /// Bu tarayıcıdan aynı ad ve değişiklik zamanıyla daha önce görülenler.
    pub known: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_read_from_bearer_then_cookie() {
        let mut h = HeaderMap::new();
        assert_eq!(session_key(&h), None);
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; loupe_s=cookie-key; b=2"));
        assert_eq!(session_key(&h), Some("cookie-key"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer bearer-key"));
        assert_eq!(session_key(&h), Some("bearer-key"));
        let long = format!("Bearer {}", "x".repeat(MAX_KEY_LEN + 1));
        h.insert(header::AUTHORIZATION, HeaderValue::from_str(&long).unwrap());
        assert_eq!(session_key(&h), None);
    }

    #[test]
    fn fingerprint_is_domain_separated() {
        let k = new_session_key();
        assert_eq!(k.len(), 43);
        assert_ne!(session_fingerprint(&k), sha256(k.as_bytes()));
        assert_eq!(session_fingerprint(&k), session_fingerprint(&k));
        assert_ne!(session_fingerprint(&k), session_fingerprint(&new_session_key()));
    }

    #[test]
    fn every_asset_is_served_with_csp() {
        for (path, ctype, body) in ASSETS {
            let res = asset(path).unwrap();
            assert_eq!(res.headers()[header::CONTENT_TYPE], *ctype);
            assert_eq!(res.headers()[header::CONTENT_SECURITY_POLICY], CSP);
            assert!(!body.is_empty());
        }
        assert!(asset("/m/../Cargo.toml").is_none());
    }
}
