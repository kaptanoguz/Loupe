//! Ağ servisleri: Wi-Fi LAN sunucusu (mDNS duyurusu, mTLS, tus) ve hedef
//! başına bulut yedekleme döngüsü.
//!
//! Eşleşme QR'ı (ve onay penceresi) Adım 3B'dedir; bu yüzden şu an yeni
//! telefon eşleşemez, yalnızca daha önce eşleşmiş cihazlar bağlanır. Bulut
//! hedefi kimlik bilgisi ya da cihaz anahtarı eksikse `needs_setup` durumunda
//! bekler, hiçbir şey yüklenmez.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use loupe_cloud::provider::Credentials;
use loupe_cloud::targets::Target;
use loupe_cloud::{BackupEngine, Keyring, LibraryRoots};
use loupe_core::{LibraryId, now_ms};
use loupe_db::Pool;
use loupe_lan::{
    AutoApprove, Discovery, IdentityPem, LanConfig, LanServer, ServerIdentity, ServiceAd, SystemClock, Telemetry,
};
use serde::Serialize;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::library::Libraries;
use crate::secrets::SecretStore;
use crate::{AppError, AppResult};

/// Secret deposundaki adlar.
pub const LAN_IDENTITY_KEY: &str = "lan-identity";
pub const AGE_DEVICE_KEY: &str = "age-device-key";
/// Hedefler arası tekrar deneme aralığı (kuyruk boşken).
pub const CLOUD_IDLE: Duration = Duration::from_secs(60);

/// Bulut motoru kütüphane köklerini her yüklemede yeniden sorar (disk çıkarılabilir).
#[derive(Debug)]
pub struct SharedRoots(pub Arc<Libraries>);

impl LibraryRoots for SharedRoots {
    fn root(&self, library: LibraryId) -> Option<PathBuf> {
        self.0.root(library).ok().flatten()
    }
}

fn identity_to_json(p: &IdentityPem) -> String {
    serde_json::json!({
        "install_id": p.install_id,
        "ca_cert_pem": p.ca_cert_pem,
        "ca_key_pem": p.ca_key_pem,
        "server_cert_pem": p.server_cert_pem,
        "server_key_pem": p.server_key_pem,
    })
    .to_string()
}

fn identity_from_json(s: &str) -> AppResult<IdentityPem> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| AppError::Invalid(format!("LAN kimliği: {e}")))?;
    let f = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| AppError::Invalid(format!("LAN kimliği: {k} eksik")))
    };
    Ok(IdentityPem {
        install_id: f("install_id")?,
        ca_cert_pem: f("ca_cert_pem")?,
        ca_key_pem: f("ca_key_pem")?,
        server_cert_pem: f("server_cert_pem")?,
        server_key_pem: f("server_key_pem")?,
    })
}

