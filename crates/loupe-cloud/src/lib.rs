//! Loupe 3-2-1 bulut yedeği (RFC-001B §3, RFC-001D).
//!
//! - [`crypto`]: istemci tarafı `age` (X25519) akış şifrelemesi. Her nesne iki
//!   alıcıya şifrelenir: cihaz anahtarı (günlük) ve kurtarma anahtarı (kâğıtta).
//! - [`provider`]: [`CloudProvider`] trait'i ve S3 uyumlu (AWS, B2, R2, Wasabi),
//!   WebDAV ve yerel/NAS uygulamaları.
//! - [`limiter`]: bayt/sn token bucket (hedef başına + genel).
//! - [`backoff`]: tam rastgele titreşimli exponential backoff.
//! - [`queue`]: `sync_queue` ↔ `sync_status` durum makinesi; her geçiş ve
//!   kuyruk değişikliği tek transaction'da.
//! - [`engine`]: kuyruktan iş alır; dosyayı okurken BLAKE3 hesaplar, şifreler,
//!   hız sınırıyla akıtır, uzak checksum'ı doğrular ve `sync_status`'u yazar.
//!   Diskte şifreli geçici kopya oluşmaz.

pub mod backoff;
pub mod crypto;
pub mod engine;
pub mod error;
pub mod limiter;
pub mod provider;
pub mod queue;
pub mod targets;

pub use crypto::{Keyring, RecoveryKey};
pub use engine::{BackupEngine, LibraryRoots, UploadOutcome, UploadReport, VerifyMode};
pub use error::{CloudError, ErrorClass};
pub use limiter::{Limiter, TokenBucket};
pub use provider::{
    ByteStream, ChecksumAlgo, CloudProvider, ObjectMeta, RemoteChecksum, UploadReceipt, local::LocalProvider,
    s3::S3Provider, webdav::WebDavProvider,
};
