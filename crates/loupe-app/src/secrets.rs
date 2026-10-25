//! Gizli anahtar deposu (LAN kimliği, bulut kimlik bilgileri, age cihaz anahtarı).
//!
//! Hedef Secret Service (GNOME Keyring / KWallet) arka ucudur; ayarlar ekranıyla
//! birlikte gelir. O zamana kadar [`FileSecretStore`] kullanılır: `0700`
//! dizinde `0600` dosyalar, kullanıcı dışında kimse okuyamaz. Değerler
//! bellekte `Zeroizing` ile tutulur.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::{AppError, AppResult};

pub trait SecretStore: Send + Sync + std::fmt::Debug {
    fn get(&self, key: &str) -> AppResult<Option<zeroize::Zeroizing<String>>>;
    fn set(&self, key: &str, value: &str) -> AppResult<()>;
}

#[derive(Debug, Clone)]
pub struct FileSecretStore {
    dir: PathBuf,
}

impl FileSecretStore {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, key: &str) -> AppResult<PathBuf> {
        let ok = !key.is_empty()
            && key.len() <= 128
            && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            && !key.starts_with('.');
        if !ok {
            return Err(AppError::Invalid(format!("geçersiz anahtar adı: {key}")));
        }
        Ok(self.dir.join(key))
    }
}

fn ensure_private_dir(dir: &Path) -> AppResult<()> {
    fs::create_dir_all(dir).map_err(AppError::io(format!("dizin {}", dir.display())))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(AppError::io("dizin izinleri"))
}

impl SecretStore for FileSecretStore {
    fn get(&self, key: &str) -> AppResult<Option<zeroize::Zeroizing<String>>> {
        let path = self.path(key)?;
        match fs::read_to_string(&path) {
            Ok(s) => Ok(Some(zeroize::Zeroizing::new(s))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AppError::io(format!("gizli değer {key}"))(e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> AppResult<()> {
        ensure_private_dir(&self.dir)?;
        let path = self.path(key)?;
        let tmp = self.dir.join(format!(".{key}.tmp"));
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(AppError::io(format!("gizli değer {key}")))?;
        f.write_all(value.as_bytes()).map_err(AppError::io("gizli değer yazma"))?;
        f.sync_all().map_err(AppError::io("gizli değer fsync"))?;
        drop(f);
        fs::rename(&tmp, &path).map_err(AppError::io("gizli değer rename"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_private_and_keys_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(dir.path().join("secrets"));
        assert!(store.get("lan-identity").unwrap().is_none());
        store.set("lan-identity", "gizli").unwrap();
        assert_eq!(store.get("lan-identity").unwrap().unwrap().as_str(), "gizli");
        let mode = fs::metadata(dir.path().join("secrets/lan-identity")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dmode = fs::metadata(dir.path().join("secrets")).unwrap().permissions().mode();
        assert_eq!(dmode & 0o777, 0o700);
        for bad in ["../x", "a/b", "", ".gizli"] {
            assert!(store.set(bad, "x").is_err(), "{bad}");
        }
    }
}
