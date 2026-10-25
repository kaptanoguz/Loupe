//! İçe aktarma oturumları: kaynak klasörü tarama, atomik importer ile
//! Kopyala/Taşı/Hardlink, referans kütüphanesini yerinde tarama ve ardından
//! küçük resim + metadata işlerinin kuyruğa konması.
//!
//! Oturum ilerlemesi `import_sessions` tablosundadır (sayaçları katalog aynı
//! transaction'da artırır); arayüz [`ImportSessionDto`]'yu yoklar.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use loupe_core::{ImportMode, ImportSessionId, LibraryId, MediaId, SessionMode, SessionState};
use loupe_db::models::{ImportSession, NewImportSession};
use loupe_db::repo::{import as import_repo, library};
use loupe_db::{DbImportCatalog, Pool};
use loupe_jobs::{JobKind, JobQueue, NewJob, Priority};
use loupe_storage::ImportOutcome;
use serde::{Deserialize, Serialize};

use crate::library::{Libraries, walk_media};
use crate::{AppError, AppResult};

/// İçe aktarılan her medya için sıradaki işler. `md` küçük resmi AI ardıl
/// işlerini, metadata işi ters coğrafi kodlamayı tetikler.
pub fn post_import_jobs(ids: &[MediaId]) -> Vec<NewJob> {
    ids.iter()
        .flat_map(|&id| {
            [
                NewJob::media(JobKind::Thumb, id, "sm", Priority::Interactive),
                NewJob::media(JobKind::MetadataFull, id, "", Priority::Normal),
                NewJob::media(JobKind::Thumb, id, "md", Priority::Normal),
            ]
        })
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub library_id: i64,
    pub source: PathBuf,
    /// `copy` | `move` | `hardlink`
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionDto {
    pub id: i64,
    pub library_id: i64,
    pub mode: &'static str,
    pub source: String,
    /// `scanning` | `running` | `done` | `failed` | `cancelled` | `paused`
    pub state: &'static str,
    pub total_files: u64,
    pub total_bytes: u64,
    pub done_files: u64,
    pub done_bytes: u64,
    pub skipped_dup: u64,
    pub failed_files: u64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub last_error: Option<String>,
}

impl ImportSessionDto {
    fn from_row(s: &ImportSession, last_error: Option<String>) -> Self {
        Self {
            id: s.id.0,
            library_id: s.library_id.0,
            mode: s.mode.as_str(),
            source: s.source_root.clone(),
            state: s.state.as_str(),
            total_files: s.total_files,
            total_bytes: s.total_bytes,
            done_files: s.done_files,
            done_bytes: s.done_bytes,
            skipped_dup: s.skipped_dup,
            failed_files: s.failed_files,
            started_at: s.started_at,
            finished_at: s.finished_at,
            last_error,
        }
    }
}

#[derive(Debug, Default)]
struct Running {
    cancel: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
}

/// Arka planda çalışan oturumlar.
type NewMediaHook = Arc<dyn Fn(&[MediaId]) + Send + Sync>;

pub struct Imports {
    pool: Arc<Pool>,
    queue: Arc<JobQueue>,
    libraries: Arc<Libraries>,
    running: Mutex<HashMap<ImportSessionId, Running>>,
    on_new: Option<NewMediaHook>,
}

impl std::fmt::Debug for Imports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Imports").field("running", &self.running).finish_non_exhaustive()
    }
}

impl Imports {
    #[must_use]
    pub fn new(pool: Arc<Pool>, queue: Arc<JobQueue>, libraries: Arc<Libraries>) -> Self {
        Self { pool, queue, libraries, running: Mutex::new(HashMap::new()), on_new: None }
    }

    /// Yeni medya kataloğa girince (ör. bulut kuyruğu) çağrılır.
    #[must_use]
    pub fn with_new_media_hook(mut self, hook: impl Fn(&[MediaId]) + Send + Sync + 'static) -> Self {
        self.on_new = Some(Arc::new(hook));
        self
    }

    /// Kaynak klasörü yönetilen kütüphaneye aktarır (arka planda).
    pub fn start(self: &Arc<Self>, req: &ImportRequest) -> AppResult<ImportSessionDto> {
        let mode: ImportMode = req.mode.parse().map_err(|_| AppError::Invalid(format!("import modu: {}", req.mode)))?;
        let lib = LibraryId(req.library_id);
        let importer = self.libraries.importer(lib)?;
        let source = std::fs::canonicalize(&req.source)
            .map_err(AppError::io(format!("kaynak klasör {}", req.source.display())))?;
        if let Some(root) = self.libraries.root(lib)?
            && source.starts_with(&root)
        {
            return Err(AppError::Invalid("kaynak klasör kütüphanenin içinde".into()));
        }
        let session = self.create_session(lib, SessionMode::from(mode), &source)?;
        let this = self.clone();
        self.spawn(session, move |run| {
            this.run(session, run, &source, |path, catalog| {
                let imp = importer.lock().map_err(|_| AppError::Poisoned)?;
                Ok(match imp.import_file(catalog, path, mode, Some(session))? {
                    ImportOutcome::Imported { media_id, .. } => Some(media_id),
                    ImportOutcome::Duplicate { .. } => None,
                })
            })
        })?;
        self.status(session)
    }

