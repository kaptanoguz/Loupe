//! Şemadaki `CHECK (x IN (...))` kümelerinin Rust karşılıkları.
//! Her enum'un `as_str` değeri SQL'deki metinle birebir aynıdır.

use std::fmt;
use std::str::FromStr;

use crate::CoreError;

macro_rules! sql_enum {
    ($(#[$doc:meta])* $name:ident : $kind:literal { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl FromStr for $name {
            type Err = CoreError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(CoreError::UnknownVariant { kind: $kind, value: s.to_owned() }),
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

sql_enum!(
    /// Kullanıcının seçtiği import modu.
    ImportMode: "import modu" {
        Copy => "copy",
        Move => "move",
        Hardlink => "hardlink",
    }
);

sql_enum!(
    /// `media_files.placement`: dosyanın kütüphaneye nasıl geldiği.
    Placement: "placement" {
        Copy => "copy",
        Move => "move",
        Hardlink => "hardlink",
        Reference => "reference",
        Lan => "lan",
    }
);

impl From<ImportMode> for Placement {
    fn from(m: ImportMode) -> Self {
        match m {
            ImportMode::Copy => Self::Copy,
            ImportMode::Move => Self::Move,
            ImportMode::Hardlink => Self::Hardlink,
        }
    }
}

sql_enum!(
    /// `import_sessions.mode`
    SessionMode: "oturum modu" {
        Copy => "copy",
        Move => "move",
        Hardlink => "hardlink",
        Reference => "reference",
        Watch => "watch",
        Lan => "lan",
    }
);

impl From<ImportMode> for SessionMode {
    fn from(m: ImportMode) -> Self {
        match m {
            ImportMode::Copy => Self::Copy,
            ImportMode::Move => Self::Move,
            ImportMode::Hardlink => Self::Hardlink,
        }
    }
}

sql_enum!(
    /// `import_sessions.state`
    SessionState: "oturum durumu" {
        Scanning => "scanning",
        Running => "running",
        Paused => "paused",
        Done => "done",
        Failed => "failed",
        Cancelled => "cancelled",
    }
);

sql_enum!(
    /// `import_items.state` (RFC-001A §3.4 durum makinesi)
    ImportItemState: "import öğesi durumu" {
        Pending => "pending",
        Hashed => "hashed",
        Staged => "staged",
        Placed => "placed",
        Committed => "committed",
        SourceRemoved => "source_removed",
        SkippedDuplicate => "skipped_duplicate",
        Failed => "failed",
    }
);

sql_enum!(
    /// `libraries.kind`
    LibraryKind: "kütüphane türü" {
        Internal => "internal",
        Removable => "removable",
        Network => "network",
    }
);

sql_enum!(
    /// `libraries.state`
    LibraryState: "kütüphane durumu" {
        Online => "online",
        Offline => "offline",
        Readonly => "readonly",
        Error => "error",
    }
);

sql_enum!(
    /// `media_files.file_type`
    MediaFileType: "dosya türü" {
        Jpeg => "jpeg",
        Heic => "heic",
        Avif => "avif",
        Png => "png",
        Tiff => "tiff",
        Webp => "webp",
        Jxl => "jxl",
        Raw => "raw",
        Dng => "dng",
        Video => "video",
        Other => "other",
    }
);

sql_enum!(
    /// `media_files.taken_at_source`
    TakenAtSource: "çekim zamanı kaynağı" {
        Exif => "exif",
        Gps => "gps",
        Filename => "filename",
        Mtime => "mtime",
        User => "user",
    }
);

sql_enum!(
    /// `media_files.verify_state`
    VerifyState: "doğrulama durumu" {
        Ok => "ok",
        Missing => "missing",
        Mismatch => "mismatch",
        Unreadable => "unreadable",
        Offline => "offline",
    }
);

sql_enum!(
    /// `verify_runs.trigger`
    VerifyTrigger: "doğrulama tetikleyicisi" {
        Manual => "manual",
        Scheduled => "scheduled",
        Remount => "remount",
    }
);

sql_enum!(
    /// `media_files.color_label` (renk etiketi; arayüzde 2px köşeli kare)
    ColorLabel: "renk etiketi" {
        Red => "red",
        Yellow => "yellow",
        Green => "green",
        Blue => "blue",
        Purple => "purple",
    }
);

sql_enum!(
    /// `media_files.flag`
    Flag: "işaret" {
        None => "none",
        Pick => "pick",
        Reject => "reject",
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all() {
        for v in Placement::ALL {
            assert_eq!(v.as_str().parse::<Placement>(), Ok(*v));
        }
        for v in ImportItemState::ALL {
            assert_eq!(v.as_str().parse::<ImportItemState>(), Ok(*v));
        }
        assert!("symlink".parse::<ImportMode>().is_err());
    }
}
