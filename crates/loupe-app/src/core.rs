//! Uygulama durumu: veritabanı havuzu, iş kuyruğu ve işçiler, kütüphane
//! izleyicileri, `thumb://` işleyicisi ve ağ servisleri tek yerde açılır ve
//! kapanır. Tauri kabuğu yalnızca bu yapının metotlarını çağırır.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

use crate::ai::{self, AiRuntime, AiStatusDto};
use crate::cloud::{self, CloudSettingsDto, CloudTargetInput, EncryptionDto, Recovery, RecoveryDraftDto};
use crate::culling::{self, CullRequest, CullResult};
use crate::full::{self, FullRes};
use crate::import::{ImportRequest, ImportSessionDto, Imports, post_import_jobs};
use crate::inspector::{self, InspectorDetail};
use crate::library::{Libraries, LibraryDto};
use crate::moments::{self, EventCard};
use crate::network::{CloudTargetStatusDto, DeviceDto, LanStatusDto, Network, PairingQrDto};
use crate::people::{self, FaceRef, PeopleDto};
use crate::search::{self, PaletteRequest, PaletteResult, RankCache};
use crate::secrets::{FileSecretStore, LayeredSecretStore, SecretStore};
use crate::timeline::{self, SmartAlbumDto, TimelineRequest, TimelineSlice, View};
use crate::verify::{self, Verifier, VerifyRunDto};
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
    pub ai: AiStatusDto,
    /// Süren "Kütüphaneyi Doğrula" sayısı.
    pub verifying: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewCount {
    /// `all` | `favorites` | `videos` | `events` | `people` | `places`
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
    pub albums: Vec<crate::albums::AlbumDto>,
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
    full: FullRes,
    ai: AiRuntime,
    models: crate::models::Models,
    search: loupe_search::SearchEngine,
    ranks: RankCache,
    recovery: Recovery,
    verifier: Arc<Verifier>,
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
        let file = FileSecretStore::new(cfg.data_dir.join("secrets"));
        let secrets = Arc::new(if cfg.secret_service {
            LayeredSecretStore::new(file)
        } else {
            LayeredSecretStore::file_only(file)
        });
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

        let full = FullRes::new(pool.clone(), libraries.clone(), cfg.full_dir());
        let ai = AiRuntime::open(cfg.ai.clone());
        let models = crate::models::Models::new(cfg.data_dir.join("models"))?;
        let search = match &ai.engine {
            Some(e) => loupe_search::SearchEngine::new(pool.clone(), e.clip_text.clone()),
            None => loupe_search::SearchEngine::keyword_only(pool.clone()),
        };
        let core = Arc::new(Self {
            cfg,
            pool,
            queue,
            libraries,
            imports,
            network,
            thumbs,
            protocol,
            full,
            ai,
            models,
            search,
            ranks: RankCache::default(),
            recovery: Recovery::default(),
            verifier: Arc::new(Verifier::default()),
            workers: Mutex::new(None),
            watchers: Mutex::new(Vec::new()),
        });
        core.libraries.refresh()?;
        // Disk çevrimdışıyken verilen puanların sidecar'ları.
        culling::flush_dirty_sidecars(&core.libraries, &core.pool, 10_000)?;
        core.start_workers()?;
        // ffmpeg sonradan kurulduysa daha önce başarısız olan video işleri yeniden denenir.
        if core.cfg.workers.is_some() && loupe_media::video::available() {
            let _requeued = core.queue.retry_dead_for_file_type(&[JobKind::Thumb, JobKind::MetadataFull], "video");
        }
        // HEIC çözücüsü (libheif) sistem paketidir; güncellenince yeni iPhone
        // fotoğrafları açılabilir. Başarısız küçük resimler her açılışta bir kez yeniden denenir.
        if core.cfg.workers.is_some() {
            let _requeued = core.queue.retry_dead_for_file_type(&[JobKind::Thumb], "heic");
        }
        // İçe aktarma sürerken yalnızca ızgara küçük resmi üretilir; yüz, CLIP
        // ve metadata aktarma bitince başlar.
        let weak = Arc::downgrade(&core);
        core.imports.set_busy_hook(move |busy| {
            if let Some(c) = weak.upgrade()
                && let Ok(Some(w)) = c.workers.lock().as_deref()
            {
                w.set_importing(busy);
            }
        });
        core.restart_watchers()?;
        core.scan_unscanned_libraries();
        // Model sonradan geldiyse eski fotoğraflar; her açılışta bir etkinlik kümelemesi.
        ai::backfill(&core.pool, &core.queue, core.ai.active())?;
        Ok(core)
    }

    fn start_workers(&self) -> AppResult<()> {
        let Some(wcfg) = self.cfg.workers.clone() else { return Ok(()) };
        let geo = GeoIndex::load_dir(&self.cfg.geonames_dir()).ok().map(Arc::new);
        let libs = self.libraries.clone();
        let thumbnailer = Thumbnailer::new(self.thumbs.clone(), loupe_media::thumb::default_backend());
        // `md` küçük resmi çıkınca yüz, CLIP ve (gecikmeli) etkinlik işleri açılır.
        let pipeline =
            MediaPipeline::new(self.pool.clone(), thumbnailer, geo, move |m| libs.media_path(m).ok().flatten());
        let handler: Arc<dyn JobHandler> = Arc::new(
            JobRouter::new()
                .route(&[JobKind::Thumb, JobKind::MetadataFull, JobKind::Geocode], Arc::new(pipeline))
                .route(&ai::job_kinds(), self.ai_handler()),
        );
        let pool = WorkerPool::start(self.queue.clone(), handler, wcfg)?;
        if let Some(mode) = ai::get_setting(&self.pool, ai::SETTING_MODE)?.and_then(|v| v.as_str().map(str::to_owned)) {
            pool.set_mode(ai::parse_mode(&mode)?);
        }
        if ai::get_setting(&self.pool, ai::SETTING_PAUSED)?.and_then(|v| v.as_bool()) == Some(true) {
            pool.pause();
        }
        *self.workers.lock().map_err(|_| AppError::Poisoned)? = Some(pool);
        Ok(())
    }

    /// AI iş türlerinin işleyicisi (testler kuyruğu bununla elle çalıştırır).
    #[must_use]
    pub fn ai_handler(&self) -> Arc<dyn JobHandler> {
        self.ai.handler(self.pool.clone(), self.thumbs.clone())
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
        self.ranks.clear();
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
    /// `thumb://orig/<blake3>`: Loupe 1:1 için tam çözünürlük ([`crate::full`]).
    pub fn handle_thumb<B>(&self, req: &Request<B>) -> Response<Cow<'static, [u8]>> {
        match full::parse(req.uri()) {
            Some(hash) => self.full.handle(req, &hash),
            None => self.protocol.handle(req),
        }
    }

    #[must_use]
    pub fn thumb_uri(size: ThumbSize, hash: &loupe_core::Blake3Hash) -> String {
        ThumbProtocol::uri_for(size, hash)
    }

    // ───────────────────────────── Komutlar ─────────────────────────────

    pub fn libraries(&self) -> AppResult<Vec<LibraryDto>> {
        self.libraries.list()
    }

    /// Ayarlar → Kütüphaneler: ad, klasör düzeni, varsayılan mod, birincil.
    pub fn update_library(&self, id: i64, patch: &crate::library::LibraryPatch) -> AppResult<LibraryDto> {
        self.libraries.update(LibraryId(id), patch)
    }

    /// Klasörü Loupe'den kaldırır: kataloğdaki kayıtları, yüzleri, vektörleri
    /// ve işleri silinir; diskteki dosyalara dokunulmaz. Telefon aktarımı bu
    /// kütüphaneye yazıyorsa LAN sunucusu durdurulur (`true` döner, çağıran
    /// yeniden başlatır). İçe aktarma sürerken reddedilir.
    pub async fn remove_library(self: &Arc<Self>, id: i64) -> AppResult<(u64, bool)> {
        let lib = LibraryId(id);
        let busy = self.pool.read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM import_sessions WHERE library_id = ?1 AND state IN ('scanning','running'))",
                [id],
                |r| r.get::<_, bool>(0),
            )?)
        })?;
        if busy {
            return Err(AppError::Invalid("bu klasöre içe aktarma sürüyor; bitince kaldırın".into()));
        }
        let lan_stopped = self.network.stop_lan_for(lib).await;
        let this = self.clone();
        let removed = tokio::task::spawn_blocking(move || {
            let n = this.libraries.remove(lib)?;
            this.ranks.clear();
            this.restart_watchers()?;
            Ok::<_, AppError>(n)
        })
        .await
        .map_err(|e| AppError::Join(e.to_string()))??;
        Ok((removed, lan_stopped))
    }

    /// Medyayı sistemin varsayılan uygulamasında açar (video oynatıcı vb.).
    pub fn open_media(&self, id: i64) -> AppResult<()> {
        let path = self.media_file(id)?;
        open_path(&path)
    }

    /// Dosyanın klasörünü dosya yöneticisinde açar ve dosyayı seçili gösterir.
    /// Döner: klasör yolu.
    pub fn reveal_media(&self, id: i64) -> AppResult<String> {
        let path = self.media_file(id)?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.clone());
        reveal_path(&path, &dir)?;
        Ok(dir.to_string_lossy().into_owned())
    }

    /// Medyanın diskteki yolu (kütüphanesi bağlı olmalı).
    fn media_file(&self, id: i64) -> AppResult<PathBuf> {
        let m = self
            .pool
            .read(|c| loupe_db::repo::media::get(c, MediaId(id)))?
            .ok_or_else(|| AppError::Invalid(format!("medya {id} yok")))?;
        self.libraries.media_path(&m)?.ok_or_else(|| AppError::LibraryOffline("dosyanın diski bağlı değil".into()))
    }

    /// Seçili fotoğrafları diskten siler: Çöp Kutusu'na taşır ve kataloğdan çıkarır.
    pub fn trash_media(&self, ids: &[i64]) -> AppResult<crate::trash::TrashResultDto> {
        self.trash_media_with(ids, &crate::trash::SystemTrash)
    }

    pub fn trash_media_with(
        &self,
        ids: &[i64],
        trasher: &dyn crate::trash::Trasher,
    ) -> AppResult<crate::trash::TrashResultDto> {
        let out = crate::trash::trash_media(&self.pool, &self.libraries, trasher, ids)?;
        self.ranks.clear();
        Ok(out)
    }

    /// Diskleri yeniden yoklar (disk takıldı/çıkarıldı).
    pub fn refresh_libraries(self: &Arc<Self>) -> AppResult<Vec<LibraryDto>> {
        let libs = self.libraries.refresh()?;
        culling::flush_dirty_sidecars(&self.libraries, &self.pool, 10_000)?;
        self.restart_watchers()?;
        Ok(libs)
    }

    /// Klasörü kütüphane olarak ekler ve içindeki mevcut fotoğrafları yerinde
    /// kataloglar. Yönetilen kütüphane de taranır: içe aktarma hedefi olması
    /// klasörde zaten duran fotoğrafları gizlememeli (dosyalara dokunulmaz,
    /// yerleşimleri `Reference` kalır).
    pub fn add_library_root(
        self: &Arc<Self>,
        path: &std::path::Path,
        name: Option<&str>,
        managed: bool,
    ) -> AppResult<(LibraryDto, Option<ImportSessionDto>)> {
        let lib = self.libraries.add_root(path, name, managed)?;
        self.restart_watchers()?;
        let scan = match lib.root {
            Some(_) => Some(self.imports.scan_reference(LibraryId(lib.id))?),
            None => None,
        };
        Ok((lib, scan))
    }

    /// Hiç taranmamış çevrimiçi kütüphaneler (önceki sürümde eklenen yönetilen
    /// kütüphaneler taranmıyordu) açılışta bir kez yerinde taranır.
    fn scan_unscanned_libraries(self: &Arc<Self>) {
        let Ok(libs) = self.libraries.list() else { return };
        for lib in libs.iter().filter(|l| l.root.is_some()) {
            let scanned = self.pool.read(|c| {
                Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM import_sessions WHERE library_id = ?1)", [lib.id], |r| {
                    r.get::<_, bool>(0)
                })?)
            });
            if matches!(scanned, Ok(false)) {
                // Tarama arka planda; başlatılamazsa (disk az önce çıkarıldı) sonraki açılışta denenir.
                let _ignored = self.imports.scan_reference(LibraryId(lib.id));
            }
        }
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
        let ranked = self.pool.read(|c| req.view.ranked_query(c).map_err(to_db))?;
        match ranked {
            Some(q) => {
                let ids = search::ranked_ids(&self.search, &self.ranks, &q)?;
                timeline::slice_ranked(&self.pool, req, Some(&ids))
            }
            None => timeline::slice(&self.pool, req),
        }
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
        self.ranks.clear();
        culling::apply(&self.libraries, &self.pool, req)
    }

    // ───────────────────────────── arama paleti ─────────────────────────────

    pub fn search_palette(&self, req: &PaletteRequest) -> AppResult<PaletteResult> {
        search::palette(&self.search, &self.ranks, req)
    }

    pub fn save_smart_album(
        &self,
        name: &str,
        query: &loupe_search::SearchQuery,
        pinned: bool,
    ) -> AppResult<timeline::SmartAlbumDto> {
        search::save_smart_album(&self.pool, name, query, pinned)
    }

    pub fn delete_smart_album(&self, id: i64) -> AppResult<()> {
        search::delete_smart_album(&self.pool, id)
    }

    // ───────────────────────────── albümler ─────────────────────────────

    pub fn create_album(&self, name: &str) -> AppResult<crate::albums::AlbumDto> {
        crate::albums::create(&self.pool, name)
    }

    pub fn rename_album(&self, id: i64, name: &str) -> AppResult<crate::albums::AlbumDto> {
        crate::albums::rename(&self.pool, id, name)
    }

    pub fn delete_album(&self, id: i64) -> AppResult<()> {
        crate::albums::delete(&self.pool, id)
    }

    pub fn add_to_album(&self, id: i64, media: &[i64]) -> AppResult<u64> {
        crate::albums::add(&self.pool, id, media)
    }

    pub fn remove_from_album(&self, id: i64, media: &[i64]) -> AppResult<u64> {
        crate::albums::remove(&self.pool, id, media)
    }

    // ───────────────────────────── kişiler ─────────────────────────────

    pub fn people(&self) -> AppResult<PeopleDto> {
        people::list(&self.pool)
    }

    pub fn person_faces(&self, person: Option<i64>, cluster: Option<i64>, limit: u32) -> AppResult<Vec<FaceRef>> {
        people::faces(&self.pool, person, cluster, limit)
    }

    pub fn name_cluster(&self, cluster: i64, name: &str) -> AppResult<i64> {
        self.ranks.clear();
        people::name_cluster(&self.pool, cluster, name)
    }

    pub fn rename_person(&self, person: i64, name: &str) -> AppResult<i64> {
        self.ranks.clear();
        people::rename_person(&self.pool, person, name)
    }

    pub fn merge_clusters(&self, into: i64, from: &[i64]) -> AppResult<()> {
        self.ranks.clear();
        people::merge(&self.pool, into, from)
    }

    pub fn merge_persons(&self, into: i64, from: &[i64]) -> AppResult<()> {
        self.ranks.clear();
        people::merge_persons(&self.pool, into, from)
    }

    pub fn move_face(&self, face: i64, to_cluster: i64) -> AppResult<()> {
        self.ranks.clear();
        people::move_face(&self.pool, face, to_cluster)
    }

    pub fn assign_face(&self, face: i64, person: i64) -> AppResult<()> {
        self.ranks.clear();
        people::assign_face(&self.pool, face, person)
    }

    pub fn detach_face(&self, face: i64) -> AppResult<i64> {
        self.ranks.clear();
        people::detach_face(&self.pool, face)
    }

    pub fn reject_face(&self, face: i64) -> AppResult<()> {
        self.ranks.clear();
        people::reject_face(&self.pool, face)
    }

    pub fn ignore_cluster(&self, cluster: i64, ignored: bool) -> AppResult<()> {
        people::ignore_cluster(&self.pool, cluster, ignored)
    }

    // ───────────────────────────── etkinlikler ─────────────────────────────

    pub fn events(&self) -> AppResult<Vec<EventCard>> {
        moments::list(&self.pool)
    }

    pub fn rename_event(&self, id: i64, title: &str) -> AppResult<EventCard> {
        self.ranks.clear();
        moments::rename(&self.pool, id, title)
    }

    pub fn exclude_from_event(&self, id: i64, media: &[i64]) -> AppResult<EventCard> {
        self.ranks.clear();
        moments::exclude(&self.pool, id, media)
    }

    pub fn set_event_locked(&self, id: i64, locked: bool) -> AppResult<EventCard> {
        moments::set_locked(&self.pool, id, locked)
    }

    pub fn create_event(&self, title: &str, media: &[i64]) -> AppResult<EventCard> {
        self.ranks.clear();
        moments::create(&self.pool, title, media)
    }

    /// Etkinlik kümelemesini hemen çalıştırır (kilitli etkinliklere dokunmaz).
    pub fn recluster_events(&self) -> AppResult<Vec<EventCard>> {
        let detector = loupe_ai::EventDetector::default();
        loupe_ai::jobs::write_tx(&self.pool, |c| detector.recluster(c))?;
        self.ranks.clear();
        moments::list(&self.pool)
    }

    // ───────────────────────────── AI ve kaynaklar ─────────────────────────────

    pub fn ai_status(&self) -> AppResult<AiStatusDto> {
        let (paused, mode) = match self.workers.lock().map_err(|_| AppError::Poisoned)?.as_ref() {
            Some(w) => (w.throttle().is_paused(), w.throttle().mode()),
            None => (
                ai::get_setting(&self.pool, ai::SETTING_PAUSED)?.and_then(|v| v.as_bool()).unwrap_or(false),
                ai::get_setting(&self.pool, ai::SETTING_MODE)?
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .map_or(Ok(loupe_jobs::ResourceMode::Balanced), |m| ai::parse_mode(&m))?,
            ),
        };
        let (pending, faces_done, clip_done, images) = ai::queue_counts(&self.pool)?;
        let reason = match (&self.ai.mode, &self.ai.error) {
            (_, Some(e)) => Some(e.clone()),
            (ai::AiMode::Off { reason }, None) => Some(reason.clone()),
            _ => None,
        };
        Ok(AiStatusDto {
            mode: if self.ai.active() { self.ai.mode.kind() } else { "off" },
            reason,
            semantic: self.search.is_semantic(),
            paused,
            resource_mode: ai::mode_name(mode),
            pending,
            faces_done,
            clip_done,
            images,
        })
    }

    /// Model dosyalarının durumu ve süren indirme.
    pub fn models_status(&self) -> AppResult<crate::models::ModelsDto> {
        self.models.status(self.ai.active() && self.ai.mode.kind() == "onnx")
    }

    /// Eksik ya da bozuk modelleri arka planda indirir (tokio çalışma zamanı içinde çağrılır).
    pub fn models_install(&self) -> AppResult<crate::models::ModelsDto> {
        self.models.install()?;
        self.models_status()
    }

    pub fn models_cancel(&self) -> AppResult<crate::models::ModelsDto> {
        self.models.cancel();
        self.models_status()
    }

    /// Testler: indiriciyi ve manifesti dışarıdan verir.
    #[must_use]
    pub const fn models(&self) -> &crate::models::Models {
        &self.models
    }

    /// Arka plan işlerini duraklatır/sürdürür (görünen küçük resimler durmaz).
    pub fn set_paused(&self, paused: bool) -> AppResult<AiStatusDto> {
        ai::put_setting(&self.pool, ai::SETTING_PAUSED, &serde_json::Value::Bool(paused))?;
        if let Some(w) = self.workers.lock().map_err(|_| AppError::Poisoned)?.as_ref() {
            if paused { w.pause() } else { w.resume() }
        }
        self.ai_status()
    }

    /// `quiet` | `balanced` | `full`: aynı anda çalışan genel işçi sayısı.
    pub fn set_resource_mode(&self, mode: &str) -> AppResult<AiStatusDto> {
        let m = ai::parse_mode(mode)?;
        ai::put_setting(&self.pool, ai::SETTING_MODE, &serde_json::Value::from(ai::mode_name(m)))?;
        if let Some(w) = self.workers.lock().map_err(|_| AppError::Poisoned)?.as_ref() {
            w.set_mode(m);
        }
        self.ai_status()
    }

    /// Kullanıcı arayüzde etkin (kaydırıyor, puanlıyor): toplu işler bekler.
    pub fn set_user_active(&self, active: bool) -> AppResult<()> {
        if let Some(w) = self.workers.lock().map_err(|_| AppError::Poisoned)?.as_ref() {
            w.set_user_active(active);
        }
        Ok(())
    }

    // ───────────────────────────── Wi-Fi eşleşme ─────────────────────────────

    pub async fn pairing_qr(&self) -> AppResult<PairingQrDto> {
        self.network.pairing_qr().await
    }

    pub async fn cancel_pairing(&self) {
        self.network.cancel_pairing().await;
    }

    pub fn devices(&self) -> AppResult<Vec<DeviceDto>> {
        self.network.devices()
    }

    pub async fn revoke_device(&self, id: i64) -> AppResult<Vec<DeviceDto>> {
        self.network.revoke_device(id).await?;
        self.network.devices()
    }

    pub fn set_conflict_policy(&self, id: i64, policy: loupe_lan::ConflictPolicy) -> AppResult<Vec<DeviceDto>> {
        self.network.set_conflict_policy(id, policy)?;
        self.network.devices()
    }

    // ───────────────────────────── bulut ─────────────────────────────

    pub fn cloud_settings(&self) -> AppResult<CloudSettingsDto> {
        cloud::list(&self.pool, self.network.secrets().as_ref())
    }

    /// Hedef ekler (`id` yoksa) ya da günceller; yedekleme döngüleri yeniden kurulur.
    pub fn save_cloud_target(&self, id: Option<i64>, input: &CloudTargetInput) -> AppResult<CloudSettingsDto> {
        let secrets = self.network.secrets().as_ref();
        match id {
            Some(id) => cloud::update(&self.pool, secrets, id, input)?,
            None => {
                cloud::create(&self.pool, secrets, input)?;
            }
        }
        self.network.restart_cloud()?;
        self.cloud_settings()
    }

    pub fn set_cloud_target_enabled(&self, id: i64, enabled: bool) -> AppResult<CloudSettingsDto> {
        cloud::set_enabled(&self.pool, id, enabled)?;
        self.network.restart_cloud()?;
        self.cloud_settings()
    }

    pub fn delete_cloud_target(&self, id: i64) -> AppResult<CloudSettingsDto> {
        cloud::delete(&self.pool, self.network.secrets().as_ref(), id)?;
        self.network.restart_cloud()?;
        self.cloud_settings()
    }

    pub fn set_global_bandwidth(&self, mbps: Option<f64>) -> AppResult<CloudSettingsDto> {
        cloud::set_global_bandwidth(&self.pool, mbps)?;
        self.network.restart_cloud()?;
        self.cloud_settings()
    }

    /// "Bulut Senkronizasyonunu Başlat": durmuş döngüler kurulur, çalışanlar uyanır.
    pub fn start_cloud_sync(&self) -> AppResult<Vec<CloudTargetStatusDto>> {
        self.network.restart_cloud()
    }

    pub fn generate_recovery_key(&self) -> AppResult<RecoveryDraftDto> {
        self.recovery.generate(&self.pool)
    }

    pub fn export_recovery_key(&self, path: &std::path::Path) -> AppResult<()> {
        self.recovery.export(path)
    }

    pub fn confirm_recovery_key(&self, answers: &[(usize, String)]) -> AppResult<EncryptionDto> {
        let enc = self.recovery.confirm(&self.pool, self.network.secrets().as_ref(), answers)?;
        self.network.restart_cloud()?;
        Ok(enc)
    }

    pub fn cancel_recovery_key(&self) {
        self.recovery.cancel();
    }

    pub fn check_recovery_key(&self, input: &str) -> AppResult<bool> {
        cloud::check_recovery(&self.pool, input)
    }

    // ───────────────────────────── doğrulama ─────────────────────────────

    /// "Kütüphaneyi Doğrula (Bit-Rot)". `library` yoksa çevrimiçi hepsi.
    pub fn verify_library(&self, library: Option<i64>) -> AppResult<Vec<i64>> {
        self.verifier.start(&self.pool, &self.libraries, library)
    }

    pub fn cancel_verify(&self, library: Option<i64>) -> AppResult<usize> {
        self.verifier.cancel(library)
    }

    pub fn verify_runs(&self, limit: u32) -> AppResult<Vec<VerifyRunDto>> {
        verify::recent(&self.pool, limit)
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
                ViewCount { key: "videos", count: timeline::count(&self.pool, &View::Videos)? },
                ViewCount { key: "events", count: groups.0 },
                ViewCount { key: "people", count: groups.1 },
                ViewCount { key: "places", count: groups.2 },
            ],
            smart_albums: timeline::smart_albums(&self.pool)?,
            albums: crate::albums::list(&self.pool)?,
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
            ai: self.ai_status()?,
            verifying: self.verifier.active(),
        })
    }
}

