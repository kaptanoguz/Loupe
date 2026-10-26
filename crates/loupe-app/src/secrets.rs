//! Gizli anahtar deposu (LAN kimliği, bulut kimlik bilgileri, age cihaz anahtarı).
//!
//! - [`SecretServiceStore`]: Linux Secret Service (GNOME Keyring, KWallet)
//!   DBus üzerinden; değerler oturum anahtarıyla (DH + AES) şifreli taşınır.
//! - [`FileSecretStore`]: başsız kurulum ve CI yedeği. `0700` dizinde `0600`
//!   dosyalar; kullanıcı dışında kimse okuyamaz.
//! - [`LayeredSecretStore`]: Secret Service varsa onu kullanır ve dosyadaki
//!   eski değerleri okunduklarında ona taşır (sonra dosyayı siler); yoksa
//!   dosyaya düşer. `LOUPE_SECRETS=file` Secret Service'i hiç denemez.
//!
//! Değerler bellekte `Zeroizing` ile tutulur.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::{AppError, AppResult};

pub trait SecretStore: Send + Sync + std::fmt::Debug {
    fn get(&self, key: &str) -> AppResult<Option<zeroize::Zeroizing<String>>>;
    fn set(&self, key: &str, value: &str) -> AppResult<()>;
    /// Yoksa da başarılıdır.
    fn delete(&self, key: &str) -> AppResult<()>;
    /// Ayarlar ekranında gösterilen arka uç: `secret-service` | `file`.
    fn backend(&self) -> &'static str;
}

/// Anahtar adları dosya adı olarak da kullanılır: yalnızca `[A-Za-z0-9._-]`.
pub fn check_key(key: &str) -> AppResult<()> {
    let ok = !key.is_empty()
        && key.len() <= 128
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !key.starts_with('.');
    if ok { Ok(()) } else { Err(AppError::Invalid(format!("geçersiz anahtar adı: {key}"))) }
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
        check_key(key)?;
        Ok(self.dir.join(key))
    }
}

fn ensure_private_dir(dir: &Path) -> AppResult<()> {
    fs::create_dir_all(dir).map_err(AppError::io(format!("dizin {}", dir.display())))?;
    // Windows'ta kullanıcı profilindeki dizin zaten yalnızca o kullanıcıya açıktır.
    #[cfg(unix)]
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(AppError::io("dizin izinleri"))?;
    Ok(())
}

/// `0600` dosyaya atomik yazar (geçici dosya + rename).
pub fn write_private(path: &Path, value: &[u8]) -> AppResult<()> {
    let dir = path.parent().ok_or_else(|| AppError::Invalid("dosya yolu".into()))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("secret");
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts.open(&tmp).map_err(AppError::io(format!("{}", tmp.display())))?;
    f.write_all(value).map_err(AppError::io("gizli değer yazma"))?;
    f.sync_all().map_err(AppError::io("gizli değer fsync"))?;
    drop(f);
    fs::rename(&tmp, path).map_err(AppError::io("gizli değer rename"))
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
        write_private(&self.path(key)?, value.as_bytes())
    }

    fn delete(&self, key: &str) -> AppResult<()> {
        match fs::remove_file(self.path(key)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::io(format!("gizli değer {key}"))(e)),
        }
    }

    fn backend(&self) -> &'static str {
        "file"
    }
}

/// Secret Service öğelerinin öznitelikleri: `application=loupe`, `key=<ad>`.
const APP_ATTR: &str = "loupe";

/// Linux Secret Service (DBus). Her işlem kendi bağlantısını açar: işlemler
/// seyrektir (açılış, ayar değişikliği) ve uzun ömürlü DBus bağlantısı
/// kilitlenen anahtarlık durumunda bayatlayabilir.
#[derive(Debug, Clone, Copy, Default)]
pub struct SecretServiceStore;

fn ss_err(e: secret_service::Error) -> AppError {
    AppError::Secret(e.to_string())
}

impl SecretServiceStore {
    /// Servis erişilebilir mi (oturum veriyolu var, varsayılan anahtarlık açılabiliyor).
    pub fn probe() -> AppResult<Self> {
        let ss =
            secret_service::blocking::SecretService::connect(secret_service::EncryptionType::Dh).map_err(ss_err)?;
        ss.get_default_collection().map_err(ss_err)?;
        Ok(Self)
    }

