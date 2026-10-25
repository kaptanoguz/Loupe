//! 3-2-1 bulut ayarları: hedefler (S3/B2/R2/Wasabi/WebDAV/yerel NAS), bant
//! genişliği ve age kurtarma anahtarı.
//!
//! Sırlar (erişim anahtarları, cihaz anahtarı) veritabanına girmez;
//! [`crate::SecretStore`]'dadır (`cloud-target-<uuid>`, `age-device-key`).
//!
//! Kurtarma anahtarı istemcide üretilir ve yalnızca bir kez gösterilir:
//! 1. [`Recovery::generate`]: anahtar bellekte bekler, ekranda 4'lü gruplar.
//! 2. Kullanıcı kaydeder (kopyala ya da `0600` dosyaya dışa aktar).
//! 3. [`Recovery::confirm`]: rastgele 3 grubu geri yazar; doğruysa cihaz
//!    anahtarı üretilir, kurtarma alıcısı (`age1…`) ayarlara yazılır ve
//!    bellekteki gizli anahtar düşürülür. Bundan sonra anahtar bir daha
//!    gösterilemez; yalnızca [`check_recovery`] ile doğrulanabilir.

use std::path::Path;
use std::sync::Mutex;

use loupe_cloud::targets::NewTarget;
use loupe_cloud::{Keyring, RecoveryKey};
use loupe_core::now_ms;
use loupe_db::Pool;
use serde::{Deserialize, Serialize};

use crate::ai::{get_setting, put_setting};
use crate::network::{AGE_DEVICE_KEY, GLOBAL_BW_KEY};
use crate::secrets::{SecretStore, write_private};
use crate::{AppError, AppResult};

