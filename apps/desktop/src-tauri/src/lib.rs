//! Loupe masaüstü kabuğu (Tauri v2).
//!
//! İş mantığının tamamı `loupe-app`'tedir; burada yalnızca:
//! - `thumb://` şeması `AppCore::handle_thumb`'a bağlanır (asenkron: disk
//!   okuması WebView iş parçacığını bekletmez),
//! - komutlar `spawn_blocking` ile çekirdeğe iletilir (SQLite ve dosya
//!   sistemi çağrıları async çalışma zamanını tıkamaz),
//! - pencere kapanınca servisler düzgün kapatılır.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use loupe_app::{
    AiStatusDto, AppConfig, AppCore, AppError, AppResult, CloudSettingsDto, CloudTargetInput, CloudTargetStatusDto,
    CullRequest, CullResult, DeviceDto, EncryptionDto, EventCard, FaceRef, ImportRequest, ImportSessionDto,
    InspectorDetail, LibraryDto, ModelsDto, PairingQrDto, PaletteRequest, PaletteResult, PeopleDto, RecoveryDraftDto,
    SidebarDto, SmartAlbumDto, StatusDto, TimelineRequest, TimelineSlice, VerifyRunDto,
};
use serde::{Deserialize, Serialize};
use tauri::{Manager, Runtime, State};
use tauri_plugin_dialog::DialogExt;

/// LAN sunucusu tüm arayüzlerde sabit portta dinler (doluysa sistemin verdiği porta düşer; mDNS duyurur).
pub const LAN_BIND: &str = "0.0.0.0:47123";

pub struct Desktop {
    pub core: Arc<AppCore>,
}

impl std::fmt::Debug for Desktop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Desktop").finish_non_exhaustive()
    }
}

type Cmd<T> = Result<T, AppError>;

async fn blocking<T: Send + 'static>(
    state: &State<'_, Desktop>,
    f: impl FnOnce(&Arc<AppCore>) -> AppResult<T> + Send + 'static,
) -> Cmd<T> {
    let core = state.core.clone();
    tauri::async_runtime::spawn_blocking(move || f(&core)).await.map_err(|e| AppError::Join(e.to_string()))?
}

/// Ağ servislerini (yeniden) dener: ilk kütüphane eklenince LAN da açılabilir.
fn start_network(core: Arc<AppCore>) {
    tauri::async_runtime::spawn(async move {
        let discovery: Option<Arc<dyn loupe_lan::Discovery>> =
            loupe_lan::discovery::MdnsDiscovery::new(&hostname()).ok().map(|d| Arc::new(d) as _);
        let bind: SocketAddr = LAN_BIND.parse().unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0)));
        // Hata durum çubuğunda görünür (`lan.state = error`); uygulama çalışmaya devam eder.
        let _status = core.start_network(bind, discovery).await;
    });
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "loupe".to_owned())
}

async fn pick_folder<R: Runtime>(app: &tauri::AppHandle<R>, title: &'static str) -> Cmd<Option<PathBuf>> {
    let app = app.clone();
    let picked =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_title(title).blocking_pick_folder())
            .await
            .map_err(|e| AppError::Join(e.to_string()))?;
    picked.map(|p| p.into_path().map_err(|e| AppError::Invalid(e.to_string()))).transpose()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedLibrary {
    pub library: LibraryDto,
    /// Referans kütüphanesinin yerinde taranması.
    pub scan: Option<ImportSessionDto>,
}

/// Kütüphane kökü ekler. `path` verilmezse klasör seçme penceresi açılır;
/// vazgeçilirse `null`.
#[tauri::command]
async fn pick_or_add_library_root<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, Desktop>,
    path: Option<String>,
    name: Option<String>,
    managed: Option<bool>,
) -> Cmd<Option<AddedLibrary>> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => match pick_folder(&app, "Kütüphane klasörü").await? {
            Some(p) => p,
            None => return Ok(None),
        },
    };
    let managed = managed.unwrap_or(true);
    let (library, scan) = blocking(&state, move |c| c.add_library_root(&path, name.as_deref(), managed)).await?;
    start_network(state.core.clone());
    Ok(Some(AddedLibrary { library, scan }))
}

