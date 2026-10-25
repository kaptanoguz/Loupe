#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Uçtan uca bulut yedeği: gerçek SQLite katalog, gerçek age şifrelemesi,
//! yerel/NAS hedefi ve S3 + WebDAV protokolünü konuşan sahte bir sunucu.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes as AxBytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use loupe_cloud::crypto::decrypt_from;
use loupe_cloud::provider::Credentials;
use loupe_cloud::provider::s3::S3Config;
use loupe_cloud::queue::{self, write_tx};
use loupe_cloud::targets::{self, NewTarget};
use loupe_cloud::{
    BackupEngine, ChecksumAlgo, CloudError, CloudProvider, ErrorClass, Keyring, LibraryRoots, LocalProvider,
    RecoveryKey, S3Provider, UploadOutcome, WebDavProvider,
};
use loupe_core::{
    Blake3Hash, CommitOutcome, ImportMode, LibraryId, LibraryKind, MediaFileType, MediaId, NewMediaFile, Placement,
    RelPath, TakenAtSource,
};
use loupe_db::models::NewLibrary;
use loupe_db::repo::{library, media};
use loupe_db::{Pool, PoolConfig};
use md5::{Digest, Md5};
use tempfile::TempDir;
use zeroize::Zeroizing;

const MIB: usize = 1024 * 1024;

struct Fixture {
    tmp: TempDir,
    root: PathBuf,
    pool: Arc<Pool>,
    lib: LibraryId,
}

