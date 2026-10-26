//! Loupe masaüstü uygulamasının çekirdeği.
//!
//! Tauri'ye bağımlı değildir: kabuk (`apps/desktop/src-tauri`) yalnızca
//! [`AppCore`] metotlarını komut olarak açar ve `thumb://` isteklerini
//! [`AppCore::handle_thumb`]'a verir. Böylece komutların tamamı başsız
//! (WebView olmadan) test edilir.

pub mod ai;
pub mod cloud;
pub mod config;
pub mod core;
pub mod culling;
pub mod error;
pub mod full;
pub mod import;
pub mod inspector;
pub mod iphone;
pub mod library;
pub mod models;
pub mod moments;
pub mod network;
pub mod people;
pub mod search;
pub mod secrets;
pub mod timeline;
pub mod trash;
pub mod verify;

pub use ai::{AiMode, AiStatusDto};
pub use cloud::{CloudSettingsDto, CloudTargetDto, CloudTargetInput, EncryptionDto, RecoveryDraftDto};
pub use config::AppConfig;
pub use core::{AppCore, QueueDto, SidebarDto, StatusDto, ViewCount};
pub use culling::{CullRequest, CullResult};
pub use error::{AppError, AppResult};
pub use import::{ImportRequest, ImportSessionDto};
pub use inspector::{Histogram, InspectorDetail};
pub use library::{LibraryDto, LibraryPatch};
pub use loupe_search::SearchQuery;
pub use models::{ModelJobDto, ModelsDto};
pub use moments::EventCard;
pub use network::{CloudTargetStatusDto, DeviceDto, LanStatusDto, PairingQrDto};
pub use people::{FaceRef, PeopleDto, PersonCard};
pub use search::{PaletteRequest, PaletteResult};
pub use secrets::{FileSecretStore, LayeredSecretStore, SecretServiceStore, SecretStore};
pub use timeline::{
    Arrange, Grouping, Section, SmartAlbumDto, SortDir, SortKey, TileItem, TimelineRequest, TimelineSlice, View,
};
pub use verify::VerifyRunDto;
