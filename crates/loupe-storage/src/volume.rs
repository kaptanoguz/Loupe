//! Disk kimliği ve kütüphane işareti (RFC-001A §3.1).
//!
//! Kütüphane mutlak yolla değil `(volume_uuid, rel_root)` çiftiyle tanınır:
//! harici disk `/run/media/u/Arsiv` yerine `/run/media/u/Arsiv1` olarak takılsa
//! da bulunur. Ağ diskleri için anahtar `network_source`'tur (`nas:/photos`).
//! Doğruluğun son kaynağı kökteki `.loupe/library.json` işaretidir: işaret yoksa
//! veya başka kütüphaneye aitse kütüphane asla "yeni" sanılmaz, hata döner.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use loupe_core::LibraryKind;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fsutil::{self, fsync_dir};
use crate::{LOUPE_DIR, StorageError, TMP_DIR};

/// `/proc/self/mountinfo` satırı.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub mount_id: u32,
    pub major: u32,
    pub minor: u32,
    /// Bağlanan dosya sisteminin içindeki kök (bind mount'ta alt dizin).
    pub root: String,
    pub mount_point: PathBuf,
    pub fs_type: String,
    /// `/dev/nvme0n1p2`, `nas.local:/volume1/photos`, `//nas/photos`…
    pub source: String,
}

/// Ağ dosya sistemleri: UUID yok, kimlik `source`.
const NETWORK_FS: &[&str] = &["nfs", "nfs4", "cifs", "smb3", "smbfs", "9p", "ceph", "glusterfs", "fuse.sshfs"];

