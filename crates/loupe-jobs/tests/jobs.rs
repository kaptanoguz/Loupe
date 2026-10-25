//! loupe-jobs entegrasyon testleri: gerçek SQLite dosyası, gerçek işçi havuzu.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use loupe_core::{
    Blake3Hash, ImportMode, LibraryId, LibraryKind, MediaId, NewMediaFile, Placement, RelPath, TakenAtSource, classify,
};
use loupe_db::models::NewLibrary;
use loupe_db::repo::{library, location, thumbnail};
use loupe_db::{Pool, PoolConfig, insert_media_batch};
use loupe_jobs::{
    Job, JobContext, JobError, JobHandler, JobKind, JobQueue, JobState, MediaPipeline, NewJob, Priority, ResourceMode,
    RetryPolicy, WorkerConfig, WorkerPool,
};
use loupe_media::thumb::ImageBackend;
use loupe_media::{GeoIndex, ThumbCache, Thumbnailer};
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
                    uuid: *b"loupe-jobs-test!",
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

    /// Diskte gerçekten bulunan (ya da bulunmayan) dosyalar için medya satırları.
    fn add_media(&self, names: &[String]) -> Vec<MediaId> {
        let rows: Vec<NewMediaFile> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let path = self.media_dir().join(name);
                let hash = std::fs::read(&path)
                    .map_or_else(|_| *blake3::hash(name.as_bytes()).as_bytes(), |b| *blake3::hash(&b).as_bytes());
                let kind = classify(&path);
                NewMediaFile {
                    library_id: self.lib,
                    rel_path: RelPath::new(name).unwrap(),
                    original_filename: name.clone(),
                    placement: Placement::Copy,
                    import_session_id: None,
                    file_type: kind.file_type,
                    raw_format: kind.raw_format.map(str::to_owned),
                    mime: kind.mime.to_owned(),
                    size_bytes: 1,
                    mtime_ns: 0,
                    inode: Some(i as u64 + 1),
                    device_id: Some(1),
                    blake3: Blake3Hash(hash),
                    phash: None,
                    width: None,
                    height: None,
                    taken_at_utc: 0,
                    tz_offset_min: None,
                    taken_at_source: TakenAtSource::Mtime,
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

    fn numbered_media(&self, n: usize) -> Vec<MediaId> {
        self.add_media(&(0..n).map(|i| format!("2026/IMG_{i:05}.JPG")).collect::<Vec<_>>())
    }
}

fn thumb_job(m: MediaId, variant: &str, p: Priority) -> NewJob {
    NewJob::media(JobKind::Thumb, m, variant, p)
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy { base: Duration::from_millis(20), max: Duration::from_millis(200) }
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    cond()
}

/// Testlerin işleyicisi: başlama sırası ve öncelik kaydı, isteğe bağlı panik/hata.
#[derive(Default)]
struct Recorder {
    started: Mutex<Vec<(MediaId, u8)>>,
    finished: Mutex<Vec<(MediaId, u8)>>,
    work: Duration,
    panic_on: Option<MediaId>,
    transient_on: Option<MediaId>,
}

impl JobHandler for Recorder {
    fn run(&self, job: &Job, _ctx: &JobContext<'_>) -> Result<(), JobError> {
        let m = job.media_id.unwrap();
        self.started.lock().unwrap().push((m, job.priority));
        if Some(m) == self.panic_on {
            panic!("işleyici hatası: medya {}", m.0);
        }
        if Some(m) == self.transient_on {
            return Err(JobError::transient("BUSY", "kaynak meşgul"));
        }
        std::thread::sleep(self.work);
        self.finished.lock().unwrap().push((m, job.priority));
        Ok(())
    }
}

// ───────────────────────────── kuyruk ─────────────────────────────

#[test]
fn enqueue_is_idempotent_and_keeps_the_most_urgent_priority() {
    let e = env();
    let m = e.numbered_media(1200);
    let q = JobQueue::new(Arc::clone(&e.pool));

    let a = q.enqueue(&thumb_job(m[0], "sm", Priority::Background)).unwrap();
    let b = q.enqueue(&thumb_job(m[0], "sm", Priority::Immediate)).unwrap();
    let c = q.enqueue(&thumb_job(m[0], "sm", Priority::Idle)).unwrap();
    assert!(a == b && b == c);
    assert_eq!(q.get(a).unwrap().unwrap().priority, Priority::Immediate.level());

    let jobs: Vec<_> = m.iter().map(|&id| thumb_job(id, "md", Priority::Background)).collect();
    assert_eq!(q.enqueue_batch(&jobs).unwrap(), 1200);
    assert_eq!(q.enqueue_batch(&jobs).unwrap(), 1200);
    assert_eq!(q.stats().unwrap().pending, 1201);

    // Medyasız (global) işler de tekilleşir.
    let g = NewJob::global(JobKind::ClusterFaces, Priority::Idle);
    assert_eq!(q.enqueue(&g).unwrap(), q.enqueue(&g).unwrap());

    // Şema kısıtı: thumb varyantı yalnızca sm/md/lg.
    assert!(q.enqueue(&thumb_job(m[1], "grid", Priority::Normal)).is_err());
}

#[test]
fn retry_with_exponential_backoff_then_dead_letter() {
    let e = env();
    let m = e.numbered_media(2);
    let q = JobQueue::with_retry(Arc::clone(&e.pool), fast_retry());
    assert_eq!(fast_retry().delay(1), Duration::from_millis(20));
    assert_eq!(fast_retry().delay(3), Duration::from_millis(80));
    assert_eq!(fast_retry().delay(30), Duration::from_millis(200));

    let id = q.enqueue(&thumb_job(m[0], "sm", Priority::Normal).with_max_attempts(3)).unwrap();
    let busy = JobError::transient("BUSY", "disk meşgul");
    let lease = Duration::from_secs(60);

    let j = q.claim("w", 9, lease).unwrap().unwrap();
    assert_eq!((j.id, j.attempts), (id, 1));
    assert_eq!(q.fail(id, "w", &busy).unwrap(), JobState::Failed);
    // Geri çekilme süresi dolmadan tekrar alınmaz.
    assert!(q.claim("w", 9, lease).unwrap().is_none());
    let r = q.get(id).unwrap().unwrap();
    assert_eq!((r.state, r.last_error_code.as_deref()), (JobState::Failed, Some("BUSY")));

    assert!(wait_until(Duration::from_secs(2), || q.claim("w", 9, lease).unwrap().is_some()));
    assert_eq!(q.fail(id, "w", &busy).unwrap(), JobState::Failed);
    let t = Instant::now();
    assert!(wait_until(Duration::from_secs(2), || q.claim("w", 9, lease).unwrap().is_some()));
    assert!(t.elapsed() >= Duration::from_millis(30), "ikinci geri çekilme ~40 ms olmalı");
    assert_eq!(q.fail(id, "w", &busy).unwrap(), JobState::Dead, "3. deneme son hakkı");

    let dead = q.dead_letters(10).unwrap();
    assert_eq!((dead.len(), dead[0].attempts), (1, 3));
    assert!(q.retry_dead(id).unwrap());
    assert_eq!(q.get(id).unwrap().unwrap().state, JobState::Pending);

    // Kalıcı hata ilk denemede dead-letter'a gider.
    let id2 = q.enqueue(&thumb_job(m[1], "sm", Priority::Immediate)).unwrap();
    let j = q.claim("w", 9, lease).unwrap().unwrap();
    assert_eq!(j.id, id2);
    assert_eq!(q.fail(id2, "w", &JobError::permanent("DECODE", "bozuk")).unwrap(), JobState::Dead);
    // Dead iş yeniden eklenince diriltilmez (tarama döngüsü bozuk dosyayı tekrar denemesin).
    q.enqueue(&thumb_job(m[1], "sm", Priority::Immediate)).unwrap();
    assert_eq!(q.get(id2).unwrap().unwrap().state, JobState::Dead);
    // Kirası başka işçideyse sonuç yazılamaz.
    assert!(q.fail(id, "başkası", &busy).is_err());
}

#[test]
fn viewport_bump_preempts_50k_background_jobs() {
    let e = env();
    let m = e.numbered_media(50_000);
    let q = JobQueue::new(Arc::clone(&e.pool));
    let jobs: Vec<_> = m.iter().map(|&id| thumb_job(id, "sm", Priority::Background)).collect();
    let t = Instant::now();
    q.enqueue_batch(&jobs).unwrap();
    eprintln!("50 000 iş kuyruğa: {:?}", t.elapsed());

    let visible: Vec<MediaId> = m[30_000..30_010].to_vec();
    let t = Instant::now();
    assert_eq!(q.bump_viewport_priority(&visible).unwrap(), 10);
    let claimed: Vec<Job> = (0..10).map(|_| q.claim("w", 9, Duration::from_secs(60)).unwrap().unwrap()).collect();
    eprintln!("yükseltme + 10 alma: {:?}", t.elapsed());
    let got: HashSet<MediaId> = claimed.iter().map(|j| j.media_id.unwrap()).collect();
    assert_eq!(got, visible.iter().copied().collect());
    assert!(claimed.iter().all(|j| j.priority == Priority::Immediate.level()));
    // Sonraki iş yine arka plandan, en eski kimlikten.
    assert_eq!(q.claim("w", 9, Duration::from_secs(60)).unwrap().unwrap().media_id, Some(m[0]));

    // Viewport kaydı: önceki görünenler eski önceliğine döner.
    let a: Vec<MediaId> = m[100..110].to_vec();
    let b: Vec<MediaId> = m[200..205].to_vec();
    assert_eq!(q.bump_viewport_priority(&a).unwrap(), 10);
    assert_eq!(q.bump_viewport_priority(&b).unwrap(), 5);
    let prio = |id| q.find(JobKind::Thumb, Some(id), "sm").unwrap().unwrap().priority;
    assert_eq!(prio(a[0]), Priority::Background.level());
    assert_eq!(prio(b[0]), Priority::Immediate.level());
    // Kendi önceliği zaten yüksek olan iş düşürülmez.
    q.enqueue(&thumb_job(m[300], "sm", Priority::Interactive)).unwrap();
    q.bump_viewport_priority(&[m[300]]).unwrap();
    q.bump_viewport_priority(&[]).unwrap();
    assert_eq!(prio(m[300]), Priority::Interactive.level());
    assert_eq!(prio(b[0]), Priority::Background.level());
}

// ───────────────────────────── işçi havuzu ─────────────────────────────

/// Kullanıcının istediği senaryo: 500 Background iş işlenirken eklenen 10
/// Immediate iş, sıradaki arka plan işlerinden önce biter.
#[test]
fn immediate_jobs_added_after_500_background_finish_first() {
    let e = env();
    let m = e.numbered_media(510);
    let q = Arc::new(JobQueue::new(Arc::clone(&e.pool)));
    q.enqueue_batch(&m[..500].iter().map(|&id| thumb_job(id, "sm", Priority::Background)).collect::<Vec<_>>()).unwrap();

    let rec = Arc::new(Recorder { work: Duration::from_millis(3), ..Default::default() });
    let cfg = WorkerConfig { workers: 4, reserved_immediate: 1, mode: ResourceMode::Full, ..Default::default() };
    let pool = WorkerPool::start(Arc::clone(&q), rec.clone(), cfg).unwrap();

    assert!(wait_until(Duration::from_secs(10), || rec.finished.lock().unwrap().len() >= 30));
    let urgent: Vec<MediaId> = m[500..].to_vec();
    q.enqueue_batch(&urgent.iter().map(|&id| thumb_job(id, "sm", Priority::Immediate)).collect::<Vec<_>>()).unwrap();
    let bg_done_at_enqueue = rec.finished.lock().unwrap().len();

    let urgent_set: HashSet<MediaId> = urgent.iter().copied().collect();
    assert!(wait_until(Duration::from_secs(10), || {
        rec.finished.lock().unwrap().iter().filter(|(id, _)| urgent_set.contains(id)).count() == 10
    }));
    let finished = rec.finished.lock().unwrap().clone();
    let started = rec.started.lock().unwrap().clone();
    pool.shutdown();

    let last_urgent = finished.iter().rposition(|(id, _)| urgent_set.contains(id)).unwrap();
    let bg_between =
        finished[bg_done_at_enqueue..last_urgent].iter().filter(|(id, _)| !urgent_set.contains(id)).count();
    eprintln!(
        "eklenme anında {bg_done_at_enqueue} arka plan işi bitmişti; acil işler bitene kadar {bg_between} arka plan işi daha bitti"
    );
    // Yalnızca o an zaten çalışmakta olan arka plan işleri (en çok işçi sayısı kadar)
    // ve son acil işle aynı anda başlayanlar araya girebilir.
    assert!(bg_between <= 2 * 4, "{bg_between} arka plan işi acil işlerin önüne geçti");
    assert!(finished.len() < 500, "acil işler 500 arka plan işini beklememeli");

    // Başlama sırasında 10 acil iş, araya en çok (işçi−1) kayıt yarışıyla girer.
    let first = started.iter().position(|(id, _)| urgent_set.contains(id)).unwrap();
    let last = started.iter().rposition(|(id, _)| urgent_set.contains(id)).unwrap();
    assert!(last - first < 10 + 4, "acil işler dağınık başladı: {first}..{last}");
    assert!(started[first..=last].iter().filter(|(id, _)| urgent_set.contains(id)).all(|(_, p)| *p == 0));
}

#[test]
fn worker_survives_panicking_handler_and_transient_errors() {
    let e = env();
    let m = e.numbered_media(20);
    let q = Arc::new(JobQueue::with_retry(Arc::clone(&e.pool), fast_retry()));
    q.enqueue_batch(
        &m.iter().map(|&id| thumb_job(id, "sm", Priority::Normal).with_max_attempts(2)).collect::<Vec<_>>(),
    )
    .unwrap();
    let rec = Arc::new(Recorder { panic_on: Some(m[3]), transient_on: Some(m[5]), ..Default::default() });
    let cfg = WorkerConfig { workers: 2, reserved_immediate: 0, mode: ResourceMode::Full, ..Default::default() };
    let pool = WorkerPool::start(Arc::clone(&q), rec.clone(), cfg).unwrap();
    assert!(wait_until(Duration::from_secs(10), || {
        let s = q.stats().unwrap();
        s.pending == 0 && s.running == 0 && s.failed == 0
    }));
    pool.shutdown();

    assert_eq!(rec.finished.lock().unwrap().len(), 18, "diğer 18 iş tamamlanmalı");
    let p = q.find(JobKind::Thumb, Some(m[3]), "sm").unwrap().unwrap();
    assert_eq!((p.state, p.last_error_code.as_deref(), p.attempts), (JobState::Dead, Some("PANIC"), 1));
    assert!(p.last_error_msg.unwrap().contains("işleyici hatası"));
    let t = q.find(JobKind::Thumb, Some(m[5]), "sm").unwrap().unwrap();
    assert_eq!((t.state, t.last_error_code.as_deref(), t.attempts), (JobState::Dead, Some("BUSY"), 2));
    assert_eq!(q.stats().unwrap().dead, 2);
}

#[test]
fn crashed_worker_lease_is_reclaimed_on_start() {
    let e = env();
    let m = e.numbered_media(1);
    let q = Arc::new(JobQueue::new(Arc::clone(&e.pool)));
    let id = q.enqueue(&thumb_job(m[0], "lg", Priority::Normal)).unwrap();
    // Önceki süreç işi aldı ve çöktü.
    q.claim("eski-süreç", 9, Duration::from_millis(1)).unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(q.get(id).unwrap().unwrap().state, JobState::Running);

    let rec = Arc::new(Recorder::default());
    let pool =
        WorkerPool::start(Arc::clone(&q), rec.clone(), WorkerConfig { workers: 1, ..Default::default() }).unwrap();
    assert!(wait_until(Duration::from_secs(5), || rec.finished.lock().unwrap().len() == 1));
    pool.shutdown();
    assert!(q.get(id).unwrap().is_none(), "tamamlanan iş silinir");
}

#[test]
fn pause_user_activity_and_resource_mode_throttle_background_only() {
    let e = env();
    let m = e.numbered_media(6);
    let q = Arc::new(JobQueue::new(Arc::clone(&e.pool)));
    let rec = Arc::new(Recorder::default());
    let cfg = WorkerConfig { workers: 3, reserved_immediate: 1, mode: ResourceMode::Quiet, ..Default::default() };
    let pool = WorkerPool::start(Arc::clone(&q), rec.clone(), cfg).unwrap();
    assert_eq!(pool.throttle().general_limit(), 1);
    let done = |id: MediaId| rec.finished.lock().unwrap().iter().any(|(m, _)| *m == id);

    pool.pause();
    q.enqueue(&thumb_job(m[0], "sm", Priority::Background)).unwrap();
    q.enqueue(&thumb_job(m[1], "sm", Priority::Immediate)).unwrap();
    assert!(wait_until(Duration::from_secs(5), || done(m[1])), "duraklatılmışken de görünen iş çalışır");
    std::thread::sleep(Duration::from_millis(100));
    assert!(!done(m[0]), "duraklatılmışken arka plan işi çalışmamalı");
    pool.resume();
    assert!(wait_until(Duration::from_secs(5), || done(m[0])));

    pool.set_user_active(true);
    q.enqueue(&thumb_job(m[2], "sm", Priority::Background)).unwrap();
    q.enqueue(&thumb_job(m[3], "sm", Priority::Interactive)).unwrap();
    assert!(wait_until(Duration::from_secs(5), || done(m[3])));
    std::thread::sleep(Duration::from_millis(100));
    assert!(!done(m[2]), "kullanıcı etkinken toplu tarama beklemeli");
    pool.set_user_active(false);
    assert!(wait_until(Duration::from_secs(5), || done(m[2])));

    pool.set_mode(ResourceMode::Full);
    assert_eq!(pool.throttle().general_limit(), 2);
    pool.shutdown();
}

// ───────────────────────────── medya hattı ─────────────────────────────

fn media_fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../loupe-media/tests/fixtures").join(rel)
}

