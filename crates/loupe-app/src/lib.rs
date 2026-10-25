//! Loupe masaüstü uygulamasının çekirdeği.
//!
//! Tauri'ye bağımlı değildir: kabuk (`apps/desktop/src-tauri`) yalnızca
//! [`AppCore`] metotlarını komut olarak açar ve `thumb://` isteklerini
//! [`AppCore::handle_thumb`]'a verir. Böylece komutların tamamı başsız
//! (WebView olmadan) test edilir.

pub mod config;
pub mod core;
pub mod culling;
pub mod error;
pub mod import;
pub mod inspector;
pub mod library;
pub mod network;
pub mod secrets;
pub mod timeline;

pub use config::AppConfig;
pub use core::{AppCore, QueueDto, SidebarDto, StatusDto, ViewCount};
pub use culling::{CullRequest, CullResult};
pub use error::{AppError, AppResult};
pub use import::{ImportRequest, ImportSessionDto};
pub use inspector::{Histogram, InspectorDetail};
pub use library::LibraryDto;
pub use network::{CloudTargetStatusDto, LanStatusDto};
pub use secrets::{FileSecretStore, SecretStore};
pub use timeline::{Grouping, Section, TileItem, TimelineRequest, TimelineSlice, View};
