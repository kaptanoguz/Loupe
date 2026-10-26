//! loupe-storage entegrasyon testleri: gerçek dosyalar (tempfile) + gerçek katalog (loupe-db).

// Test yardımcıları #[test] dışında olduğu için clippy.toml muafiyeti onları kapsamaz.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use image::{DynamicImage, ImageBuffer, Rgb};
use loupe_core::{
    ImportMode, ImportSessionId, LibraryId, LibraryKind, Placement, SessionMode, VerifyOutcome, VerifyState,
    VerifyTrigger,
};
use loupe_db::models::{NewImportSession, NewLibrary};
use loupe_db::repo::{import as import_repo, library, media, verify as verify_repo};
use loupe_db::{DbImportCatalog, DbVerifyCatalog, Pool, PoolConfig};
use loupe_storage::import::ExpectedPaths;
use loupe_storage::verify::VerifyOptions;
use loupe_storage::volume::{LibraryLocator, probe_capabilities};
use loupe_storage::{
    ImportOutcome, Importer, ImporterConfig, LibraryMarker, LibraryWatcher, StorageError, VolumeProbe, WatchEvent,
    verify_library,
};
use tempfile::TempDir;

/// 2025-10-24 12:00:00 UTC → klasör 2025/10/24
const SHOT: u64 = 1_761_307_200;

struct Fixture {
    tmp: TempDir,
    root: PathBuf,
    card: PathBuf,
    pool: Pool,
    lib: LibraryId,
    session: ImportSessionId,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Fotograflar");
        let card = tmp.path().join("EOS_DIGITAL/DCIM/100CANON");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&card).unwrap();
        let marker = LibraryMarker::new("Fotoğraflar");
        marker.write(&root).unwrap();

        let pool = Pool::open(&tmp.path().join("catalog.db"), PoolConfig::default()).unwrap();
        let lib = pool
            .write(|c| {
                library::insert(
                    c,
                    &NewLibrary {
                        uuid: *marker.library_uuid.as_bytes(),
                        name: "Fotoğraflar".into(),
                        kind: LibraryKind::Internal,
                        volume_uuid: None,
                        volume_label: None,
                        network_source: None,
                        fs_type: None,
                        rel_root: String::new(),
                        last_mount_path: None,
                        supports_hardlink: true,
                        supports_noreplace: true,
                        supports_inotify: true,
                        is_managed: true,
                        default_import_mode: ImportMode::Copy,
                        is_primary: true,
                    },
                )
            })
            .unwrap();
        let session = pool
            .write(|c| {
                import_repo::create_session(
                    c,
                    &NewImportSession {
                        library_id: lib,
                        mode: SessionMode::Copy,
                        source_root: card.display().to_string(),
                        source_volume: None,
                    },
                )
            })
            .unwrap();
        Self { tmp, root, card, pool, lib, session }
    }

    fn importer(&self) -> Importer {
        Importer::open(&self.root, ImporterConfig::new(self.lib)).unwrap()
    }

    fn catalog(&self) -> DbImportCatalog<'_> {
        DbImportCatalog::new(&self.pool, Some(self.session))
    }

    fn jpeg(&self, dir: &Path, name: &str, seed: u32) -> PathBuf {
        let path = dir.join(name);
        write_jpeg(&path, seed);
        path
    }

    fn library_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out.retain(|p| {
            let rel = p.strip_prefix(&self.root).unwrap();
            !rel.starts_with(".loupe") && !rel.starts_with(".loupe-tmp")
        });
        out.sort();
        out
    }

    fn staging_is_empty(&self) -> bool {
        fs::read_dir(self.root.join(".loupe-tmp")).unwrap().next().is_none()
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() { walk(&p, out) } else { out.push(p) }
    }
}