    /// Referans (yönetilmeyen) kütüphaneyi yerinde tarar; dosyalara dokunmaz.
    pub fn scan_reference(self: &Arc<Self>, lib: LibraryId) -> AppResult<ImportSessionDto> {
        let root = self
            .libraries
            .root(lib)?
            .ok_or_else(|| AppError::LibraryOffline(format!("kütüphane {} çevrimdışı", lib.0)))?;
        let session = self.create_session(lib, SessionMode::Reference, &root)?;
        let (this, libs) = (self.clone(), self.libraries.clone());
        self.spawn(session, move |run| {
            this.run(session, run, &root, |path, _| libs.index_in_place(lib, &root, path, Some(session)))
        })?;
        self.status(session)
    }

    pub fn status(&self, id: ImportSessionId) -> AppResult<ImportSessionDto> {
        let s = self
            .pool
            .read(|c| import_repo::get_session(c, id))?
            .ok_or(AppError::NotFound { entity: "import oturumu", id: id.0 })?;
        let last = self
            .running
            .lock()
            .map_err(|_| AppError::Poisoned)?
            .get(&id)
            .and_then(|r| r.last_error.lock().ok().and_then(|e| e.clone()));
        Ok(ImportSessionDto::from_row(&s, last))
    }

    /// Son oturumlar (yeniden başlatmadan sonra arayüz durumu geri yükler).
    pub fn recent(&self, limit: u32) -> AppResult<Vec<ImportSessionDto>> {
        let ids: Vec<i64> = self.pool.read(|c| {
            Ok(c.prepare_cached("SELECT id FROM import_sessions ORDER BY started_at DESC, id DESC LIMIT ?1")?
                .query_map([limit], |r| r.get(0))?
                .collect::<Result<_, _>>()?)
        })?;
        ids.into_iter().map(|id| self.status(ImportSessionId(id))).collect()
    }

    pub fn cancel(&self, id: ImportSessionId) -> AppResult<bool> {
        let running = self.running.lock().map_err(|_| AppError::Poisoned)?;
        Ok(running.get(&id).is_some_and(|r| {
            r.cancel.store(true, Ordering::Relaxed);
            true
        }))
    }

    /// Bitmemiş oturum var mı (durum çubuğu).
    pub fn active(&self) -> AppResult<usize> {
        Ok(self.pool.read(import_repo::active_sessions)?.len())
    }

    fn create_session(&self, lib: LibraryId, mode: SessionMode, source: &Path) -> AppResult<ImportSessionId> {
        self.pool.read(|c| library::require(c, lib))?;
        Ok(self.pool.write(|c| {
            import_repo::create_session(
                c,
                &NewImportSession {
                    library_id: lib,
                    mode,
                    source_root: source.display().to_string(),
                    source_volume: None,
                },
            )
        })?)
    }

    fn spawn(
        &self,
        session: ImportSessionId,
        work: impl FnOnce(&Running) -> AppResult<()> + Send + 'static,
    ) -> AppResult<()> {
        let run = Running::default();
        let handle = Running { cancel: run.cancel.clone(), last_error: run.last_error.clone() };
        self.running.lock().map_err(|_| AppError::Poisoned)?.insert(session, run);
        let pool = self.pool.clone();
        std::thread::Builder::new()
            .name(format!("loupe-import-{}", session.0))
            .spawn(move || {
                if let Err(e) = work(&handle) {
                    if let Ok(mut last) = handle.last_error.lock() {
                        *last = Some(e.to_string());
                    }
                    let _ignored = pool.write(|c| import_repo::set_session_state(c, session, SessionState::Failed));
                }
            })
            .map_err(AppError::io("import iş parçacığı"))?;
        Ok(())
    }

    fn run(
        &self,
        session: ImportSessionId,
        run: &Running,
        source: &Path,
        mut one: impl FnMut(&Path, &mut DbImportCatalog<'_>) -> AppResult<Option<MediaId>>,
    ) -> AppResult<()> {
        let files = walk_media(source)?;
        let bytes: u64 = files.iter().map(|f| f.1).sum();
        self.pool.write(|c| {
            import_repo::set_session_totals(c, session, files.len() as u64, bytes)?;
            import_repo::set_session_state(c, session, SessionState::Running)
        })?;
        let mut catalog = DbImportCatalog::new(&self.pool, Some(session));
        let mut fresh = Vec::new();
        for (path, _) in &files {
            if run.cancel.load(Ordering::Relaxed) {
                self.pool.write(|c| import_repo::set_session_state(c, session, SessionState::Cancelled))?;
                return self.flush(&mut fresh);
            }
            match one(path, &mut catalog) {
                Ok(Some(id)) => fresh.push(id),
                Ok(None) => {}
                Err(e) => {
                    if let Ok(mut last) = run.last_error.lock() {
                        *last = Some(format!("{}: {e}", path.display()));
                    }
                    let p = import_repo::Progress { failed_files: 1, ..Default::default() };
                    self.pool.write(|c| import_repo::add_progress(c, session, p))?;
                }
            }
            // Küçük resimler içe aktarma bitmeden görünmeye başlasın.
            if fresh.len() >= loupe_jobs::ENQUEUE_BATCH / 4 {
                self.flush(&mut fresh)?;
            }
        }
        self.flush(&mut fresh)?;
        self.pool.write(|c| import_repo::set_session_state(c, session, SessionState::Done))?;
        Ok(())
    }

    fn flush(&self, fresh: &mut Vec<MediaId>) -> AppResult<()> {
        if !fresh.is_empty() {
            self.queue.enqueue_batch(&post_import_jobs(fresh))?;
            if let Some(hook) = &self.on_new {
                hook(fresh);
            }
            fresh.clear();
        }
        Ok(())
    }
}
