//! Loupe Wi-Fi senkronizasyonu (RFC-001B §2, RFC-001D).
//!
//! - [`discovery`]: `_loupe._tcp.local.` duyurusu. Gerçek ağda mDNS
//!   ([`discovery::MdnsDiscovery`]), testlerde bellek içi kayıt.
//! - [`identity`]: yerel CA + sunucu sertifikası (`rcgen`, ECDSA P-256) ve
//!   telefonun CSR'ından istemci sertifikası imzalama.
//! - [`pairing`]: 60 sn'lik tek kullanımlık eşleşme belirteci ve
//!   `loupe://pair?…` QR yükü.
//! - [`tls`]: `rustls` sunucu yapılandırması (eşleşmiş cihaz doğrulayıcısı) ve
//!   sabitlenmiş sunucu doğrulayıcısıyla istemci yapılandırması.
//! - [`conflict`]: aynı `device_asset_id`, farklı BLAKE3: politika kararları.
//! - [`tus`]: tus 1.0 oturumları; parça → fsync → offset, sonunda BLAKE3 ve
//!   atomik içe aktarma.
//! - [`telemetry`]: bayt/sn, ETA ve doğrulanan dosya sayısı yayını.
//! - [`server`]: HTTP/2 + TLS 1.3 sunucusu ve uç noktaları.

pub mod clock;
pub mod conflict;
pub mod discovery;
pub mod error;
pub mod identity;
pub mod pairing;
pub mod server;
pub mod store;
pub mod telemetry;
pub mod tls;
pub mod tus;

pub use clock::{Clock, ManualClock, SystemClock};
pub use conflict::{Classification, ConflictPolicy, Decision, Resolution, UploadKind};
pub use discovery::{Discovery, MemoryDiscovery, SERVICE_TYPE, ServiceAd};
pub use error::LanError;
pub use identity::{IdentityPem, IssuedClientCert, SERVER_NAME, ServerIdentity};
pub use pairing::{PAIRING_TTL, PairingManager, PairingPayload, PairingToken};
pub use server::{AutoApprove, LanConfig, LanServer, PairingApprover, PairingRequestInfo};
pub use telemetry::{SyncStatus, Telemetry};
pub use tls::{DeviceRegistry, ServerPin, client_config, server_config};

/// Tek bir PATCH isteğinin en büyük gövdesi (RFC-001B §2.6: 8 MiB parça).
pub const MAX_PATCH_BYTES: u64 = 8 * 1024 * 1024;
/// Yarım kalan yükleme oturumunun ömrü.
pub const UPLOAD_TTL_MS: i64 = 7 * 24 * 3600 * 1000;
