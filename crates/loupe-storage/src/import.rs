//! Atomik importer (RFC-001A §3.3–3.4).
//!
//! Her mod için sıra:
//!
//! | Mod | Adımlar |
//! |---|---|
//! | Kopyala | hash → kopya kontrolü → `.loupe-tmp/` altına kopyala (kaynağı yeniden hash'leyerek) → fsync → önbellekten at → yeniden oku, BLAKE3 doğrula → `rename(NOREPLACE)` → dizin fsync → DB |
//! | Taşı (aynı FS) | hash → kopya kontrolü → kaynak değişmedi mi → `rename(NOREPLACE)` → iki dizini fsync → DB (başarısızsa geri taşı) |
//! | Taşı (farklı FS) | Kopyala ile aynı → DB → kaynağı sil → kaynak dizini fsync |
//! | Hardlink | hash → kopya kontrolü → `.loupe-tmp/` altına `link` (EXDEV → `CrossDevice`) → `rename(NOREPLACE)` → DB |
//!
//! Kaynak hiçbir modda DB commit'inden önce silinmez. Hedefte aynı ad varsa
//! üzerine yazılmaz, `_1`, `_2`… eki denenir. Kütüphane kilidi (`flock`) import
//! boyunca tutulur; ikinci bir importer `LibraryLocked` alır.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use loupe_core::{
    Blake3Hash, CommitOutcome, ImportCatalog, ImportMode, ImportSessionId, LibraryId, MediaId, NewMediaFile, Placement,
    RelPath, classify, now_ms,
};
use rustix::io::Errno;
use uuid::Uuid;

use crate::fsutil::{LibraryLock, create_dir_all_durable, drop_page_cache, fsync_dir, rename_noreplace, space};
use crate::hash::{FileDigest, HashOptions, hash_file, hash_reader};
use crate::layout::{DateResolver, MtimeResolver, candidate_name, render_dir};
use crate::volume::LibraryMarker;
use crate::{StorageError, TMP_DIR};

#[derive(Debug, Clone)]
pub struct ImporterConfig {
    pub library_id: LibraryId,
    /// `libraries.layout_template`
    pub layout_template: String,
    pub hash: HashOptions,
    /// Kopyadan sonra hedefi diskten yeniden okuyup BLAKE3 doğrula.
    pub verify_copy: bool,
    /// Kopyadan sonra diskte bırakılacak en az boş alan.
    pub space_reserve: u64,
    /// Ad çakışmasında denenecek en fazla ek (`_1` … `_n`).
    pub max_name_attempts: u32,
}

impl ImporterConfig {
    #[must_use]
    pub fn new(library_id: LibraryId) -> Self {
        Self {
            library_id,
            layout_template: "{yyyy}/{mm}/{dd}".to_owned(),
            hash: HashOptions { drop_cache: true, ..HashOptions::default() },
            verify_copy: true,
            space_reserve: 64 * 1024 * 1024,
            max_name_attempts: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportOutcome {
    Imported {
        media_id: MediaId,
        rel_path: RelPath,
        placement: Placement,
        blake3: Blake3Hash,
        /// Taşı modunda kaynak kaldırıldı mı? `false` ise dosya güvenle kütüphanede
        /// ve katalogda; yalnızca kaynağın silinmesi yeniden denenmeli.
        source_removed: bool,
    },
    /// Aynı içerik zaten katalogda: hiçbir dosyaya dokunulmadı.
    Duplicate { existing: MediaId, blake3: Blake3Hash },
}

/// Importer'ın kendi yazdığı yollar. İzleyici bunları "dış değişiklik" sayıp
/// ikinci kez import etmesin diye paylaşılır; kayıtlar kısa süre sonra düşer.
#[derive(Debug, Default)]
pub struct ExpectedPaths {
    inner: Mutex<HashMap<PathBuf, Instant>>,
}

impl ExpectedPaths {
    const TTL: Duration = Duration::from_secs(30);

    pub fn expect(&self, path: &Path) {
        if let Ok(mut m) = self.inner.lock() {
            let now = Instant::now();
            m.retain(|_, t| now.duration_since(*t) < Self::TTL);
            m.insert(path.to_path_buf(), now);
        }
    }

    /// Yol beklenen bir yazmaysa `true` (ve kaydı tüketir).
    pub fn take(&self, path: &Path) -> bool {
        self.inner.lock().ok().and_then(|mut m| m.remove(path)).is_some_and(|t| t.elapsed() < Self::TTL)
    }
}

/// Hata yolunda geçici dosyayı siler; başarıda `disarm` edilir.
struct TempGuard(Option<PathBuf>);

impl TempGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ignored = fs::remove_file(p);
        }
    }
}

