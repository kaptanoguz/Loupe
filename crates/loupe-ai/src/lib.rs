//! Loupe çevrimdışı AI motoru.
//!
//! - [`engine`]: çıkarım trait'leri (`FaceDetector`, `FaceEmbedder`,
//!   `ClipImageEncoder`, `ClipTextEncoder`).
//! - [`mock`]: model dosyası gerektirmeyen, milisaniyelik ve deterministik
//!   [`mock::DeterministicMockEngine`] (CI ve birim testleri).
//! - `onnx` (`ort` özelliği): SCRFD, ArcFace ve çok dilli CLIP (M-CLIP) ONNX
//!   Runtime üzerinde.
//! - [`lifecycle`]: tembel yükleme ve `idle_timeout` sonunda oturumu düşüren
//!   [`lifecycle::ManagedModel`] / [`lifecycle::IdleReaper`].
//! - [`faces`]: yüz kaydı ve artımlı kişi kümeleme (birleştir, ayır, taşı).
//! - [`clip`]: `vec_media_clip` yazma ve anlamsal benzerlik.
//! - [`events`]: zaman + konum tabanlı etkinlik algılama; kullanıcının
//!   dokunduğu etkinlikler asla ezilmez.
//! - [`models`] / [`fetch`]: model manifesti, SHA-256 doğrulaması ve atomik
//!   indirme (`loupe-models` CLI ve Ayarlar → Yapay Zekâ).
//! - [`jobs`]: `face_detect`, `clip_embed`, `cluster_faces`, `cluster_events`
//!   iş işleyicisi.

pub mod clip;
pub mod embedding;
pub mod engine;
pub mod error;
pub mod events;
pub mod faces;
pub mod fetch;
pub mod jobs;
pub mod lifecycle;
pub mod mock;
pub mod model;
pub mod models;
#[cfg(feature = "ort")]
pub mod onnx;

pub use embedding::Embedding;
pub use engine::{BBox, ClipImageEncoder, ClipTextEncoder, DetectedFace, FaceDetector, FaceEmbedder};
pub use error::AiError;
pub use events::{EventConfig, EventDetector, EventId, EventReport};
pub use faces::{ClusterConfig, ClusterId, FaceClusterer, FaceId, PersonId};
pub use jobs::{AiEngine, AiJobHandler};
pub use lifecycle::{IdleReaper, ManagedModel, Unloadable};
pub use mock::DeterministicMockEngine;
pub use model::{ModelId, ModelInfo, ModelTask};
