use loupe_db::DbError;
use loupe_storage::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum LanError {
    #[error("veritabanı: {0}")]
    Db(#[from] DbError),

    #[error("depolama: {0}")]
    Storage(#[from] StorageError),

    #[error("sertifika: {0}")]
    Cert(#[from] rcgen::Error),

    #[error("TLS: {0}")]
    Tls(#[from] rustls::Error),

    #[error("G/Ç ({context}): {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("keşif: {0}")]
    Discovery(String),

    /// Belirteç geçersiz, süresi dolmuş, tüketilmiş ya da kullanıcı reddetti.
    /// Hangisi olduğu bilinçli olarak söylenmez (RFC-001B §2.4).
    #[error("eşleşme başarısız")]
    PairingFailed,

    #[error("eşleşme denemesi sınırı aşıldı")]
    RateLimited,

    #[error("geçersiz QR yükü: {0}")]
    InvalidPayload(String),

    #[error("geçersiz istek: {0}")]
    BadRequest(String),

    #[error("cihaz kimliği yok veya iptal edilmiş")]
    Unauthorized,

    #[error("yükleme bulunamadı")]
    UploadNotFound,

    #[error("yükleme oturumu kapandı ({0})")]
    UploadGone(String),

    #[error("offset uyuşmuyor: sunucu {server}, istemci {client}")]
    OffsetMismatch { server: u64, client: u64 },

    #[error("parça checksum'ı uyuşmuyor")]
    ChunkChecksum,

    #[error("dosyanın BLAKE3'ü bildirilenle uyuşmuyor")]
    FileChecksum,

    #[error("yükleme boyutu aşıldı")]
    TooLarge,

    #[error("cihaz başına etkin yükleme sınırı aşıldı")]
    TooManyUploads,

    #[error("istek gövdesi yarıda kesildi")]
    Aborted,

    #[error("arka plan görevi: {0}")]
    Join(String),
}

impl LanError {
    pub(crate) fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> Self {
        move |source| Self::Io { context, source }
    }
}

impl From<rusqlite::Error> for LanError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(e))
    }
}

impl From<tokio::task::JoinError> for LanError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Join(e.to_string())
    }
}
