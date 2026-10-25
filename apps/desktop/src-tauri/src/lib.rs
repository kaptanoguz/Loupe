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
    AppConfig, AppCore, AppError, AppResult, CullRequest, CullResult, ImportRequest, ImportSessionDto, InspectorDetail,
    LibraryDto, SidebarDto, StatusDto, TimelineRequest, TimelineSlice,
};
use serde::Serialize;
use tauri::{Manager, Runtime, State};
use tauri_plugin_dialog::DialogExt;

/// LAN sunucusu tüm arayüzlerde, sistemin verdiği bir portta dinler (mDNS duyurur).
pub const LAN_BIND: &str = "0.0.0.0:0";

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