fn to_db(e: AppError) -> loupe_db::DbError {
    match e {
        AppError::Db(d) => d,
        other => loupe_db::DbError::Validation(other.to_string()),
    }
}

/// Dosyayı sistemin varsayılan uygulamasında açar.
fn open_path(path: &Path) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(not(any(target_os = "macos", windows)))]
    let program = "xdg-open";
    spawn_detached(program, &[path.as_os_str()])
}

/// Linux: `org.freedesktop.FileManager1.ShowItems` (Nemo, Nautilus, Dolphin…) dosyayı
/// seçili gösterir; servis yoksa klasör `xdg-open` ile açılır. macOS: `open -R`.
/// Windows: `explorer /select,`.
fn reveal_path(path: &Path, dir: &Path) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = dir;
        spawn_detached("open", &[std::ffi::OsStr::new("-R"), path.as_os_str()])
    }
    #[cfg(windows)]
    {
        let _ = dir;
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        spawn_detached("explorer", &[arg.as_os_str()])
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let shown = std::process::Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--reply-timeout=3000",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{}", file_uri(path)))
            .arg("string:")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if shown { Ok(()) } else { spawn_detached("xdg-open", &[dir.as_os_str()]) }
    }
}

/// Programı arka planda başlatır; çıktısı yok sayılır.
fn spawn_detached(program: &str, args: &[&std::ffi::OsStr]) -> AppResult<()> {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(AppError::io(format!("{program} çalıştırılamadı")))?;
    Ok(())
}

/// `file://` URI'si: yol baytları yüzde kodlanır (`dbus-send` virgülü dizi
/// ayırıcısı sayar; virgül de kodlanır).
#[cfg(all(unix, not(target_os = "macos")))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn file_uri_escapes_spaces_commas_and_utf8() {
        assert_eq!(
            file_uri(Path::new("/media/Data/Tatil, 2024/Göreme 1.jpg")),
            "file:///media/Data/Tatil%2C%202024/G%C3%B6reme%201.jpg"
        );
    }
}
