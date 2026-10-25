//! Loupe katalog veritabanı.
//!
//! - [`Pool`]: tek yazar + okuyucu bağlantıları (WAL; okuyucular yazarı beklemez).
//! - [`migrate`]: gömülü `migrations/NNNN_*.sql` dosyalarını sırayla, her birini
//!   kendi `BEGIN IMMEDIATE` transaction'ında uygular ve doğrular.
//! - [`repo`]: Library, MediaFile, ImportSession/ImportItem ve Verify CRUD'u.
//! - [`batch`]: 500'lük `BEGIN IMMEDIATE` transaction'larla toplu yazma.
//! - [`catalog`]: `loupe-core` katalog trait'lerinin SQLite uygulaması.

pub mod batch;
pub mod catalog;
mod connection;
mod convert;
pub mod error;
pub mod migrate;
pub mod models;
mod pool;
pub mod repo;
mod vec_ext;

pub use batch::{BATCH_SIZE, BatchReport, insert_import_items_batch, insert_media_batch};
pub use catalog::{DbImportCatalog, DbVerifyCatalog};
pub use connection::{ConnectionConfig, Role, haversine_km, open_connection, turkish_lowercase};
pub use error::DbError;
pub use migrate::{MIGRATIONS, Migration, MigrationReport, migrate, validate_schema};
pub use pool::{Pool, PoolConfig};

/// Çok dilli CLIP (M-CLIP XLM-RoBERTa Large + ViT-B/32) gömme boyutu.
/// `vec_media_clip` (migrasyon 0003) bu sayıyla oluşturulur; açılışta doğrulanır.
pub const CLIP_DIM: usize = 512;

/// ArcFace yüz gömme boyutu (`vec_faces`, `vec_face_centroids`).
pub const FACE_DIM: usize = 512;

pub type Result<T, E = DbError> = std::result::Result<T, E>;
