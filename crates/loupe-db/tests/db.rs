//! loupe-db entegrasyon testleri: gerçek dosya üzerinde SQLite + sqlite-vec.

// Test yardımcıları #[test] dışında olduğu için clippy.toml muafiyeti onları kapsamaz.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use loupe_core::{
    Blake3Hash, ColorLabel, CommitOutcome, Flag, ImportCatalog, ImportItemState, ImportMode, LibraryKind, LibraryState,
    MediaFileType, NewMediaFile, PHash, Placement, RelPath, SessionMode, SessionState, TakenAtSource, VerifyState,
};
use loupe_db::models::{LibraryPresence, NewImportItem, NewImportSession, NewLibrary, UserFieldsUpdate};
use loupe_db::repo::{import, library, media};
use loupe_db::{BATCH_SIZE, CLIP_DIM, DbError, DbImportCatalog, Pool, PoolConfig, insert_media_batch, migrate};
use tempfile::TempDir;

fn open() -> (TempDir, Pool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::open(&dir.path().join("catalog.db"), PoolConfig::default()).unwrap();
    (dir, pool)
}

fn new_library(name: &str, primary: bool) -> NewLibrary {
    let mut uuid = [0u8; 16];
    uuid[..name.len().min(16)].copy_from_slice(&name.as_bytes()[..name.len().min(16)]);
    NewLibrary {
        uuid,
        name: name.to_owned(),
        kind: LibraryKind::Internal,
        volume_uuid: Some("0b5e-test".to_owned()),
        volume_label: None,
        network_source: None,
        fs_type: Some("ext4".to_owned()),
        rel_root: name.to_owned(),
        last_mount_path: Some(format!("/home/u/{name}")),
        supports_hardlink: true,
        supports_noreplace: true,
        supports_inotify: true,
        is_managed: true,
        default_import_mode: ImportMode::Copy,
        is_primary: primary,
    }
}

fn hash_of(i: u64) -> Blake3Hash {
    Blake3Hash(*blake3::hash(&i.to_le_bytes()).as_bytes())
}

fn media_row(lib: loupe_core::LibraryId, i: u64) -> NewMediaFile {
    NewMediaFile {
        library_id: lib,
        rel_path: RelPath::new(&format!("2026/10/{:02}/IMG_{i:05}.JPG", i % 28 + 1)).unwrap(),
        original_filename: format!("IMG_{i:05}.JPG"),
        placement: Placement::Copy,
        import_session_id: None,
        file_type: MediaFileType::Jpeg,
        raw_format: None,
        mime: "image/jpeg".to_owned(),
        size_bytes: 1_000 + i,
        mtime_ns: 1_700_000_000_000_000_000,
        inode: Some(u64::MAX - i),
        device_id: Some(2049),
        blake3: hash_of(i),
        phash: Some(PHash(0xF0F0_0000_0000_0000 | i)),
        width: Some(6000),
        height: Some(4000),
        taken_at_utc: 1_761_300_000_000 + i64::try_from(i).unwrap(),
        tz_offset_min: Some(180),
        taken_at_source: TakenAtSource::Mtime,
        imported_at: 1_761_400_000_000,
    }
}

#[test]
fn migrations_apply_once_and_pragmas_hold() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    {
        let pool = Pool::open(&path, PoolConfig::default()).unwrap();
        let r = pool.migration_report();
        assert_eq!((r.from_version, r.to_version, r.applied.clone()), (0, 5, vec![1, 2, 3, 4, 5]));
        pool.write(|c| {
            let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
            let sync: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
            let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
            let mmap: i64 = c.query_row("PRAGMA mmap_size", [], |r| r.get(0))?;
            let busy: i64 = c.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
            assert_eq!((mode.as_str(), sync, fk, mmap, busy), ("wal", 1, 1, 256 * 1024 * 1024, 5000));
            Ok(())
        })
        .unwrap();
        // Okuyucu da aynı ayarlarla ve salt okunur açılır.
        pool.read(|c| {
            let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
            assert_eq!(fk, 1);
            assert!(c.execute("INSERT INTO settings (key, value, updated_at) VALUES ('x', '1', 0)", []).is_err());
            Ok(())
        })
        .unwrap();
    }
    let pool = Pool::open(&path, PoolConfig::default()).unwrap();
    let r = pool.migration_report();
    assert_eq!((r.from_version, r.to_version, r.applied.len()), (5, 5, 0));
}