pub struct Importer {
    root: PathBuf,
    root_dev: u64,
    cfg: ImporterConfig,
    dates: Box<dyn DateResolver>,
    expected: Option<Arc<ExpectedPaths>>,
    _lock: LibraryLock,
}

impl std::fmt::Debug for Importer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Importer").field("root", &self.root).field("cfg", &self.cfg).finish_non_exhaustive()
    }
}

impl Importer {
    /// Kütüphane kökünü açar: işaret dosyası olmalı, kilit alınır, önceki
    /// çökmeden kalan `.loupe-tmp/` artıkları temizlenir.
    pub fn open(root: &Path, cfg: ImporterConfig) -> Result<Self, StorageError> {
        let root = fs::canonicalize(root).map_err(StorageError::io("kütüphane kökü", root))?;
        LibraryMarker::read(&root)?;
        let lock = LibraryLock::acquire(&root)?;
        create_dir_all_durable(&root.join(TMP_DIR))?;
        let root_dev = fs::metadata(&root).map_err(StorageError::io("stat", &root))?.dev();
        let imp = Self { root, root_dev, cfg, dates: Box::new(MtimeResolver), expected: None, _lock: lock };
        imp.clean_staging()?;
        Ok(imp)
    }

    #[must_use]
    pub fn with_date_resolver(mut self, r: Box<dyn DateResolver>) -> Self {
        self.dates = r;
        self
    }