fn write_jpeg(path: &Path, seed: u32) {
    let img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(320, 240, |x, y| {
        let v = ((x * seed + y * 3) % 256) as u8;
        let w = if (x / 40 + y / (30 + seed)).is_multiple_of(2) { 220 } else { 30 };
        Rgb([v, w, ((x + y + seed) % 256) as u8])
    }));
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90).encode_image(&img).unwrap();
    fs::write(path, bytes).unwrap();
    let f = File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(SHOT)).unwrap();
}

/// tempdir'den farklı bir dosya sisteminde yazılabilir dizin (yoksa `None`).
fn other_filesystem(than: &Path) -> Option<TempDir> {
    let dev = fs::metadata(than).ok()?.dev();
    ["/dev/shm", "/run/user", "/tmp"]
        .iter()
        .filter_map(|base| tempfile::tempdir_in(base).ok())
        .find(|d| fs::metadata(d.path()).map(|m| m.dev() != dev).unwrap_or(false))
}

fn imported(o: ImportOutcome) -> (loupe_core::MediaId, String, bool) {
    match o {
        ImportOutcome::Imported { media_id, rel_path, source_removed, .. } => {
            (media_id, rel_path.to_string(), source_removed)
        }
        ImportOutcome::Duplicate { existing, .. } => panic!("kopya beklenmiyordu: {existing}"),
    }
}

#[test]
fn copy_import_places_verified_file_and_keeps_source() {
    let fx = Fixture::new();
    let src = fx.jpeg(&fx.card, "IMG_0001.JPG", 1);
    let imp = fx.importer();
    let (id, rel, removed) = imported(imp.import_file(&mut fx.catalog(), &src, ImportMode::Copy, None).unwrap());

    assert_eq!(rel, "2025/10/24/IMG_0001.JPG");
    assert!(!removed);
    assert!(src.exists(), "Kopyala kaynağa dokunmaz");
    let placed = fx.root.join(&rel);
    assert_eq!(fs::read(&placed).unwrap(), fs::read(&src).unwrap());
    assert_ne!(fs::metadata(&placed).unwrap().ino(), fs::metadata(&src).unwrap().ino());
    assert!(fx.staging_is_empty(), ".loupe-tmp boş kalmalı");

    let m = fx.pool.read(|c| media::get(c, id)).unwrap().unwrap();
    assert_eq!(m.placement, Placement::Copy);
    assert_eq!(m.blake3.0, *blake3::hash(&fs::read(&placed).unwrap()).as_bytes());
    assert_eq!((m.width, m.height), (Some(320), Some(240)));
    assert!(m.phash.is_none(), "pHash içe aktarmayı yavaşlatmasın diye md küçük resminden gelir");
    assert_eq!(m.import_session_id, Some(fx.session));
    assert_eq!(m.inode, Some(fs::metadata(&placed).unwrap().ino()));
    let s = fx.pool.read(|c| import_repo::get_session(c, fx.session)).unwrap().unwrap();
    assert_eq!((s.done_files, s.done_bytes), (1, m.size_bytes));
}

#[test]
fn move_import_same_filesystem_renames_source() {
    let fx = Fixture::new();
    let src = fx.jpeg(&fx.card, "IMG_0002.JPG", 2);
    let ino = fs::metadata(&src).unwrap().ino();
    let imp = fx.importer();
    let (id, rel, removed) = imported(imp.import_file(&mut fx.catalog(), &src, ImportMode::Move, None).unwrap());

    assert!(removed && !src.exists());
    let placed = fx.root.join(&rel);
    assert_eq!(fs::metadata(&placed).unwrap().ino(), ino, "aynı FS'te taşı = rename, veri kopyalanmaz");
    let m = fx.pool.read(|c| media::get(c, id)).unwrap().unwrap();
    assert_eq!(m.placement, Placement::Move);
}

