//! Dinamik QR eşleşmesi (RFC-001B §2.3).
//!
//! Belirteç 256 bit rastgeledir, yalnızca BLAKE3 özeti bellekte tutulur ve
//! diske hiç yazılmaz. Aynı anda tek belirteç geçerlidir; yenisi üretilince eskisi
//! düşer. İlk `POST /pair` denemesinde, başarılı ya da başarısız, tüketilir: bu
//! yüzden ele geçirilen bir QR ikinci kez kullanılamaz (replay) ve tahmin
//! denemesi gerçek belirteci de yakar. Karşılaştırma sabit zamanlıdır
//! (`blake3::Hash` eşitliği).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;

use crate::{Clock, LanError};

/// Belirtecin ömrü.
pub const PAIRING_TTL: Duration = Duration::from_secs(60);
/// IP başına dakikada en fazla eşleşme denemesi.
pub const MAX_ATTEMPTS_PER_MINUTE: usize = 5;

#[derive(Debug, Clone, Copy)]
struct Active {
    hash: blake3::Hash,
    expires_at_ms: i64,
}

/// Ekrandaki QR'a giren belirteç.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingToken {
    /// 43 karakter base64url.
    pub token: String,
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for PairingToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingToken").field("expires_at_ms", &self.expires_at_ms).finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct PairingManager {
    clock: Arc<dyn Clock>,
    ttl_ms: i64,
    active: Mutex<Option<Active>>,
    attempts: Mutex<HashMap<IpAddr, Vec<i64>>>,
}

impl PairingManager {
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            ttl_ms: i64::try_from(PAIRING_TTL.as_millis()).unwrap_or(60_000),
            active: Mutex::new(None),
            attempts: Mutex::new(HashMap::new()),
        }
    }

    /// Yeni belirteç üretir; önceki belirteç anında geçersiz olur.
    pub fn issue(&self) -> Result<PairingToken, LanError> {
        let mut raw = [0u8; 32];
        rand::rng().fill_bytes(&mut raw);
        let token = URL_SAFE_NO_PAD.encode(raw);
        let expires_at_ms = self.clock.now_ms() + self.ttl_ms;
        *self.active.lock().map_err(|_| LanError::Join("eşleşme kilidi zehirlendi".into()))? =
            Some(Active { hash: blake3::hash(token.as_bytes()), expires_at_ms });
        Ok(PairingToken { token, expires_at_ms })
    }

    /// "Telefon ekle" ekranı kapandı: belirteç silinir.
    pub fn cancel(&self) {
        if let Ok(mut a) = self.active.lock() {
            *a = None;
        }
    }

    /// IP başına dakikada [`MAX_ATTEMPTS_PER_MINUTE`] deneme.
    pub fn allow_attempt(&self, ip: IpAddr) -> bool {
        let now = self.clock.now_ms();
        let Ok(mut m) = self.attempts.lock() else { return false };
        m.retain(|_, v| {
            v.retain(|t| now - t < 60_000);
            !v.is_empty()
        });
        let v = m.entry(ip).or_default();
        if v.len() >= MAX_ATTEMPTS_PER_MINUTE {
            return false;
        }
        v.push(now);
        true
    }

    /// Belirteci tüketir. Geçerli belirteç yoksa, süresi dolmuşsa ya da
    /// eşleşmiyorsa hep aynı hata döner.
    pub fn redeem(&self, presented: &str) -> Result<(), LanError> {
        let taken = self.active.lock().map_err(|_| LanError::PairingFailed)?.take();
        let Some(a) = taken else { return Err(LanError::PairingFailed) };
        let fresh = self.clock.now_ms() <= a.expires_at_ms;
        let same = blake3::hash(presented.as_bytes()) == a.hash;
        if fresh && same { Ok(()) } else { Err(LanError::PairingFailed) }
    }
}

/// `loupe://pair?…` QR yükü. `fp` kullanıcının istediği sunucu sertifikası
/// parmak izi (ilk bağlantı bununla doğrulanır), `ca` eşleşmeden sonra
/// sabitlenecek CA'nın SPKI özeti (RFC-001B §2.3).
#[derive(Clone, PartialEq, Eq)]
pub struct PairingPayload {
    pub addrs: Vec<IpAddr>,
    pub port: u16,
    pub ca_spki_sha256: [u8; 32],
    pub server_cert_sha256: [u8; 32],
    pub token: String,
    /// Unix saniye.
    pub expires_at_s: i64,
    pub name: String,
}

impl std::fmt::Debug for PairingPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingPayload")
            .field("addrs", &self.addrs)
            .field("port", &self.port)
            .field("expires_at_s", &self.expires_at_s)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

const SCHEME: &str = "loupe://pair?";

fn decode32(s: &str, what: &str) -> Result<[u8; 32], LanError> {
    let v = URL_SAFE_NO_PAD.decode(s).map_err(|_| LanError::InvalidPayload(format!("{what} base64url değil")))?;
    <[u8; 32]>::try_from(v.as_slice()).map_err(|_| LanError::InvalidPayload(format!("{what} 32 bayt değil")))
}