/// mountinfo alanlarındaki `\040` (boşluk) gibi sekizlik kaçışları çözer.
fn unescape_octal(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && let Some(oct) = b.get(i + 1..i + 4).filter(|o| o.iter().all(|c| (b'0'..=b'7').contains(c)))
        {
            let v = (u32::from(oct[0] - b'0') << 6) | (u32::from(oct[1] - b'0') << 3) | u32::from(oct[2] - b'0');
            if let Ok(byte) = u8::try_from(v) {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `/dev/disk/by-label` adlarındaki `\x20` kaçışlarını çözer.
fn unescape_hex(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && b.get(i + 1) == Some(&b'x')
            && let Some(v) = s.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_mountinfo(text: &str) -> Result<Vec<MountEntry>, StorageError> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let bad = || StorageError::MountInfoParse(line.to_owned());
        let (pre, post) = line.split_once(" - ").ok_or_else(bad)?;
        let pre: Vec<&str> = pre.split(' ').collect();
        let post: Vec<&str> = post.split(' ').collect();
        if pre.len() < 6 || post.len() < 2 {
            return Err(bad());
        }
        let (major, minor) = pre[2].split_once(':').ok_or_else(bad)?;
        out.push(MountEntry {
            mount_id: pre[0].parse().map_err(|_| bad())?,
            major: major.parse().map_err(|_| bad())?,
            minor: minor.parse().map_err(|_| bad())?,
            root: unescape_octal(pre[3]),
            mount_point: PathBuf::from(unescape_octal(pre[4])),
            fs_type: post[0].to_owned(),
            source: unescape_octal(post[1]),
        });
    }
    Ok(out)
}

/// Bir klasörün bulunduğu disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    pub mount_point: PathBuf,
    /// Mount noktasına göre kök; mount noktasının kendisiyse boş.
    pub rel_root: String,
    pub fs_type: String,
    pub source: String,
    pub uuid: Option<String>,
    pub label: Option<String>,
    pub network_source: Option<String>,
    pub kind: LibraryKind,
}

/// `/dev/disk/by-*` bağlantısı: ad + hedef.
#[derive(Debug, Clone)]
struct DiskLink {
    name: String,
    target: PathBuf,
    rdev: Option<(u32, u32)>,
}

/// Disk sorgulayıcı. Yollar testte sahte dizinlerle değiştirilebilir.
#[derive(Debug, Clone)]
pub struct VolumeProbe {
    pub mountinfo: PathBuf,
    pub by_uuid: PathBuf,
    pub by_label: PathBuf,
}

impl Default for VolumeProbe {
    fn default() -> Self {
        Self {
            mountinfo: PathBuf::from("/proc/self/mountinfo"),
            by_uuid: PathBuf::from("/dev/disk/by-uuid"),
            by_label: PathBuf::from("/dev/disk/by-label"),
        }
    }
}

impl VolumeProbe {
    pub fn mounts(&self) -> Result<Vec<MountEntry>, StorageError> {
        let text = fs::read_to_string(&self.mountinfo).map_err(StorageError::io("mountinfo okuma", &self.mountinfo))?;
        parse_mountinfo(&text)
    }

    fn links(dir: &Path, hex_names: bool) -> Vec<DiskLink> {
        let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
        rd.filter_map(Result::ok)
            .filter_map(|e| {
                let raw = e.file_name().to_str()?.to_owned();
                let target = fs::canonicalize(e.path()).ok()?;
                let rdev = fs::metadata(&target)
                    .ok()
                    .filter(|m| m.file_type().is_block_device())
                    .map(|m| (rustix::fs::major(m.rdev()), rustix::fs::minor(m.rdev())));
                let name = if hex_names { unescape_hex(&raw) } else { raw };
                Some(DiskLink { name, target, rdev })
            })
            .collect()
    }

    /// Mount satırının disk bağlantısı: önce aygıt numarası (blok aygıtı), sonra
    /// kaynak yolunun gerçek hedefi. Btrfs alt birimleri ve LVM ikinciyle bulunur.
    fn match_link<'l>(links: &'l [DiskLink], m: &MountEntry) -> Option<&'l DiskLink> {
        let by_dev = links.iter().find(|l| l.rdev == Some((m.major, m.minor)));
        by_dev.or_else(|| {
            let src = Path::new(&m.source);
            if !src.is_absolute() {
                return None;
            }
            let canon = fs::canonicalize(src).ok()?;
            links.iter().find(|l| l.target == canon)
        })
    }

    fn describe(m: &MountEntry, uuids: &[DiskLink], labels: &[DiskLink]) -> (Option<String>, Option<String>) {
        let uuid = Self::match_link(uuids, m).map(|l| l.name.clone());
        let label = Self::match_link(labels, m).map(|l| l.name.clone());
        (uuid, label)
    }

    /// Klasörün diskini bulur: en uzun eşleşen mount noktası; aynı uzunlukta
    /// birden çok varsa (üst üste mount) son bağlanan.
    pub fn resolve(&self, folder: &Path) -> Result<VolumeInfo, StorageError> {
        let folder = fs::canonicalize(folder).map_err(StorageError::io("yol çözme", folder))?;
        let mounts = self.mounts()?;
        let m = mounts
            .iter()
            .filter(|m| folder.starts_with(&m.mount_point))
            .max_by_key(|m| m.mount_point.components().count())
            .ok_or_else(|| StorageError::MountNotFound(folder.clone()))?;
        let rel_root = folder.strip_prefix(&m.mount_point).ok().and_then(Path::to_str).unwrap_or_default().to_owned();
        let network = NETWORK_FS.contains(&m.fs_type.as_str());
        let (uuid, label) = if network {
            (None, None)
        } else {
            Self::describe(m, &Self::links(&self.by_uuid, false), &Self::links(&self.by_label, true))
        };
        let removable_path = m.mount_point.starts_with("/run/media") || m.mount_point.starts_with("/media");
        let kind = if network {
            LibraryKind::Network
        } else if removable_path && uuid.is_some() {
            LibraryKind::Removable
        } else {
            LibraryKind::Internal
        };
        Ok(VolumeInfo {
            mount_point: m.mount_point.clone(),
            rel_root,
            fs_type: m.fs_type.clone(),
            source: m.source.clone(),
            uuid,
            label,
            network_source: network.then(|| m.source.clone()),
            kind,
        })
    }

    /// Kütüphaneyi şu an bağlı disklerde arar (disk yeniden takıldığında).
    ///
    /// 1. Son bilinen mount noktasında işaret eşleşiyorsa orası.
    /// 2. Değilse aynı UUID'li (ağ diskinde aynı kaynaklı) her mount noktasında
    ///    `<mount>/<rel_root>/.loupe/library.json` okunur; kimlik eşleşmeli.
    /// 3. Disk hiç bağlı değilse `VolumeNotMounted`; bağlı ama işaret yok veya
    ///    farklıysa ilgili hata (kütüphane yeni sanılmaz).
    pub fn locate(&self, loc: &LibraryLocator<'_>) -> Result<Located, StorageError> {
        if let Some(last) = loc.last_mount_point {
            let root = join_root(last, loc.rel_root);
            if matches!(LibraryMarker::read(&root), Ok(mk) if mk.library_uuid == loc.library_uuid) {
                return Ok(Located { root, mount_point: last.to_path_buf(), moved: false });
            }
        }

        let mounts = self.mounts()?;
        let uuids = Self::links(&self.by_uuid, false);
        let mut first_err: Option<StorageError> = None;
        let mut volume_seen = false;
        for m in &mounts {
            let same_volume = match (loc.volume_uuid, loc.network_source) {
                (Some(want), _) => Self::match_link(&uuids, m).is_some_and(|l| l.name.eq_ignore_ascii_case(want)),
                (None, Some(src)) => m.source == src,
                (None, None) => false,
            };
            if !same_volume {
                continue;
            }
            volume_seen = true;
            let root = join_root(&m.mount_point, loc.rel_root);
            match LibraryMarker::read(&root) {
                Ok(mk) if mk.library_uuid == loc.library_uuid => {
                    let moved = loc.last_mount_point != Some(m.mount_point.as_path());
                    return Ok(Located { root, mount_point: m.mount_point.clone(), moved });
                }
                Ok(mk) => {
                    first_err.get_or_insert(StorageError::MarkerMismatch {
                        path: root,
                        expected: loc.library_uuid.to_string(),
                        found: mk.library_uuid.to_string(),
                    });
                }
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        match first_err {
            Some(e) if volume_seen => Err(e),
            _ => Err(StorageError::VolumeNotMounted {
                volume: loc.volume_uuid.or(loc.network_source).unwrap_or("?").to_owned(),
            }),
        }
    }
}

fn join_root(mount: &Path, rel_root: &str) -> PathBuf {
    if rel_root.is_empty() { mount.to_path_buf() } else { mount.join(rel_root) }
}

/// Katalogda kayıtlı kütüphaneyi bulmak için gereken bilgiler.
#[derive(Debug, Clone)]
pub struct LibraryLocator<'a> {
    pub library_uuid: Uuid,
    pub volume_uuid: Option<&'a str>,
    pub network_source: Option<&'a str>,
    pub rel_root: &'a str,
    pub last_mount_point: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub root: PathBuf,
    pub mount_point: PathBuf,
    /// Disk son bilinen yerden farklı bir yola takılmış; katalog güncellenmeli.
    pub moved: bool,
}