/// İçe aktarma kaynağı (kamera kartı, telefon klasörü) seçer.
#[tauri::command]
async fn pick_import_source<R: Runtime>(app: tauri::AppHandle<R>) -> Cmd<Option<String>> {
    Ok(pick_folder(&app, "İçe aktarılacak klasör").await?.map(|p| p.display().to_string()))
}

/// Kabloyla bağlı iPhone'ların DCIM klasörleri (gerekirse bağlar; birkaç saniye sürebilir).
#[tauri::command]
async fn find_iphone() -> Cmd<Vec<loupe_app::iphone::IphoneDto>> {
    Ok(tauri::async_runtime::spawn_blocking(loupe_app::iphone::find).await.unwrap_or_default())
}

/// Klasörü Loupe'den kaldırır (dosyalar diskte kalır). Kaldırılan kayıt sayısını döner.
#[tauri::command]
async fn remove_library(state: State<'_, Desktop>, id: i64) -> Cmd<u64> {
    let (removed, lan_stopped) = state.core.remove_library(id).await?;
    if lan_stopped {
        start_network(state.core.clone());
    }
    Ok(removed)
}

/// Kütüphane ayarlarını değiştirir (ad, klasör düzeni, varsayılan mod, birincil).
#[tauri::command]
async fn update_library(state: State<'_, Desktop>, id: i64, patch: loupe_app::LibraryPatch) -> Cmd<LibraryDto> {
    blocking(&state, move |c| c.update_library(id, &patch)).await
}

/// Videoyu (ya da herhangi bir medyayı) sistemin varsayılan uygulamasında açar.
#[tauri::command]
async fn open_media(state: State<'_, Desktop>, id: i64) -> Cmd<()> {
    blocking(&state, move |c| c.open_media(id)).await
}

/// Dosyanın klasörünü dosya yöneticisinde açar (dosya seçili). Döner: klasör.
#[tauri::command]
async fn reveal_media(state: State<'_, Desktop>, id: i64) -> Cmd<String> {
    blocking(&state, move |c| c.reveal_media(id)).await
}

/// Seçili fotoğrafları Çöp Kutusu'na taşır (arayüz önce onay ister).
#[tauri::command]
async fn trash_media(state: State<'_, Desktop>, ids: Vec<i64>) -> Cmd<loupe_app::trash::TrashResultDto> {
    blocking(&state, move |c| c.trash_media(&ids)).await
}

#[tauri::command]
async fn list_libraries(state: State<'_, Desktop>) -> Cmd<Vec<LibraryDto>> {
    blocking(&state, |c| c.libraries()).await
}

#[tauri::command]
async fn refresh_libraries(state: State<'_, Desktop>) -> Cmd<Vec<LibraryDto>> {
    blocking(&state, AppCore::refresh_libraries).await
}

#[tauri::command]
async fn start_import(state: State<'_, Desktop>, req: ImportRequest) -> Cmd<ImportSessionDto> {
    blocking(&state, move |c| c.start_import(&req)).await
}

#[tauri::command]
async fn import_status(state: State<'_, Desktop>, id: i64) -> Cmd<ImportSessionDto> {
    blocking(&state, move |c| c.import_status(id)).await
}

#[tauri::command]
async fn recent_imports(state: State<'_, Desktop>, limit: Option<u32>) -> Cmd<Vec<ImportSessionDto>> {
    blocking(&state, move |c| c.recent_imports(limit.unwrap_or(10))).await
}

#[tauri::command]
async fn cancel_import(state: State<'_, Desktop>, id: i64) -> Cmd<bool> {
    blocking(&state, move |c| c.cancel_import(id)).await
}