#[test]
fn move_import_across_filesystems_copies_verifies_then_removes_source() {
    let fx = Fixture::new();
    let Some(other) = other_filesystem(fx.tmp.path()) else {
        eprintln!("ikinci dosya sistemi yok, test atlandı");
        return;
    };
    let src = fx.jpeg(other.path(), "IMG_0003.JPG", 3);
    let bytes = fs::read(&src).unwrap();
    let imp = fx.importer();
    let (_, rel, removed) = imported(imp.import_file(&mut fx.catalog(), &src, ImportMode::Move, None).unwrap());
    assert!(removed && !src.exists(), "DB commit'inden sonra kaynak silinir");
    assert_eq!(fs::read(fx.root.join(rel)).unwrap(), bytes);
    assert!(fx.staging_is_empty());
}

#[test]
fn hardlink_import_shares_inode() {
    let fx = Fixture::new();
    let src = fx.jpeg(&fx.card, "IMG_0004.JPG", 4);
    let imp = fx.importer();
    let (id, rel, removed) = imported(imp.import_file(&mut fx.catalog(), &src, ImportMode::Hardlink, None).unwrap());
    assert!(!removed && src.exists());
    let (a, b) = (fs::metadata(&src).unwrap(), fs::metadata(fx.root.join(&rel)).unwrap());
    assert_eq!((a.ino(), b.nlink()), (b.ino(), 2));
    assert_eq!(fx.pool.read(|c| media::get(c, id)).unwrap().unwrap().placement, Placement::Hardlink);
    assert!(fx.staging_is_empty());
}

#[test]
fn hardlink_across_filesystems_is_rejected() {
    let fx = Fixture::new();
    let Some(other) = other_filesystem(fx.tmp.path()) else {
        eprintln!("ikinci dosya sistemi yok, test atlandı");
        return;
    };
    let src = fx.jpeg(other.path(), "IMG_0005.JPG", 5);
    let imp = fx.importer();
    let err = imp.import_file(&mut fx.catalog(), &src, ImportMode::Hardlink, None).unwrap_err();
    assert!(matches!(err, StorageError::CrossDevice { .. }), "{err}");
    assert!(fx.library_files().is_empty() && fx.staging_is_empty());
}

#[test]
fn duplicate_blake3_is_blocked_in_every_mode() {
    let fx = Fixture::new();
    let first = fx.jpeg(&fx.card, "IMG_0006.JPG", 6);
    let imp = fx.importer();
    let (id, _, _) = imported(imp.import_file(&mut fx.catalog(), &first, ImportMode::Copy, None).unwrap());

    // Aynı içerik, farklı ad ve klasör.
    let other_dir = fx.card.join("../101CANON");
    fs::create_dir_all(&other_dir).unwrap();
    let copy = other_dir.join("kopya.jpg");
    fs::copy(&first, &copy).unwrap();
    for mode in [ImportMode::Copy, ImportMode::Move, ImportMode::Hardlink] {
        match imp.import_file(&mut fx.catalog(), &copy, mode, None).unwrap() {
            ImportOutcome::Duplicate { existing, .. } => assert_eq!(existing, id),
            other => panic!("{mode}: kopya engellenmedi: {other:?}"),
        }
        assert!(copy.exists(), "{mode}: kopya bulunca kaynağa dokunulmaz");
    }
    assert_eq!(fx.library_files().len(), 1);
    assert_eq!(fx.pool.read(|c| media::count(c, fx.lib)).unwrap(), 1);
    let s = fx.pool.read(|c| import_repo::get_session(c, fx.session)).unwrap().unwrap();
    assert_eq!((s.done_files, s.skipped_dup), (1, 3));
}

#[test]
fn same_name_different_content_gets_suffix() {
    let fx = Fixture::new();
    let a = fx.jpeg(&fx.card, "IMG_0007.JPG", 7);
    let dir_b = fx.card.join("../101CANON");
    fs::create_dir_all(&dir_b).unwrap();
    let b = fx.jpeg(&dir_b, "IMG_0007.JPG", 8);
    let imp = fx.importer();
    let (_, r1, _) = imported(imp.import_file(&mut fx.catalog(), &a, ImportMode::Copy, None).unwrap());
    let (_, r2, _) = imported(imp.import_file(&mut fx.catalog(), &b, ImportMode::Copy, None).unwrap());
    assert_eq!((r1.as_str(), r2.as_str()), ("2025/10/24/IMG_0007.JPG", "2025/10/24/IMG_0007_1.JPG"));
}