#[test]
fn edited_migration_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    drop(Pool::open(&path, PoolConfig::default()).unwrap());
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute("UPDATE _loupe_migrations SET checksum = ?1 WHERE version = 2", ["0".repeat(64)]).unwrap();
    drop(c);
    assert!(matches!(Pool::open(&path, PoolConfig::default()), Err(DbError::ChecksumMismatch { version: 2, .. })));
}

#[test]
fn newer_schema_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    drop(Pool::open(&path, PoolConfig::default()).unwrap());
    let c = rusqlite::Connection::open(&path).unwrap();
    c.pragma_update(None, "user_version", 99).unwrap();
    drop(c);
    assert!(matches!(Pool::open(&path, PoolConfig::default()), Err(DbError::SchemaTooNew { found: 99, known: 5 })));
}

#[test]
fn failed_migration_rolls_back_completely() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let mut c = loupe_db::open_connection(&path, loupe_db::Role::Writer, &Default::default()).unwrap();
    // 0001'in oluşturacağı bir tabloyu önceden yarat: migrasyon yarıda patlamalı.
    c.execute_batch("CREATE TABLE persons (x INTEGER)").unwrap();
    let err = migrate(&mut c).unwrap_err();
    assert!(matches!(err, DbError::MigrationFailed { version: 1, .. }), "{err}");
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    let libs: i64 =
        c.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'libraries'", [], |r| r.get(0)).unwrap();
    assert_eq!((v, libs), (0, 0), "yarım migrasyon kalmamalı");
}