fn photo_bytes(seed: u32, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Fotograflar");
        std::fs::create_dir_all(&root).unwrap();
        let pool = Arc::new(Pool::open(&tmp.path().join("catalog.db"), PoolConfig::default()).unwrap());
        let lib = pool
            .write(|c| {
                library::insert(
                    c,
                    &NewLibrary {
                        uuid: *b"loupe-cloud-test",
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
        Self { tmp, root, pool, lib }
    }

    /// Kütüphaneye dosya yazar ve kataloglar.
    fn add_photo(&self, rel: &str, bytes: &[u8]) -> (MediaId, Blake3Hash) {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let hash = Blake3Hash(*blake3::hash(bytes).as_bytes());
        let row = NewMediaFile {
            library_id: self.lib,
            rel_path: RelPath::new(rel).unwrap(),
            original_filename: rel.rsplit('/').next().unwrap().to_owned(),
            placement: Placement::Copy,
            import_session_id: None,
            file_type: MediaFileType::Jpeg,
            raw_format: None,
            mime: "image/jpeg".into(),
            size_bytes: bytes.len() as u64,
            mtime_ns: 0,
            inode: None,
            device_id: None,
            blake3: hash,
            phash: None,
            width: None,
            height: None,
            taken_at_utc: 1_761_390_000_000,
            tz_offset_min: Some(180),
            taken_at_source: TakenAtSource::Exif,
            imported_at: 1_761_390_000_000,
        };
        let id = match self.pool.write(|c| media::insert(c, &row)).unwrap() {
            CommitOutcome::Inserted(id) | CommitOutcome::Duplicate(id) => id,
        };
        (id, hash)
    }

    fn roots(&self) -> Arc<dyn LibraryRoots> {
        Arc::new(HashMap::from([(self.lib, self.root.clone())]))
    }

    fn target(&self, provider: &str, config: serde_json::Value, recipient: Option<&str>) -> targets::Target {
        let id = write_tx(&self.pool, |c| {
            targets::insert(
                c,
                &NewTarget {
                    name: provider,
                    provider,
                    config,
                    secret_ref: "loupe/test",
                    key_prefix: "loupe/",
                    age_recipient: recipient,
                    bandwidth_bps: None,
                },
                0,
            )
        })
        .unwrap();
        self.pool.read(|c| Ok(targets::get(c, id))).unwrap().unwrap().unwrap()
    }

    fn enqueue(&self, target: i64, media_id: MediaId) {
        write_tx(&self.pool, |c| queue::enqueue_upload(c, target, media_id, 5, loupe_core::now_ms())).unwrap();
    }

    fn status(&self, media_id: MediaId, target: i64) -> queue::SyncStatusRow {
        self.pool.read(|c| Ok(queue::status(c, media_id, target))).unwrap().unwrap().unwrap()
    }

    fn queue_rows(&self) -> Vec<(String, i64, Option<String>)> {
        self.pool
            .read(|c| {
                Ok(c.prepare("SELECT state, attempts, last_error_code FROM sync_queue ORDER BY id")?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<Result<_, _>>()?)
            })
            .unwrap()
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(files_under(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn encrypted_backup_round_trip_restores_with_recovery_key() {
    let fx = Fixture::new();
    let original = photo_bytes(41, 3 * MIB + 12_345);
    let (media_id, hash) = fx.add_photo("2026/10/24/DSC_0412.JPG", &original);

    // Şifreleme açılırken: kurtarma anahtarı bir kez gösterilir, kâğıda yazılır.
    let recovery = RecoveryKey::generate();
    let paper = recovery.display().to_string();
    let keys = Keyring::generate(&recovery.public().to_string()).unwrap();

    let nas = fx.tmp.path().join("nas");
    std::fs::create_dir_all(&nas).unwrap();
    let target = fx.target("local", serde_json::json!({ "path": nas }), Some(&recovery.public().to_string()));
    assert!(target.encrypted);
    let provider = loupe_cloud::provider::from_target(&target.provider, &target.config, None).unwrap();
    let engine = BackupEngine::new(fx.pool.clone(), provider, target.clone(), Some(keys.clone()), fx.roots()).unwrap();

    fx.enqueue(target.id, media_id);
    assert_eq!(fx.status(media_id, target.id).state, "queued");

    let reports = engine.drain().await.unwrap();
    assert_eq!(reports.len(), 1);
    let UploadOutcome::Synced { remote_key, remote_size, checksum_algo } = &reports[0].outcome else {
        panic!("{reports:?}")
    };
    let hex = hash.to_hex();
    assert_eq!(remote_key, &format!("loupe/objects/{}/{}/{hex}.age", &hex[..2], &hex[2..4]));
    assert_eq!(*checksum_algo, "sha256");

    // sync_status: Synced + Encrypted + doğrulama zamanı; kuyruk boş.
    let st = fx.status(media_id, target.id);
    assert_eq!(st.state, "synced");
    assert_eq!(st.encryption, "age");
    assert_eq!(st.key_fingerprint.as_deref(), Some(keys.fingerprint().as_str()));
    assert_eq!(st.local_checksum, hash.0.to_vec());
    assert_eq!(st.remote_checksum_algo.as_deref(), Some("sha256"));
    assert!(st.uploaded_at.is_some() && st.last_verified_at == st.uploaded_at);
    assert!(fx.queue_rows().is_empty());

    // Buluttaki nesne şifreli: ne içerik ne dosya adı görünür; geçici dosya kalmadı.
    let object = nas.join(remote_key);
    let cipher = std::fs::read(&object).unwrap();
    assert_eq!(cipher.len() as u64, *remote_size);
    assert!(cipher.starts_with(b"age-encryption.org/v1\n"));
    assert!(!cipher.windows(32).any(|w| w == &original[100_000..100_032]));
    assert_eq!(files_under(&nas), vec![object.clone()]);
    assert!(!remote_key.contains("DSC_0412"));

    // Bilgisayar kayboldu: kâğıttaki anahtar elle yazılır (küçük harf, boşluklar).
    let typed = RecoveryKey::parse(&paper.to_lowercase()).unwrap();
    let dest = fx.tmp.path().join("geri/DSC_0412.JPG");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let restored = engine.restore(media_id, typed.identity(), &dest).await.unwrap();
    assert_eq!(restored, hash);
    assert_eq!(*blake3::hash(&std::fs::read(&dest).unwrap()).as_bytes(), hash.0, "orijinal BLAKE3 eşleşir");

    // Standart age biçimi: uygulama olmadan da açılır (`age -d -i kurtarma.txt`).
    let mut plain = Vec::new();
    std::io::Read::read_to_end(&mut decrypt_from(typed.identity(), cipher.as_slice()).unwrap(), &mut plain).unwrap();
    assert_eq!(plain, original);

    // Yanlış anahtar: hata, hedef dosya oluşmaz.
    let wrong = fx.tmp.path().join("geri/yanlis.JPG");
    assert!(engine.restore(media_id, RecoveryKey::generate().identity(), &wrong).await.is_err());
    assert!(!wrong.exists());
    assert!(files_under(&fx.tmp.path().join("geri")).len() == 1, "yarım dosya kalmaz");

    // Periyodik tam doğrulama (cihaz anahtarıyla geri okuma).
    engine.verify_remote(media_id).await.unwrap();

    // Hedefin kayıtlı kurtarma alıcısı dışında bir anahtarlıkla yedek alınmaz.
    let other = Keyring::generate(&RecoveryKey::generate().public().to_string()).unwrap();
    let provider = loupe_cloud::provider::from_target(&target.provider, &target.config, None).unwrap();
    let mismatch = BackupEngine::new(fx.pool.clone(), provider, target, Some(other), fx.roots());
    assert!(matches!(mismatch, Err(CloudError::Config(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_local_file_never_reaches_the_cloud() {
    let fx = Fixture::new();
    let bytes = photo_bytes(7, 2 * MIB);
    let (media_id, _) = fx.add_photo("2026/10/24/IMG_0001.JPG", &bytes);
    // Bit çürümesi: diskteki dosya katalogdan farklı.
    let mut rotten = bytes.clone();
    rotten[MIB + 17] ^= 0x40;
    std::fs::write(fx.root.join("2026/10/24/IMG_0001.JPG"), &rotten).unwrap();

    let recovery = RecoveryKey::generate();
    let nas = fx.tmp.path().join("nas");
    std::fs::create_dir_all(&nas).unwrap();
    let target = fx.target("local", serde_json::json!({ "path": nas }), Some(&recovery.public().to_string()));
    let keys = Keyring::generate(&recovery.public().to_string()).unwrap();
    let engine = BackupEngine::new(
        fx.pool.clone(),
        Arc::new(LocalProvider::new(&nas).unwrap()),
        target.clone(),
        Some(keys),
        fx.roots(),
    )
    .unwrap();
    fx.enqueue(target.id, media_id);

    let r = engine.run_once().await.unwrap().unwrap();
    assert_eq!(r.outcome, UploadOutcome::Failed { class: ErrorClass::LocalCorrupt });
    assert!(files_under(&nas).is_empty(), "bozuk dosya ve yarım nesne yok");
    assert_eq!(fx.status(media_id, target.id).state, "error");
    assert_eq!(fx.status(media_id, target.id).last_error_code.as_deref(), Some("LOCAL_CORRUPT"));
    assert_eq!(fx.queue_rows(), vec![("dead".to_owned(), 1, Some("LOCAL_CORRUPT".to_owned()))]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_library_is_retried_with_backoff() {
    let fx = Fixture::new();
    let (media_id, _) = fx.add_photo("2026/10/24/IMG_0002.JPG", &photo_bytes(9, 4096));
    let nas = fx.tmp.path().join("nas");
    let target = fx.target("local", serde_json::json!({ "path": nas }), None);
    // Kütüphanenin diski takılı değil.
    let roots: Arc<dyn LibraryRoots> = Arc::new(HashMap::<LibraryId, PathBuf>::new());
    let engine =
        BackupEngine::new(fx.pool.clone(), Arc::new(LocalProvider::new(&nas).unwrap()), target.clone(), None, roots)
            .unwrap();
    fx.enqueue(target.id, media_id);

    let before = loupe_core::now_ms();
    let r = engine.run_once().await.unwrap().unwrap();
    let UploadOutcome::Retrying { class, at_ms } = r.outcome else { panic!("{r:?}") };
    assert_eq!(class, ErrorClass::LocalMissing);
    // 1. deneme: 2^1 × 30 sn × [0.5, 1.0] = 30–60 sn.
    assert!(at_ms >= before + 30_000 && at_ms <= loupe_core::now_ms() + 60_000, "{}", at_ms - before);
    assert_eq!(fx.status(media_id, target.id).state, "queued");
    assert_eq!(fx.queue_rows(), vec![("failed".to_owned(), 1, Some("LOCAL_MISSING".to_owned()))]);
    assert!(engine.run_once().await.unwrap().is_none(), "backoff dolmadan iş alınmaz");

    // Şifresiz hedefte nesne yerel düzenle aynı yoldadır.
    let engine = BackupEngine::new(
        fx.pool.clone(),
        Arc::new(LocalProvider::new(&nas).unwrap()),
        target.clone(),
        None,
        fx.roots(),
    )
    .unwrap();
    fx.enqueue(target.id, media_id);
    let r = engine.drain().await.unwrap();
    assert!(
        matches!(&r[0].outcome, UploadOutcome::Synced { remote_key, .. } if remote_key == "loupe/2026/10/24/IMG_0002.JPG")
    );
    assert_eq!(fx.status(media_id, target.id).encryption, "none");
}

// ───────────────────────── Sahte S3 + WebDAV sunucusu ─────────────────────────

#[derive(Default)]
struct Store {
    objects: HashMap<String, Vec<u8>>,
    /// upload_id → (parça no → içerik)
    uploads: HashMap<String, std::collections::BTreeMap<u16, Vec<u8>>>,
    aborted: usize,
}

type Shared = Arc<Mutex<Store>>;

fn query(req: &Request) -> HashMap<String, String> {
    req.uri()
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_owned(), v.to_owned())
        })
        .collect()
}

fn md5_hex(b: &[u8]) -> String {
    Md5::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

async fn fake(State(st): State<Shared>, req: Request) -> Response {
    let q = query(&req);
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let headers: HeaderMap = req.headers().clone();
    let body: AxBytes = axum::body::to_bytes(req.into_body(), 64 * MIB).await.unwrap();
    let mut s = st.lock().unwrap();
    if let Some(md5) = headers.get("content-md5") {
        let want = STANDARD.encode(Md5::digest(&body));
        if md5.to_str().unwrap() != want {
            return (StatusCode::BAD_REQUEST, "<Error><Code>BadDigest</Code></Error>").into_response();
        }
    }
    let etag = |b: &[u8]| [("etag", format!("\"{}\"", md5_hex(b)))];
    match (method.as_str(), q.contains_key("uploads"), q.get("uploadId")) {
        ("POST", true, _) => {
            let id = format!("u{}", s.uploads.len() + 1);
            s.uploads.insert(id.clone(), Default::default());
            format!("<InitiateMultipartUploadResult><Bucket>b</Bucket><Key>k</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>")
                .into_response()
        }
        ("PUT", _, Some(id)) => {
            let n: u16 = q["partNumber"].parse().unwrap();
            let e = etag(&body);
            s.uploads.get_mut(id).unwrap().insert(n, body.to_vec());
            (StatusCode::OK, e).into_response()
        }
        ("POST", _, Some(id)) => {
            let parts = s.uploads.remove(id).unwrap();
            let mut all = Vec::new();
            let mut md5s = Vec::new();
            for p in parts.values() {
                all.extend_from_slice(p);
                md5s.extend_from_slice(&Md5::digest(p));
            }
            let tag = format!("{}-{}", md5_hex(&md5s), parts.len());
            s.objects.insert(path, all);
            format!("<CompleteMultipartUploadResult><ETag>\"{tag}\"</ETag></CompleteMultipartUploadResult>")
                .into_response()
        }
        ("DELETE", _, Some(id)) => {
            s.uploads.remove(id);
            s.aborted += 1;
            StatusCode::NO_CONTENT.into_response()
        }
        ("PUT", _, None) => {
            let e = etag(&body);
            s.objects.insert(path, body.to_vec());
            (StatusCode::CREATED, e).into_response()
        }
        ("MKCOL", _, _) => StatusCode::CREATED.into_response(),
        ("HEAD", _, _) | ("GET", _, _) => match s.objects.get(&path) {
            None => StatusCode::NOT_FOUND.into_response(),
            Some(o) => {
                // Multipart nesnenin ETag'i S3'teki gibi saklanmaz; burada tekli PUT gibi hesaplanır.
                let e = if path.starts_with("/dav/") { None } else { Some(etag(o)) };
                let len = [("content-length", o.len().to_string())];
                let body = if method == Method::HEAD { Vec::new() } else { o.clone() };
                match e {
                    Some(e) => (len, e, body).into_response(),
                    None => (len, body).into_response(),
                }
            }
        },
        ("DELETE", _, None) => match s.objects.remove(&path) {
            Some(_) => StatusCode::NO_CONTENT.into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn fake_server() -> (String, Shared) {
    let st: Shared = Arc::default();
    let app = Router::new().fallback(fake).with_state(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), st)
}

fn chunks(bytes: &[u8], size: usize) -> loupe_cloud::ByteStream {
    let v: Vec<Result<Bytes, CloudError>> = bytes.chunks(size).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
    Box::pin(futures_util::stream::iter(v))
}

async fn collect(s: loupe_cloud::ByteStream) -> Vec<u8> {
    use futures_util::TryStreamExt;
    s.try_fold(Vec::new(), |mut acc, b| async move {
        acc.extend_from_slice(&b);
        Ok(acc)
    })
    .await
    .unwrap()
}

fn creds() -> Credentials {
    Credentials { user: "AKIATEST".into(), secret: Zeroizing::new("gizli".into()) }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s3_provider_single_put_multipart_and_abort() {
    let (base, st) = fake_server().await;
    let http = loupe_cloud::provider::http_client(None).unwrap();
    let cfg = S3Config {
        endpoint: base,
        bucket: "yedek".into(),
        region: "eu-central-003".into(),
        path_style: true,
        part_size: 8 * MIB,
    };
    let s3 = S3Provider::new(cfg, creds(), http).unwrap().with_part_size_unchecked(64 * 1024);

    // Küçük nesne: tekli PUT, ETag = MD5.
    let small = photo_bytes(1, 10_000);
    let r = s3.upload_stream("loupe/a.bin", chunks(&small, 4096)).await.unwrap();
    assert_eq!(r.expected.as_ref().unwrap().value, md5_hex(&small));
    assert_eq!(s3.head_object("loupe/a.bin").await.unwrap().unwrap().checksum, r.expected);

    // Büyük nesne: 64 KiB parçalar, beklenen ETag yerelde hesaplanır.
    let big = photo_bytes(2, 200 * 1024 + 7);
    let r = s3.upload_stream("loupe/b.bin", chunks(&big, 10_000)).await.unwrap();
    let exp = r.expected.unwrap();
    assert_eq!(exp.algo, ChecksumAlgo::Etag);
    assert!(exp.value.ends_with("-4"), "{}", exp.value);
    assert_eq!(r.size, big.len() as u64);
    assert_eq!(collect(s3.download_stream("loupe/b.bin").await.unwrap()).await, big);

    // Akış yarıda hata: multipart iptal edilir, nesne oluşmaz.
    let mut items: Vec<Result<Bytes, CloudError>> = big.chunks(50_000).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
    items.insert(3, Err(CloudError::Network("kesildi".into())));
    let err = s3.upload_stream("loupe/c.bin", Box::pin(futures_util::stream::iter(items))).await;
    assert!(err.is_err());
    assert!(s3.head_object("loupe/c.bin").await.unwrap().is_none());
    assert_eq!(st.lock().unwrap().aborted, 1);
    assert!(st.lock().unwrap().uploads.is_empty());

    s3.delete_object("loupe/a.bin").await.unwrap();
    s3.delete_object("loupe/a.bin").await.unwrap();
    assert!(s3.head_object("loupe/a.bin").await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webdav_target_is_verified_by_reading_back() {
    let (base, _st) = fake_server().await;
    let fx = Fixture::new();
    let original = photo_bytes(77, MIB + 3);
    let (media_id, hash) = fx.add_photo("2026/10/25/IMG_0100.JPG", &original);

    let recovery = RecoveryKey::generate();
    let keys = Keyring::generate(&recovery.public().to_string()).unwrap();
    let target =
        fx.target("webdav", serde_json::json!({ "url": format!("{base}/dav") }), Some(&recovery.public().to_string()));
    let provider: Arc<dyn CloudProvider> = Arc::new(
        WebDavProvider::new(&format!("{base}/dav"), Some(creds()), loupe_cloud::provider::http_client(None).unwrap())
            .unwrap(),
    );
    let engine = BackupEngine::new(fx.pool.clone(), provider.clone(), target.clone(), Some(keys), fx.roots()).unwrap();
    fx.enqueue(target.id, media_id);

    let r = engine.drain().await.unwrap();
    let UploadOutcome::Synced { remote_key, checksum_algo, .. } = &r[0].outcome else { panic!("{r:?}") };
    // WebDAV checksum vermez: nesne geri indirilip çözüldü, düz metin BLAKE3'ü karşılaştırıldı.
    assert_eq!(*checksum_algo, "blake3");
    let st = fx.status(media_id, target.id);
    assert_eq!(st.remote_checksum.as_deref(), Some(hash.to_hex().as_str()));
    assert_eq!(st.state, "synced");
    assert!(provider.head_object(remote_key).await.unwrap().unwrap().checksum.is_none());

    let dest = fx.tmp.path().join("IMG_0100.JPG");
    engine.restore(media_id, recovery.identity(), &dest).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), original);
}