pub const PROVIDERS: [&str; 6] = ["s3", "b2", "r2", "wasabi", "webdav", "local"];
/// Arayüzde MB/s: 1 MB = 1 000 000 bayt.
pub const MB: f64 = 1_000_000.0;
pub const RECIPIENT_KEY: &str = "cloud.age_recipient";
pub const FINGERPRINT_KEY: &str = "cloud.key_fingerprint";
pub const CREATED_KEY: &str = "cloud.recovery_created_at";
/// Kurtarma onayında sorulan grup sayısı.
pub const CHALLENGE: usize = 3;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudTargetInput {
    pub name: String,
    pub provider: String,
    /// Gizli olmayan ayarlar: `endpoint`, `bucket`, `region`, `url`, `path`.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Erişim anahtarı kimliği / kullanıcı adı. Güncellemede `None`: değişmez.
    pub user: Option<String>,
    pub secret: Option<String>,
    #[serde(default = "default_prefix")]
    pub key_prefix: String,
    #[serde(default = "yes")]
    pub encrypted: bool,
    pub bandwidth_mbps: Option<f64>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn default_prefix() -> String {
    "loupe/".into()
}

const fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudTargetDto {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub config: serde_json::Value,
    pub key_prefix: String,
    pub encrypted: bool,
    pub bandwidth_mbps: Option<f64>,
    pub enabled: bool,
    pub has_credentials: bool,
    /// Erişim anahtarı kimliği (gizli değil); gizli anahtar asla dönmez.
    pub user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSettingsDto {
    pub targets: Vec<CloudTargetDto>,
    pub global_bandwidth_mbps: Option<f64>,
    pub encryption: EncryptionDto,
    /// `secret-service` | `file`
    pub secret_backend: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionDto {
    pub configured: bool,
    /// `age1…`
    pub recipient: Option<String>,
    pub fingerprint: Option<String>,
    pub device_key: bool,
    pub created_at: Option<i64>,
}

fn bps(mbps: Option<f64>) -> AppResult<Option<u64>> {
    match mbps {
        None => Ok(None),
        Some(v) if v.is_finite() && v > 0.0 && v <= 100_000.0 =>
        {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Ok(Some((v * MB).round() as u64))
        }
        Some(0.0) => Ok(None),
        Some(v) => Err(AppError::Invalid(format!("bant genişliği 0–100000 MB/s olmalı: {v}"))),
    }
}

#[allow(clippy::cast_precision_loss)]
fn mbps(bps: Option<u64>) -> Option<f64> {
    bps.map(|b| (b as f64 / MB * 100.0).round() / 100.0)
}

fn validate(input: &CloudTargetInput) -> AppResult<()> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(AppError::Invalid("hedef adı 1–80 karakter olmalı".into()));
    }
    if !PROVIDERS.contains(&input.provider.as_str()) {
        return Err(AppError::Invalid(format!("desteklenmeyen sağlayıcı: {}", input.provider)));
    }
    if !input.config.is_object() {
        return Err(AppError::Invalid("config bir nesne olmalı".into()));
    }
    let p = input.key_prefix.trim();
    if p.starts_with('/') || p.contains("..") || p.contains('\\') {
        return Err(AppError::Invalid("anahtar öneki göreli olmalı".into()));
    }
    Ok(())
}

fn creds_json(user: &str, secret: &str) -> String {
    serde_json::json!({ "user": user, "secret": secret }).to_string()
}

/// Sağlayıcı ayarı eksiksiz mi (ağa çıkmadan).
fn check_provider(input: &CloudTargetInput, has_creds: bool) -> AppResult<()> {
    let creds = has_creds
        .then(|| loupe_cloud::provider::Credentials { user: "-".into(), secret: zeroize::Zeroizing::new("-".into()) });
    // Yerel hedefte dizin burada oluşturulmaz; yalnızca yol var mı bakılır.
    if input.provider == "local" {
        let path = input.config.get("path").and_then(serde_json::Value::as_str).unwrap_or("");
        if !Path::new(path).is_absolute() {
            return Err(AppError::Invalid("yerel hedef için mutlak bir klasör yolu gerekli".into()));
        }
        return Ok(());
    }
    loupe_cloud::provider::from_target(&input.provider, &input.config, creds)?;
    Ok(())
}

pub fn encryption(pool: &Pool, secrets: &dyn SecretStore) -> AppResult<EncryptionDto> {
    let s = |k: &str| -> AppResult<Option<String>> {
        Ok(get_setting(pool, k)?.and_then(|v| v.as_str().map(str::to_owned)))
    };
    let recipient = s(RECIPIENT_KEY)?;
    Ok(EncryptionDto {
        configured: recipient.is_some(),
        recipient,
        fingerprint: s(FINGERPRINT_KEY)?,
        device_key: secrets.get(AGE_DEVICE_KEY)?.is_some(),
        created_at: get_setting(pool, CREATED_KEY)?.and_then(|v| v.as_i64()),
    })
}

pub fn list(pool: &Pool, secrets: &dyn SecretStore) -> AppResult<CloudSettingsDto> {
    type Row = (i64, String, String, String, String, String, Option<i64>, bool, bool);
    let rows: Vec<Row> = pool.read(|c| {
        Ok(c.prepare_cached(
            "SELECT id, name, provider, config, secret_ref, key_prefix, bandwidth_bps, enabled, encryption = 'age'
               FROM cloud_targets ORDER BY id",
        )?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
        })?
        .collect::<Result<_, _>>()?)
    })?;
    let mut targets = Vec::with_capacity(rows.len());
    for (id, name, provider, config, secret_ref, key_prefix, bw, enabled, encrypted) in rows {
        let user = if secret_ref.is_empty() {
            None
        } else {
            secrets.get(&secret_ref)?.and_then(|raw| {
                serde_json::from_str::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|v| v.get("user").and_then(serde_json::Value::as_str).map(str::to_owned))
            })
        };
        targets.push(CloudTargetDto {
            id,
            name,
            provider,
            config: serde_json::from_str(&config).unwrap_or(serde_json::Value::Null),
            key_prefix,
            encrypted,
            bandwidth_mbps: mbps(bw.and_then(|b| u64::try_from(b).ok())),
            enabled,
            has_credentials: user.is_some(),
            user,
        });
    }
    Ok(CloudSettingsDto {
        targets,
        global_bandwidth_mbps: mbps(get_setting(pool, GLOBAL_BW_KEY)?.and_then(|v| v.as_u64())),
        encryption: encryption(pool, secrets)?,
        secret_backend: secrets.backend(),
    })
}

