//! Uygulama durumu: veritabanı havuzu, iş kuyruğu ve işçiler, kütüphane
//! izleyicileri, `thumb://` işleyicisi ve ağ servisleri tek yerde açılır ve
//! kapanır. Tauri kabuğu yalnızca bu yapının metotlarını çağırır.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use http::{Request, Response};
use loupe_core::{ImportSessionId, LibraryId, MediaId};
use loupe_db::{Pool, PoolConfig};
use loupe_jobs::{JobHandler, JobKind, JobQueue, JobRouter, MediaPipeline, NewJob, Priority, WorkerPool};
use loupe_lan::Discovery;
use loupe_media::{GeoIndex, ThumbCache, ThumbProtocol, ThumbSize, Thumbnailer};
use loupe_storage::{LibraryWatcher, WatchEvent};
use serde::Serialize;

use crate::culling::{self, CullRequest, CullResult};
use crate::import::{ImportRequest, ImportSessionDto, Imports, post_import_jobs};
use crate::inspector::{self, InspectorDetail};
use crate::library::{Libraries, LibraryDto};
use crate::network::{CloudTargetStatusDto, LanStatusDto, Network};
use crate::secrets::{FileSecretStore, SecretStore};
use crate::timeline::{self, SmartAlbumDto, TimelineRequest, TimelineSlice, View};
use crate::{AppConfig, AppError, AppResult};

/// Görünür alan bildirimi başına en çok medya (tek ekran + overscan ~150–400).
pub const MAX_VIEWPORT: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueDto {
    pub pending: u64,
    pub running: u64,
    pub failed: u64,
    pub dead: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub total_media: u64,
    pub queue: QueueDto,
    pub workers: &'static str,
    pub imports_active: usize,
    pub libraries_online: usize,
    pub libraries_offline: usize,
    pub free_bytes: Option<i64>,
    pub lan: LanStatusDto,
    pub cloud: Vec<CloudTargetStatusDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewCount {
    /// `all` | `favorites` | `events` | `people` | `places`
    pub key: &'static str,
    /// Tümü/Favoriler: fotoğraf; Etkinlik/Kişi/Yer: grup sayısı.
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidebarDto {
    pub libraries: Vec<LibraryDto>,
    pub views: Vec<ViewCount>,
    pub smart_albums: Vec<SmartAlbumDto>,
}

pub struct AppCore {
    cfg: AppConfig,
    pool: Arc<Pool>,
    queue: Arc<JobQueue>,
    libraries: Arc<Libraries>,
    imports: Arc<Imports>,
    network: Arc<Network>,
    thumbs: ThumbCache,
    protocol: ThumbProtocol,
    workers: Mutex<Option<WorkerPool>>,
    watchers: Mutex<Vec<(LibraryId, LibraryWatcher)>>,
}

impl std::fmt::Debug for AppCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppCore").field("cfg", &self.cfg).finish_non_exhaustive()
    }
}

impl AppCore {
    pub fn open(cfg: AppConfig) -> AppResult<Arc<Self>> {
        let secrets = Arc::new(FileSecretStore::new(cfg.data_dir.join("secrets")));
        Self::open_with(cfg, secrets)
    }

    pub fn open_with(cfg: AppConfig, secrets: Arc<dyn SecretStore>) -> AppResult<Arc<Self>> {
        std::fs::create_dir_all(&cfg.data_dir).map_err(AppError::io(format!("{}", cfg.data_dir.display())))?;
        let pool = Arc::new(Pool::open(&cfg.catalog_path(), PoolConfig::default())?);
        let queue = Arc::new(JobQueue::new(pool.clone()));
        let libraries = Arc::new(Libraries::new(pool.clone(), cfg.volume_probe.clone()));
        let network = Arc::new(Network::new(pool.clone(), libraries.clone(), secrets));
        let net = Arc::downgrade(&network);
        let imports =
            Arc::new(Imports::new(pool.clone(), queue.clone(), libraries.clone()).with_new_media_hook(move |ids| {
                if let Some(n) = net.upgrade() {
                    let _queued = n.enqueue_new(ids);
                }
            }));
        let thumbs = ThumbCache::new(cfg.thumbs_dir());

        // Önbellekte olmayan küçük resim istendi: en acil öncelikle üret.
        let (p, q) = (pool.clone(), queue.clone());
        let protocol = ThumbProtocol::new(thumbs.clone()).with_miss_hook(move |hash, size| {
            if let Ok(Some(id)) = p.read(|c| loupe_db::repo::media::find_by_blake3(c, &hash)) {
                let _queued = q.enqueue(&NewJob::media(JobKind::Thumb, id, size.as_str(), Priority::Immediate));
            }
        });

        let core = Arc::new(Self {
            cfg,
            pool,
            queue,
            libraries,
            imports,
            network,
            thumbs,
            protocol,
            workers: Mutex::new(None),
            watchers: Mutex::new(Vec::new()),
        });
        core.libraries.refresh()?;
        // Disk çevrimdışıyken verilen puanların sidecar'ları.
        culling::flush_dirty_sidecars(&core.libraries, &core.pool, 10_000)?;
        core.start_workers()?;
        core.restart_watchers()?;
        Ok(core)
    }

