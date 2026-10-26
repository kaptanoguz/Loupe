//! Kabloyla bağlı iPhone'u bulma. GNOME/Cinnamon'da gvfs, iPhone'un fotoğraf
//! deposunu (AFC) `$XDG_RUNTIME_DIR/gvfs/afc:host=<UDID>/` altına bağlar;
//! `DCIM` klasörü sıradan bir içe aktarma kaynağıdır. Bağlı değilse
//! `idevice_id -l` ile görülen cihazlar `gio mount afc://<UDID>/` ile bağlanır.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

/// `gio mount` telefonda "Bu bilgisayara güven" onayını bekleyebilir.
const MOUNT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IphoneDto {
    pub udid: String,
    /// İçe aktarma kaynağı: `…/DCIM`.
    pub dcim: String,
}

/// Bağlı iPhone'ların DCIM klasörleri; gerekirse önce bağlar.
#[must_use]
pub fn find() -> Vec<IphoneDto> {
    let Some(gvfs) = gvfs_dir() else { return Vec::new() };
    let found = scan(&gvfs);
    if !found.is_empty() {
        return found;
    }
    let mut mounted = false;
    for udid in connected_udids() {
        mounted |= mount(&udid);
    }
    if mounted { scan(&gvfs) } else { Vec::new() }
}

fn gvfs_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("gvfs"))
}

/// `afc:host=<UDID>` bağlantıları. `,port=3` gibi ekler belge (uygulama)
/// deposudur, fotoğraflar değil.
#[must_use]
pub fn scan(gvfs: &Path) -> Vec<IphoneDto> {
    let Ok(entries) = std::fs::read_dir(gvfs) else { return Vec::new() };
    let mut out: Vec<IphoneDto> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let udid = name.strip_prefix("afc:host=")?;
            if udid.is_empty() || udid.contains(',') {
                return None;
            }
            let dcim = e.path().join("DCIM");
            dcim.is_dir().then(|| IphoneDto { udid: udid.to_owned(), dcim: dcim.display().to_string() })
        })
        .collect();
    out.sort_by(|a, b| a.udid.cmp(&b.udid));
    out
}

fn connected_udids() -> Vec<String> {
    let Ok(out) = Command::new("idevice_id").arg("-l").stderr(Stdio::null()).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .map(str::to_owned)
        .collect()
}

fn mount(udid: &str) -> bool {
    let Ok(mut child) = Command::new("gio")
        .args(["mount", &format!("afc://{udid}/")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < MOUNT_TIMEOUT => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ignored = child.kill();
                let _ignored = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_photo_store_and_ignores_document_store() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("afc:host=00008110-ABC/DCIM/100APPLE")).unwrap();
        std::fs::create_dir_all(dir.path().join("afc:host=00008110-ABC,port=3/Documents")).unwrap();
        std::fs::create_dir_all(dir.path().join("afc:host=LOCKED")).unwrap();
        std::fs::create_dir_all(dir.path().join("smb-share:server=nas/DCIM")).unwrap();
        let found = scan(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].udid, "00008110-ABC");
        assert!(found[0].dcim.ends_with("afc:host=00008110-ABC/DCIM"));
        assert!(scan(&dir.path().join("yok")).is_empty());
    }
}
