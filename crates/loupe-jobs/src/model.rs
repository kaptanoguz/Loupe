//! Kuyruk veri tipleri: öncelik, iş türü, iş ve iş hatası.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use loupe_core::MediaId;

/// `jobs.base_priority` (0 en acil). Ara değerler (1, 3, 4, 6, 8) RFC-001B'deki
/// P sınıfları içindir; API beş adlandırılmış seviye sunar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Şu an ekranda görünen medya (viewport).
    Immediate,
    /// Açık denetçi / QuickLook ve komşuları.
    Interactive,
    /// Yeni içe aktarılanlar.
    Normal,
    /// Kütüphanenin geri kalanı (toplu tarama).
    Background,
    /// Kümeleme, sidecar yazımı gibi bekleyebilecek işler.
    Idle,
}

impl Priority {
    #[must_use]
    pub const fn level(self) -> u8 {
        match self {
            Self::Immediate => 0,
            Self::Interactive => 2,
            Self::Normal => 5,
            Self::Background => 7,
            Self::Idle => 9,
        }
    }

    /// Ara değerler bir alttaki adlandırılmış seviyeye yuvarlanır.
    #[must_use]
    pub const fn from_level(level: u8) -> Self {
        match level {
            0..=1 => Self::Immediate,
            2..=4 => Self::Interactive,
            5..=6 => Self::Normal,
            7..=8 => Self::Background,
            _ => Self::Idle,
        }
    }
}

macro_rules! text_enum {
    ($(#[$doc:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl FromStr for $name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, String> {
                match s {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(format!("bilinmeyen {}: {s}", stringify!($name))),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

text_enum!(
    /// `jobs.kind` (şemadaki CHECK kümesiyle birebir).
    JobKind {
        Thumb => "thumb",
        MetadataFull => "metadata_full",
        Phash => "phash",
        Geocode => "geocode",
        FaceDetect => "face_detect",
        ClipEmbed => "clip_embed",
        ClusterFaces => "cluster_faces",
        ClusterEvents => "cluster_events",
        SidecarWrite => "sidecar_write",
    }
);

text_enum!(
    /// `jobs.state`. Başarılı iş satırı silinir; `dead` dead-letter kuyruğudur.
    JobState {
        Pending => "pending",
        Running => "running",
        Failed => "failed",
        Dead => "dead",
    }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub i64);

/// Kuyruğa eklenecek iş. Aynı `(kind, media_id, variant)` ikinci kez eklenirse
/// yeni satır açılmaz; daha acil olan öncelik kalır.
///
/// `delay` verilen işler sondan tetiklemeli debounce ile çalışır: bekleyen iş
/// yeniden eklendikçe başlangıcı ileri kayar. Medyasız (küresel) iş çalışırken
/// yeniden eklenirse bittiğinde bir kez daha çalışır; istek kaybolmaz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub kind: JobKind,
    pub media_id: Option<MediaId>,
    /// `thumb` için `sm` | `md` | `lg`; diğer türlerde genelde boş.
    pub variant: String,
    pub priority: Priority,
    /// `None` = şema varsayılanı (5).
    pub max_attempts: Option<u32>,
    /// `None` = hemen hazır.
    pub delay: Option<Duration>,
}

impl NewJob {
    #[must_use]
    pub fn media(kind: JobKind, media_id: MediaId, variant: &str, priority: Priority) -> Self {
        Self { kind, media_id: Some(media_id), variant: variant.to_owned(), priority, max_attempts: None, delay: None }
    }

    /// Medyaya bağlı olmayan iş (`cluster_faces`, `cluster_events`): kütüphanede tektir.
    #[must_use]
    pub fn global(kind: JobKind, priority: Priority) -> Self {
        Self { kind, media_id: None, variant: String::new(), priority, max_attempts: None, delay: None }
    }

    #[must_use]
    pub const fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    #[must_use]
    pub const fn with_max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = Some(n);
        self
    }
}

/// İşçinin aldığı (kiraladığı) iş.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub media_id: Option<MediaId>,
    pub variant: String,
    /// 0–9; [`Priority::from_level`] ile adlandırılır.
    pub priority: u8,
    /// Bu çalıştırma dahil kaç kez başlatıldı.
    pub attempts: u32,
    pub max_attempts: u32,
}

/// İşin kuyruktaki son durumu (izleme ve testler için).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRecord {
    pub id: JobId,
    pub kind: JobKind,
    pub media_id: Option<MediaId>,
    pub variant: String,
    pub priority: u8,
    pub state: JobState,
    pub attempts: u32,
    pub max_attempts: u32,
    pub run_after: i64,
    pub last_error_code: Option<String>,
    pub last_error_msg: Option<String>,
}

/// İş işleyicisinin hatası. `retryable = false` ise iş doğrudan dead-letter'a gider.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct JobError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl JobError {
    /// Geçici hata (disk takılı değil, kilit, ağ): geri çekilerek yeniden denenir.
    #[must_use]
    pub fn transient(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into(), retryable: true }
    }

    /// Kalıcı hata (bozuk dosya, desteklenmeyen biçim): yeniden denenmez.
    #[must_use]
    pub fn permanent(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into(), retryable: false }
    }
}

/// Kuyruk sayaçları.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub pending: u64,
    pub running: u64,
    pub failed: u64,
    pub dead: u64,
}