    fn start_workers(&self) -> AppResult<()> {
        let Some(wcfg) = self.cfg.workers.clone() else { return Ok(()) };
        let geo = GeoIndex::load_dir(&self.cfg.geonames_dir()).ok().map(Arc::new);
        let libs = self.libraries.clone();
        let thumbnailer = Thumbnailer::new(self.thumbs.clone(), loupe_media::thumb::default_backend());
        // AI işleri (yüz, CLIP) model ayarlarıyla birlikte bağlanır; şimdilik ardıl iş açılmaz.
        let pipeline =
            MediaPipeline::new(self.pool.clone(), thumbnailer, geo, move |m| libs.media_path(m).ok().flatten())
                .without_ai_followups();
        let handler: Arc<dyn JobHandler> = Arc::new(
            JobRouter::new().route(&[JobKind::Thumb, JobKind::MetadataFull, JobKind::Geocode], Arc::new(pipeline)),
        );
        let pool = WorkerPool::start(self.queue.clone(), handler, wcfg)?;
        *self.workers.lock().map_err(|_| AppError::Poisoned)? = Some(pool);
        Ok(())
    }

    /// Çevrimiçi kütüphaneler için inotify izleyicilerini (yeniden) kurar.
    fn restart_watchers(self: &Arc<Self>) -> AppResult<()> {
        if !self.cfg.watch {
            return Ok(());
        }
        let mut watchers = self.watchers.lock().map_err(|_| AppError::Poisoned)?;
        for (_, w) in watchers.drain(..) {
            w.stop();
        }
        for (lib, root) in self.libraries.roots()? {
            let (tx, rx) = mpsc::channel::<WatchEvent>();
            let watcher = match LibraryWatcher::start(
                &root,
                self.cfg.watch_debounce,
                tx,
                Some(self.libraries.expected_paths()),
            ) {
                Ok(w) => w,
                // inotify sınırı dolmuş olabilir; kütüphane yine de kullanılabilir.
                Err(_) => continue,
            };
            let core = Arc::downgrade(self);
            std::thread::Builder::new()
                .name(format!("loupe-watch-{}", lib.0))
                .spawn(move || {
                    // İzleyici durunca gönderici düşer ve döngü biter.
                    while let Ok(ev) = rx.recv() {
                        let Some(core) = core.upgrade() else { break };
                        if let Ok(fresh) = core.libraries.apply_watch_event(lib, &ev) {
                            let _ignored = core.after_new_media(&fresh);
                        }
                    }
                })
                .map_err(AppError::io("izleyici iş parçacığı"))?;
            watchers.push((lib, watcher));
        }
        Ok(())
    }