fn recipient(pool: &Pool) -> AppResult<Option<String>> {
    Ok(get_setting(pool, RECIPIENT_KEY)?.and_then(|v| v.as_str().map(str::to_owned)))
}

/// Yeni hedef. Şifreli hedef için önce kurtarma anahtarı kurulmalıdır.
pub fn create(pool: &Pool, secrets: &dyn SecretStore, input: &CloudTargetInput) -> AppResult<i64> {
    validate(input)?;
    let has_creds = input.user.is_some() && input.secret.is_some();
    check_provider(input, has_creds)?;
    let age = if input.encrypted {
        Some(recipient(pool)?.ok_or_else(|| AppError::Invalid("önce kurtarma anahtarını oluşturun".into()))?)
    } else {
        None
    };
    let secret_ref = if input.provider == "local" {
        String::new()
    } else {
        format!("cloud-target-{}", uuid::Uuid::new_v4().simple())
    };
    if let (false, Some(user), Some(secret)) = (secret_ref.is_empty(), &input.user, &input.secret) {
        secrets.set(&secret_ref, &creds_json(user, secret))?;
    }
    let t = NewTarget {
        name: input.name.trim(),
        provider: &input.provider,
        config: input.config.clone(),
        secret_ref: &secret_ref,
        key_prefix: input.key_prefix.trim(),
        age_recipient: age.as_deref(),
        bandwidth_bps: bps(input.bandwidth_mbps)?,
    };
    let inserted = pool.write(|c| {
        let id = loupe_cloud::targets::insert(c, &t, now_ms()).map_err(cloud_to_db)?;
        c.execute("UPDATE cloud_targets SET enabled = ?2 WHERE id = ?1", rusqlite::params![id, input.enabled])?;
        Ok(id)
    });
    match inserted {
        Ok(id) => Ok(id),
        Err(e) => {
            if !secret_ref.is_empty() {
                let _cleanup = secrets.delete(&secret_ref);
            }
            Err(e.into())
        }
    }
}

/// Hedefi günceller. `user`/`secret` verilmezse kimlik bilgisi aynı kalır.
pub fn update(pool: &Pool, secrets: &dyn SecretStore, id: i64, input: &CloudTargetInput) -> AppResult<()> {
    validate(input)?;
    let target = pool
        .read(|c| loupe_cloud::targets::get(c, id).map_err(cloud_to_db))?
        .ok_or(AppError::NotFound { entity: "bulut hedefi", id })?;
    if target.provider != input.provider {
        return Err(AppError::Invalid("sağlayıcı değiştirilemez; yeni hedef ekleyin".into()));
    }
    let mut secret_ref = target.secret_ref.clone();
    if let (Some(user), Some(secret)) = (&input.user, &input.secret)
        && input.provider != "local"
    {
        if secret_ref.is_empty() {
            secret_ref = format!("cloud-target-{}", uuid::Uuid::new_v4().simple());
        }
        secrets.set(&secret_ref, &creds_json(user, secret))?;
    }
    let has_creds = !secret_ref.is_empty() && secrets.get(&secret_ref)?.is_some();
    check_provider(input, has_creds)?;
    // Şifreleme açılınca güncel alıcı; kapatılamaz (karışık hedef olmasın).
    if target.encrypted && !input.encrypted {
        return Err(AppError::Invalid("şifreli hedefin şifrelemesi kapatılamaz".into()));
    }
    let age = if input.encrypted {
        Some(
            target
                .age_recipient
                .clone()
                .or(recipient(pool)?)
                .ok_or_else(|| AppError::Invalid("önce kurtarma anahtarını oluşturun".into()))?,
        )
    } else {
        None
    };
    let bw = bps(input.bandwidth_mbps)?.map(i64::try_from).transpose().map_err(|_| AppError::Invalid("bant".into()))?;
    pool.write(|c| {
        c.execute(
            "UPDATE cloud_targets SET name = ?2, config = ?3, secret_ref = ?4, key_prefix = ?5, bandwidth_bps = ?6,
                    enabled = ?7, encryption = ?8, age_recipient = ?9, updated_at = ?10
              WHERE id = ?1",
            rusqlite::params![
                id,
                input.name.trim(),
                input.config.to_string(),
                secret_ref,
                input.key_prefix.trim(),
                bw,
                input.enabled,
                if age.is_some() { "age" } else { "none" },
                age,
                now_ms()
            ],
        )?;
        Ok(())
    })?;
    Ok(())
}