#[test]
fn bit_rot_and_missing_files_are_reported() {
    let fx = Fixture::new();
    let imp = fx.importer();
    let mut rels = Vec::new();
    for i in 0..3u32 {
        let src = fx.jpeg(&fx.card, &format!("IMG_10{i:02}.JPG"), 20 + i);
        rels.push(imported(imp.import_file(&mut fx.catalog(), &src, ImportMode::Copy, None).unwrap()));
    }
    drop(imp);

    // 1. dosyada tek bit çevir (boyut aynı kalır), 2. dosyayı sil.
    let rotten = fx.root.join(&rels[1].1);
    let mut f = OpenOptions::new().read(true).write(true).open(&rotten).unwrap();
    f.seek(SeekFrom::Start(1_000)).unwrap();
    let mut byte = [0u8; 1];
    std::io::Read::read_exact(&mut f, &mut byte).unwrap();
    f.seek(SeekFrom::Start(1_000)).unwrap();
    f.write_all(&[byte[0] ^ 0x01]).unwrap();
    f.sync_all().unwrap();
    drop(f);
    fs::remove_file(fx.root.join(&rels[2].1)).unwrap();

    let mut cat = DbVerifyCatalog::start(&fx.pool, fx.lib, VerifyTrigger::Manual).unwrap();
    let opts = VerifyOptions { quarantine: true, batch_size: 2, ..VerifyOptions::default() };
    let report = verify_library(&mut cat, &fx.root, &opts, None).unwrap();
    let run = cat.finish().unwrap();

    assert_eq!((report.checked, report.ok), (3, 1));
    assert_eq!(report.count(|o| matches!(o, VerifyOutcome::Mismatch { .. })), 1);
    assert_eq!(report.count(|o| matches!(o, VerifyOutcome::Missing)), 1);
    let bad = report.findings.iter().find(|f| matches!(f.outcome, VerifyOutcome::Mismatch { .. })).unwrap();
    assert_eq!(bad.entry.media_id, rels[1].0);
    let q = bad.quarantine.as_ref().unwrap();
    assert_eq!(fs::read(q.to_path(&fx.root)).unwrap(), fs::read(&rotten).unwrap());

    assert_eq!((run.total, run.checked, run.ok, run.mismatched, run.missing), (3, 3, 1, 1, 1));
    assert_eq!(run.state, "done");
    let states: Vec<VerifyState> =
        rels.iter().map(|r| fx.pool.read(|c| media::get(c, r.0)).unwrap().unwrap().verify_state).collect();
    assert_eq!(states, [VerifyState::Ok, VerifyState::Mismatch, VerifyState::Missing]);
    let findings = fx.pool.read(|c| verify_repo::open_findings(c, fx.lib)).unwrap();
    assert_eq!(findings.len(), 2);
    assert!(
        findings
            .iter()
            .any(|f| f.kind == VerifyState::Mismatch && f.actual_blake3.is_some() && f.quarantine_rel.is_some())
    );
}

#[test]
fn verify_refuses_unmounted_library() {
    let fx = Fixture::new();
    let imp = fx.importer();
    let src = fx.jpeg(&fx.card, "IMG_2000.JPG", 40);
    let (id, _, _) = imported(imp.import_file(&mut fx.catalog(), &src, ImportMode::Copy, None).unwrap());
    drop(imp);
    // Disk çıkarıldı: mount noktası boş bir dizin.
    fs::rename(&fx.root, fx.tmp.path().join("cikarildi")).unwrap();
    fs::create_dir(&fx.root).unwrap();

    let mut cat = DbVerifyCatalog::start(&fx.pool, fx.lib, VerifyTrigger::Scheduled).unwrap();
    let err = verify_library(&mut cat, &fx.root, &VerifyOptions::default(), None).unwrap_err();
    assert!(matches!(err, StorageError::MarkerMissing(_)), "{err}");
    let m = fx.pool.read(|c| media::get(c, id)).unwrap().unwrap();
    assert_eq!(m.verify_state, VerifyState::Ok, "bağlı olmayan disk 'kayıp' diye işaretlenmez");
}

