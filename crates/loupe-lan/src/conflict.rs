//! Telefon çakışması (RFC-001B §2.7).
//!
//! Çakışma = telefondaki varlık daha önce aktarılmış (`device_assets`'te aynı
//! `device_asset_id`) **ve** BLAKE3'ü farklı: telefonda düzenlenmiş. Yalnızca
//! dosya adının aynı olması çakışma değildir.

use loupe_core::{Blake3Hash, MediaId};
use rusqlite::{Connection, OptionalExtension};

use crate::LanError;

/// `devices.conflict_policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Telefona ve masaüstüne sor (varsayılan).
    AskUser,
    /// Yeni sürümü tut; eskisi çöp kutusuna (30 gün geri alınabilir).
    ReplaceWithNewer,
    /// İkisini de tut; yenisi `_1` ekiyle yazılır, ikisi aynı yakın kopya grubunda.
    KeepBoth,
    /// Aktarma; bu sürüm bir daha sorulmaz.
    Skip,
}

impl ConflictPolicy {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::AskUser => "ask",
            Self::ReplaceWithNewer => "keep_newest",
            Self::KeepBoth => "keep_both",
            Self::Skip => "skip",
        }
    }

    pub fn from_db(s: &str) -> Result<Self, LanError> {
        Ok(match s {
            "ask" => Self::AskUser,
            "keep_newest" => Self::ReplaceWithNewer,
            "keep_both" => Self::KeepBoth,
            "skip" => Self::Skip,
            other => return Err(LanError::BadRequest(format!("bilinmeyen çakışma politikası: {other}"))),
        })
    }
}

/// Kullanıcının (ya da telefonun) bir çakışma için verdiği karar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    ReplaceWithNewer,
    KeepBoth,
    Skip,
}

impl Resolution {
    pub fn parse(s: &str) -> Result<Self, LanError> {
        Ok(match s {
            "replace_with_newer" | "keep_newest" => Self::ReplaceWithNewer,
            "keep_both" => Self::KeepBoth,
            "skip" => Self::Skip,
            other => return Err(LanError::BadRequest(format!("bilinmeyen çözüm: {other}"))),
        })
    }
}

/// Manifest öğesinin katalog karşısındaki durumu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    /// Aynı içerik zaten katalogda (bu telefondan ya da başka yerden) veya bu
    /// sürüm daha önce "atla" ile kapatıldı: ağdan hiç geçmez.
    AlreadySynced { media_id: Option<MediaId> },
    /// Bu varlık hiç aktarılmamış.
    New,
    /// Aynı `device_asset_id`, farklı BLAKE3.
    Conflict { previous: Option<MediaId> },
}

/// Bir yüklemenin katalogdaki etkisi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadKind {
    New,
    KeepBoth { previous: MediaId },
    ReplaceWithNewer { previous: MediaId },
}

