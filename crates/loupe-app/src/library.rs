//! Kütüphane kökleri: ekleme, disk bağlı/çıkarılmış durumu, importer'lar ve
//! izleyiciden gelen değişikliklerin kataloğa işlenmesi.
//!
//! Her yönetilen kütüphane için tek bir [`Importer`] açılır ve kilidi
//! uygulama boyunca tutulur; içe aktarma ve LAN sunucusu aynı nesneyi paylaşır
//! (aynı kütüphaneye ikinci `Importer::open` kilit hatası verir).

use std::collections::HashMap;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use loupe_core::{
    CommitOutcome, ImportCatalog, ImportMode, ImportSessionId, LibraryId, LibraryKind, LibraryState, MediaId,
    NewMediaFile, Placement, RelPath, VerifyState, classify, now_ms,
};
use loupe_db::models::{Library, LibraryPresence, MediaFile, NewLibrary};
use loupe_db::repo::{library, media};
use loupe_db::{DbImportCatalog, Pool};
use loupe_storage::import::ExpectedPaths;
use loupe_storage::layout::CaptureTime;
use loupe_storage::volume::{LibraryLocator, probe_capabilities};
use loupe_storage::{
    DateResolver, HashOptions, Importer, ImporterConfig, LibraryMarker, StorageError, VolumeProbe, WatchEvent,
    hash_file,
};
use serde::{Deserialize, Serialize};

use crate::{AppError, AppResult};

/// Çekim zamanını EXIF'ten (yoksa dosya zamanından) alır; kütüphane içi
/// `{yyyy}/{mm}/{dd}` klasörü buna göre seçilir.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExifDateResolver;

impl DateResolver for ExifDateResolver {
    fn resolve(&self, path: &Path, meta: &Metadata) -> CaptureTime {
        if let Ok(m) = loupe_media::read_metadata(path)
            && let Some(t) = m.taken_at
        {
            return CaptureTime { utc_ms: t.utc_ms, tz_offset_min: t.tz_offset_min, source: t.source };
        }
        loupe_storage::MtimeResolver.resolve(path, meta)
    }
}

/// Kenar çubuğundaki kütüphane satırı.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDto {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub kind: &'static str,
    /// `online` | `offline` | `readonly` | `error`
    pub state: &'static str,
    pub state_reason: Option<String>,
    pub root: Option<String>,
    pub volume_uuid: Option<String>,
    pub volume_label: Option<String>,
    pub fs_type: Option<String>,
    pub managed: bool,
    pub primary: bool,
    /// İçe aktarmanın varsayılan klasör düzeni.
    pub layout_template: String,
    /// `copy` | `move` | `hardlink`
    pub default_import_mode: &'static str,
    pub media_count: u64,
    pub total_bytes: Option<i64>,
    pub free_bytes: Option<i64>,
}

fn uuid_string(bytes: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*bytes).hyphenated().to_string()
}

/// [`Libraries::update`] girdisi.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPatch {
    pub name: Option<String>,
    pub layout_template: Option<String>,
    pub default_import_mode: Option<String>,
    pub primary: Option<bool>,
}

pub struct Libraries {
    pool: Arc<Pool>,
    probe: VolumeProbe,
    roots: RwLock<HashMap<LibraryId, PathBuf>>,
    importers: Mutex<HashMap<LibraryId, Arc<Mutex<Importer>>>>,
    expected: Arc<ExpectedPaths>,
}

impl std::fmt::Debug for Libraries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Libraries").field("roots", &self.roots).finish_non_exhaustive()
    }
}

impl Libraries {
    #[must_use]
    pub fn new(pool: Arc<Pool>, probe: VolumeProbe) -> Self {
        Self {
            pool,
            probe,
            roots: RwLock::new(HashMap::new()),
            importers: Mutex::new(HashMap::new()),
            expected: Arc::new(ExpectedPaths::default()),
        }
    }

