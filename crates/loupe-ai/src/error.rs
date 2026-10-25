use thiserror::Error;

#[derive(Debug, Error)]
pub enum AiError {
    #[error(transparent)]
    Db(#[from] loupe_db::DbError),

    #[error(transparent)]
    Media(#[from] loupe_media::MediaError),

    #[error("model yüklenemedi ({name}): {reason}")]
    ModelLoad { name: String, reason: String },

    #[error("çıkarım başarısız ({model}): {reason}")]
    Inference { model: String, reason: String },

    #[error("vektör boyutu {found}, beklenen {expected}")]
    Dimension { expected: usize, found: usize },

    #[error("geçersiz girdi: {0}")]
    InvalidInput(String),

    #[error("{entity} {id} bulunamadı")]
    NotFound { entity: &'static str, id: i64 },

    #[error("dosya işlemi: {0}")]
    Io(String),

    #[error("indirme başarısız ({url}): {reason}")]
    Download { url: String, reason: String },

    /// İndirilen ya da diskteki dosya manifestteki boyut/özetle tutmuyor.
    #[error("doğrulama başarısız ({file}): beklenen {expected}, gelen {actual}")]
    Verify { file: String, expected: String, actual: String },

    #[error("iptal edildi")]
    Cancelled,

    #[error("iç kilit zehirlendi")]
    Poisoned,
}

impl From<rusqlite::Error> for AiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(loupe_db::DbError::from(e))
    }
}
