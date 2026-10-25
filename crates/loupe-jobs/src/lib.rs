//! Loupe kalıcı iş kuyruğu.
//!
//! - [`queue`]: SQLite `jobs` tablosu üzerinde atomik alma, kira, viewport
//!   önceliği, üstel geri çekilme ve dead-letter.
//! - [`worker`]: iş parçacığı havuzu, ayrılmış acil işçi, panik yalıtımı.
//! - [`throttle`]: duraklat/devam, kaynak modu, kullanıcı etkinliği.
//! - [`handlers`]: `loupe-media` işlerini (küçük resim, metadata, konum) kuyruğa bağlar.

pub mod handlers;
pub mod model;
pub mod queue;
pub mod throttle;
pub mod worker;

pub use handlers::MediaPipeline;
pub use model::{Job, JobError, JobId, JobKind, JobRecord, JobState, NewJob, Priority, QueueStats};
pub use queue::{ENQUEUE_BATCH, JobQueue, RetryPolicy};
pub use throttle::{ResourceMode, Throttle};
pub use worker::{JobContext, JobHandler, JobRouter, WorkerConfig, WorkerPool};

#[derive(Debug, thiserror::Error)]
pub enum JobsError {
    #[error(transparent)]
    Db(#[from] loupe_db::DbError),

    #[error("iş {0:?} artık bu işçide değil (kira süresi doldu)")]
    LeaseLost(JobId),

    #[error("iç kilit zehirlendi")]
    Poisoned,

    #[error("işçi iş parçacığı başlatılamadı: {0}")]
    Spawn(#[source] std::io::Error),
}