#[tauri::command]
async fn get_timeline_slice(state: State<'_, Desktop>, req: TimelineRequest) -> Cmd<TimelineSlice> {
    blocking(&state, move |c| c.timeline_slice(&req)).await
}

#[tauri::command]
async fn prioritize_viewport(state: State<'_, Desktop>, ids: Vec<i64>) -> Cmd<usize> {
    blocking(&state, move |c| c.prioritize_viewport(&ids)).await
}

#[tauri::command]
async fn get_media_inspector_detail(state: State<'_, Desktop>, id: i64) -> Cmd<InspectorDetail> {
    blocking(&state, move |c| c.inspector_detail(id)).await
}

#[tauri::command]
async fn set_rating_label_favorite(state: State<'_, Desktop>, req: CullRequest) -> Cmd<Vec<CullResult>> {
    blocking(&state, move |c| c.set_rating_label_favorite(&req)).await
}

#[tauri::command]
async fn get_sidebar(state: State<'_, Desktop>) -> Cmd<SidebarDto> {
    blocking(&state, |c| c.sidebar()).await
}

#[tauri::command]
async fn get_status(state: State<'_, Desktop>) -> Cmd<StatusDto> {
    state.core.status().await
}

// ───────────────────────────── arama paleti ─────────────────────────────

#[tauri::command]
async fn search_palette(state: State<'_, Desktop>, req: PaletteRequest) -> Cmd<PaletteResult> {
    blocking(&state, move |c| c.search_palette(&req)).await
}

#[tauri::command]
async fn save_smart_album(
    state: State<'_, Desktop>,
    name: String,
    query: loupe_app::SearchQuery,
    pinned: Option<bool>,
) -> Cmd<SmartAlbumDto> {
    blocking(&state, move |c| c.save_smart_album(&name, &query, pinned.unwrap_or(false))).await
}

#[tauri::command]
async fn delete_smart_album(state: State<'_, Desktop>, id: i64) -> Cmd<()> {
    blocking(&state, move |c| c.delete_smart_album(id)).await
}

// ───────────────────────────── albümler ─────────────────────────────

#[tauri::command]
async fn create_album(state: State<'_, Desktop>, name: String) -> Cmd<loupe_app::AlbumDto> {
    blocking(&state, move |c| c.create_album(&name)).await
}

#[tauri::command]
async fn rename_album(state: State<'_, Desktop>, id: i64, name: String) -> Cmd<loupe_app::AlbumDto> {
    blocking(&state, move |c| c.rename_album(id, &name)).await
}

#[tauri::command]
async fn delete_album(state: State<'_, Desktop>, id: i64) -> Cmd<()> {
    blocking(&state, move |c| c.delete_album(id)).await
}

/// Döner: albüme yeni eklenen fotoğraf sayısı (zaten içindekiler atlanır).
#[tauri::command]
async fn add_to_album(state: State<'_, Desktop>, id: i64, ids: Vec<i64>) -> Cmd<u64> {
    blocking(&state, move |c| c.add_to_album(id, &ids)).await
}

#[tauri::command]
async fn remove_from_album(state: State<'_, Desktop>, id: i64, ids: Vec<i64>) -> Cmd<u64> {
    blocking(&state, move |c| c.remove_from_album(id, &ids)).await
}

// ───────────────────────────── kişiler ─────────────────────────────

#[tauri::command]
async fn list_people(state: State<'_, Desktop>) -> Cmd<PeopleDto> {
    blocking(&state, |c| c.people()).await
}

#[tauri::command]
async fn person_faces(
    state: State<'_, Desktop>,
    person: Option<i64>,
    cluster: Option<i64>,
    limit: Option<u32>,
) -> Cmd<Vec<FaceRef>> {
    blocking(&state, move |c| c.person_faces(person, cluster, limit.unwrap_or(500))).await
}

#[tauri::command]
async fn name_cluster(state: State<'_, Desktop>, cluster: i64, name: String) -> Cmd<i64> {
    blocking(&state, move |c| c.name_cluster(cluster, &name)).await
}

