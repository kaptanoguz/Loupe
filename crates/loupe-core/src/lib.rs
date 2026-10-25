//! Loupe ortak tipleri.
//!
//! `loupe-db` ve `loupe-storage` birbirine bağımlı değildir; ikisi de bu crate'e
//! bağlıdır. Depolama katmanının veritabanından istediği her şey
//! [`catalog`] modülündeki trait'lerle tanımlanır, `loupe-db` bunları uygular.

pub mod catalog;
pub mod enums;
pub mod error;
pub mod file_type;
pub mod hash;
pub mod ids;
pub mod path;
pub mod time;

pub use catalog::{CommitOutcome, ImportCatalog, NewMediaFile, VerifyCatalog, VerifyEntry, VerifyOutcome};
pub use enums::{
    ColorLabel, Flag, ImportItemState, ImportMode, LibraryKind, LibraryState, MediaFileType, Placement, SessionMode,
    SessionState, TakenAtSource, VerifyState, VerifyTrigger,
};
pub use error::CoreError;
pub use file_type::{FileKind, classify};
pub use hash::{Blake3Hash, PHash};
pub use ids::{ImportItemId, ImportSessionId, LibraryId, MediaId, VerifyRunId};
pub use path::RelPath;
pub use time::now_ms;
