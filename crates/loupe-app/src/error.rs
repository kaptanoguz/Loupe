use serde::Serialize;

/// Arayüze dönen hata. Tauri komutları bunu `{ code, message }` olarak
/// serileştirir; arayüz `code`'a göre karar verir, `message`'ı gösterir.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Core(#[from] loupe_core::CoreError),

    #[error(transparent)]
    Db(#[from] loupe_db::DbError),

    #[error(transparent)]
    Storage(#[from] loupe_storage::StorageError),

    #[error(transparent)]
    Jobs(#[from] loupe_jobs::JobsError),

    #[error(transparent)]
    Media(#[from] loupe_media::MediaError),

    #[error(transparent)]
    Search(#[from] loupe_search::SearchError),

    #[error(transparent)]
    Lan(#[from] loupe_lan::LanError),

    #[error(transparent)]
    Cloud(#[from] loupe_cloud::CloudError),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{entity} bulunamadı: {id}")]
    NotFound { entity: &'static str, id: i64 },

    #[error("geçersiz istek: {0}")]
    Invalid(String),

    #[error("kütüphane çevrimdışı: {0}")]
    LibraryOffline(String),

    #[error("iç kilit zehirlendi")]
    Poisoned,

    #[error("arka plan görevi: {0}")]
    Join(String),
}

impl AppError {
    pub(crate) fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> Self {
        let context = context.into();
        move |source| Self::Io { context, source }
    }

    /// Arayüzün karar verdiği kısa, sabit kod.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Core(_) => "CORE",
            Self::Db(_) => "DB",
            Self::Storage(_) => "STORAGE",
            Self::Jobs(_) => "JOBS",
            Self::Media(_) => "MEDIA",
            Self::Search(_) => "SEARCH",
            Self::Lan(_) => "LAN",
            Self::Cloud(_) => "CLOUD",
            Self::Io { .. } => "IO",
            Self::NotFound { .. } => "NOT_FOUND",
            Self::Invalid(_) => "INVALID",
            Self::LibraryOffline(_) => "LIBRARY_OFFLINE",
            Self::Poisoned => "POISONED",
            Self::Join(_) => "JOIN",
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(loupe_db::DbError::Sqlite(e))
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("AppError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
