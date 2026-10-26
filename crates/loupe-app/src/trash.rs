//! Seçili fotoğrafları diskten silme. Dosyalar kalıcı silinmez: freedesktop
//! Çöp Kutusu'na taşınır (`gio trash`; ev dizinindekiler `~/.local/share/Trash`,
//! başka disktekiler o diskin `.Trash-<uid>` klasörü). Dosya yöneticisinden
//! geri alınabilir; geri gelen dosyayı klasör izleyicisi yeniden kataloglar.
//! Loupe'un sidecar'ları (`.xmp`, `.loupe.json`) dosyayla birlikte taşınır.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use loupe_core::MediaId;
use loupe_db::Pool;
use loupe_db::repo::media;
use loupe_media::sidecar::Sidecar;
use serde::Serialize;

use crate::library::Libraries;
use crate::{AppError, AppResult};

/// Bir `gio trash` çağrısındaki en çok dosya (komut satırı sınırı).
const CHUNK: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashFailure {
    pub id: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashResultDto {
    pub trashed: u64,
    pub failed: Vec<TrashFailure>,
}

/// Dosyaları Çöp Kutusu'na taşıyan arka uç (testlerde sahte).
pub trait Trasher: Send + Sync {
    /// Dosyaları taşır; taşınamayanlar diskte kalır (çağıran varlığa bakar).
    fn trash(&self, paths: &[PathBuf]) -> AppResult<()>;
}

/// `gio trash` (GLib; GNOME, Cinnamon, MATE, XFCE).
#[derive(Debug, Default, Clone, Copy)]
pub struct GioTrash;

impl Trasher for GioTrash {
    fn trash(&self, paths: &[PathBuf]) -> AppResult<()> {
        // Bir dosya taşınamasa da diğerleri taşınır; sonucu dosya varlığı söyler.
        let _status = Command::new("gio")
            .arg("trash")
            .arg("--")
            .args(paths)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(AppError::io("gio trash (Çöp Kutusu) çalıştırılamadı"))?;
        Ok(())
    }
}

/// Seçili medyayı Çöp Kutusu'na taşır ve kataloğdan çıkarır. Diski bağlı
/// olmayan ya da taşınamayan dosyalar katalogda kalır ve nedeniyle döner.
pub fn trash_media(pool: &Pool, libs: &Libraries, trasher: &dyn Trasher, ids: &[i64]) -> AppResult<TrashResultDto> {
    let mut out = TrashResultDto::default();
    let mut pending: Vec<(MediaId, PathBuf)> = Vec::new();
    let mut gone: Vec<MediaId> = Vec::new();
    for &id in ids {
        let id = MediaId(id);
        let Some(m) = pool.read(|c| media::get(c, id))? else { continue };
        match libs.media_path(&m)? {
            None => out.failed.push(TrashFailure { id: id.0, reason: "disk bağlı değil".into() }),
            // Dosya zaten yok: yalnızca katalog kaydı kalmış.
            Some(p) if !p.exists() => gone.push(id),
            Some(p) => pending.push((id, p)),
        }
    }
    for chunk in pending.chunks(CHUNK) {
        let mut paths: Vec<PathBuf> = Vec::with_capacity(chunk.len() * 2);
        for (_, p) in chunk {
            paths.push(p.clone());
            let sc = Sidecar::for_media(p);
            for side in [sc.json_path(), sc.xmp_path()] {
                if side != *p && side.is_file() {
                    paths.push(side);
                }
            }
        }
        trasher.trash(&paths)?;
        for (id, p) in chunk {
            if p.exists() {
                out.failed.push(TrashFailure { id: id.0, reason: "Çöp Kutusu'na taşınamadı".into() });
            } else {
                gone.push(*id);
            }
        }
    }
    for batch in gone.chunks(500) {
        pool.write(|c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for id in batch {
                tx.execute("DELETE FROM sync_status WHERE media_id = ?1", [id.0])?;
                media::delete(&tx, *id)?;
            }
            tx.commit()?;
            Ok(())
        })?;
        out.trashed += batch.len() as u64;
    }
    Ok(out)
}