    #[must_use]
    pub fn expected_paths(&self) -> Arc<ExpectedPaths> {
        self.expected.clone()
    }

    /// Şu an erişilebilir kökler.
    pub fn roots(&self) -> AppResult<HashMap<LibraryId, PathBuf>> {
        Ok(self.roots.read().map_err(|_| AppError::Poisoned)?.clone())
    }

    pub fn root(&self, id: LibraryId) -> AppResult<Option<PathBuf>> {
        Ok(self.roots.read().map_err(|_| AppError::Poisoned)?.get(&id).cloned())
    }

    /// Medyanın diskteki yolu; kütüphanesi çevrimdışıysa `None`.
    pub fn media_path(&self, m: &MediaFile) -> AppResult<Option<PathBuf>> {
        Ok(self.root(m.library_id)?.map(|r| m.rel_path.to_path(&r)))
    }

    /// Yönetilen ve çevrimiçi kütüphanenin importer'ı.
    pub fn importer(&self, id: LibraryId) -> AppResult<Arc<Mutex<Importer>>> {
        self.importers
            .lock()
            .map_err(|_| AppError::Poisoned)?
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::LibraryOffline(format!("kütüphane {} çevrimdışı ya da yönetilmiyor", id.0)))
    }

    /// Ayarlar → Kütüphaneler: ad, varsayılan klasör düzeni, varsayılan mod,
    /// birincil. Verilmeyen alan değişmez.
    pub fn update(&self, id: LibraryId, patch: &LibraryPatch) -> AppResult<LibraryDto> {
        if let Some(name) = patch.name.as_deref().map(str::trim) {
            if name.is_empty() || name.len() > 120 {
                return Err(AppError::Invalid("kütüphane adı boş ya da çok uzun".into()));
            }
            self.pool.write(|c| library::rename(c, id, name))?;
        }
        if let Some(t) = patch.layout_template.as_deref().map(str::trim) {
            crate::import::validate_layout(t)?;
            self.pool.write(|c| library::set_layout(c, id, t))?;
            if let Some(imp) = self.importers.lock().map_err(|_| AppError::Poisoned)?.get(&id) {
                imp.lock().map_err(|_| AppError::Poisoned)?.set_layout_template(t);
            }
        }
        if let Some(m) = patch.default_import_mode.as_deref() {
            let mode: loupe_core::ImportMode =
                m.parse().map_err(|_| AppError::Invalid(format!("içe aktarma modu: {m}")))?;
            self.pool.write(|c| library::set_import_mode(c, id, mode))?;
        }
        if patch.primary == Some(true) {
            self.pool.write(|c| library::set_primary(c, id))?;
        }
        self.list()?
            .into_iter()
            .find(|l| l.id == id.0)
            .ok_or_else(|| AppError::Invalid(format!("kütüphane {} yok", id.0)))
    }

    /// Kütüphaneyi kataloğdan kaldırır; klasördeki dosyalara dokunmaz.
    /// Silinen medya kaydı sayısını döner.
    pub fn remove(&self, id: LibraryId) -> AppResult<u64> {
        self.pool.read(|c| library::require(c, id))?;
        self.importers.lock().map_err(|_| AppError::Poisoned)?.remove(&id);
        self.roots.write().map_err(|_| AppError::Poisoned)?.remove(&id);
        let mut total = 0u64;
        loop {
            // 500'lük işlemler: büyük kütüphanede yazma kilidi uzun tutulmaz.
            let n = self.pool.write(|c| library::remove_media_batch(c, id, 500))?;
            if n == 0 {
                break;
            }
            total += n as u64;
        }
        self.pool.write(|c| library::remove_emptied(c, id))?;
        Ok(total)
    }

    /// Her kütüphaneyi bağlı disklerde arar, durumunu kataloğa yazar,
    /// çevrimiçi yönetilen kütüphanelerin importer'ını açar.
    pub fn refresh(&self) -> AppResult<Vec<LibraryDto>> {
        let libs = self.pool.read(library::list)?;
        let mut roots = HashMap::new();
        for lib in &libs {
            let locator = LibraryLocator {
                library_uuid: uuid::Uuid::from_bytes(lib.uuid),
                volume_uuid: lib.volume_uuid.as_deref(),
                network_source: lib.network_source.as_deref(),
                rel_root: &lib.rel_root,
                last_mount_point: lib.last_mount_path.as_deref().map(Path::new),
            };
            let presence = match self.probe.locate(&locator) {
                Ok(found) => {
                    let space = loupe_storage::fsutil::space(&found.root).ok();
                    roots.insert(lib.id, found.root);
                    LibraryPresence {
                        state: LibraryState::Online,
                        state_reason: None,
                        last_mount_path: Some(found.mount_point.display().to_string()),
                        total_bytes: space.map(|s| s.total),
                        free_bytes: space.map(|s| s.available),
                    }
                }
                Err(e) => LibraryPresence {
                    state: LibraryState::Offline,
                    state_reason: Some(offline_reason(&e).to_owned()),
                    last_mount_path: lib.last_mount_path.clone(),
                    total_bytes: None,
                    free_bytes: None,
                },
            };
            self.pool.write(|c| library::update_presence(c, lib.id, &presence))?;
        }

        let mut importers = self.importers.lock().map_err(|_| AppError::Poisoned)?;
        importers.retain(|id, _| roots.contains_key(id));
        for lib in libs.iter().filter(|l| l.is_managed) {
            if let (Some(root), false) = (roots.get(&lib.id), importers.contains_key(&lib.id)) {
                let mut cfg = ImporterConfig::new(lib.id);
                cfg.layout_template.clone_from(&lib.layout_template);
                let imp = Importer::open(root, cfg)?
                    .with_date_resolver(Box::new(ExifDateResolver))
                    .with_expected_paths(self.expected.clone());
                importers.insert(lib.id, Arc::new(Mutex::new(imp)));
            }
        }
        drop(importers);
        *self.roots.write().map_err(|_| AppError::Poisoned)? = roots;
        self.list()
    }

    pub fn list(&self) -> AppResult<Vec<LibraryDto>> {
        let roots = self.roots()?;
        Ok(self.pool.read(|c| {
            let libs = library::list(c)?;
            libs.into_iter()
                .map(|l| {
                    let count = media::count(c, l.id)?;
                    Ok(to_dto(&l, roots.get(&l.id), count))
                })
                .collect()
        })?)
    }

    /// Klasörü kütüphane olarak ekler. İçinde `.loupe/library.json` varsa
    /// (disk başka bilgisayardan geldi, katalog yeniden kuruldu) aynı kimlikle
    /// yeniden bağlanır; yoksa yeni kütüphane oluşturulur.
    pub fn add_root(&self, path: &Path, name: Option<&str>, managed: bool) -> AppResult<LibraryDto> {
        let path = std::fs::canonicalize(path).map_err(AppError::io(format!("klasör {}", path.display())))?;
        if !path.is_dir() {
            return Err(AppError::Invalid(format!("{} bir klasör değil", path.display())));
        }
        let marker = match LibraryMarker::read(&path) {
            Ok(mk) => mk,
            Err(StorageError::MarkerMissing(_)) => {
                let fallback = path.file_name().and_then(|n| n.to_str()).unwrap_or("Fotoğraflar");
                let mk = LibraryMarker::new(name.unwrap_or(fallback));
                mk.write(&path)?;
                mk
            }
            Err(e) => return Err(e.into()),
        };
        let uuid = *marker.library_uuid.as_bytes();
        let existing = self.pool.read(|c| library::find_by_uuid(c, &uuid))?;
        let id = match existing {
            Some(lib) => lib.id,
            None => {
                let info = self.probe.resolve(&path)?;
                let caps = probe_capabilities(&path)?;
                let has_primary = self.pool.read(library::list)?.iter().any(|l| l.is_primary);
                let row = NewLibrary {
                    uuid,
                    name: name.map_or_else(|| marker.name.clone(), str::to_owned),
                    kind: info.kind,
                    volume_uuid: info.uuid.clone(),
                    volume_label: info.label.clone(),
                    network_source: info.network_source.clone(),
                    fs_type: Some(info.fs_type.clone()),
                    rel_root: info.rel_root.clone(),
                    last_mount_path: Some(info.mount_point.display().to_string()),
                    supports_hardlink: caps.hardlink,
                    supports_noreplace: caps.noreplace,
                    supports_inotify: info.kind != LibraryKind::Network,
                    is_managed: managed,
                    default_import_mode: ImportMode::Copy,
                    is_primary: !has_primary && managed,
                };
                self.pool.write(|c| library::insert(c, &row))?
            }
        };
        self.refresh()?.into_iter().find(|l| l.id == id.0).ok_or(AppError::NotFound { entity: "kütüphane", id: id.0 })
    }

    /// Kütüphanenin içinde bulunan dosyayı yerinde kataloğa ekler (referans
    /// kütüphane taraması ve izleyicinin gördüğü yeni dosyalar). Medya değilse
    /// ya da aynı içerik zaten kayıtlıysa `None`.
    pub fn index_in_place(
        &self,
        lib: LibraryId,
        root: &Path,
        path: &Path,
        session: Option<ImportSessionId>,
    ) -> AppResult<Option<MediaId>> {
        let kind = classify(path);
        if !kind.is_media() {
            return Ok(None);
        }
        let rel = RelPath::new(
            path.strip_prefix(root)
                .ok()
                .and_then(Path::to_str)
                .ok_or_else(|| AppError::Invalid(format!("{} kütüphane dışında", path.display())))?,
        )?;
        if rel.is_internal() || self.pool.read(|c| media::find_by_rel_path(c, lib, &rel))?.is_some() {
            return Ok(None);
        }
        let digest = hash_file(path, &HashOptions::default())?;
        let mut catalog = DbImportCatalog::new(&self.pool, session);
        if let Some(existing) = catalog.find_by_blake3(&digest.blake3)? {
            catalog.note_duplicate(existing)?;
            return Ok(None);
        }
        let meta = std::fs::metadata(path).map_err(AppError::io(format!("stat {}", path.display())))?;
        let capture = ExifDateResolver.resolve(path, &meta);
        let row = NewMediaFile {
            library_id: lib,
            original_filename: rel.file_name().to_owned(),
            rel_path: rel,
            placement: Placement::Reference,
            import_session_id: session,
            file_type: kind.file_type,
            raw_format: kind.raw_format.map(str::to_owned),
            mime: kind.mime.to_owned(),
            size_bytes: digest.size,
            mtime_ns: digest.mtime_ns,
            inode: Some(meta.ino()),
            device_id: Some(meta.dev()),
            blake3: digest.blake3,
            phash: digest.phash,
            width: digest.dimensions.map(|d| d.0),
            height: digest.dimensions.map(|d| d.1),
            taken_at_utc: capture.utc_ms,
            tz_offset_min: capture.tz_offset_min,
            taken_at_source: capture.source,
            imported_at: now_ms(),
        };
        Ok(match catalog.commit_media(&row)? {
            CommitOutcome::Inserted(id) => Some(id),
            CommitOutcome::Duplicate(_) => None,
        })
    }

    /// İzleyici olayını kataloğa işler. Yeni medya kimliklerini döndürür
    /// (çağıran küçük resim ve metadata işlerini kuyruğa koyar).
    pub fn apply_watch_event(&self, lib: LibraryId, ev: &WatchEvent) -> AppResult<Vec<MediaId>> {
        let Some(root) = self.root(lib)? else { return Ok(Vec::new()) };
        let rel_of = |p: &Path| p.strip_prefix(&root).ok().and_then(Path::to_str).and_then(|s| RelPath::new(s).ok());
        match ev {
            WatchEvent::Created(p) => Ok(self.index_in_place(lib, &root, p, None)?.into_iter().collect()),
            WatchEvent::DirCreated(dir) => {
                let mut out = Vec::new();
                for f in walk_media(dir)? {
                    out.extend(self.index_in_place(lib, &root, &f.0, None)?);
                }
                Ok(out)
            }
            WatchEvent::Removed(p) => {
                if let Some(rel) = rel_of(p)
                    && let Some(m) = self.pool.read(|c| media::find_by_rel_path(c, lib, &rel))?
                {
                    self.pool.write(|c| media::set_verify_state(c, m.id, VerifyState::Missing))?;
                }
                Ok(Vec::new())
            }
            WatchEvent::Renamed { from, to } => {
                match (rel_of(from), rel_of(to)) {
                    (Some(old), Some(new)) => {
                        if let Some(m) = self.pool.read(|c| media::find_by_rel_path(c, lib, &old))? {
                            let mtime = std::fs::metadata(to)
                                .map(|md| md.mtime().saturating_mul(1_000_000_000).saturating_add(md.mtime_nsec()))
                                .unwrap_or(m.mtime_ns);
                            self.pool.write(|c| media::update_location(c, m.id, &new, mtime))?;
                            return Ok(Vec::new());
                        }
                        Ok(self.index_in_place(lib, &root, to, None)?.into_iter().collect())
                    }
                    // Kütüphane dışından içeri taşındı.
                    (None, Some(_)) => Ok(self.index_in_place(lib, &root, to, None)?.into_iter().collect()),
                    _ => Ok(Vec::new()),
                }
            }
            WatchEvent::Rescan => {
                let mut out = Vec::new();
                for f in walk_media(&root)? {
                    out.extend(self.index_in_place(lib, &root, &f.0, None)?);
                }
                Ok(out)
            }
            // İçerik değişikliği bit-rot doğrulamasının işi (Verify Library).
            WatchEvent::Modified(_) | WatchEvent::Error(_) => Ok(Vec::new()),
        }
    }
}