    fn with<T>(
        &self,
        f: impl FnOnce(&secret_service::blocking::Collection<'_>) -> Result<T, secret_service::Error>,
    ) -> AppResult<T> {
        let ss =
            secret_service::blocking::SecretService::connect(secret_service::EncryptionType::Dh).map_err(ss_err)?;
        let col = ss.get_default_collection().map_err(ss_err)?;
        col.ensure_unlocked().map_err(ss_err)?;
        f(&col).map_err(ss_err)
    }
}

fn attrs(key: &str) -> HashMap<&str, &str> {
    HashMap::from([("application", APP_ATTR), ("key", key)])
}

impl SecretStore for SecretServiceStore {
    fn get(&self, key: &str) -> AppResult<Option<zeroize::Zeroizing<String>>> {
        check_key(key)?;
        let raw = self.with(|col| {
            let items = col.search_items(attrs(key))?;
            match items.first() {
                Some(item) => {
                    item.ensure_unlocked()?;
                    item.get_secret().map(Some)
                }
                None => Ok(None),
            }
        })?;
        raw.map(|bytes| {
            let bytes = zeroize::Zeroizing::new(bytes);
            String::from_utf8(bytes.to_vec())
                .map(zeroize::Zeroizing::new)
                .map_err(|_| AppError::Secret(format!("{key}: UTF-8 değil")))
        })
        .transpose()
    }

    fn set(&self, key: &str, value: &str) -> AppResult<()> {
        check_key(key)?;
        self.with(|col| {
            col.create_item(&format!("Loupe: {key}"), attrs(key), value.as_bytes(), true, "text/plain").map(drop)
        })
    }

    fn delete(&self, key: &str) -> AppResult<()> {
        check_key(key)?;
        self.with(|col| {
            for item in col.search_items(attrs(key))? {
                item.delete()?;
            }
            Ok(())
        })
    }

    fn backend(&self) -> &'static str {
        "secret-service"
    }
}

/// Secret Service varsa o, yoksa `0600` dosyalar.
#[derive(Debug)]
pub struct LayeredSecretStore {
    file: FileSecretStore,
    allow_service: bool,
    primary: OnceLock<Option<SecretServiceStore>>,
}

impl LayeredSecretStore {
    #[must_use]
    pub fn new(file: FileSecretStore) -> Self {
        let allow_service = std::env::var("LOUPE_SECRETS").as_deref() != Ok("file");
        Self { file, allow_service, primary: OnceLock::new() }
    }

    /// Yalnızca dosya (testler, `LOUPE_SECRETS=file`).
    #[must_use]
    pub fn file_only(file: FileSecretStore) -> Self {
        Self { file, allow_service: false, primary: OnceLock::new() }
    }

    fn primary(&self) -> Option<&SecretServiceStore> {
        self.primary.get_or_init(|| if self.allow_service { SecretServiceStore::probe().ok() } else { None }).as_ref()
    }
}

impl SecretStore for LayeredSecretStore {
    fn get(&self, key: &str) -> AppResult<Option<zeroize::Zeroizing<String>>> {
        let Some(ss) = self.primary() else { return self.file.get(key) };
        if let Some(v) = ss.get(key)? {
            return Ok(Some(v));
        }
        // Secret Service sonradan kuruldu: dosyadaki eski değeri taşı.
        let Some(v) = self.file.get(key)? else { return Ok(None) };
        if ss.set(key, &v).is_ok() {
            self.file.delete(key)?;
        }
        Ok(Some(v))
    }

    fn set(&self, key: &str, value: &str) -> AppResult<()> {
        match self.primary() {
            Some(ss) => {
                ss.set(key, value)?;
                self.file.delete(key)
            }
            None => self.file.set(key, value),
        }
    }

    fn delete(&self, key: &str) -> AppResult<()> {
        if let Some(ss) = self.primary() {
            ss.delete(key)?;
        }
        self.file.delete(key)
    }

    fn backend(&self) -> &'static str {
        if self.primary().is_some() { "secret-service" } else { "file" }
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
        #[cfg(unix)]
        {
            let mode = fs::metadata(dir.path().join("secrets/lan-identity")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let dmode = fs::metadata(dir.path().join("secrets")).unwrap().permissions().mode();
            assert_eq!(dmode & 0o777, 0o700);
        }
        for bad in ["../x", "a/b", "", ".gizli"] {
            assert!(store.set(bad, "x").is_err(), "{bad}");
        }
        store.delete("lan-identity").unwrap();
        store.delete("lan-identity").unwrap();
        assert!(store.get("lan-identity").unwrap().is_none());
        assert_eq!(store.backend(), "file");
    }

    #[test]
    fn layered_without_service_falls_back_to_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = LayeredSecretStore::file_only(FileSecretStore::new(dir.path().join("s")));
        assert_eq!(store.backend(), "file");
        store.set("cloud-1", r#"{"user":"u","secret":"s"}"#).unwrap();
        assert!(store.get("cloud-1").unwrap().unwrap().contains("\"u\""));
        store.delete("cloud-1").unwrap();
        assert!(store.get("cloud-1").unwrap().is_none());
    }
}
