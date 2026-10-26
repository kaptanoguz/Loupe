//! Uygulama dizinleri ve açılış ayarları.

use std::path::{Path, PathBuf};
use std::time::Duration;

use loupe_jobs::WorkerConfig;
use loupe_storage::volume::VolumeProbe;

use crate::ai::AiMode;
use crate::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct AppConfig {
    /// `catalog.db`, `lan/identity.json`, `geonames/` (XDG_DATA_HOME/loupe).
    pub data_dir: PathBuf,
    /// Küçük resim önbelleği `thumbs/` (XDG_CACHE_HOME/loupe).
    pub cache_dir: PathBuf,
    /// mDNS örnek adı ve LAN sertifikasının CN'i.
    pub device_name: String,
    /// `None`: işçi havuzu başlatılmaz (testler kuyruğu elle çalıştırır).
    pub workers: Option<WorkerConfig>,
    /// Çevrimiçi kütüphaneleri inotify ile izle.
    pub watch: bool,
    pub watch_debounce: Duration,
    pub volume_probe: VolumeProbe,
    /// Yüz/CLIP modelleri: `onnx`, `mock` ya da `off` (etkinlikler yine kümelenir).
    pub ai: AiMode,
    /// Sırlar için Linux Secret Service denensin mi (testlerde kapalı: gerçek anahtarlığa yazılmaz).
    pub secret_service: bool,
}

impl AppConfig {
    /// Gerçek kurulum: Linux'ta XDG dizinleri, macOS'ta `~/Library`, Windows'ta
    /// `%LOCALAPPDATA%\Loupe` (bkz. [`loupe_ai::models::platform_dirs`]).
    pub fn from_env() -> AppResult<Self> {
        let (data_dir, cache_dir) = loupe_ai::models::platform_dirs()
            .ok_or_else(|| AppError::Invalid("kullanıcı dizini bulunamadı (HOME / LOCALAPPDATA tanımsız)".into()))?;
        let device_name = std::fs::read_to_string("/etc/hostname")
            .ok()
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Loupe".to_owned());
        Ok(Self {
            ai: AiMode::from_env(&data_dir),
            secret_service: true,
            data_dir,
            cache_dir,
            device_name,
            workers: Some(WorkerConfig::default()),
            watch: true,
            watch_debounce: Duration::from_secs(2),
            volume_probe: VolumeProbe::default(),
        })
    }

    /// Testler: her şey tek dizinde, işçi ve izleyici kapalı.
    #[must_use]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
            device_name: "Loupe Test".to_owned(),
            workers: None,
            watch: false,
            watch_debounce: Duration::from_millis(200),
            volume_probe: VolumeProbe::default(),
            ai: AiMode::Off { reason: "test".into() },
            secret_service: false,
        }
    }

    #[must_use]
    pub fn catalog_path(&self) -> PathBuf {
        self.data_dir.join("catalog.db")
    }

    #[must_use]
    pub fn thumbs_dir(&self) -> PathBuf {
        self.cache_dir.join("thumbs")
    }

    /// `thumb://orig` için döndürülmüş tam boy kopyalar.
    #[must_use]
    pub fn full_dir(&self) -> PathBuf {
        self.cache_dir.join("full")
    }

    #[must_use]
    pub fn lan_identity_path(&self) -> PathBuf {
        self.data_dir.join("lan").join("identity.json")
    }

    #[must_use]
    pub fn geonames_dir(&self) -> PathBuf {
        self.data_dir.join("geonames")
    }
}