impl PairingPayload {
    /// Telefon kamerasının açabileceği QR adresi (RFC-001G):
    /// `https://<adres>:<port>/m#t=<belirteç>`. Belirteç parça kısmındadır;
    /// tarayıcı onu istek satırına koymaz, sayfa gövdede gönderir. Birden çok
    /// adres varsa özel IPv4 adresi seçilir (telefonlar çoğunlukla oradadır).
    #[must_use]
    pub fn web_url(&self) -> Option<String> {
        let pick = self
            .addrs
            .iter()
            .find(|a| matches!(a, IpAddr::V4(v4) if v4.is_private()))
            .or_else(|| self.addrs.iter().find(|a| !a.is_loopback()))
            .or_else(|| self.addrs.first())?;
        let host = match pick {
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(v6) => format!("[{v6}]"),
        };
        Some(format!("https://{host}:{}/m#t={}", self.port, self.token))
    }

    #[must_use]
    pub fn to_uri(&self) -> String {
        let addrs: Vec<String> = self.addrs.iter().map(ToString::to_string).collect();
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("v", "1")
            .append_pair("a", &addrs.join(","))
            .append_pair("p", &self.port.to_string())
            .append_pair("ca", &URL_SAFE_NO_PAD.encode(self.ca_spki_sha256))
            .append_pair("fp", &URL_SAFE_NO_PAD.encode(self.server_cert_sha256))
            .append_pair("t", &self.token)
            .append_pair("x", &self.expires_at_s.to_string())
            .append_pair("n", &self.name)
            .finish();
        format!("{SCHEME}{query}")
    }

    pub fn parse(uri: &str) -> Result<Self, LanError> {
        let query =
            uri.strip_prefix(SCHEME).ok_or_else(|| LanError::InvalidPayload("şema loupe://pair değil".into()))?;
        let mut m: HashMap<String, String> = HashMap::new();
        for (k, v) in form_urlencoded::parse(query.as_bytes()) {
            m.insert(k.into_owned(), v.into_owned());
        }
        let get = |k: &str| m.get(k).map(String::as_str).ok_or_else(|| LanError::InvalidPayload(format!("{k} eksik")));
        if get("v")? != "1" {
            return Err(LanError::InvalidPayload("desteklenmeyen sürüm".into()));
        }
        let addrs = get("a")?
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<IpAddr>().map_err(|_| LanError::InvalidPayload(format!("geçersiz adres: {s}"))))
            .collect::<Result<Vec<_>, _>>()?;
        if addrs.is_empty() {
            return Err(LanError::InvalidPayload("adres yok".into()));
        }
        let token = get("t")?.to_owned();
        if decode32(&token, "t").is_err() {
            return Err(LanError::InvalidPayload("belirteç 256 bit olmalı".into()));
        }
        Ok(Self {
            addrs,
            port: get("p")?.parse().map_err(|_| LanError::InvalidPayload("geçersiz port".into()))?,
            ca_spki_sha256: decode32(get("ca")?, "ca")?,
            server_cert_sha256: decode32(get("fp")?, "fp")?,
            token,
            expires_at_s: get("x")?.parse().map_err(|_| LanError::InvalidPayload("geçersiz x".into()))?,
            name: get("n")?.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManualClock;

    #[test]
    fn token_is_single_use_expires_and_is_replaced() {
        let clock = Arc::new(ManualClock::new(1_000_000));
        let m = PairingManager::new(clock.clone());

        let t = m.issue().unwrap();
        assert_eq!(t.token.len(), 43);
        m.redeem(&t.token).unwrap();
        assert!(matches!(m.redeem(&t.token), Err(LanError::PairingFailed)), "ikinci kullanım");

        let t = m.issue().unwrap();
        clock.advance(PAIRING_TTL + Duration::from_millis(1));
        assert!(m.redeem(&t.token).is_err(), "süresi dolmuş");

        let old = m.issue().unwrap();
        let new = m.issue().unwrap();
        assert!(m.redeem(&old.token).is_err(), "eskisi yenisi gelince geçersiz");
        assert!(m.redeem(&new.token).is_err(), "yanlış deneme gerçek belirteci de tüketir");

        let t = m.issue().unwrap();
        m.cancel();
        assert!(m.redeem(&t.token).is_err());
    }

    #[test]
    fn attempts_are_rate_limited_per_ip() {
        let clock = Arc::new(ManualClock::new(0));
        let m = PairingManager::new(clock.clone());
        let ip: IpAddr = "192.168.1.30".parse().unwrap();
        for _ in 0..MAX_ATTEMPTS_PER_MINUTE {
            assert!(m.allow_attempt(ip));
        }
        assert!(!m.allow_attempt(ip));
        assert!(m.allow_attempt("192.168.1.31".parse().unwrap()));
        clock.advance(Duration::from_secs(61));
        assert!(m.allow_attempt(ip));
    }

    #[test]
    fn payload_round_trips_and_stays_compact() {
        let p = PairingPayload {
            addrs: vec!["192.168.1.20".parse().unwrap(), "fd12:3456::20".parse().unwrap()],
            port: 47123,
            ca_spki_sha256: [7; 32],
            server_cert_sha256: [9; 32],
            token: URL_SAFE_NO_PAD.encode([1u8; 32]),
            expires_at_s: 1_761_394_860,
            name: "Oğuz-Masaüstü".into(),
        };
        let uri = p.to_uri();
        assert!(uri.starts_with("loupe://pair?v=1&a=192.168.1.20%2Cfd12%3A3456%3A%3A20&p=47123"), "{uri}");
        assert!(uri.len() < 300, "QR küçük kalmalı: {}", uri.len());
        assert_eq!(PairingPayload::parse(&uri).unwrap(), p);
        assert!(PairingPayload::parse("https://pair?v=1").is_err());
        assert!(PairingPayload::parse(&uri.replace("v=1", "v=2")).is_err());
    }
}