pub fn set_enabled(pool: &Pool, id: i64, enabled: bool) -> AppResult<()> {
    let n = pool.write(|c| {
        Ok(c.execute(
            "UPDATE cloud_targets SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, enabled, now_ms()],
        )?)
    })?;
    if n == 0 {
        return Err(AppError::NotFound { entity: "bulut hedefi", id });
    }
    Ok(())
}

/// Hedefi ve kimlik bilgisini siler. Buluttaki nesnelere dokunulmaz.
pub fn delete(pool: &Pool, secrets: &dyn SecretStore, id: i64) -> AppResult<()> {
    let target = pool
        .read(|c| loupe_cloud::targets::get(c, id).map_err(cloud_to_db))?
        .ok_or(AppError::NotFound { entity: "bulut hedefi", id })?;
    pool.write(|c| {
        c.execute("DELETE FROM cloud_targets WHERE id = ?1", [id])?;
        Ok(())
    })?;
    if !target.secret_ref.is_empty() {
        secrets.delete(&target.secret_ref)?;
    }
    Ok(())
}

pub fn set_global_bandwidth(pool: &Pool, mbps_value: Option<f64>) -> AppResult<()> {
    let v = bps(mbps_value)?.map_or(serde_json::Value::Null, serde_json::Value::from);
    put_setting(pool, GLOBAL_BW_KEY, &v)
}

// ───────────────────────────── kurtarma anahtarı ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryDraftDto {
    /// `AGE-SECRET-KEY-1`
    pub prefix: String,
    /// Gövde, 4'lü gruplar (ekranda ve kâğıtta).
    pub groups: Vec<String>,
    /// Tek satır (kopyala / `age -d -i` dosyası).
    pub secret: String,
    /// `age1…`: yedeklerin şifrelendiği açık anahtar.
    pub recipient: String,
    /// Kullanıcının geri yazacağı grup numaraları (0'dan).
    pub challenge: Vec<usize>,
    /// Kurulu bir anahtar değiştirilecek (eski yedekler için eski anahtar gerekir).
    pub replaces: bool,
}

/// Onay bekleyen kurtarma anahtarı (yalnızca bellekte).
#[derive(Debug, Default)]
pub struct Recovery {
    pending: Mutex<Option<(RecoveryKey, Vec<usize>)>>,
}

impl Recovery {
    pub fn generate(&self, pool: &Pool) -> AppResult<RecoveryDraftDto> {
        let key = RecoveryKey::generate();
        let challenge = key.challenge(CHALLENGE);
        let secret = key.secret();
        let draft = RecoveryDraftDto {
            prefix: "AGE-SECRET-KEY-1".into(),
            groups: key.groups().iter().map(|g| g.to_string()).collect(),
            secret: secret.to_string(),
            recipient: key.public().to_string(),
            challenge: challenge.clone(),
            replaces: recipient(pool)?.is_some(),
        };
        *self.pending.lock().map_err(|_| AppError::Poisoned)? = Some((key, challenge));
        Ok(draft)
    }