#[test]
fn library_lock_marker_and_staging_cleanup() {
    let fx = Fixture::new();
    let imp = fx.importer();
    assert!(matches!(Importer::open(&fx.root, ImporterConfig::new(fx.lib)), Err(StorageError::LibraryLocked(_))));
    drop(imp);

    // Çökmeden kalan yarım dosya açılışta temizlenir.
    fs::write(fx.root.join(".loupe-tmp/0123.part"), b"yarim").unwrap();
    let imp = fx.importer();
    assert!(fx.staging_is_empty());
    drop(imp);

    let bare = fx.tmp.path().join("isaretsiz");
    fs::create_dir(&bare).unwrap();
    assert!(matches!(Importer::open(&bare, ImporterConfig::new(fx.lib)), Err(StorageError::MarkerMissing(_))));

    let inside = fx.root.join("2025");
    fs::create_dir_all(&inside).unwrap();
    write_jpeg(&inside.join("x.jpg"), 3);
    let imp = fx.importer();
    let err = imp.import_file(&mut fx.catalog(), &inside.join("x.jpg"), ImportMode::Copy, None).unwrap_err();
    assert!(matches!(err, StorageError::SourceInsideLibrary(_)));
}

#[test]
fn volume_resolves_and_relocates_after_remount() {
    let tmp = tempfile::tempdir().unwrap();
    // Gerçek sistem: tempdir'in diski bulunur.
    let info = VolumeProbe::default().resolve(tmp.path()).unwrap();
    assert!(tmp.path().canonicalize().unwrap().starts_with(&info.mount_point));
    let caps = probe_capabilities(tmp.path()).unwrap();
    assert!(caps.hardlink);

    // Sahte sistem: aynı disk (UUID) önce mnt-a'ya, sonra mnt-b'ye bağlanıyor.
    let base = tmp.path().canonicalize().unwrap();
    let dev = base.join("dev");
    let by_uuid = base.join("by-uuid");
    fs::create_dir_all(&dev).unwrap();
    fs::create_dir_all(&by_uuid).unwrap();
    fs::write(dev.join("sdb1"), b"").unwrap();
    std::os::unix::fs::symlink(dev.join("sdb1"), by_uuid.join("ABCD-1234")).unwrap();
    let (mnt_a, mnt_b) = (base.join("mnt-a"), base.join("mnt-b"));
    let lib_rel = "Arsiv/Fotograflar";
    fs::create_dir_all(mnt_a.join(lib_rel)).unwrap();
    let marker = LibraryMarker::new("Arşiv");
    marker.write(&mnt_a.join(lib_rel)).unwrap();

    let mountinfo = base.join("mountinfo");
    let write_mounts = |mp: &Path| {
        fs::write(
            &mountinfo,
            format!(
                "36 1 259:2 / / rw - ext4 /dev/nvme0n1p2 rw\n97 36 8:17 / {} rw - exfat {} rw\n",
                mp.display(),
                dev.join("sdb1").display()
            ),
        )
        .unwrap();
    };
    let probe = VolumeProbe { mountinfo: mountinfo.clone(), by_uuid: by_uuid.clone(), by_label: base.join("yok") };
    write_mounts(&mnt_a);

    let resolved = probe.resolve(&mnt_a.join(lib_rel)).unwrap();
    assert_eq!((resolved.uuid.as_deref(), resolved.rel_root.as_str()), (Some("ABCD-1234"), lib_rel));
    let loc = LibraryLocator {
        library_uuid: marker.library_uuid,
        volume_uuid: Some("ABCD-1234"),
        network_source: None,
        rel_root: lib_rel,
        last_mount_point: Some(&mnt_a),
    };
    let found = probe.locate(&loc).unwrap();
    assert_eq!((found.root.clone(), found.moved), (mnt_a.join(lib_rel), false));

    // Disk çıkarılıp başka yola takıldı.
    fs::rename(&mnt_a, &mnt_b).unwrap();
    fs::create_dir(&mnt_a).unwrap();
    write_mounts(&mnt_b);
    let found = probe.locate(&loc).unwrap();
    assert_eq!((found.root, found.mount_point, found.moved), (mnt_b.join(lib_rel), mnt_b.clone(), true));

    // Aynı diskte başka kütüphane işareti: yeni kütüphane sanılmaz.
    LibraryMarker::new("Başka").write(&mnt_b.join(lib_rel)).unwrap();
    assert!(matches!(probe.locate(&loc), Err(StorageError::MarkerMismatch { .. })));

    // Disk hiç bağlı değil.
    fs::write(&mountinfo, "36 1 259:2 / / rw - ext4 /dev/nvme0n1p2 rw\n").unwrap();
    let loc = LibraryLocator { last_mount_point: None, ..loc };
    assert!(matches!(probe.locate(&loc), Err(StorageError::VolumeNotMounted { .. })));
}