#[test]
fn clip_dimension_matches_rust_constant() {
    let (_d, pool) = open();
    assert_eq!(CLIP_DIM, 512);
    pool.write(|c| {
        assert_eq!(loupe_db::migrate::vec_dimension(c, "vec_media_clip")?, Some(CLIP_DIM));
        let lib = library::insert(c, &new_library("clip", true))?;
        let CommitOutcome::Inserted(id) = media::insert(c, &media_row(lib, 1))? else { panic!("eklenmeli") };
        let v = |n: usize| format!("[{}]", vec!["0.01"; n].join(","));
        c.execute(
            "INSERT INTO vec_media_clip (media_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![id.0, v(CLIP_DIM)],
        )?;
        let wrong = c.execute(
            "INSERT INTO vec_media_clip (media_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![id.0 + 1, v(768)],
        );
        assert!(wrong.is_err(), "768 boyutlu vektör reddedilmeli");
        // Medya silinince vektör de tetikleyiciyle silinir.
        media::delete(c, id)?;
        let left: i64 = c.query_row("SELECT count(*) FROM vec_media_clip", [], |r| r.get(0))?;
        assert_eq!(left, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn library_crud() {
    let (_d, pool) = open();
    pool.write(|c| {
        let id = library::insert(c, &new_library("Arsiv", true))?;
        let lib = library::require(c, id)?;
        assert_eq!(
            (lib.name.as_str(), lib.state, lib.layout_template.as_str()),
            ("Arsiv", LibraryState::Online, "{yyyy}/{mm}/{dd}")
        );
        assert_eq!(library::find_by_uuid(c, &lib.uuid)?.map(|l| l.id), Some(id));
        assert_eq!(library::find_by_volume(c, "0b5e-test")?.len(), 1);

        library::rename(c, id, "Arşiv 4TB")?;
        library::update_presence(
            c,
            id,
            &LibraryPresence {
                state: LibraryState::Offline,
                state_reason: Some("VOLUME_NOT_MOUNTED".into()),
                last_mount_path: None,
                total_bytes: Some(4_000_000_000_000),
                free_bytes: Some(1_000_000_000_000),
            },
        )?;
        let lib = library::require(c, id)?;
        assert_eq!((lib.name.as_str(), lib.state), ("Arşiv 4TB", LibraryState::Offline));
        assert_eq!(lib.last_mount_path.as_deref(), Some("/home/u/Arsiv"), "ipucu yolu korunur");

        // İkinci birincil kütüphane kısmi UNIQUE indeksle reddedilir.
        assert!(library::insert(c, &new_library("Ikinci", true)).is_err());
        // Çıkarılabilir disk UUID'siz tanımlanamaz (CHECK).
        let mut removable = new_library("Kart", false);
        removable.kind = LibraryKind::Removable;
        removable.volume_uuid = None;
        assert!(library::insert(c, &removable).is_err());

        let other = library::insert(c, &new_library("Bos", false))?;
        assert_eq!(library::list(c)?.len(), 2);
        assert!(library::delete(c, other)?);
        // İçinde medya olan kütüphane silinemez (ON DELETE RESTRICT).
        media::insert(c, &media_row(id, 7))?;
        assert!(library::delete(c, id).is_err());
        Ok(())
    })
    .unwrap();
}

#[test]
fn media_crud_and_duplicate_blocking() {
    let (_d, pool) = open();
    pool.write(|c| {
        let lib = library::insert(c, &new_library("Ana", true))?;
        let row = media_row(lib, 42);
        let CommitOutcome::Inserted(id) = media::insert(c, &row)? else { panic!("ilk ekleme") };

        // Aynı içerik farklı yolla: satır yazılmaz, mevcut kimlik döner.
        let mut same = row.clone();
        same.rel_path = RelPath::new("2026/10/25/kopya.jpg")?;
        assert_eq!(media::insert(c, &same)?, CommitOutcome::Duplicate(id));
        // Aynı yola farklı içerik: gerçek hata (sessizce yutulmaz).
        let mut clash = media_row(lib, 43);
        clash.rel_path = row.rel_path.clone();
        assert!(media::insert(c, &clash).is_err());

        let m = media::get(c, id)?.unwrap();
        assert_eq!((m.blake3, m.inode, m.phash), (row.blake3, row.inode, row.phash));
        assert_eq!(m.last_verified_at, row.imported_at);
        assert_eq!(media::find_by_rel_path(c, lib, &row.rel_path)?.map(|m| m.id), Some(id));
        assert_eq!(media::find_by_blake3(c, &row.blake3)?, Some(id));

        media::update_user_fields(
            c,
            id,
            &UserFieldsUpdate {
                rating: Some(4),
                color_label: Some(Some(ColorLabel::Green)),
                flag: Some(Flag::Pick),
                ..Default::default()
            },
        )?;
        let dirty: i64 = c.query_row("SELECT sidecar_dirty FROM media_files WHERE id = ?1", [id.0], |r| r.get(0))?;
        let m = media::get(c, id)?.unwrap();
        assert_eq!((m.rating, m.color_label, m.flag, dirty), (4, Some(ColorLabel::Green), Flag::Pick, 1));
        media::update_user_fields(c, id, &UserFieldsUpdate { color_label: Some(None), ..Default::default() })?;
        assert_eq!(media::get(c, id)?.unwrap().color_label, None);

        media::set_verify_state(c, id, VerifyState::Missing)?;
        let m = media::get(c, id)?.unwrap();
        assert!(m.missing_since.is_some());
        media::set_verify_state(c, id, VerifyState::Ok)?;
        assert_eq!(media::get(c, id)?.unwrap().missing_since, None);

        let near = PHash(row.phash.unwrap().0 ^ 0b101);
        assert_eq!(media::near_duplicates(c, near, 3)?, vec![(id, 2)]);

        media::soft_delete(c, id)?;
        assert_eq!(media::count(c, lib)?, 0);
        media::restore(c, id)?;
        assert_eq!(media::list_page(c, lib, None, 10)?.len(), 1);
        assert!(media::delete(c, id)?);
        Ok(())
    })
    .unwrap();
}

#[test]
fn import_session_and_items() {
    let (_d, pool) = open();
    let lib = pool.write(|c| library::insert(c, &new_library("Ana", true))).unwrap();
    let session = pool
        .write(|c| {
            import::create_session(
                c,
                &NewImportSession {
                    library_id: lib,
                    mode: SessionMode::Move,
                    source_root: "/run/media/u/EOS_DIGITAL/DCIM".into(),
                    source_volume: Some("1234-ABCD".into()),
                },
            )
        })
        .unwrap();

    let items: Vec<NewImportItem> = (0..1_200)
        .map(|i| NewImportItem {
            source_path: format!("100CANON/IMG_{i:04}.CR3"),
            size_bytes: 25_000_000,
            source_mtime_ns: i,
        })
        .collect();
    let ids = pool.write(|c| loupe_db::insert_import_items_batch(c, session, &items)).unwrap();
    assert_eq!(ids.len(), 1_200);
    // Yeniden tarama aynı öğeleri ikinci kez eklemez.
    assert!(pool.write(|c| loupe_db::insert_import_items_batch(c, session, &items)).unwrap().is_empty());

    pool.write(|c| {
        import::set_session_totals(c, session, 1_200, 1_200 * 25_000_000)?;
        import::set_session_state(c, session, SessionState::Running)?;
        let h = hash_of(1);
        import::set_item_state(c, ids[0], ImportItemState::Hashed, Some(&h), None, None)?;
        import::set_item_state(c, ids[1], ImportItemState::Failed, None, None, Some("EIO"))?;
        import::set_item_state(c, ids[1], ImportItemState::Failed, None, None, Some("EIO"))?;
        import::set_item_state(c, ids[2], ImportItemState::SkippedDuplicate, None, None, None)?;
        let first = import::get_item(c, ids[0])?.unwrap();
        assert_eq!((first.state, first.blake3), (ImportItemState::Hashed, Some(h)));
        assert_eq!(import::get_item(c, ids[1])?.unwrap().attempts, 2);
        assert_eq!(import::resumable_items(c, session)?.len(), 1_199);
        assert_eq!(import::count_items(c, session, ImportItemState::Failed)?, 1);
        assert_eq!(import::active_sessions(c)?.len(), 1);
        Ok(())
    })
    .unwrap();

    // Katalog üzerinden yazılan medya oturum sayaçlarını aynı transaction'da artırır.
    let mut cat = DbImportCatalog::new(&pool, Some(session));
    let row = media_row(lib, 9);
    assert!(matches!(cat.commit_media(&row).unwrap(), CommitOutcome::Inserted(_)));
    assert!(matches!(cat.commit_media(&row).unwrap(), CommitOutcome::Duplicate(_)));
    assert!(cat.find_by_blake3(&row.blake3).unwrap().is_some());

    pool.write(|c| {
        import::set_session_state(c, session, SessionState::Done)?;
        let s = import::get_session(c, session)?.unwrap();
        assert_eq!((s.done_files, s.done_bytes, s.skipped_dup), (1, row.size_bytes, 1));
        assert!(s.finished_at.is_some());
        assert_eq!(s.state, SessionState::Done);
        let m = media::find_by_blake3(c, &row.blake3)?.and_then(|id| media::get(c, id).ok().flatten()).unwrap();
        assert_eq!(m.import_session_id, Some(session));
        Ok(())
    })
    .unwrap();
}

/// 1.000 kayıt: tam olarak 2 transaction (500 + 500) açılmalı ve hızlı bitmeli.
#[test]
fn batch_insert_1000_rows_in_500_row_transactions() {
    let (_d, pool) = open();
    let lib = pool.write(|c| library::insert(c, &new_library("Ana", true))).unwrap();
    let rows: Vec<NewMediaFile> = (0..1_000).map(|i| media_row(lib, i)).collect();

    let commits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&commits);
    pool.write(|c| {
        c.commit_hook(Some(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            false
        }))?;
        Ok(())
    })
    .unwrap();

    let t = Instant::now();
    let report = pool.write(|c| insert_media_batch(c, &rows)).unwrap();
    let elapsed = t.elapsed();
    eprintln!("1000 satır, {} transaction: {elapsed:?}", report.transactions);

    assert_eq!(BATCH_SIZE, 500);
    assert_eq!(report.transactions, 2);
    assert_eq!(commits.load(Ordering::SeqCst), 2);
    assert_eq!((report.inserted(), report.duplicates()), (1_000, 0));
    assert!(elapsed.as_secs_f64() < 2.0, "toplu yazma yavaş: {elapsed:?}");
    assert_eq!(pool.read(|c| media::count(c, lib)).unwrap(), 1_000);

    // Aynı girdi ikinci kez: hiçbir satır yazılmaz, hepsi kopya olarak raporlanır.
    let again = pool.write(|c| insert_media_batch(c, &rows)).unwrap();
    assert_eq!((again.inserted(), again.duplicates(), again.transactions), (0, 1_000, 2));
    assert_eq!(
        again.outcomes[0],
        CommitOutcome::Duplicate(match report.outcomes[0] {
            CommitOutcome::Inserted(id) | CommitOutcome::Duplicate(id) => id,
        })
    );

    // Karşılaştırma: aynı yük satır başına transaction ile (yalnızca bilgi).
    let (_d2, slow) = open();
    let lib2 = slow.write(|c| library::insert(c, &new_library("Ana", true))).unwrap();
    let t = Instant::now();
    slow.write(|c| {
        for i in 0..1_000 {
            media::insert(c, &media_row(lib2, i))?;
        }
        Ok(())
    })
    .unwrap();
    eprintln!("1000 satır, satır başına autocommit: {:?}", t.elapsed());
}

#[test]
fn batch_chunk_failure_rolls_back_only_that_chunk() {
    let (_d, pool) = open();
    let lib = pool.write(|c| library::insert(c, &new_library("Ana", true))).unwrap();
    let mut rows: Vec<NewMediaFile> = (0..700).map(|i| media_row(lib, i)).collect();
    // 2. parçadaki bir satır 1. parçadaki bir yolla çakışsın (farklı içerik).
    rows[650].rel_path = rows[10].rel_path.clone();
    assert!(pool.write(|c| insert_media_batch(c, &rows)).is_err());
    assert_eq!(pool.read(|c| media::count(c, lib)).unwrap(), 500, "ilk parça kalıcı, ikinci parça geri alınmış");
}

#[test]
fn migration_0004_maps_pending_thumb_jobs_to_new_size_classes() {
    use loupe_db::{ConnectionConfig, MIGRATIONS, Role, open_connection};
    let dir = tempfile::tempdir().unwrap();
    let c = open_connection(&dir.path().join("v3.db"), Role::Writer, &ConnectionConfig::default()).unwrap();
    for m in &MIGRATIONS[..3] {
        c.execute_batch(m.sql).unwrap();
    }
    let lib = library::insert(&c, &new_library("Ana", true)).unwrap();
    let id = match media::insert(&c, &media_row(lib, 1)).unwrap() {
        CommitOutcome::Inserted(id) | CommitOutcome::Duplicate(id) => id,
    };
    for (variant, prio) in [("grid", 0), ("preview", 2), ("full", 5)] {
        c.execute(
            "INSERT INTO jobs (kind, media_id, variant, base_priority, run_after, created_at, updated_at)
             VALUES ('thumb', ?1, ?2, ?3, 0, 0, 0)",
            rusqlite::params![id.0, variant, prio],
        )
        .unwrap();
    }
    c.execute_batch(MIGRATIONS[3].sql).unwrap();

    let got: Vec<(String, i64)> = c
        .prepare("SELECT variant, base_priority FROM jobs ORDER BY base_priority")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(got, vec![("sm".into(), 0), ("md".into(), 2), ("lg".into(), 5)]);
    let user_version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(user_version, 4);
    // Eski sınıf adları artık kabul edilmez.
    let bad = c.execute(
        "INSERT INTO jobs (kind, media_id, variant, base_priority, run_after, created_at, updated_at)
         VALUES ('thumb', ?1, 'grid', 0, 0, 0, 0)",
        [id.0],
    );
    assert!(bad.is_err());
    for m in &MIGRATIONS[4..] {
        c.execute_batch(m.sql).unwrap();
    }
    loupe_db::validate_schema(&c).unwrap();
}