/// `<kök>/.loupe/library.json`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryMarker {
    pub format: u32,
    pub library_uuid: Uuid,
    pub name: String,
    pub created_at: i64,
}

impl LibraryMarker {
    pub const FORMAT: u32 = 1;
    pub const FILE: &'static str = "library.json";

    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            format: Self::FORMAT,
            library_uuid: Uuid::new_v4(),
            name: name.to_owned(),
            created_at: loupe_core::now_ms(),
        }
    }

    #[must_use]
    pub fn path(root: &Path) -> PathBuf {
        root.join(LOUPE_DIR).join(Self::FILE)
    }

    pub fn read(root: &Path) -> Result<Self, StorageError> {
        let path = Self::path(root);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::MarkerMissing(root.to_path_buf()));
            }
            Err(e) => return Err(StorageError::io("işaret okuma", &path)(e)),
        };
        let mk: Self = serde_json::from_slice(&bytes)
            .map_err(|e| StorageError::InvalidMarker { path: path.clone(), reason: e.to_string() })?;
        if mk.format != Self::FORMAT {
            return Err(StorageError::InvalidMarker { path, reason: format!("bilinmeyen biçim {}", mk.format) });
        }
        Ok(mk)
    }

    /// Atomik yazar: geçici dosya → fsync → rename → dizin fsync.
    pub fn write(&self, root: &Path) -> Result<(), StorageError> {
        let dir = root.join(LOUPE_DIR);
        fsutil::create_dir_all_durable(&dir)?;
        let path = dir.join(Self::FILE);
        let tmp = dir.join(format!("{}.{}.tmp", Self::FILE, Uuid::new_v4().simple()));
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| StorageError::InvalidMarker { path: path.clone(), reason: e.to_string() })?;
        let mut f = File::create(&tmp).map_err(StorageError::io("işaret yazma", &tmp))?;
        f.write_all(&json).map_err(StorageError::io("işaret yazma", &tmp))?;
        f.sync_all().map_err(StorageError::io("işaret fsync", &tmp))?;
        drop(f);
        fs::rename(&tmp, &path).map_err(StorageError::io("işaret rename", &path))?;
        fsync_dir(&dir)
    }
}