fn wait_for(rx: &mpsc::Receiver<WatchEvent>, pred: impl Fn(&WatchEvent) -> bool) -> Vec<WatchEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            let done = pred(&ev);
            seen.push(ev);
            if done {
                return seen;
            }
        }
    }
    panic!("beklenen olay gelmedi; görülenler: {seen:?}");
}

#[test]
fn watcher_reports_external_changes_and_ignores_own_writes() {
    let fx = Fixture::new();
    let expected = Arc::new(ExpectedPaths::default());
    let (tx, rx) = mpsc::channel();
    let watcher = LibraryWatcher::start(&fx.root, Duration::from_millis(150), tx, Some(Arc::clone(&expected))).unwrap();
    let root = watcher.root().to_path_buf();

    // Dışarıdan eklenen dosya.
    let ext_dir = root.join("Disaridan");
    fs::create_dir(&ext_dir).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let ext = ext_dir.join("dis.jpg");
    write_jpeg(&ext, 50);
    wait_for(&rx, |e| *e == WatchEvent::Created(ext.clone()));

    // Importer'ın kendi yazması (.loupe-tmp → ağaç) olay üretmemeli.
    let imp = fx.importer().with_expected_paths(Arc::clone(&expected));
    let src = fx.jpeg(&fx.card, "IMG_3000.JPG", 51);
    let (_, rel, _) = imported(imp.import_file(&mut fx.catalog(), &src, ImportMode::Copy, None).unwrap());
    let own = root.join(&rel);
    let sentinel = ext_dir.join("isaret.jpg");
    std::thread::sleep(Duration::from_millis(400));
    write_jpeg(&sentinel, 52);
    let seen = wait_for(&rx, |e| *e == WatchEvent::Created(sentinel.clone()));
    let mentions = |p: &Path, e: &WatchEvent| match e {
        WatchEvent::Created(x) | WatchEvent::Modified(x) | WatchEvent::Removed(x) => x == p,
        WatchEvent::Renamed { from, to } => from == p || to == p,
        _ => false,
    };
    assert!(!seen.iter().any(|e| mentions(&own, e)), "importer yazması olay üretti: {seen:?}");
    assert!(!seen.iter().any(|e| matches!(e, WatchEvent::Created(p) if p.starts_with(root.join(".loupe-tmp")))));

    // Silme ve yeniden adlandırma.
    let renamed = ext_dir.join("yeni-ad.jpg");
    fs::rename(&ext, &renamed).unwrap();
    wait_for(&rx, |e| *e == WatchEvent::Renamed { from: ext.clone(), to: renamed.clone() });
    fs::remove_file(&renamed).unwrap();
    wait_for(&rx, |e| *e == WatchEvent::Removed(renamed.clone()));
    watcher.stop();
}