fn offline_reason(e: &StorageError) -> &'static str {
    match e {
        StorageError::VolumeNotMounted { .. } => "VOLUME_NOT_MOUNTED",
        StorageError::MarkerMismatch { .. } => "MARKER_MISMATCH",
        StorageError::MarkerMissing(_) => "MARKER_MISSING",
        _ => "UNREACHABLE",
    }
}

fn to_dto(l: &Library, root: Option<&PathBuf>, media_count: u64) -> LibraryDto {
    LibraryDto {
        id: l.id.0,
        uuid: uuid_string(&l.uuid),
        name: l.name.clone(),
        kind: l.kind.as_str(),
        state: l.state.as_str(),
        state_reason: l.state_reason.clone(),
        root: root.map(|r| r.display().to_string()),
        volume_uuid: l.volume_uuid.clone(),
        volume_label: l.volume_label.clone(),
        fs_type: l.fs_type.clone(),
        managed: l.is_managed,
        primary: l.is_primary,
        layout_template: l.layout_template.clone(),
        default_import_mode: l.default_import_mode.as_str(),
        media_count,
        total_bytes: l.total_bytes,
        free_bytes: l.free_bytes,
    }
}

/// Klasördeki medya dosyaları (gizli klasörler ve `.loupe*` hariç), ad sırasıyla.
pub fn walk_media(dir: &Path) -> AppResult<Vec<(PathBuf, u64)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            // Okunamayan alt klasör taramayı durdurmaz.
            Err(e) if d != dir => {
                let _ignored = e;
                continue;
            }
            Err(e) => return Err(AppError::io(format!("klasör {}", d.display()))(e)),
        };
        for e in entries.flatten() {
            let p = e.path();
            let hidden = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
            let Ok(ft) = e.file_type() else { continue };
            if hidden {
                continue;
            }
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() && classify(&p).is_media() {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.push((p, size));
            }
        }
    }
    out.sort();
    Ok(out)
}