    /// Bekleyen anahtarı `0600` dosyaya yazar (ör. USB belleğe).
    pub fn export(&self, path: &Path) -> AppResult<()> {
        let guard = self.pending.lock().map_err(|_| AppError::Poisoned)?;
        let (key, _) = guard.as_ref().ok_or_else(|| AppError::Invalid("bekleyen kurtarma anahtarı yok".into()))?;
        let body = zeroize::Zeroizing::new(format!(
            "# Loupe kurtarma anahtarı. Bu dosyayı kaybederseniz şifreli yedekler açılamaz.\n\
             # Açık anahtar: {}\n{}\n",
            key.public(),
            key.secret().as_str()
        ));
        write_private(path, body.as_bytes())
    }

    /// Geri yazılan gruplar doğruysa anahtarı kurar ve bellekten düşürür.
    pub fn confirm(
        &self,
        pool: &Pool,
        secrets: &dyn SecretStore,
        answers: &[(usize, String)],
    ) -> AppResult<EncryptionDto> {
        let mut guard = self.pending.lock().map_err(|_| AppError::Poisoned)?;
        let (key, challenge) =
            guard.as_ref().ok_or_else(|| AppError::Invalid("bekleyen kurtarma anahtarı yok".into()))?;
        let asked: Vec<usize> = answers.iter().map(|(i, _)| *i).collect();
        if asked != *challenge {
            return Err(AppError::Invalid("istenen grupların hepsi yanıtlanmalı".into()));
        }
        let pairs: Vec<(usize, &str)> = answers.iter().map(|(i, a)| (*i, a.as_str())).collect();
        if !key.verify_challenge(&pairs) {
            return Err(AppError::Invalid("gruplar eşleşmedi; anahtarı yeniden kontrol edin".into()));
        }
        let recipient = key.public().to_string();
        // Cihaz anahtarı varsa korunur (önceki yedekler onunla da açılır).
        let keyring = match secrets.get(AGE_DEVICE_KEY)? {
            Some(device) => Keyring::from_device_secret(&device, &recipient)?,
            None => Keyring::generate(&recipient)?,
        };
        secrets.set(AGE_DEVICE_KEY, &keyring.device_secret())?;
        put_setting(pool, RECIPIENT_KEY, &serde_json::Value::from(recipient.clone()))?;
        put_setting(pool, FINGERPRINT_KEY, &serde_json::Value::from(keyring.fingerprint()))?;
        put_setting(pool, CREATED_KEY, &serde_json::Value::from(now_ms()))?;
        // Şifreli hedefler yeni alıcıyla devam eder (`sync_status.key_fingerprint` hangisiyle olduğunu tutar).
        pool.write(|c| {
            c.execute(
                "UPDATE cloud_targets SET age_recipient = ?1, updated_at = ?2 WHERE encryption = 'age'",
                rusqlite::params![recipient, now_ms()],
            )?;
            Ok(())
        })?;
        *guard = None;
        drop(guard);
        encryption(pool, secrets)
    }

    pub fn cancel(&self) {
        if let Ok(mut g) = self.pending.lock() {
            *g = None;
        }
    }
}

/// Elinizdeki kurtarma anahtarı kurulu olanla aynı mı (yazım hatası dahil).
pub fn check_recovery(pool: &Pool, input: &str) -> AppResult<bool> {
    let Some(want) = recipient(pool)? else { return Err(AppError::Invalid("kurtarma anahtarı kurulmamış".into())) };
    let key = RecoveryKey::parse(input)?;
    Ok(key.public().to_string() == want)
}

fn cloud_to_db(e: loupe_cloud::CloudError) -> loupe_db::DbError {
    match e {
        loupe_cloud::CloudError::Db(d) => d,
        other => loupe_db::DbError::Validation(other.to_string()),
    }
}