    #[must_use]
    pub fn with_expected_paths(mut self, e: Arc<ExpectedPaths>) -> Self {
        self.expected = Some(e);
        self
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `.loupe-tmp/` içindeki yarım dosyaları siler. Kilit tutulduğu için orada
    /// devam eden başka bir yazma olamaz. Silinen dosya sayısını döndürür.
    pub fn clean_staging(&self) -> Result<usize, StorageError> {
        let dir = self.root.join(TMP_DIR);
        let mut n = 0;
        for e in fs::read_dir(&dir).map_err(StorageError::io("geçici dizin", &dir))? {
            let p = e.map_err(StorageError::io("geçici dizin", &dir))?.path();
            if p.is_file() && fs::remove_file(&p).is_ok() {
                n += 1;
            }
        }
        if n > 0 {
            fsync_dir(&dir)?;
        }
        Ok(n)
    }

    pub fn import_file<C: ImportCatalog>(
        &self,
        catalog: &mut C,
        src: &Path,
        mode: ImportMode,
        session: Option<ImportSessionId>,
    ) -> Result<ImportOutcome, StorageError> {
        let src_meta = fs::symlink_metadata(src).map_err(StorageError::io("stat", src))?;
        if !src_meta.is_file() {
            return Err(StorageError::NotAFile(src.to_path_buf()));
        }
        let src = fs::canonicalize(src).map_err(StorageError::io("yol çözme", src))?;
        if src.starts_with(&self.root) {
            return Err(StorageError::SourceInsideLibrary(src));
        }
        let file_name =
            src.file_name().and_then(|n| n.to_str()).ok_or_else(|| StorageError::NotAFile(src.clone()))?.to_owned();

        // 1. hashed
        let digest = hash_file(&src, &self.cfg.hash)?;

        // 2. Tam kopya engeli: dosya sistemine dokunmadan önce.
        if let Some(existing) = catalog.find_by_blake3(&digest.blake3).map_err(StorageError::catalog)? {
            catalog.note_duplicate(existing).map_err(StorageError::catalog)?;
            return Ok(ImportOutcome::Duplicate { existing, blake3: digest.blake3 });
        }

        let capture = self.dates.resolve(&src, &src_meta);
        let dir_rel = render_dir(&self.cfg.layout_template, &capture)?;
        let same_fs = digest.device == self.root_dev;

        // 3. staged
        let mut guard = TempGuard(None);
        let staged: PathBuf = match mode {
            ImportMode::Copy => self.stage_copy(&src, &digest, &mut guard)?,
            ImportMode::Move if same_fs => {
                ensure_unchanged(&src, &digest)?;
                src.clone()
            }
            ImportMode::Move => self.stage_copy(&src, &digest, &mut guard)?,
            ImportMode::Hardlink if !same_fs => {
                return Err(StorageError::CrossDevice { src, dst: self.root.clone() });
            }
            ImportMode::Hardlink => self.stage_link(&src, &digest, &mut guard)?,
        };

        // 4. placed
        let (rel_path, target) = self.place(&staged, &dir_rel, &file_name)?;
        guard.disarm();
        if let Some(parent) = staged.parent() {
            fsync_dir(parent)?;
        }
        let placed = fs::metadata(&target).map_err(StorageError::io("stat", &target))?;

        // 5. committed: dosya → fsync → DB
        let kind = classify(&src);
        let row = NewMediaFile {
            library_id: self.cfg.library_id,
            rel_path: rel_path.clone(),
            original_filename: file_name,
            placement: mode.into(),
            import_session_id: session,
            file_type: kind.file_type,
            raw_format: kind.raw_format.map(str::to_owned),
            mime: kind.mime.to_owned(),
            size_bytes: digest.size,
            mtime_ns: placed.mtime().saturating_mul(1_000_000_000).saturating_add(placed.mtime_nsec()),
            inode: Some(placed.ino()),
            device_id: Some(placed.dev()),
            blake3: digest.blake3,
            phash: digest.phash,
            width: digest.dimensions.map(|d| d.0),
            height: digest.dimensions.map(|d| d.1),
            taken_at_utc: capture.utc_ms,
            tz_offset_min: capture.tz_offset_min,
            taken_at_source: capture.source,
            imported_at: now_ms(),
        };
        let moved_in_place = mode == ImportMode::Move && same_fs;
        let outcome = match catalog.commit_media(&row) {
            Ok(o) => o,
            Err(e) => {
                Self::undo_placement(&target, moved_in_place.then_some(src.as_path()));
                return Err(StorageError::catalog(e));
            }
        };
        let media_id = match outcome {
            CommitOutcome::Inserted(id) => id,
            CommitOutcome::Duplicate(existing) => {
                // Hash kontrolü ile commit arasında başka bir import aynı içeriği yazdı.
                Self::undo_placement(&target, moved_in_place.then_some(src.as_path()));
                return Ok(ImportOutcome::Duplicate { existing, blake3: digest.blake3 });
            }
        };

        // 6. source_removed (yalnızca Taşı)
        let source_removed = match mode {
            ImportMode::Move if same_fs => true,
            ImportMode::Move => remove_source(&src).is_ok(),
            ImportMode::Copy | ImportMode::Hardlink => false,
        };
        Ok(ImportOutcome::Imported {
            media_id,
            rel_path,
            placement: mode.into(),
            blake3: digest.blake3,
            source_removed,
        })
    }

    fn tmp_path(&self, ext: &str) -> PathBuf {
        self.root.join(TMP_DIR).join(format!("{}.{ext}", Uuid::new_v4().simple()))
    }

    /// Kaynağı geçici dosyaya kopyalar. Kopyalarken kaynağı yeniden hash'ler
    /// (hash ile kopya arasında değiştiyse yakalanır), fsync eder, sonra hedefi
    /// sayfa önbelleğinden atıp diskten okuyarak doğrular.
    fn stage_copy(&self, src: &Path, digest: &FileDigest, guard: &mut TempGuard) -> Result<PathBuf, StorageError> {
        let free = space(&self.root)?;
        let needed = digest.size.saturating_add(self.cfg.space_reserve);
        if free.available < needed {
            return Err(StorageError::InsufficientSpace { path: self.root.clone(), needed, available: free.available });
        }

        let tmp = self.tmp_path("part");
        let mut input = File::open(src).map_err(StorageError::io("okuma için açma", src))?;
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(StorageError::io("geçici dosya", &tmp))?;
        guard.0 = Some(tmp.clone());

        let src_hash = copy_hashing(&mut input, &mut out, self.cfg.hash.buffer_size)
            .map_err(StorageError::io("kopyalama", &tmp))?;
        if src_hash != digest.blake3 {
            return Err(StorageError::SourceChanged(src.to_path_buf()));
        }
        if let Ok(m) = input.metadata().and_then(|m| m.modified()) {
            let _ignored = out.set_modified(m);
        }
        out.sync_all().map_err(StorageError::io("fsync", &tmp))?;
        drop_page_cache(&out);
        drop_page_cache(&input);
        drop(out);

        if self.cfg.verify_copy {
            let mut f = File::open(&tmp).map_err(StorageError::io("doğrulama okuması", &tmp))?;
            drop_page_cache(&f);
            let (actual, _) = hash_reader(&mut f, self.cfg.hash.buffer_size, None)
                .map_err(StorageError::io("doğrulama okuması", &tmp))?;
            drop_page_cache(&f);
            if actual != digest.blake3 {
                return Err(StorageError::VerifyMismatch { path: tmp, expected: digest.blake3, actual });
            }
        }
        Ok(tmp)
    }

    fn stage_link(&self, src: &Path, digest: &FileDigest, guard: &mut TempGuard) -> Result<PathBuf, StorageError> {
        ensure_unchanged(src, digest)?;
        let tmp = self.tmp_path("link");
        fs::hard_link(src, &tmp).map_err(|e| match e.raw_os_error() {
            Some(c) if c == Errno::XDEV.raw_os_error() => {
                StorageError::CrossDevice { src: src.to_path_buf(), dst: self.root.clone() }
            }
            Some(c) if c == Errno::PERM.raw_os_error() || c == Errno::OPNOTSUPP.raw_os_error() => {
                StorageError::HardlinkUnsupported(self.root.clone())
            }
            _ => StorageError::io("hardlink", &tmp)(e),
        })?;
        guard.0 = Some(tmp.clone());
        let linked = fs::metadata(&tmp).map_err(StorageError::io("stat", &tmp))?;
        if linked.ino() != digest.inode {
            return Err(StorageError::SourceChanged(src.to_path_buf()));
        }
        Ok(tmp)
    }

    /// Hedef dizini oluşturur ve `NOREPLACE` ile yerleştirir; ad doluysa ek dener.
    fn place(&self, from: &Path, dir_rel: &RelPath, file_name: &str) -> Result<(RelPath, PathBuf), StorageError> {
        let dir = dir_rel.to_path(&self.root);
        create_dir_all_durable(&dir)?;
        for n in 0..=self.cfg.max_name_attempts {
            let rel = dir_rel.join(&candidate_name(file_name, n))?;
            let target = rel.to_path(&self.root);
            if let Some(e) = &self.expected {
                e.expect(&target);
            }
            match rename_noreplace(from, &target) {
                Ok(()) => {
                    fsync_dir(&dir)?;
                    return Ok((rel, target));
                }
                Err(StorageError::TargetExists(_)) => {
                    if let Some(e) = &self.expected {
                        e.take(&target);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(StorageError::TargetExists(dir.join(file_name)))
    }

    /// DB commit'i olmadıysa yerleştirmeyi geri al. Aynı FS'te taşınan dosya
    /// kaynağına geri döner; kopya ve link silinir (kaynak hâlâ yerinde).
    fn undo_placement(target: &Path, move_back_to: Option<&Path>) {
        match move_back_to {
            Some(src) => {
                if rename_noreplace(target, src).is_ok()
                    && let Some(p) = src.parent()
                {
                    let _ignored = fsync_dir(p);
                }
            }
            None => {
                let _ignored = fs::remove_file(target);
            }
        }
        if let Some(p) = target.parent() {
            let _ignored = fsync_dir(p);
        }
    }
}

fn copy_hashing<R: Read, W: Write>(r: &mut R, w: &mut W, buffer_size: usize) -> io::Result<Blake3Hash> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; buffer_size.max(4096)];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        w.write_all(&buf[..n])?;
    }
    Ok(Blake3Hash(*hasher.finalize().as_bytes()))
}

/// Hash'lendikten sonra kaynağa dokunulmadı mı (boyut, mtime, inode)?
fn ensure_unchanged(src: &Path, d: &FileDigest) -> Result<(), StorageError> {
    let m = fs::metadata(src).map_err(StorageError::io("stat", src))?;
    let mtime_ns = m.mtime().saturating_mul(1_000_000_000).saturating_add(m.mtime_nsec());
    if m.len() != d.size || mtime_ns != d.mtime_ns || m.ino() != d.inode {
        return Err(StorageError::SourceChanged(src.to_path_buf()));
    }
    Ok(())
}

fn remove_source(src: &Path) -> Result<(), StorageError> {
    fs::remove_file(src).map_err(StorageError::io("kaynağı silme", src))?;
    if let Some(p) = src.parent() {
        fsync_dir(p)?;
    }
    Ok(())
}
