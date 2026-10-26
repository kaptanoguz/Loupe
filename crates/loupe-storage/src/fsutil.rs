//! Dayanıklı dosya sistemi işlemleri: fsync, üzerine yazmayan rename, kilit,
//! sayfa önbelleği tavsiyesi, boş alan.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use rustix::fs::{CWD, RenameFlags};
#[cfg(unix)]
use rustix::io::Errno;

use crate::{LOUPE_DIR, StorageError};

/// `fs::canonicalize`; Windows'ta `\\?\C:\…` önekini atar ki yollar `C:\…`
/// biçiminde kalsın (kullanıcıya gösterilen ve sürücü köküyle karşılaştırılan yol).
pub fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    let p = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
        {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(p)
}

/// Dizin girdisinin (oluşturma / rename / silme) diske yazılması için üst dizini fsync eder.
/// Windows'ta dizin açılamaz; NTFS üst veriyi kendi günlüğüyle korur.
pub fn fsync_dir(dir: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    {
        let f = File::open(dir).map_err(StorageError::io("dizin açma", dir))?;
        f.sync_all().map_err(StorageError::io("dizin fsync", dir))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
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
#[cfg(unix)]
pub fn rename_noreplace(from: &Path, to: &Path) -> Result<(), StorageError> {
    match rustix::fs::renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(StorageError::TargetExists(to.to_path_buf())),
        Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => rename_checked(from, to),
        Err(e) => Err(StorageError::io("renameat2", to)(e.into())),
    }
}

/// Windows: önce sabit bağlantı (hedef varsa başarısız olur, atomiktir) sonra
/// kaynağı silme; bağlantı desteklenmiyorsa (FAT, exFAT) var-mı-kontrolü + rename.
#[cfg(not(unix))]
pub fn rename_noreplace(from: &Path, to: &Path) -> Result<(), StorageError> {
    match fs::hard_link(from, to) {
        Ok(()) => fs::remove_file(from).map_err(StorageError::io("rename", from)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(StorageError::TargetExists(to.to_path_buf())),
        Err(_) => rename_checked(from, to),
    }
}

fn rename_checked(from: &Path, to: &Path) -> Result<(), StorageError> {
    if to.symlink_metadata().is_ok() {
        return Err(StorageError::TargetExists(to.to_path_buf()));
    }
    fs::rename(from, to).map_err(StorageError::io("rename", to))
}

/// Sayfa önbelleğinden at: doğrulama okuması RAM'deki kopyayı değil diski okusun,
/// büyük import'lar kullanıcının çalışma kümesini önbellekten itmesin.
/// Tavsiyedir; desteklenmezse sessizce yok sayılır.
pub fn drop_page_cache(file: &File) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let _ignored = rustix::fs::fadvise(file, 0, None, rustix::fs::Advice::DontNeed);
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = file;
    }
}

/// Normal kullanıcının yazabileceği boş alan ve toplam (bayt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceInfo {
    pub total: u64,
    pub available: u64,
}

#[cfg(unix)]
pub fn space(path: &Path) -> Result<SpaceInfo, StorageError> {
    let s = rustix::fs::statvfs(path).map_err(|e| StorageError::io("statvfs", path)(e.into()))?;
    let unit = if s.f_frsize > 0 { s.f_frsize } else { s.f_bsize };
    Ok(SpaceInfo { total: s.f_blocks.saturating_mul(unit), available: s.f_bavail.saturating_mul(unit) })
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub fn space(path: &Path) -> Result<SpaceInfo, StorageError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` NUL ile biten geçerli bir UTF-16 yoldur; çıktılar yerel u64'lerdir.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
    if ok == 0 {
        return Err(StorageError::io("boş alan", path)(io::Error::last_os_error()));
    }
    Ok(SpaceInfo { total, available })
}

/// Kütüphane başına tek yazıcı: `<kök>/.loupe/lock` üzerinde özel, beklemesiz kilit
/// (Unix'te `flock`, Windows'ta `LockFileEx`).
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
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(fs::TryLockError::WouldBlock) => Err(StorageError::LibraryLocked(root.to_path_buf())),
            Err(fs::TryLockError::Error(e)) => Err(StorageError::io("kilit", &path)(e)),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