/// Kütüphane kökündeki dosya sisteminin yetenekleri (bağlanınca ölçülür).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub hardlink: bool,
    pub noreplace: bool,
}

/// Geçici dizinde gerçek bir deneme yaparak ölçer; varsayım yok.
pub fn probe_capabilities(root: &Path) -> Result<Capabilities, StorageError> {
    let dir = root.join(TMP_DIR);
    fsutil::create_dir_all_durable(&dir)?;
    let tag = Uuid::new_v4().simple().to_string();
    let a = dir.join(format!("probe-{tag}-a"));
    let b = dir.join(format!("probe-{tag}-b"));
    let c = dir.join(format!("probe-{tag}-c"));
    File::create(&a).map_err(StorageError::io("yetenek denemesi", &a))?;
    let hardlink = fs::hard_link(&a, &b).is_ok();
    let noreplace =
        rustix::fs::renameat_with(rustix::fs::CWD, &a, rustix::fs::CWD, &c, rustix::fs::RenameFlags::NOREPLACE).is_ok();
    for p in [&a, &b, &c] {
        let _ignored = fs::remove_file(p);
    }
    Ok(Capabilities { hardlink, noreplace })
}

/// Tüm disklerin UUID → mount noktası eşlemesi (disk takıldı olayında hangi
/// kütüphanelerin geri geldiğini bulmak için).
pub fn mounted_uuids(probe: &VolumeProbe) -> Result<HashMap<String, PathBuf>, StorageError> {
    let mounts = probe.mounts()?;
    let uuids = VolumeProbe::links(&probe.by_uuid, false);
    Ok(mounts
        .iter()
        .filter_map(|m| VolumeProbe::match_link(&uuids, m).map(|l| (l.name.clone(), m.mount_point.clone())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mountinfo_with_escapes() {
        let text = "\
36 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
97 36 8:17 / /run/media/u/Arsiv\\0404TB rw,nosuid shared:50 - exfat /dev/sdb1 rw,uid=1000
98 36 0:55 / /mnt/nas rw - nfs4 nas.local:/volume1/photos rw,vers=4.2
";
        let m = parse_mountinfo(text).unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[1].mount_point, PathBuf::from("/run/media/u/Arsiv 4TB"));
        assert_eq!((m[1].major, m[1].minor, m[1].fs_type.as_str()), (8, 17, "exfat"));
        assert_eq!(m[2].source, "nas.local:/volume1/photos");
        assert!(parse_mountinfo("bozuk satır").is_err());
    }

    #[test]
    fn unescapes_labels() {
        assert_eq!(unescape_hex("Arsiv\\x204TB"), "Arsiv 4TB");
        assert_eq!(unescape_octal("a\\134b"), "a\\b");
    }
}
