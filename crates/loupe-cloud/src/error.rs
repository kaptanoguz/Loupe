use std::path::PathBuf;
use std::time::Duration;

use loupe_db::DbError;

/// RFC-001B §3.4 hata sınıfları. Kuyruk davranışı sınıfa göre seçilir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Bağlantı sıfırlandı, zaman aşımı, DNS: backoff ile yeniden dene.
    NetTransient,
    /// 429 / 503 / SlowDown: `Retry-After` varsa ona uy.
    Throttled,
    /// 500 / 502 / 504.
    Server,
    /// 401: kullanıcı yeniden giriş yapana dek `error`.
    Auth,
    /// Depolama dolu: hedefin kuyruğu durur.
    Quota,
    /// 403, bucket yok: kalıcı.
    Forbidden,
    /// Yerel dosya yok ya da disk takılı değil.
    LocalMissing,
    /// Yerel dosyanın BLAKE3'ü katalogdakiyle uyuşmuyor: buluta çıkmaz.
    LocalCorrupt,
    /// Yapılandırma, anahtar ya da desteklenmeyen sağlayıcı.
    Permanent,
}

impl ErrorClass {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NetTransient => "NET_TRANSIENT",
            Self::Throttled => "THROTTLED",
            Self::Server => "SERVER",
            Self::Auth => "AUTH",
            Self::Quota => "QUOTA",
            Self::Forbidden => "FORBIDDEN",
            Self::LocalMissing => "LOCAL_MISSING",
            Self::LocalCorrupt => "LOCAL_CORRUPT",
            Self::Permanent => "PERMANENT",
        }
    }

    /// Backoff ile yeniden denenir mi?
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(self, Self::NetTransient | Self::Throttled | Self::Server | Self::LocalMissing)
    }

    #[must_use]
    pub const fn from_status(status: u16) -> Self {
        match status {
            401 => Self::Auth,
            403 | 404 => Self::Forbidden,
            429 | 503 => Self::Throttled,
            507 => Self::Quota,
            500..=599 => Self::Server,
            _ => Self::Permanent,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("veritabanı: {0}")]
    Db(#[from] DbError),

    #[error("HTTP {status}: {body}")]
    Http { status: u16, retry_after: Option<Duration>, body: String },

    #[error("ağ: {0}")]
    Network(String),

    #[error("G/Ç ({context}): {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("şifreleme: {0}")]
    Crypto(String),

    #[error("geçersiz anahtar: {0}")]
    InvalidKey(String),

    #[error("yerel dosya yok: {0}")]
    LocalMissing(PathBuf),

    #[error("yerel dosya bozuk: BLAKE3 {actual}, katalogda {expected}")]
    LocalCorrupt { expected: String, actual: String },

    #[error("uzak nesne doğrulanamadı: {0}")]
    RemoteMismatch(String),

    #[error("yapılandırma: {0}")]
    Config(String),

    #[error("desteklenmeyen sağlayıcı: {0}")]
    Unsupported(String),

    #[error("arka plan görevi: {0}")]
    Join(String),
}

impl CloudError {
    pub(crate) fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> Self {
        move |source| Self::Io { context, source }
    }

    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Http { status, .. } => ErrorClass::from_status(*status),
            Self::Network(_) | Self::RemoteMismatch(_) => ErrorClass::NetTransient,
            Self::LocalMissing(_) => ErrorClass::LocalMissing,
            Self::LocalCorrupt { .. } => ErrorClass::LocalCorrupt,
            Self::Io { source, .. } if source.kind() == std::io::ErrorKind::StorageFull => ErrorClass::Quota,
            Self::Io { .. } | Self::Join(_) => ErrorClass::NetTransient,
            Self::Db(_) | Self::Crypto(_) | Self::InvalidKey(_) | Self::Config(_) | Self::Unsupported(_) => {
                ErrorClass::Permanent
            }
        }
    }

    /// Sunucunun istediği bekleme (`Retry-After`).
    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Http { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for CloudError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(e))
    }
}

impl From<reqwest::Error> for CloudError {
    fn from(e: reqwest::Error) -> Self {
        match e.status() {
            Some(s) => Self::Http { status: s.as_u16(), retry_after: None, body: e.to_string() },
            None => Self::Network(e.to_string()),
        }
    }
}

impl From<tokio::task::JoinError> for CloudError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Join(e.to_string())
    }
}

impl From<CloudError> for std::io::Error {
    fn from(e: CloudError) -> Self {
        Self::other(e)
    }
}