#[tauri::command]
async fn rename_person(state: State<'_, Desktop>, person: i64, name: String) -> Cmd<i64> {
    blocking(&state, move |c| c.rename_person(person, &name)).await
}

#[tauri::command]
async fn merge_clusters(state: State<'_, Desktop>, into: i64, from: Vec<i64>) -> Cmd<()> {
    blocking(&state, move |c| c.merge_clusters(into, &from)).await
}

#[tauri::command]
async fn merge_persons(state: State<'_, Desktop>, into: i64, from: Vec<i64>) -> Cmd<()> {
    blocking(&state, move |c| c.merge_persons(into, &from)).await
}

#[tauri::command]
async fn move_face(state: State<'_, Desktop>, face: i64, cluster: i64) -> Cmd<()> {
    blocking(&state, move |c| c.move_face(face, cluster)).await
}

#[tauri::command]
async fn assign_face(state: State<'_, Desktop>, face: i64, person: i64) -> Cmd<()> {
    blocking(&state, move |c| c.assign_face(face, person)).await
}

#[tauri::command]
async fn detach_face(state: State<'_, Desktop>, face: i64) -> Cmd<i64> {
    blocking(&state, move |c| c.detach_face(face)).await
}

#[tauri::command]
async fn reject_face(state: State<'_, Desktop>, face: i64) -> Cmd<()> {
    blocking(&state, move |c| c.reject_face(face)).await
}

#[tauri::command]
async fn ignore_cluster(state: State<'_, Desktop>, cluster: i64, ignored: bool) -> Cmd<()> {
    blocking(&state, move |c| c.ignore_cluster(cluster, ignored)).await
}

// ───────────────────────────── etkinlikler ─────────────────────────────

#[tauri::command]
async fn list_events(state: State<'_, Desktop>) -> Cmd<Vec<EventCard>> {
    blocking(&state, |c| c.events()).await
}

#[tauri::command]
async fn rename_event(state: State<'_, Desktop>, id: i64, title: String) -> Cmd<EventCard> {
    blocking(&state, move |c| c.rename_event(id, &title)).await
}

#[tauri::command]
async fn exclude_from_event(state: State<'_, Desktop>, id: i64, media: Vec<i64>) -> Cmd<EventCard> {
    blocking(&state, move |c| c.exclude_from_event(id, &media)).await
}

#[tauri::command]
async fn set_event_locked(state: State<'_, Desktop>, id: i64, locked: bool) -> Cmd<EventCard> {
    blocking(&state, move |c| c.set_event_locked(id, locked)).await
}

// ───────────────────────────── AI ve kaynaklar ─────────────────────────────

#[tauri::command]
async fn ai_status(state: State<'_, Desktop>) -> Cmd<AiStatusDto> {
    blocking(&state, |c| c.ai_status()).await
}

#[tauri::command]
async fn set_paused(state: State<'_, Desktop>, paused: bool) -> Cmd<AiStatusDto> {
    blocking(&state, move |c| c.set_paused(paused)).await
}

#[tauri::command]
async fn set_resource_mode(state: State<'_, Desktop>, mode: String) -> Cmd<AiStatusDto> {
    blocking(&state, move |c| c.set_resource_mode(&mode)).await
}

#[tauri::command]
async fn models_status(state: State<'_, Desktop>) -> Cmd<ModelsDto> {
    blocking(&state, |c| c.models_status()).await
}

/// İndirme arka planda sürer; ilerleme `models_status` ile yoklanır.
#[tauri::command]
async fn models_install(state: State<'_, Desktop>) -> Cmd<ModelsDto> {
    state.core.models_install()
}

#[tauri::command]
async fn models_cancel(state: State<'_, Desktop>) -> Cmd<ModelsDto> {
    blocking(&state, |c| c.models_cancel()).await
}

// ───────────────────────────── Wi-Fi eşleşme ─────────────────────────────