impl UploadKind {
    #[must_use]
    pub const fn resolution_db(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::KeepBoth { .. } => "keep_both",
            Self::ReplaceWithNewer { .. } => "keep_newest",
        }
    }

    #[must_use]
    pub const fn previous(self) -> Option<MediaId> {
        match self {
            Self::New => None,
            Self::KeepBoth { previous } | Self::ReplaceWithNewer { previous } => Some(previous),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    AlreadySynced {
        media_id: Option<MediaId>,
    },
    Skip,
    Upload(UploadKind),
    /// Politika "sor" ve istemci karar göndermedi.
    Ask {
        previous: Option<MediaId>,
    },
}

/// Sınıflandırma + cihaz politikası + (varsa) istemcinin kararı → ne yapılacak.
#[must_use]
pub fn decide(class: Classification, policy: ConflictPolicy, requested: Option<Resolution>) -> Decision {
    match class {
        Classification::AlreadySynced { media_id } => Decision::AlreadySynced { media_id },
        Classification::New => Decision::Upload(UploadKind::New),
        Classification::Conflict { previous } => {
            let choice = match (requested, policy) {
                (Some(r), _) => r,
                (None, ConflictPolicy::ReplaceWithNewer) => Resolution::ReplaceWithNewer,
                (None, ConflictPolicy::KeepBoth) => Resolution::KeepBoth,
                (None, ConflictPolicy::Skip) => Resolution::Skip,
                (None, ConflictPolicy::AskUser) => return Decision::Ask { previous },
            };
            match (choice, previous) {
                (Resolution::Skip, _) => Decision::Skip,
                // Eski medya katalogdan tamamen silinmişse yeni bir aktarım gibidir.
                (_, None) => Decision::Upload(UploadKind::New),
                (Resolution::KeepBoth, Some(previous)) => Decision::Upload(UploadKind::KeepBoth { previous }),
                (Resolution::ReplaceWithNewer, Some(previous)) => {
                    Decision::Upload(UploadKind::ReplaceWithNewer { previous })
                }
            }
        }
    }
}

/// `device_assets` ve `media_files.blake3` ile sınıflandırır (salt okunur).
pub fn classify(
    conn: &Connection,
    device_id: i64,
    device_asset_id: &str,
    blake3: &Blake3Hash,
) -> Result<Classification, LanError> {
    if let Some(m) = loupe_db::repo::media::find_by_blake3(conn, blake3)? {
        return Ok(Classification::AlreadySynced { media_id: Some(m) });
    }
    let prev: Option<(Vec<u8>, Option<i64>)> = conn
        .query_row(
            "SELECT blake3, media_id FROM device_assets WHERE device_id = ?1 AND client_asset_id = ?2",
            rusqlite::params![device_id, device_asset_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match prev {
        None => Classification::New,
        Some((b, media)) if b.as_slice() == blake3.as_bytes() => {
            Classification::AlreadySynced { media_id: media.map(MediaId) }
        }
        Some((_, media)) => {
            // Önceki medya hâlâ duruyor mu? Kalıcı silinmişse bağ NULL'dur.
            Classification::Conflict { previous: media.map(MediaId) }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: MediaId = MediaId(7);

    #[test]
    fn decisions_follow_policy_and_explicit_resolution() {
        let c = Classification::Conflict { previous: Some(P) };
        assert_eq!(decide(c, ConflictPolicy::AskUser, None), Decision::Ask { previous: Some(P) });
        assert_eq!(
            decide(c, ConflictPolicy::AskUser, Some(Resolution::KeepBoth)),
            Decision::Upload(UploadKind::KeepBoth { previous: P })
        );
        assert_eq!(
            decide(c, ConflictPolicy::ReplaceWithNewer, None),
            Decision::Upload(UploadKind::ReplaceWithNewer { previous: P })
        );
        assert_eq!(decide(c, ConflictPolicy::KeepBoth, Some(Resolution::Skip)), Decision::Skip);
        assert_eq!(decide(c, ConflictPolicy::Skip, None), Decision::Skip);
        assert_eq!(
            decide(Classification::Conflict { previous: None }, ConflictPolicy::KeepBoth, None),
            Decision::Upload(UploadKind::New)
        );
        assert_eq!(decide(Classification::New, ConflictPolicy::Skip, None), Decision::Upload(UploadKind::New));
        assert_eq!(
            decide(Classification::AlreadySynced { media_id: Some(P) }, ConflictPolicy::AskUser, None),
            Decision::AlreadySynced { media_id: Some(P) }
        );
    }

    #[test]
    fn policy_names_match_schema() {
        for p in
            [ConflictPolicy::AskUser, ConflictPolicy::ReplaceWithNewer, ConflictPolicy::KeepBoth, ConflictPolicy::Skip]
        {
            assert_eq!(ConflictPolicy::from_db(p.as_db()).unwrap(), p);
        }
        assert!(ConflictPolicy::from_db("newest").is_err());
    }
}
