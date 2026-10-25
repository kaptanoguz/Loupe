//! loupe-ai entegrasyon testleri: gerçek SQLite + sqlite-vec, deterministik mock motor.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::{Rgb, RgbImage};
use loupe_ai::events::{self, EventDetector, EventId};
use loupe_ai::faces::{Assignment, FaceClusterer};
use loupe_ai::jobs::{AI_JOB_KINDS, AiEngine, AiJobHandler, write_tx};
use loupe_ai::mock::{MockClip, MockFaceDetector, MockFaceEmbedder};
use loupe_ai::{
    ClipImageEncoder, ClusterId, DeterministicMockEngine, FaceDetector, FaceEmbedder, IdleReaper, ManagedModel,
    Unloadable, clip,
};
use loupe_core::{
    Blake3Hash, ImportMode, LibraryId, LibraryKind, MediaId, NewMediaFile, Placement, RelPath, TakenAtSource, classify,
};
use loupe_db::models::NewLibrary;
use loupe_db::repo::library;
use loupe_db::repo::location::{self, NewLocation};
use loupe_db::{Pool, PoolConfig, insert_media_batch};
use loupe_jobs::{
    JobKind, JobQueue, JobRouter, MediaPipeline, NewJob, Priority, ResourceMode, WorkerConfig, WorkerPool,
};
use loupe_media::thumb::ImageBackend;
use loupe_media::{ThumbCache, Thumbnailer};
use rusqlite::params;
use tempfile::TempDir;

// ───────────────────────────── yardımcılar ─────────────────────────────