/// Kurulumun kalıcı LAN kimliği: yoksa üretilir ve saklanır. Eşleşmiş
/// telefonlar CA'yı sabitlediği için her açılışta aynı olmalıdır.
pub fn load_or_create_identity(store: &dyn SecretStore, name: &str) -> AppResult<ServerIdentity> {
    if let Some(saved) = store.get(LAN_IDENTITY_KEY)? {
        return Ok(ServerIdentity::from_pem(&identity_from_json(&saved)?)?);
    }
    let id = ServerIdentity::generate(name, now_ms())?;
    store.set(LAN_IDENTITY_KEY, &identity_to_json(&id.to_pem()))?;
    Ok(id)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanStatusDto {
    /// `off` | `listening` | `no_library` | `error`
    pub state: &'static str,
    pub port: Option<u16>,
    pub paired_devices: u64,
    pub active_uploads: u32,
    pub bytes_per_sec: f64,
    pub verified_files: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudTargetStatusDto {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub encrypted: bool,
    /// `idle` | `uploading` | `needs_setup` | `error` | `disabled`
    pub state: &'static str,
    pub pending: u64,
    pub synced: u64,
    pub last_error: Option<String>,
}

#[derive(Debug)]
struct CloudWorker {
    target_id: i64,
    state: Arc<Mutex<(&'static str, Option<String>)>>,
    task: Option<JoinHandle<()>>,
}

#[derive(Debug)]
pub struct Network {
    pool: Arc<Pool>,
    libraries: Arc<Libraries>,
    secrets: Arc<dyn SecretStore>,
    lan: tokio::sync::Mutex<Option<LanServer>>,
    lan_error: Mutex<Option<(&'static str, String)>>,
    discovery: Mutex<Option<(Arc<dyn Discovery>, String)>>,
    cloud: Mutex<Vec<CloudWorker>>,
    wake: Arc<Notify>,
}

impl Network {
    #[must_use]
    pub fn new(pool: Arc<Pool>, libraries: Arc<Libraries>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            pool,
            libraries,
            secrets,
            lan: tokio::sync::Mutex::new(None),
            lan_error: Mutex::new(None),
            discovery: Mutex::new(None),
            cloud: Mutex::new(Vec::new()),
            wake: Arc::new(Notify::new()),
        }
    }

    /// Telefonların geleceği kütüphane: birincil, yoksa ilk çevrimiçi yönetilen.
    fn lan_library(&self) -> AppResult<Option<LibraryId>> {
        let libs = self.libraries.list()?;
        let online = |l: &&crate::library::LibraryDto| l.managed && l.root.is_some();
        Ok(libs.iter().filter(online).find(|l| l.primary).or_else(|| libs.iter().find(online)).map(|l| LibraryId(l.id)))
    }

    /// LAN sunucusunu başlatır ve mDNS ile duyurur. Kütüphane yoksa sessizce bekler.
    pub async fn start_lan(
        &self,
        bind: SocketAddr,
        name: &str,
        discovery: Option<Arc<dyn Discovery>>,
    ) -> AppResult<Option<SocketAddr>> {
        let mut slot = self.lan.lock().await;
        if let Some(s) = slot.as_ref() {
            return Ok(Some(s.local_addr()));
        }
        let Some(lib) = self.lan_library()? else {
            self.set_lan_error(Some(("no_library", "yönetilen çevrimiçi kütüphane yok".into())))?;
            return Ok(None);
        };
        let started = async {
            let identity = load_or_create_identity(self.secrets.as_ref(), name)?;
            let importer = self.libraries.importer(lib)?;
            // Eşleşme ancak masaüstünde QR gösterilince açılır (jeton yalnızca o an üretilir).
            let server = LanServer::start_shared(
                LanConfig::new(bind, lib, name),
                self.pool.clone(),
                identity,
                importer,
                Arc::new(SystemClock),
                Arc::new(AutoApprove),
            )
            .await?;
            Ok::<_, AppError>(server)
        }
        .await;
        let server = match started {
            Ok(s) => s,
            Err(e) => {
                self.set_lan_error(Some(("error", e.to_string())))?;
                return Err(e);
            }
        };
        let addr = server.local_addr();
        if let Some(d) = discovery {
            let ad = ServiceAd::for_server(&server.identity(), name, addr.port(), Vec::new());
            // mDNS yalnızca kolaylık: duyurulamasa da IP ile bağlanılabilir.
            if d.advertise(&ad).is_ok() {
                *self.discovery.lock().map_err(|_| AppError::Poisoned)? = Some((d, ad.instance));
            }
        }
        self.set_lan_error(None)?;
        *slot = Some(server);
        Ok(Some(addr))
    }

    fn set_lan_error(&self, e: Option<(&'static str, String)>) -> AppResult<()> {
        *self.lan_error.lock().map_err(|_| AppError::Poisoned)? = e;
        Ok(())
    }

    pub async fn lan_telemetry(&self) -> Option<Arc<Telemetry>> {
        self.lan.lock().await.as_ref().map(LanServer::telemetry)
    }

    pub async fn lan_status(&self) -> AppResult<LanStatusDto> {
        let paired: i64 = self
            .pool
            .read(|c| Ok(c.query_row("SELECT count(*) FROM devices WHERE revoked_at IS NULL", [], |r| r.get(0))?))?;
        let guard = self.lan.lock().await;
        let err = self.lan_error.lock().map_err(|_| AppError::Poisoned)?.clone();
        Ok(match guard.as_ref() {
            Some(s) => {
                let t = s.telemetry().snapshot();
                LanStatusDto {
                    state: "listening",
                    port: Some(s.local_addr().port()),
                    paired_devices: u64::try_from(paired).unwrap_or(0),
                    active_uploads: t.active_uploads,
                    bytes_per_sec: t.bytes_per_sec,
                    verified_files: t.verified_files,
                    error: None,
                }
            }
            None => LanStatusDto {
                state: err.as_ref().map_or("off", |e| e.0),
                port: None,
                paired_devices: u64::try_from(paired).unwrap_or(0),
                active_uploads: 0,
                bytes_per_sec: 0.0,
                verified_files: 0,
                error: err.map(|e| e.1),
            },
        })
    }

    fn credentials(&self, t: &Target) -> AppResult<Option<Credentials>> {
        if t.secret_ref.is_empty() || t.provider == "local" {
            return Ok(None);
        }
        let Some(raw) = self.secrets.get(&t.secret_ref)? else { return Ok(None) };
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| AppError::Invalid(format!("{}: kimlik bilgisi bozuk", t.name)))?;
        let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str).map(str::to_owned);
        Ok(s("user")
            .zip(s("secret"))
            .map(|(user, secret)| Credentials { user, secret: zeroize::Zeroizing::new(secret) }))
    }

    /// Etkin her hedef için bir yedekleme döngüsü başlatır.
    pub fn start_cloud(&self) -> AppResult<Vec<CloudTargetStatusDto>> {
        let ids: Vec<i64> = self.pool.read(|c| {
            Ok(c.prepare("SELECT id FROM cloud_targets WHERE enabled = 1 ORDER BY id")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?)
        })?;
        let mut workers = self.cloud.lock().map_err(|_| AppError::Poisoned)?;
        for id in ids {
            if workers.iter().any(|w| w.target_id == id) {
                continue;
            }
            let state = Arc::new(Mutex::new(("idle", None)));
            let task = match self.engine(id) {
                Ok(Some(engine)) => {
                    Some(tokio::spawn(cloud_loop(engine, self.pool.clone(), id, state.clone(), self.wake.clone())))
                }
                Ok(None) => {
                    set(&state, "needs_setup", None);
                    None
                }
                Err(e) => {
                    set(&state, "error", Some(e.to_string()));
                    None
                }
            };
            workers.push(CloudWorker { target_id: id, state, task });
        }
        drop(workers);
        self.cloud_status()
    }

    fn engine(&self, id: i64) -> AppResult<Option<BackupEngine>> {
        let target = self
            .pool
            .read(|c| Ok(loupe_cloud::targets::get(c, id)))?
            .map_err(AppError::from)?
            .ok_or(AppError::NotFound { entity: "bulut hedefi", id })?;
        let creds = self.credentials(&target)?;
        let needs_creds = !target.secret_ref.is_empty() && target.provider != "local";
        if needs_creds && creds.is_none() {
            return Ok(None);
        }
        let keyring = match (&target.encrypted, &target.age_recipient) {
            (true, Some(recipient)) => match self.secrets.get(AGE_DEVICE_KEY)? {
                Some(device) => Some(Keyring::from_device_secret(&device, recipient)?),
                None => return Ok(None),
            },
            _ => None,
        };
        let provider = loupe_cloud::provider::from_target(&target.provider, &target.config, creds)?;
        Ok(Some(BackupEngine::new(
            self.pool.clone(),
            provider,
            target,
            keyring,
            Arc::new(SharedRoots(self.libraries.clone())),
        )?))
    }

    /// Yeni medyayı çalışan her hedefin kuyruğuna koyar ve döngüleri uyandırır.
    pub fn enqueue_new(&self, ids: &[loupe_core::MediaId]) -> AppResult<()> {
        let targets: Vec<i64> = self
            .cloud
            .lock()
            .map_err(|_| AppError::Poisoned)?
            .iter()
            .filter(|w| w.task.is_some())
            .map(|w| w.target_id)
            .collect();
        if targets.is_empty() || ids.is_empty() {
            return Ok(());
        }
        loupe_cloud::queue::write_tx(&self.pool, |c| {
            for &t in &targets {
                for &m in ids {
                    loupe_cloud::queue::enqueue_upload(c, t, m, 5, now_ms())?;
                }
            }
            Ok(())
        })?;
        self.wake.notify_waiters();
        Ok(())
    }

    pub fn cloud_status(&self) -> AppResult<Vec<CloudTargetStatusDto>> {
        let states: Vec<(i64, (&'static str, Option<String>))> = self
            .cloud
            .lock()
            .map_err(|_| AppError::Poisoned)?
            .iter()
            .map(|w| (w.target_id, w.state.lock().map(|s| s.clone()).unwrap_or(("error", None))))
            .collect();
        Ok(self.pool.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT t.id, t.name, t.provider, t.encryption = 'age', t.enabled,
                        (SELECT count(*) FROM sync_queue q WHERE q.target_id = t.id AND q.state IN ('pending','failed','running')),
                        (SELECT count(*) FROM sync_status s WHERE s.target_id = t.id AND s.state = 'synced')
                   FROM cloud_targets t ORDER BY t.id",
            )?;
            let rows = stmt.query_map([], |r| {
                let id: i64 = r.get(0)?;
                let enabled: bool = r.get(4)?;
                let (state, last_error) = states
                    .iter()
                    .find(|(t, _)| *t == id)
                    .map_or((if enabled { "idle" } else { "disabled" }, None), |(_, s)| s.clone());
                Ok(CloudTargetStatusDto {
                    id,
                    name: r.get(1)?,
                    provider: r.get(2)?,
                    encrypted: r.get(3)?,
                    state,
                    pending: u64::try_from(r.get::<_, i64>(5)?).unwrap_or(0),
                    synced: u64::try_from(r.get::<_, i64>(6)?).unwrap_or(0),
                    last_error,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })?)
    }

    pub async fn shutdown(&self) {
        if let Ok(mut workers) = self.cloud.lock() {
            for w in workers.iter_mut() {
                if let Some(t) = w.task.take() {
                    t.abort();
                }
            }
        }
        if let Ok(mut d) = self.discovery.lock()
            && let Some((disc, instance)) = d.take()
        {
            let _ignored = disc.withdraw(&instance);
        }
        if let Some(server) = self.lan.lock().await.take() {
            server.shutdown().await;
        }
    }
}

fn set(state: &Mutex<(&'static str, Option<String>)>, s: &'static str, err: Option<String>) {
    if let Ok(mut g) = state.lock() {
        *g = (s, err);
    }
}

async fn cloud_loop(
    engine: BackupEngine,
    pool: Arc<Pool>,
    target: i64,
    state: Arc<Mutex<(&'static str, Option<String>)>>,
    wake: Arc<Notify>,
) {
    // Hedef etkinleşince kütüphanede olup hedefte olmayan her şey kuyruğa.
    let p = pool.clone();
    let seeded = tokio::task::spawn_blocking(move || {
        loupe_cloud::queue::write_tx(&p, |c| loupe_cloud::queue::enqueue_missing(c, target, now_ms()))
    })
    .await;
    if let Ok(Err(e)) = seeded {
        set(&state, "error", Some(e.to_string()));
    }
    loop {
        set(&state, "uploading", None);
        match engine.drain().await {
            Ok(_) => set(&state, "idle", None),
            Err(e) => set(&state, "error", Some(e.to_string())),
        }
        tokio::select! {
            () = wake.notified() => {}
            () = tokio::time::sleep(CLOUD_IDLE) => {}
        }
    }
}