    fn after_new_media(&self, ids: &[MediaId]) -> AppResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.queue.enqueue_batch(&post_import_jobs(ids))?;
        self.network.enqueue_new(ids)
    }

    /// LAN sunucusu ve bulut döngüleri. Tauri'nin async çalışma zamanında çağrılır.
    pub async fn start_network(&self, lan_bind: SocketAddr, discovery: Option<Arc<dyn Discovery>>) -> AppResult<()> {
        let lan = self.network.start_lan(lan_bind, &self.cfg.device_name, discovery).await;
        self.network.start_cloud()?;
        lan.map(drop)
    }

    pub async fn shutdown(&self) {
        if let Ok(mut w) = self.watchers.lock() {
            for (_, w) in w.drain(..) {
                w.stop();
            }
        }
        if let Some(pool) = self.workers.lock().ok().and_then(|mut w| w.take()) {
            pool.shutdown();
        }
        self.network.shutdown().await;
    }

    #[must_use]
    pub const fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    #[must_use]
    pub const fn queue(&self) -> &Arc<JobQueue> {
        &self.queue
    }

    #[must_use]
    pub const fn thumbs(&self) -> &ThumbCache {
        &self.thumbs
    }

    #[must_use]
    pub const fn network(&self) -> &Arc<Network> {
        &self.network
    }

    // ───────────────────────────── thumb:// ─────────────────────────────

    /// `thumb://sm/<blake3>` isteği: önbellekteki baytlar, JSON/base64 yok.
    pub fn handle_thumb<B>(&self, req: &Request<B>) -> Response<Cow<'static, [u8]>> {
        self.protocol.handle(req)
    }

    #[must_use]
    pub fn thumb_uri(size: ThumbSize, hash: &loupe_core::Blake3Hash) -> String {
        ThumbProtocol::uri_for(size, hash)
    }

    // ───────────────────────────── Komutlar ─────────────────────────────

    pub fn libraries(&self) -> AppResult<Vec<LibraryDto>> {
        self.libraries.list()
    }

    /// Diskleri yeniden yoklar (disk takıldı/çıkarıldı).
    pub fn refresh_libraries(self: &Arc<Self>) -> AppResult<Vec<LibraryDto>> {
        let libs = self.libraries.refresh()?;
        culling::flush_dirty_sidecars(&self.libraries, &self.pool, 10_000)?;
        self.restart_watchers()?;
        Ok(libs)
    }

    /// Klasörü kütüphane olarak ekler. Referans kütüphanesi hemen taranır.
    pub fn add_library_root(
        self: &Arc<Self>,
        path: &std::path::Path,
        name: Option<&str>,
        managed: bool,
    ) -> AppResult<(LibraryDto, Option<ImportSessionDto>)> {
        let lib = self.libraries.add_root(path, name, managed)?;
        self.restart_watchers()?;
        let scan = if lib.managed { None } else { Some(self.imports.scan_reference(LibraryId(lib.id))?) };
        Ok((lib, scan))
    }

    pub fn start_import(&self, req: &ImportRequest) -> AppResult<ImportSessionDto> {
        self.imports.start(req)
    }

    pub fn import_status(&self, id: i64) -> AppResult<ImportSessionDto> {
        self.imports.status(ImportSessionId(id))
    }

    pub fn recent_imports(&self, limit: u32) -> AppResult<Vec<ImportSessionDto>> {
        self.imports.recent(limit.clamp(1, 50))
    }

    pub fn cancel_import(&self, id: i64) -> AppResult<bool> {
        self.imports.cancel(ImportSessionId(id))
    }

    pub fn timeline_slice(&self, req: &TimelineRequest) -> AppResult<TimelineSlice> {
        timeline::slice(&self.pool, req)
    }

    /// Ekrandaki medyanın bekleyen işleri (küçük resim, metadata) öne alınır.
    pub fn prioritize_viewport(&self, ids: &[i64]) -> AppResult<usize> {
        if ids.len() > MAX_VIEWPORT {
            return Err(AppError::Invalid(format!("en çok {MAX_VIEWPORT} medya")));
        }
        let ids: Vec<MediaId> = ids.iter().copied().map(MediaId).collect();
        Ok(self.queue.bump_viewport_priority(&ids)?)
    }

    pub fn inspector_detail(&self, id: i64) -> AppResult<InspectorDetail> {
        inspector::detail(&self.libraries, &self.pool, &self.thumbs, MediaId(id))
    }

    pub fn set_rating_label_favorite(&self, req: &CullRequest) -> AppResult<Vec<CullResult>> {
        culling::apply(&self.libraries, &self.pool, req)
    }

    pub fn sidebar(&self) -> AppResult<SidebarDto> {
        let libraries = self.libraries.list()?;
        let groups = self.pool.read(|c| {
            let q = |sql: &str| -> Result<u64, loupe_db::DbError> {
                let n: i64 = c.query_row(sql, [], |r| r.get(0))?;
                Ok(u64::try_from(n).unwrap_or(0))
            };
            Ok((
                q("SELECT count(*) FROM events WHERE media_count > 0")?,
                q("SELECT count(*) FROM persons WHERE is_hidden = 0 AND face_count > 0")?,
                q("SELECT count(DISTINCT location_id) FROM media_files
                    WHERE location_id IS NOT NULL AND soft_deleted_at IS NULL")?,
            ))
        })?;
        Ok(SidebarDto {
            libraries,
            views: vec![
                ViewCount { key: "all", count: timeline::count(&self.pool, &View::All)? },
                ViewCount { key: "favorites", count: timeline::count(&self.pool, &View::Favorites)? },
                ViewCount { key: "events", count: groups.0 },
                ViewCount { key: "people", count: groups.1 },
                ViewCount { key: "places", count: groups.2 },
            ],
            smart_albums: timeline::smart_albums(&self.pool)?,
        })
    }

    pub async fn status(&self) -> AppResult<StatusDto> {
        let libs = self.libraries.list()?;
        let stats = self.queue.stats()?;
        let workers = match self.workers.lock().map_err(|_| AppError::Poisoned)?.as_ref() {
            None => "off",
            Some(w) if w.throttle().is_paused() => "paused",
            Some(_) => "running",
        };
        Ok(StatusDto {
            total_media: timeline::count(&self.pool, &View::All)?,
            queue: QueueDto { pending: stats.pending, running: stats.running, failed: stats.failed, dead: stats.dead },
            workers,
            imports_active: self.imports.active()?,
            libraries_online: libs.iter().filter(|l| l.root.is_some()).count(),
            libraries_offline: libs.iter().filter(|l| l.root.is_none()).count(),
            free_bytes: libs.iter().filter(|l| l.primary).find_map(|l| l.free_bytes),
            lan: self.network.lan_status().await?,
            cloud: self.network.cloud_status()?,
        })
    }
}