#[tauri::command]
async fn pairing_qr(state: State<'_, Desktop>) -> Cmd<PairingQrDto> {
    state.core.pairing_qr().await
}

#[tauri::command]
async fn cancel_pairing(state: State<'_, Desktop>) -> Cmd<()> {
    state.core.cancel_pairing().await;
    Ok(())
}

#[tauri::command]
async fn list_devices(state: State<'_, Desktop>) -> Cmd<Vec<DeviceDto>> {
    blocking(&state, |c| c.devices()).await
}

#[tauri::command]
async fn revoke_device(state: State<'_, Desktop>, id: i64) -> Cmd<Vec<DeviceDto>> {
    state.core.revoke_device(id).await
}

#[tauri::command]
async fn set_conflict_policy(
    state: State<'_, Desktop>,
    id: i64,
    policy: loupe_lan::ConflictPolicy,
) -> Cmd<Vec<DeviceDto>> {
    blocking(&state, move |c| c.set_conflict_policy(id, policy)).await
}

// ───────────────────────────── bulut ve şifreleme ─────────────────────────────

#[tauri::command]
async fn cloud_settings(state: State<'_, Desktop>) -> Cmd<CloudSettingsDto> {
    blocking(&state, |c| c.cloud_settings()).await
}

#[tauri::command]
async fn save_cloud_target(
    state: State<'_, Desktop>,
    id: Option<i64>,
    input: CloudTargetInput,
) -> Cmd<CloudSettingsDto> {
    blocking(&state, move |c| c.save_cloud_target(id, &input)).await
}

#[tauri::command]
async fn set_cloud_target_enabled(state: State<'_, Desktop>, id: i64, enabled: bool) -> Cmd<CloudSettingsDto> {
    blocking(&state, move |c| c.set_cloud_target_enabled(id, enabled)).await
}

#[tauri::command]
async fn delete_cloud_target(state: State<'_, Desktop>, id: i64) -> Cmd<CloudSettingsDto> {
    blocking(&state, move |c| c.delete_cloud_target(id)).await
}

#[tauri::command]
async fn set_global_bandwidth(state: State<'_, Desktop>, mbps: Option<f64>) -> Cmd<CloudSettingsDto> {
    blocking(&state, move |c| c.set_global_bandwidth(mbps)).await
}

#[tauri::command]
async fn start_cloud_sync(state: State<'_, Desktop>) -> Cmd<Vec<CloudTargetStatusDto>> {
    blocking(&state, |c| c.start_cloud_sync()).await
}

#[tauri::command]
async fn generate_recovery_key(state: State<'_, Desktop>) -> Cmd<RecoveryDraftDto> {
    blocking(&state, |c| c.generate_recovery_key()).await
}

/// Bekleyen kurtarma anahtarını kullanıcının seçtiği dosyaya (`0600`) yazar.
#[tauri::command]
async fn export_recovery_key<R: Runtime>(app: tauri::AppHandle<R>, state: State<'_, Desktop>) -> Cmd<Option<String>> {
    let handle = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        handle
            .dialog()
            .file()
            .set_title("Kurtarma anahtarını kaydet")
            .set_file_name("loupe-kurtarma-anahtari.txt")
            .blocking_save_file()
    })
    .await
    .map_err(|e| AppError::Join(e.to_string()))?;
    let Some(path) = picked else { return Ok(None) };
    let path = path.into_path().map_err(|e| AppError::Invalid(e.to_string()))?;
    let shown = path.display().to_string();
    blocking(&state, move |c| c.export_recovery_key(&path)).await?;
    Ok(Some(shown))
}

#[derive(Debug, Deserialize)]
pub struct Answer {
    pub group: usize,
    pub value: String,
}

#[tauri::command]
async fn confirm_recovery_key(state: State<'_, Desktop>, answers: Vec<Answer>) -> Cmd<EncryptionDto> {
    let answers: Vec<(usize, String)> = answers.into_iter().map(|a| (a.group, a.value)).collect();
    blocking(&state, move |c| c.confirm_recovery_key(&answers)).await
}