struct Env {
    dir: TempDir,
    pool: Arc<Pool>,
    lib: LibraryId,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(Pool::open(&dir.path().join("catalog.db"), PoolConfig::default()).unwrap());
    let lib = pool
        .write(|c| {
            library::insert(
                c,
                &NewLibrary {
                    uuid: *b"loupe-ai-test!!!",
                    name: "Ana".into(),
                    kind: LibraryKind::Internal,
                    volume_uuid: Some("0b5e-test".into()),
                    volume_label: None,
                    network_source: None,
                    fs_type: Some("ext4".into()),
                    rel_root: "Ana".into(),
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
    Env { dir, pool, lib }
}

impl Env {
    fn media_dir(&self) -> PathBuf {
        self.dir.path().join("media")
    }

    /// `(ad, çekim zamanı UTC ms)`; dosya diskteyse özeti içerikten alınır.
    fn add_media(&self, items: &[(String, i64)]) -> Vec<MediaId> {
        let rows: Vec<NewMediaFile> = items
            .iter()
            .enumerate()
            .map(|(i, (name, taken))| {
                let path = self.media_dir().join(name);
                let hash = std::fs::read(&path)
                    .map_or_else(|_| *blake3::hash(name.as_bytes()).as_bytes(), |b| *blake3::hash(&b).as_bytes());
                let kind = classify(&path);
                NewMediaFile {
                    library_id: self.lib,
                    rel_path: RelPath::new(name).unwrap(),
                    original_filename: name.rsplit('/').next().unwrap().to_owned(),
                    placement: Placement::Copy,
                    import_session_id: None,
                    file_type: kind.file_type,
                    raw_format: None,
                    mime: kind.mime.to_owned(),
                    size_bytes: 1,
                    mtime_ns: 0,
                    inode: Some(i as u64 + 1),
                    device_id: Some(1),
                    blake3: Blake3Hash(hash),
                    phash: None,
                    width: None,
                    height: None,
                    taken_at_utc: *taken,
                    tz_offset_min: Some(180),
                    taken_at_source: TakenAtSource::Exif,
                    imported_at: 0,
                }
            })
            .collect();
        let report = self.pool.write(|c| insert_media_batch(c, &rows)).unwrap();
        report
            .outcomes
            .iter()
            .map(|o| match o {
                loupe_core::CommitOutcome::Inserted(id) | loupe_core::CommitOutcome::Duplicate(id) => *id,
            })
            .collect()
    }

    fn place(&self, locality: &str, admin1: &str, lat: f64, lon: f64) -> i64 {
        self.pool
            .write(|c| {
                location::upsert(
                    c,
                    &NewLocation {
                        geoname_id: None,
                        country_code: "TR".into(),
                        country: "Türkiye".into(),
                        admin1: Some(admin1.into()),
                        admin2: None,
                        locality: Some(locality.into()),
                        lat,
                        lon,
                        dataset_ver: "test".into(),
                    },
                )
            })
            .unwrap()
            .0
    }

    fn set_gps(&self, m: MediaId, lat: f64, lon: f64, loc: i64) {
        self.pool
            .write(|c| {
                c.execute(
                    "UPDATE media_files SET gps_lat = ?2, gps_lon = ?3, location_id = ?4 WHERE id = ?1",
                    params![m.0, lat, lon, loc],
                )?;
                Ok(())
            })
            .unwrap();
    }
}

/// Gri sahne, magenta çerçeveli "yüzler": her biri `(iç renk, konum)`.
fn scene(w: u32, h: u32, bg: [u8; 3], faces: &[([u8; 3], (u32, u32))], side: u32) -> RgbImage {
    let mut img = RgbImage::from_pixel(w, h, Rgb(bg));
    let b = (side / 7).max(4);
    for (inner, at) in faces {
        for y in at.1..at.1 + side {
            for x in at.0..at.0 + side {
                let border = x < at.0 + b || x >= at.0 + side - b || y < at.1 + b || y >= at.1 + side - b;
                img.put_pixel(x, y, Rgb(if border { [230, 30, 220] } else { *inner }));
            }
        }
    }
    img
}

const AYSE: [u8; 3] = [224, 160, 96];
const MEHMET: [u8; 3] = [32, 96, 224];

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

const T0: i64 = 1_791_590_400_000; // 2026-10-10T00:00:00Z
const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

// ───────────────────────────── yüz kümeleme ─────────────────────────────

#[test]
fn five_faces_of_one_person_form_one_cluster_other_person_separate_then_merge_and_split() {
    let e = env();
    let engine = AiEngine::from(DeterministicMockEngine::default());
    let ids = e.pool.write(|c| Ok(engine.register(c))).unwrap().unwrap();
    let media = e.add_media(&(0..7).map(|i| (format!("2026/IMG_{i}.JPG"), T0 + i * HOUR)).collect::<Vec<_>>());
    let fc = FaceClusterer::default();

    // Ayşe: 5 farklı fotoğraf (konum, arka plan ve ton hafifçe farklı); Mehmet: 2 fotoğraf.
    let mut faces = Vec::new();
    for (i, &m) in media.iter().enumerate() {
        let (inner, who) =
            if i < 5 { ([AYSE[0], AYSE[1] - 2 * i as u8, AYSE[2] + i as u8], "ayşe") } else { (MEHMET, "mehmet") };
        let img = scene(320, 240, [100 + 10 * i as u8, 110, 120], &[(inner, (20 + 30 * i as u32, 40))], 64);
        let detected = engine.detector.detect(&img).unwrap();
        assert_eq!(detected.len(), 1);
        let emb = engine.embedder.embed(&img, &detected[0]).unwrap();
        let inserted = write_tx(&e.pool, |c| {
            let f = fc.insert_faces(c, m, &[(detected[0].clone(), emb)], ids.detector, ids.embedder)?;
            fc.assign(c, f[0]).map(|a| (f[0], a))
        })
        .unwrap();
        faces.push((who, inserted.0, inserted.1));
    }

    let ayse_cluster = faces[0].2.cluster().unwrap();
    assert!(matches!(faces[0].2, Assignment::Created(_)));
    for (who, _, a) in &faces[1..5] {
        assert_eq!(*a, Assignment::Joined(ayse_cluster), "{who} aynı kümeye katılmalı");
    }
    let mehmet_cluster = faces[5].2.cluster().unwrap();
    assert!(matches!(faces[5].2, Assignment::Created(_)));
    assert_ne!(ayse_cluster, mehmet_cluster);
    assert_eq!(faces[6].2, Assignment::Joined(mehmet_cluster));

    let count = |c: ClusterId| -> i64 {
        e.pool
            .read(|conn| Ok(conn.query_row("SELECT face_count FROM face_clusters WHERE id = ?1", [c.0], |r| r.get(0))?))
            .unwrap()
    };
    assert_eq!((count(ayse_cluster), count(mehmet_cluster)), (5, 2));

    // Adlandırma: kişi yüz sayısı toplu güncellenir.
    let ayse = write_tx(&e.pool, |c| fc.name_cluster(c, ayse_cluster, "Ayşe")).unwrap();
    let person_faces = |p: i64| -> i64 {
        e.pool.read(|c| Ok(c.query_row("SELECT face_count FROM persons WHERE id = ?1", [p], |r| r.get(0))?)).unwrap()
    };
    assert_eq!(person_faces(ayse.0), 5);

    // Ayır: yanlış atanmış yüz kendi kümesine; merkezden düşülür.
    let wrong = faces[4].1;
    let solo = write_tx(&e.pool, |c| fc.detach_face(c, wrong)).unwrap();
    assert_eq!((count(ayse_cluster), count(solo)), (4, 1));
    assert_eq!(person_faces(ayse.0), 4);
    // Otomatik kümeleme ayrılmış yüzü geri çekmez.
    write_tx(&e.pool, |c| fc.cluster_pending(c)).unwrap();
    assert_eq!(e.pool.read(|c| Ok(fc.cluster_of(c, wrong))).unwrap().unwrap(), Some(solo));
    // Taşı: kullanıcı geri koyar, boşalan küme silinir.
    write_tx(&e.pool, |c| fc.move_face(c, wrong, ayse_cluster)).unwrap();
    assert_eq!(count(ayse_cluster), 5);
    let solo_exists: i64 = e
        .pool
        .read(|c| Ok(c.query_row("SELECT count(*) FROM face_clusters WHERE id = ?1", [solo.0], |r| r.get(0))?))
        .unwrap();
    assert_eq!(solo_exists, 0);

    // Birleştir: Mehmet kümesi Ayşe'ye katılır (kullanıcı "aynı kişi" dedi).
    write_tx(&e.pool, |c| fc.merge(c, ayse_cluster, mehmet_cluster)).unwrap();
    assert_eq!(count(ayse_cluster), 7);
    assert_eq!(person_faces(ayse.0), 7);
    let left: i64 = e
        .pool
        .read(|c| {
            Ok(c.query_row(
                "SELECT (SELECT count(*) FROM face_clusters) + (SELECT count(*) FROM vec_face_centroids)",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(left, 2, "tek küme ve tek merkez kalmalı");
    // Merkez toplamı kesindir: tüm üyelerin toplamının yönü.
    let centroid = e.pool.read(|c| Ok(fc.centroid(c, ayse_cluster))).unwrap().unwrap().unwrap();
    let members = e.pool.read(|c| Ok(fc.cluster_faces(c, ayse_cluster))).unwrap().unwrap();
    let mut sum = vec![0f32; 512];
    for f in members {
        let blob: Vec<u8> = e
            .pool
            .read(|c| Ok(c.query_row("SELECT embedding FROM vec_faces WHERE face_id = ?1", [f.0], |r| r.get(0))?))
            .unwrap();
        for (s, x) in sum.iter_mut().zip(loupe_ai::embedding::from_blob(&blob).unwrap()) {
            *s += x;
        }
    }
    let expected = loupe_ai::Embedding::normalized(sum).unwrap();
    assert!(centroid.cosine(&expected) > 0.9999);
    // v_face_person: tüm yüzler Ayşe.
    let ayse_photos: i64 = e
        .pool
        .read(|c| {
            Ok(c.query_row("SELECT count(DISTINCT media_id) FROM v_face_person WHERE person_id = ?1", [ayse.0], |r| {
                r.get(0)
            })?)
        })
        .unwrap();
    assert_eq!(ayse_photos, 7);
}

// ───────────────────────────── etkinlikler ─────────────────────────────

fn event_rows(e: &Env) -> Vec<(i64, String, i64)> {
    e.pool
        .read(|c| {
            Ok(c.prepare("SELECT id, title, media_count FROM events ORDER BY start_utc")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?)
        })
        .unwrap()
}

#[test]
fn fifty_photos_split_into_cappadocia_and_istanbul_and_user_edits_survive_recluster() {
    let e = env();
    let goreme = e.place("Göreme", "Nevşehir", 38.6431, 34.8289);
    let istanbul = e.place("İstanbul", "İstanbul", 41.0082, 28.9784);

    // Kapadokya: 10–11 Ekim, iki gün, 30 fotoğraf (gece 10 saatlik boşluk aynı etkinlik).
    let mut items = Vec::new();
    for i in 0..30i64 {
        let day = i / 15;
        items.push((format!("2026/10/kapadokya_{i:02}.jpg"), T0 + day * DAY + 5 * HOUR + (i % 15) * 40 * 60_000));
    }
    // Bir hafta sonra İstanbul: 18 Ekim, 20 fotoğraf.
    for i in 0..20i64 {
        items.push((format!("2026/10/istanbul_{i:02}.jpg"), T0 + 8 * DAY + 6 * HOUR + i * 20 * 60_000));
    }
    let ids = e.add_media(&items);
    for (i, &m) in ids.iter().enumerate() {
        if i < 30 {
            e.set_gps(m, 38.6431 + i as f64 * 1e-4, 34.8289, goreme);
        } else if i % 4 != 0 {
            // Bazı fotoğrafların konumu yok: yine zamana göre doğru etkinlikte.
            e.set_gps(m, 41.0082, 28.9784 + i as f64 * 1e-4, istanbul);
        }
    }

    let det = EventDetector::default();
    let r1 = write_tx(&e.pool, |c| det.recluster(c)).unwrap();
    assert_eq!((r1.created, r1.deleted, r1.media_in_events), (2, 0, 50));
    let ev = event_rows(&e);
    assert_eq!(ev.len(), 2);
    assert_eq!((ev[0].1.as_str(), ev[0].2), ("Ekim 2026 · Göreme", 30));
    assert_eq!((ev[1].1.as_str(), ev[1].2), ("Ekim 2026 · İstanbul", 20));
    let (kapadokya, ist) = (EventId(ev[0].0), EventId(ev[1].0));
    let (radius, lat): (f64, f64) = e
        .pool
        .read(|c| {
            Ok(c.query_row("SELECT radius_km, centroid_lat FROM events WHERE id = ?1", [kapadokya.0], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?)
        })
        .unwrap();
    assert!(radius < 1.0 && (lat - 38.6445).abs() < 0.01, "{radius} {lat}");

    // Değişiklik yoksa yeniden kümeleme hiçbir şeye dokunmaz, kimlikler sabit.
    let r2 = write_tx(&e.pool, |c| det.recluster(c)).unwrap();
    assert_eq!((r2.created, r2.updated, r2.unchanged, r2.deleted), (0, 0, 2, 0));

    // Kullanıcı: İstanbul etkinliğini yeniden adlandırır, Kapadokya'dan bir fotoğrafı çıkarır.
    write_tx(&e.pool, |c| events::rename(c, ist, "İstanbul hafta sonu")).unwrap();
    write_tx(&e.pool, |c| events::exclude_media(c, kapadokya, ids[29])).unwrap();

    // Aynı gün İstanbul'dan 3 fotoğraf daha içe aktarılır; yeniden kümeleme.
    let late = e.add_media(
        &(0..3)
            .map(|i| (format!("2026/10/istanbul_gec_{i}.jpg"), T0 + 8 * DAY + 14 * HOUR + i * 60_000))
            .collect::<Vec<_>>(),
    );
    let r3 = write_tx(&e.pool, |c| det.recluster(c)).unwrap();
    assert_eq!(r3.protected, 2);

    let ev = event_rows(&e);
    let get = |id: EventId| ev.iter().find(|r| r.0 == id.0).cloned().unwrap();
    assert_eq!(get(ist).1, "İstanbul hafta sonu", "kullanıcı başlığı ezilmez");
    assert_eq!(get(ist).2, 20, "korumalı etkinliğin fotoğrafları değişmez");
    assert_eq!(get(kapadokya).2, 29, "çıkarılan fotoğraf geri konmaz");
    let ist_media = e.pool.read(|c| Ok(events::event_media(c, ist))).unwrap().unwrap();
    assert!(late.iter().all(|m| !ist_media.contains(m)));
    let kap_media = e.pool.read(|c| Ok(events::event_media(c, kapadokya))).unwrap().unwrap();
    assert!(!kap_media.contains(&ids[29]));
    // Geç gelen fotoğraflar korumalı etkinliğe karışmadan kendi otomatik etkinliğinde.
    assert_eq!(ev.len(), 3);
    assert!(ev.iter().any(|r| r.2 == 3 && r.1 == "Ekim 2026"));

    // Kilitli olmayan, adı otomatik, elle düzenlenmemiş etkinlik ise yeniden üretilebilir:
    // kullanıcı kilidi açsa bile başlık 'user' kaldıkça korunur.
    write_tx(&e.pool, |c| events::set_locked(c, ist, false)).unwrap();
    write_tx(&e.pool, |c| det.recluster(c)).unwrap();
    assert_eq!(event_rows(&e).iter().find(|r| r.0 == ist.0).unwrap().1, "İstanbul hafta sonu");
}

#[test]
fn travel_on_the_same_day_splits_events() {
    let e = env();
    let a = e.place("Göreme", "Nevşehir", 38.6431, 34.8289);
    let b = e.place("Ankara", "Ankara", 39.9334, 32.8597);
    let items: Vec<_> = (0..8i64)
        .map(|i| (format!("yol_{i}.jpg"), T0 + if i < 4 { i * 600_000 } else { 5 * HOUR + i * 600_000 }))
        .collect();
    let ids = e.add_media(&items);
    for (i, &m) in ids.iter().enumerate() {
        if i < 4 {
            e.set_gps(m, 38.6431, 34.8289, a);
        } else {
            e.set_gps(m, 39.9334, 32.8597, b);
        }
    }
    write_tx(&e.pool, |c| EventDetector::default().recluster(c)).unwrap();
    let titles: Vec<String> = event_rows(&e).into_iter().map(|r| r.1).collect();
    assert_eq!(titles, ["Ekim 2026 · Göreme", "Ekim 2026 · Ankara"]);
}

// ───────────────────────────── boşta boşaltma ─────────────────────────────

#[test]
fn model_is_unloaded_after_idle_timeout_and_lazily_reloaded() {
    let e = env();
    std::fs::create_dir_all(e.media_dir()).unwrap();
    let cache = ThumbCache::new(e.dir.path().join("thumbs"));

    let idle = Duration::from_millis(150);
    let detector = Arc::new(ManagedModel::new(FaceDetector::info(&MockFaceDetector::default()).clone(), idle, || {
        Ok(MockFaceDetector::default())
    }));
    let embedder = Arc::new(ManagedModel::new(FaceEmbedder::info(&MockFaceEmbedder::default()).clone(), idle, || {
        Ok(MockFaceEmbedder::default())
    }));
    let reaper = IdleReaper::start(Duration::from_millis(20)).unwrap();
    reaper.watch(&(Arc::clone(&detector) as Arc<dyn Unloadable>));
    reaper.watch(&(Arc::clone(&embedder) as Arc<dyn Unloadable>));
    let clip = Arc::new(MockClip::default());
    let engine = AiEngine {
        detector: Arc::clone(&detector) as Arc<dyn FaceDetector>,
        embedder: Arc::clone(&embedder) as Arc<dyn FaceEmbedder>,
        clip_image: Arc::clone(&clip) as Arc<dyn ClipImageEncoder>,
        clip_text: clip,
    };
    let handler = AiJobHandler::new(Arc::clone(&e.pool), cache.clone(), engine);

    let ids = e.add_media(&[("a.jpg".into(), T0), ("b.jpg".into(), T0 + HOUR)]);
    for (i, &m) in ids.iter().enumerate() {
        let hash = e.pool.read(|c| loupe_db::repo::media::get(c, m)).unwrap().unwrap().blake3;
        let img = scene(320, 240, [90, 90, 90], &[(AYSE, (40 + 50 * i as u32, 60))], 72);
        put_md(&cache, &hash, &img);
    }

    assert!(!detector.is_loaded(), "ilk kullanıma kadar yüklenmez");
    assert_eq!(handler.detect_faces(ids[0]).unwrap(), 1);
    assert!(detector.is_loaded() && embedder.is_loaded());
    assert!(
        wait_until(Duration::from_secs(3), || !detector.is_loaded() && !embedder.is_loaded()),
        "boşta boşaltılmalı"
    );
    assert_eq!(detector.unload_count(), 1);

    // Yeni iş modeli yeniden yükler.
    assert_eq!(handler.detect_faces(ids[1]).unwrap(), 1);
    assert_eq!(detector.load_count(), 2);
    drop(reaper);
}

fn put_md(cache: &ThumbCache, hash: &Blake3Hash, img: &RgbImage) {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::WebP)
        .unwrap();
    cache
        .put(
            hash,
            &loupe_media::thumb::EncodedThumb {
                size: loupe_media::ThumbSize::Md,
                format: loupe_media::thumb::ThumbFormat::Webp,
                width: img.width(),
                height: img.height(),
                source_kind: loupe_media::thumb::SourceKind::Decode,
                bytes,
            },
        )
        .unwrap();
}

// ───────────────────────────── kuyruk entegrasyonu ─────────────────────────────

#[test]
fn md_thumbnail_enqueues_face_clip_and_event_jobs_which_workers_complete() {
    let e = env();
    std::fs::create_dir_all(e.media_dir()).unwrap();
    // 4 fotoğraf: üçünde Ayşe, birinde Mehmet; arka planlar gün batımı / deniz.
    let specs: [([u8; 3], [u8; 3]); 4] =
        [([240, 140, 40], AYSE), ([240, 140, 40], AYSE), ([30, 90, 200], AYSE), ([30, 90, 200], MEHMET)];
    let mut items = Vec::new();
    for (i, (bg, who)) in specs.iter().enumerate() {
        let img = scene(1200, 900, *bg, &[(*who, (200 + 60 * i as u32, 250))], 300);
        let name = format!("tatil_{i}.png");
        img.save(e.media_dir().join(&name)).unwrap();
        items.push((name, T0 + i as i64 * HOUR));
    }
    let ids = e.add_media(&items);

    let cache = ThumbCache::new(e.dir.path().join("thumbs"));
    let root = e.media_dir();
    let pipeline = MediaPipeline::new(
        Arc::clone(&e.pool),
        Thumbnailer::new(cache.clone(), Box::new(ImageBackend)),
        None,
        move |m: &loupe_db::models::MediaFile| Some(root.join(m.rel_path.as_str())),
    )
    .with_event_debounce(Duration::from_millis(100));
    let ai = AiJobHandler::new(Arc::clone(&e.pool), cache, AiEngine::from(DeterministicMockEngine::default()));
    let router = JobRouter::new()
        .route(&[JobKind::Thumb, JobKind::MetadataFull, JobKind::Geocode], Arc::new(pipeline))
        .route(&AI_JOB_KINDS, Arc::new(ai));

    let q = Arc::new(JobQueue::new(Arc::clone(&e.pool)));
    let thumbs: Vec<_> = ids.iter().map(|&m| NewJob::media(JobKind::Thumb, m, "md", Priority::Normal)).collect();
    q.enqueue_batch(&thumbs).unwrap();
    let cfg = WorkerConfig { workers: 3, reserved_immediate: 0, mode: ResourceMode::Full, ..Default::default() };
    let workers = WorkerPool::start(Arc::clone(&q), Arc::new(router), cfg).unwrap();

    let count = |sql: &str| -> i64 { e.pool.read(|c| Ok(c.query_row(sql, [], |r| r.get(0))?)).unwrap() };
    assert!(
        wait_until(Duration::from_secs(30), || {
            let s = q.stats().unwrap();
            s.pending + s.running + s.failed + s.dead == 0 && count("SELECT count(*) FROM events") == 1
        }),
        "{:?} dead={:?}",
        q.stats().unwrap(),
        q.dead_letters(10).unwrap()
    );
    workers.shutdown();

    assert_eq!(count("SELECT count(*) FROM faces"), 4);
    assert_eq!(count("SELECT count(*) FROM vec_faces"), 4);
    assert_eq!(count("SELECT count(*) FROM face_clusters"), 2, "Ayşe ve Mehmet");
    assert_eq!(count("SELECT max(face_count) FROM face_clusters"), 3);
    assert_eq!(count("SELECT count(*) FROM vec_media_clip"), 4);
    assert_eq!(count("SELECT count(*) FROM media_ml_state WHERE faces_at IS NOT NULL AND clip_at IS NOT NULL"), 4);
    assert_eq!(count("SELECT media_count FROM events"), 4);

    // Anlamsal: "gün batımı" turuncu arka planlı iki fotoğrafı öne alır.
    let text = MockClip::default();
    let qv = loupe_ai::ClipTextEncoder::encode_text(&text, "gün batımı").unwrap();
    let near = e.pool.read(|c| Ok(clip::knn(c, &qv, 2))).unwrap().unwrap();
    let mut top: Vec<_> = near.iter().map(|(m, _)| *m).collect();
    top.sort();
    assert_eq!(top, ids[..2]);

    // Aynı medyayı yeniden işlemek yüzleri çoğaltmaz (aynı model).
    let again = AiJobHandler::new(
        Arc::clone(&e.pool),
        ThumbCache::new(e.dir.path().join("thumbs")),
        AiEngine::from(DeterministicMockEngine::default()),
    );
    assert_eq!(again.detect_faces(ids[0]).unwrap(), 0);
    assert_eq!(count("SELECT count(*) FROM faces"), 4);
}
