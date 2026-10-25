//! Dayanıklı dosya sistemi işlemleri: fsync, üzerine yazmayan rename, kilit,
//! sayfa önbelleği tavsiyesi, boş alan.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use rustix::fs::{Advice, CWD, FlockOperation, RenameFlags};
use rustix::io::Errno;

use crate::{LOUPE_DIR, StorageError};

/// Dizin girdisinin (oluşturma / rename / silme) diske yazılması için üst dizini fsync eder.
pub fn fsync_dir(dir: &Path) -> Result<(), StorageError> {
    let f = File::open(dir).map_err(StorageError::io("dizin açma", dir))?;
    f.sync_all().map_err(StorageError::io("dizin fsync", dir))
}

/// `create_dir_all` + oluşturulan her yeni dizinin üstünü fsync eder; aksi halde
/// çökme sonrası dosya yerinde ama dizini yok olabilir.
pub fn create_dir_all_durable(dir: &Path) -> Result<(), StorageError> {
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d.as_os_str().is_empty() || d.is_dir() {
            break;
        }
        missing.push(d.to_path_buf());
        cur = d.parent();
    }
    for d in missing.iter().rev() {
        match fs::create_dir(d) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(StorageError::io("dizin oluşturma", d)(e)),
        }
        if let Some(parent) = d.parent() {
            fsync_dir(parent)?;
        }
    }
    Ok(())
}

/// Hedef varsa asla üzerine yazmayan rename.
///
/// `renameat2(RENAME_NOREPLACE)` desteklenmiyorsa (exFAT, bazı FUSE, eski NFS)
/// var-mı-kontrolü + `rename`'e düşer. Bu yedek yol yalnızca kütüphane kilidi
/// ([`LibraryLock`]) tutulurken güvenlidir; importer bunu garanti eder.
pub fn rename_noreplace(from: &Path, to: &Path) -> Result<(), StorageError> {
    match rustix::fs::renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(StorageError::TargetExists(to.to_path_buf())),
        Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => {
            if to.symlink_metadata().is_ok() {
                return Err(StorageError::TargetExists(to.to_path_buf()));
            }
            fs::rename(from, to).map_err(StorageError::io("rename", to))
        }
        Err(e) => Err(StorageError::io("renameat2", to)(e.into())),
    }
}

/// Sayfa önbelleğinden at: doğrulama okuması RAM'deki kopyayı değil diski okusun,
/// büyük import'lar kullanıcının çalışma kümesini önbellekten itmesin.
/// Tavsiyedir; desteklenmezse sessizce yok sayılır.
pub fn drop_page_cache(file: &File) {
    let _ignored = rustix::fs::fadvise(file, 0, None, Advice::DontNeed);
}

/// Normal kullanıcının yazabileceği boş alan ve toplam (bayt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceInfo {
    pub total: u64,
    pub available: u64,
}

pub fn space(path: &Path) -> Result<SpaceInfo, StorageError> {
    let s = rustix::fs::statvfs(path).map_err(|e| StorageError::io("statvfs", path)(e.into()))?;
    let unit = if s.f_frsize > 0 { s.f_frsize } else { s.f_bsize };
    Ok(SpaceInfo { total: s.f_blocks.saturating_mul(unit), available: s.f_bavail.saturating_mul(unit) })
}

/// Kütüphane başına tek yazıcı: `<kök>/.loupe/lock` üzerinde `flock(LOCK_EX|LOCK_NB)`.
/// Süreç çökerse çekirdek kilidi bırakır; bayat kilit dosyası sorunu yoktur.
#[derive(Debug)]
pub struct LibraryLock {
    _file: File,
    path: PathBuf,
}

impl LibraryLock {
    pub fn acquire(root: &Path) -> Result<Self, StorageError> {
        let dir = root.join(LOUPE_DIR);
        create_dir_all_durable(&dir)?;
        let path = dir.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(StorageError::io("kilit dosyası", &path))?;
        match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(Errno::WOULDBLOCK) => Err(StorageError::LibraryLocked(root.to_path_buf())),
            Err(e) => Err(StorageError::io("flock", &path)(e.into())),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
