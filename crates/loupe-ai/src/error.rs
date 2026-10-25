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

    #[error("iç kilit zehirlendi")]
    Poisoned,
}

impl From<rusqlite::Error> for AiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(loupe_db::DbError::from(e))
    }
}
