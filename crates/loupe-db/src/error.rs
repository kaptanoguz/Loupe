use loupe_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("sqlite-vec eklentisi kaydedilemedi (SQLite kodu {0})")]
    VecRegister(i32),

    #[error("migrasyon {version} ({name}) uygulanamadı: {source}")]
    MigrationFailed {
        version: u32,
        name: &'static str,
        #[source]
        source: rusqlite::Error,
    },

    #[error("migrasyon {version} ({name}) user_version'ı {found} bıraktı")]
    MigrationVersion { version: u32, name: &'static str, found: i64 },

    #[error("migrasyon {version} ({name}) uygulandıktan sonra değiştirilmiş (kayıtlı {recorded}, dosya {current})")]
    ChecksumMismatch { version: u32, name: &'static str, recorded: String, current: String },

    #[error("user_version {found} ama migrasyon {version} için geçmiş kaydı yok")]
    MissingHistory { version: u32, found: i64 },

    #[error("veritabanı şeması bu sürümden yeni ({found} > {known}); eski uygulamayla açılmaz")]
    SchemaTooNew { found: i64, known: u32 },

    #[error("şema doğrulaması başarısız: {0}")]
    Validation(String),

    #[error("{column} değeri SQLite INTEGER aralığına sığmıyor: {value}")]
    OutOfRange { column: &'static str, value: u64 },

    #[error("{entity} bulunamadı: {id}")]
    NotFound { entity: &'static str, id: i64 },

    #[error("bağlantı havuzu kilidi zehirlenmiş (önceki bir iş panikledi)")]
    PoolPoisoned,

    #[error(transparent)]
    Core(#[from] CoreError),
}