#[tauri::command]
async fn cancel_recovery_key(state: State<'_, Desktop>) -> Cmd<()> {
    state.core.cancel_recovery_key();
    Ok(())
}

#[tauri::command]
async fn check_recovery_key(state: State<'_, Desktop>, key: String) -> Cmd<bool> {
    blocking(&state, move |c| c.check_recovery_key(&key)).await
}

// ───────────────────────────── doğrulama ─────────────────────────────

#[tauri::command]
async fn verify_library(state: State<'_, Desktop>, library: Option<i64>) -> Cmd<Vec<i64>> {
    blocking(&state, move |c| c.verify_library(library)).await
}

#[tauri::command]
async fn cancel_verify(state: State<'_, Desktop>, library: Option<i64>) -> Cmd<usize> {
    blocking(&state, move |c| c.cancel_verify(library)).await
}

#[tauri::command]
async fn verify_runs(state: State<'_, Desktop>, limit: Option<u32>) -> Cmd<Vec<VerifyRunDto>> {
    blocking(&state, move |c| c.verify_runs(limit.unwrap_or(10))).await
}

/// Komutları ve `thumb://` şemasını kurar (gerçek ve test çalışma zamanı için ortak).
pub fn wire<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .register_asynchronous_uri_scheme_protocol("thumb", |ctx, request, responder| {
            let Some(desk) = ctx.app_handle().try_state::<Desktop>() else {
                responder.respond(tauri::http::Response::builder().status(503).body(Vec::new()).unwrap_or_default());
                return;
            };
            let core = desk.core.clone();
            // Disk okuması ve kaçırılan boyutun kuyruğa konması WebView iş parçacığında değil.
            tauri::async_runtime::spawn_blocking(move || responder.respond(core.handle_thumb(&request)));
        })
        .invoke_handler(tauri::generate_handler![
            pick_or_add_library_root,
            pick_import_source,
            find_iphone,
            remove_library,
            update_library,
            open_media,
            reveal_media,
            trash_media,
            list_libraries,
            refresh_libraries,
            start_import,
            import_status,
            recent_imports,
            cancel_import,
            get_timeline_slice,
            prioritize_viewport,
            get_media_inspector_detail,
            set_rating_label_favorite,
            get_sidebar,
            get_status,
            search_palette,
            save_smart_album,
            delete_smart_album,
            create_album,
            rename_album,
            delete_album,
            add_to_album,
            remove_from_album,
            list_people,
            person_faces,
            name_cluster,
            rename_person,
            merge_clusters,
            merge_persons,
            move_face,
            assign_face,
            detach_face,
            reject_face,
            ignore_cluster,
            list_events,
            rename_event,
            exclude_from_event,
            set_event_locked,
            ai_status,
            models_status,
            models_install,
            models_cancel,
            set_paused,
            set_resource_mode,
            pairing_qr,
            cancel_pairing,
            list_devices,
            revoke_device,
            set_conflict_policy,
            cloud_settings,
            save_cloud_target,
            set_cloud_target_enabled,
            delete_cloud_target,
            set_global_bandwidth,
            start_cloud_sync,
            generate_recovery_key,
            export_recovery_key,
            confirm_recovery_key,
            cancel_recovery_key,
            check_recovery_key,
            verify_library,
            cancel_verify,
            verify_runs,
        ])
}

/// Uygulamayı başlatır (bin).
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let app = wire(tauri::Builder::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let core = AppCore::open(AppConfig::from_env()?)?;
            app.manage(Desktop { core: core.clone() });
            start_network(core);
            Ok(())
        })
        .build(tauri::generate_context!())?;
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event
            && let Some(desk) = handle.try_state::<Desktop>()
        {
            tauri::async_runtime::block_on(desk.core.shutdown());
        }
    });
    Ok(())
}
