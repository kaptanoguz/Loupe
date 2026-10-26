//! Seçili fotoğrafları diskten silme. Dosyalar kalıcı silinmez: freedesktop
//! Çöp Kutusu'na taşınır (`gio trash`; ev dizinindekiler `~/.local/share/Trash`,
//! başka disktekiler o diskin `.Trash-<uid>` klasörü). Dosya yöneticisinden
//! geri alınabilir; geri gelen dosyayı klasör izleyicisi yeniden kataloglar.
//! Loupe'un sidecar'ları (`.xmp`, `.loupe.json`) dosyayla birlikte taşınır.
//!
//! Bazı disklerde sistem Çöp Kutusu kurulamaz (ör. `uid` verilmeden bağlanan
//! ntfs-3g: `.Trash-<uid>` kullanıcıya ait olamaz). O zaman dosya aynı diskte
//! kütüphanenin `.loupe/trash/<tarih>/` klasörüne taşınır (yeniden adlandırma,
//! kopya yok); taramalar gizli klasörlere girmez. Geri almak için dosya oradan
//! eski yerine taşınıp klasör yeniden içe aktarılır.

use std::path::{Path, PathBuf};
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
    /// `trashed` içinden, sistem Çöp Kutusu yerine `.loupe/trash/`e taşınanlar.
    pub loupe_trash: u64,
    /// Örnek bir `.loupe/trash` klasörü (kullanıcıya gösterilir).
    pub loupe_trash_dir: Option<String>,
    pub failed: Vec<TrashFailure>,
}

/// Dosyaları Çöp Kutusu'na taşıyan arka uç (testlerde sahte).
pub trait Trasher: Send + Sync {
    /// Dosyaları taşır; taşınamayanlar diskte kalır (çağıran varlığa bakar).
    /// Dönen metin aracın hata çıktısıdır (tanı için; boş olabilir).
    fn trash(&self, paths: &[PathBuf]) -> AppResult<String>;
}

/// `gio trash` (GLib; GNOME, Cinnamon, MATE, XFCE).
#[derive(Debug, Default, Clone, Copy)]
pub struct GioTrash;

impl Trasher for GioTrash {
    fn trash(&self, paths: &[PathBuf]) -> AppResult<String> {
        // Bir dosya taşınamasa da diğerleri taşınır; sonucu dosya varlığı söyler.
        let out = Command::new("gio")
            .arg("trash")
            .arg("--")
            .args(paths)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(AppError::io("gio trash (Çöp Kutusu) çalıştırılamadı"))?;
        Ok(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// Seçili medyayı Çöp Kutusu'na taşır ve kataloğdan çıkarır. Diski bağlı
/// olmayan ya da taşınamayan dosyalar katalogda kalır ve nedeniyle döner.
pub fn trash_media(pool: &Pool, libs: &Libraries, trasher: &dyn Trasher, ids: &[i64]) -> AppResult<TrashResultDto> {
    let mut out = TrashResultDto::default();
    let mut pending: Vec<(MediaId, PathBuf, PathBuf)> = Vec::new();
    let mut gone: Vec<MediaId> = Vec::new();
    for &id in ids {
        let id = MediaId(id);
        let Some(m) = pool.read(|c| media::get(c, id))? else { continue };
        match libs.root(m.library_id)? {
            None => out.failed.push(TrashFailure { id: id.0, reason: "disk bağlı değil".into() }),
            Some(root) => {
                let p = m.rel_path.to_path(&root);
                // Dosya zaten yok: yalnızca katalog kaydı kalmış.
                if p.exists() { pending.push((id, p, root)) } else { gone.push(id) }
            }
        }
    }
    let day = {
        let (y, m, d) = loupe_storage::layout::civil_from_days(loupe_core::now_ms().div_euclid(86_400_000));
        format!("{y:04}-{m:02}-{d:02}")
    };
    for chunk in pending.chunks(CHUNK) {
        let mut paths: Vec<PathBuf> = Vec::with_capacity(chunk.len() * 2);
        for (_, p, _) in chunk {
            paths.push(p.clone());
            paths.extend(sidecars(p));
        }
        let diag = trasher.trash(&paths)?;
        for (id, p, root) in chunk {
            if !p.exists() {
                gone.push(*id);
                continue;
            }
            let bin = root.join(".loupe").join("trash").join(&day);
            match move_into(root, &bin, p) {
                Ok(()) => {
                    gone.push(*id);
                    out.loupe_trash += 1;
                    out.loupe_trash_dir.get_or_insert_with(|| bin.display().to_string());
                }
                Err(e) => {
                    let why = diag.lines().last().filter(|l| !l.is_empty()).unwrap_or("Çöp Kutusu'na taşınamadı");
                    out.failed.push(TrashFailure { id: id.0, reason: format!("{why}; .loupe/trash: {e}") });
                }
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

fn sidecars(p: &Path) -> Vec<PathBuf> {
    let sc = Sidecar::for_media(p);
    [sc.json_path(), sc.xmp_path()].into_iter().filter(|s| s != p && s.is_file()).collect()
}

/// Dosyayı (ve sidecar'larını) kökteki göreli yoluyla `bin` altına taşır.
/// Aynı disk olduğundan `rename` yeter; varolanın üstüne yazılmaz.
fn move_into(root: &Path, bin: &Path, p: &Path) -> std::io::Result<()> {
    let rel = p.strip_prefix(root).map_err(|_| std::io::Error::other("kütüphane dışında"))?;
    let mut dest = bin.join(rel);
    let mut n = 1;
    while dest.exists() {
        let name = rel.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        dest = bin.join(rel).with_file_name(format!("{n}-{name}"));
        n += 1;
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let sides = sidecars(p);
    std::fs::rename(p, &dest)?;
    for side in sides {
        if let (Some(dir), Some(name)) = (dest.parent(), side.file_name()) {
            // Sidecar'ın adı medyanın adından türer; yeni adla eşleşsin.
            let base = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let new_base = dest.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let renamed = name.to_string_lossy().replacen(&base, &new_base, 1);
            let _ignored = std::fs::rename(&side, dir.join(renamed));
        }
    }
    Ok(())
}