#[test]
fn pipeline_thumbnails_metadata_location_and_corrupt_file_goes_dead() {
    let e = env();
    std::fs::create_dir_all(e.media_dir()).unwrap();
    std::fs::copy(media_fixture("exif/goreme_canon_r6.jpg"), e.media_dir().join("goreme.jpg")).unwrap();
    std::fs::copy(media_fixture("exif/orientation_6.jpg"), e.media_dir().join("dikey.jpg")).unwrap();
    std::fs::write(e.media_dir().join("bozuk.jpg"), b"\xFF\xD8\xFF\xE0 kesik ve bozuk").unwrap();
    let ids = e.add_media(&["goreme.jpg".into(), "dikey.jpg".into(), "bozuk.jpg".into(), "cikarilmis-disk.jpg".into()]);
    let (good, tall, corrupt, offline) = (ids[0], ids[1], ids[2], ids[3]);

    let cache = ThumbCache::new(e.dir.path().join("thumbs"));
    let geo = Arc::new(GeoIndex::load_dir(&media_fixture("geonames")).unwrap());
    let root = e.media_dir();
    let resolver = move |m: &loupe_db::models::MediaFile| {
        let p = root.join(m.rel_path.as_str());
        p.exists().then_some(p)
    };
    let handler = Arc::new(
        MediaPipeline::new(
            Arc::clone(&e.pool),
            Thumbnailer::new(cache.clone(), Box::new(ImageBackend)),
            Some(Arc::clone(&geo)),
            resolver,
        )
        .without_ai_followups(),
    );
    let q = Arc::new(JobQueue::with_retry(
        Arc::clone(&e.pool),
        RetryPolicy { base: Duration::from_secs(60), max: Duration::from_secs(60) },
    ));

    // Önce metadata (yönlendirme küçük resimden önce bilinsin), sonra küçük resimler.
    let meta: Vec<_> =
        ids.iter().map(|&id| NewJob::media(JobKind::MetadataFull, id, "", Priority::Interactive)).collect();
    q.enqueue_batch(&meta).unwrap();
    let thumbs: Vec<_> = ids.iter().map(|&id| thumb_job(id, "sm", Priority::Normal)).collect();
    q.enqueue_batch(&thumbs).unwrap();

    let cfg = WorkerConfig { workers: 1, reserved_immediate: 0, mode: ResourceMode::Full, ..Default::default() };
    let pool = WorkerPool::start(Arc::clone(&q), handler, cfg).unwrap();
    assert!(wait_until(Duration::from_secs(20), || {
        let s = q.stats().unwrap();
        s.pending == 0 && s.running == 0
    }));

    // Bozuk dosya işçiyi düşürmedi: sonradan eklenen iş de işlenir.
    q.enqueue(&thumb_job(good, "md", Priority::Immediate)).unwrap();
    assert!(wait_until(Duration::from_secs(10), || q.find(JobKind::Thumb, Some(good), "md").unwrap().is_none()));
    pool.shutdown();

    // Küçük resimler: katalog satırı ve önbellek dosyası.
    let row = e.pool.read(|c| thumbnail::get(c, good, "sm")).unwrap().unwrap();
    assert_eq!((row.width, row.height, row.format.as_str(), row.source_kind.as_str()), (256, 171, "webp", "decode"));
    let md = e.pool.read(|c| thumbnail::get(c, good, "md")).unwrap().unwrap();
    assert_eq!((md.width, md.height), (768, 512));
    assert!(cache.lookup(&row.source_blake3, loupe_media::ThumbSize::Sm).is_some());
    let tall_row = e.pool.read(|c| thumbnail::get(c, tall, "sm")).unwrap().unwrap();
    assert_eq!((tall_row.width, tall_row.height), (171, 256), "EXIF yönlendirmesi uygulanmış olmalı");

    // Metadata ve konum.
    let (model, iso, tz, lat, loc_id): (String, u32, i32, f64, i64) = e
        .pool
        .read(|c| {
            Ok(c.query_row(
                "SELECT camera_model, iso, tz_offset_min, gps_lat, location_id FROM media_files WHERE id = ?1",
                [good.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?)
        })
        .unwrap();
    assert_eq!((model.as_str(), iso, tz), ("Canon EOS R6", 100, 180));
    assert!((lat - 38.64312).abs() < 1e-6);
    let loc = e.pool.read(|c| location::get(c, location::LocationId(loc_id))).unwrap().unwrap();
    assert_eq!(
        (loc.locality.as_deref(), loc.admin1.as_deref(), loc.country.as_str(), loc.geoname_id),
        (Some("Göreme"), Some("Nevşehir"), "Turkey", Some(313_947))
    );
    assert_eq!(loc.dataset_ver, geo.dataset_ver());

    // Bozuk dosya: panik yok, ilk denemede dead-letter.
    let dead = q.find(JobKind::Thumb, Some(corrupt), "sm").unwrap().unwrap();
    assert_eq!((dead.state, dead.attempts, dead.last_error_code.as_deref()), (JobState::Dead, 1, Some("DECODE")));
    assert!(e.pool.read(|c| thumbnail::get(c, corrupt, "sm")).unwrap().is_none());

    // Takılı olmayan disk: geçici hata, geri çekilerek bekliyor.
    let off = q.find(JobKind::Thumb, Some(offline), "sm").unwrap().unwrap();
    assert_eq!((off.state, off.last_error_code.as_deref()), (JobState::Failed, Some("VOLUME_OFFLINE")));
}

// ───────────────────────────── debounce ve yeniden çalıştırma ─────────────────────────────

#[test]
fn delayed_global_job_debounces_and_rerun_request_while_running_is_not_lost() {
    let e = env();
    let q = JobQueue::new(Arc::clone(&e.pool));
    let job = NewJob::global(JobKind::ClusterEvents, Priority::Idle).with_delay(Duration::from_secs(60));

    let id = q.enqueue(&job).unwrap();
    let first = q.get(id).unwrap().unwrap().run_after;
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(q.enqueue(&job).unwrap(), id, "tek satır");
    assert!(q.get(id).unwrap().unwrap().run_after > first, "yeni istek başlangıcı ileri kaydırır");
    assert!(q.claim("w", 9, Duration::from_secs(30)).unwrap().is_none(), "gecikme dolmadan alınmaz");

    // Hemen hazır bir istek gelir, işçi alır; iş sürerken yeni istek gelir.
    let now = NewJob::global(JobKind::ClusterFaces, Priority::Idle);
    let fid = q.enqueue(&now).unwrap();
    let claimed = q.claim("w", 9, Duration::from_secs(30)).unwrap().unwrap();
    assert_eq!(claimed.id, fid);
    q.enqueue(&now).unwrap();
    q.complete(fid, "w").unwrap();
    let again = q.get(fid).unwrap().expect("iş sürerken gelen istek kaybolmamalı");
    assert_eq!((again.state, again.attempts), (JobState::Pending, 0));
    let second = q.claim("w", 9, Duration::from_secs(30)).unwrap().unwrap();
    q.complete(second.id, "w").unwrap();
    assert!(q.get(fid).unwrap().is_none(), "ikinci çalıştırmadan sonra satır silinir");
}
