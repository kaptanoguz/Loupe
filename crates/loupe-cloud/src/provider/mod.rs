//! Bulut sağlayıcı soyutlaması.
//!
//! Dört işlem: akışla yükle, akışla indir, üst veri (HEAD), sil. Gövde her
//! zaman `Bytes` akışıdır; hiçbir sağlayıcı dosyanın tamamını belleğe almaz
//! (S3 en fazla bir parça, varsayılan 8 MiB tutar).
//!
//! Doğrulama sözleşmesi: [`UploadReceipt::expected`] sağlayıcının, yüklenen
//! baytlar için `head_object`'te bildirmesi gereken checksum'dır. İkisi
//! eşleşirse uzak kopya, okurken BLAKE3'ü katalogla eşleşen akışın ta kendisidir.
//! Sağlayıcı checksum vermiyorsa (WebDAV) motor nesneyi geri okuyup çözer ve
//! düz metin BLAKE3'ünü karşılaştırır.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::Stream;
use zeroize::Zeroizing;

use crate::CloudError;

pub mod local;
pub mod s3;
pub mod webdav;

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, CloudError>> + Send>>;

/// `sync_status.remote_checksum_algo`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgo {
    Sha256,
    Md5,
    /// S3 ETag (tekli PUT'ta MD5, multipart'ta parça MD5'lerinin MD5'i + `-N`).
    Etag,
    /// Geri okuma ile doğrulanan düz metin BLAKE3'ü.
    Blake3,
}

impl ChecksumAlgo {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Md5 => "md5",
            Self::Etag => "etag",
            Self::Blake3 => "blake3",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteChecksum {
    pub algo: ChecksumAlgo,
    /// Küçük harf hex.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMeta {
    pub size: u64,
    pub checksum: Option<RemoteChecksum>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadReceipt {
    pub size: u64,
    pub expected: Option<RemoteChecksum>,
}

#[async_trait]
pub trait CloudProvider: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;

    /// Akış hata verirse nesne oluşturulmaz (yarım yükleme iptal edilir).
    async fn upload_stream(&self, key: &str, body: ByteStream) -> Result<UploadReceipt, CloudError>;

    async fn download_stream(&self, key: &str) -> Result<ByteStream, CloudError>;

    /// Nesne yoksa `None`.
    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, CloudError>;

    /// Nesne zaten yoksa da başarılıdır.
    async fn delete_object(&self, key: &str) -> Result<(), CloudError>;
}

/// Nesne anahtarı kuralları: göreli, `..` yok, boş bileşen yok.
pub(crate) fn check_key(key: &str) -> Result<(), CloudError> {
    let bad = key.is_empty()
        || key.starts_with('/')
        || key.contains('\\')
        || key.contains('\0')
        || key.split('/').any(|p| p.is_empty() || p == "." || p == "..");
    if bad { Err(CloudError::Config(format!("geçersiz nesne anahtarı: {key:?}"))) } else { Ok(()) }
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Kimlik bilgileri (Secret Service'ten). `Debug` sırrı yazmaz.
#[derive(Clone)]
pub struct Credentials {
    pub user: String,
    pub secret: Zeroizing<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("user", &self.user).finish_non_exhaustive()
    }
}

/// HTTP istemcisi: rustls (ring) + webpki kökleri; NAS'taki öz imzalı
/// sertifika için ek kök verilebilir.
pub fn http_client(extra_root_der: Option<&[u8]>) -> Result<reqwest::Client, CloudError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(der) = extra_root_der {
        roots
            .add(rustls::pki_types::CertificateDer::from(der.to_vec()))
            .map_err(|e| CloudError::Config(format!("ek kök sertifika: {e}")))?;
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| CloudError::Config(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| CloudError::Config(e.to_string()))
}

/// `cloud_targets` satırından sağlayıcı. `config` gizli olmayan JSON'dur.
pub fn from_target(
    provider: &str,
    config: &serde_json::Value,
    credentials: Option<Credentials>,
) -> Result<Arc<dyn CloudProvider>, CloudError> {
    let s = |k: &str| config.get(k).and_then(serde_json::Value::as_str);
    let need = |k: &str| s(k).ok_or_else(|| CloudError::Config(format!("{provider}: config.{k} eksik")));
    Ok(match provider {
        "local" => Arc::new(local::LocalProvider::new(need("path")?)?),
        "s3" | "b2" | "r2" | "wasabi" => {
            let creds = credentials.ok_or_else(|| CloudError::Config(format!("{provider}: erişim anahtarı yok")))?;
            let cfg = s3::S3Config {
                endpoint: need("endpoint")?.to_owned(),
                bucket: need("bucket")?.to_owned(),
                region: s("region").unwrap_or("auto").to_owned(),
                path_style: config.get("path_style").and_then(serde_json::Value::as_bool).unwrap_or(true),
                part_size: s3::DEFAULT_PART_SIZE,
            };
            Arc::new(s3::S3Provider::new(cfg, creds, http_client(None)?)?)
        }
        "webdav" => Arc::new(webdav::WebDavProvider::new(need("url")?, credentials, http_client(None)?)?),
        other => return Err(CloudError::Unsupported(other.to_owned())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_must_be_relative_and_clean() {
        for ok in ["loupe/objects/ab/cd/x.age", "a"] {
            assert!(check_key(ok).is_ok(), "{ok}");
        }
        for bad in ["", "/abs", "a/../b", "a//b", "a/./b", "a\\b", "a/"] {
            assert!(check_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unsupported_and_incomplete_targets_are_rejected() {
        let cfg = serde_json::json!({});
        assert!(matches!(from_target("gdrive", &cfg, None), Err(CloudError::Unsupported(_))));
        assert!(matches!(from_target("local", &cfg, None), Err(CloudError::Config(_))));
        assert!(matches!(
            from_target(
                "b2",
                &serde_json::json!({"endpoint": "https://s3.eu-central-003.backblazeb2.com", "bucket": "x"}),
                None
            ),
            Err(CloudError::Config(_))
        ));
    }
}
