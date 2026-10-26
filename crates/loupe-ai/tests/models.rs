#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Model yöneticisi: manifest denetimi, yerel HTTP sunucusundan indirme,
//! zip / tar.gz üyesi çıkarma, Hugging Face LFS özet çözümü, kopan indirmenin
//! `Range` ile sürmesi, bozuk ya da yanlış özetli dosyanın reddi ve harici
//! ağırlıklı ONNX grafiğinin ağırlık dosyalarıyla birlikte kurulması.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::StreamExt;
use loupe_ai::AiError;
use loupe_ai::fetch::{Fetcher, InstallReport, Progress, sha256_hex};
use loupe_ai::models::{
    FileState, LOCK_FILE, Lock, Manifest, PARTIAL_DIR, git_blob_sha1, hex, onnx_external_locations, status,
};

/// Uygulamanın beklediği dosyalar (`loupe-app::ai::MODEL_FILES`).
const APP_FILES: [&str; 5] =
    ["scrfd_10g.onnx", "arcface_w600k_r50.onnx", "mclip_image.onnx", "mclip_text/model.onnx", "mclip_tokenizer.json"];

// ───────────────────────────── sahte sunucu ─────────────────────────────

#[derive(Default)]
struct Files {
    bytes: HashMap<String, Vec<u8>>,
    /// İlk istekte yarıda kesilecek dosyalar.
    cut_once: Vec<String>,
    /// (ad, Range başlığı) — her GET.
    hits: Vec<(String, Option<String>)>,
    hf: serde_json::Value,
    /// Dizine özel, sayfalı Hugging Face listesi (`?cursor=<n>`).
    hf_pages: HashMap<String, Vec<serde_json::Value>>,
}

type Shared = Arc<Mutex<Files>>;

async fn serve_file(State(st): State<Shared>, UrlPath(name): UrlPath<String>, h: HeaderMap) -> Response {
    serve(&st, name, &h).await
}

/// `/<org>/<repo>/resolve/<sürüm>/<yol>`: `hf:<yol>` adıyla saklanan dosya.
async fn serve_resolve(
    State(st): State<Shared>,
    UrlPath((_org, _repo, _rev, path)): UrlPath<(String, String, String, String)>,
    h: HeaderMap,
) -> Response {
    serve(&st, format!("hf:{path}"), &h).await
}

async fn serve(st: &Shared, name: String, h: &HeaderMap) -> Response {
    let range = h.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let (data, cut) = {
        let mut f = st.lock().unwrap();
        f.hits.push((name.clone(), range.clone()));
        let Some(data) = f.bytes.get(&name).cloned() else { return StatusCode::NOT_FOUND.into_response() };
        let cut = f.cut_once.iter().position(|n| *n == name).map(|i| f.cut_once.remove(i)).is_some();
        (data, cut)
    };
    let start = range
        .as_deref()
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
        .unwrap_or(0);
    let body = data[start..].to_vec();
    let len = body.len();
    let status = if start > 0 { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK };
    let body = if cut {
        // Bağlantı yarıda kopar: gövde bildirilen boydan kısa biter.
        let half = Bytes::from(body[..len / 2].to_vec());
        let drop_later = futures_util::stream::once(async {
            // Önce gönderilen baytlar istemciye ulaşsın, sonra bağlantı kopsun.
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err::<Bytes, std::io::Error>(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "kopma"))
        });
        Body::from_stream(futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(half)]).chain(drop_later))
    } else {
        Body::from(body)
    };
    (status, [(header::CONTENT_LENGTH, len.to_string())], body).into_response()
}

async fn hf_tree(
    State(st): State<Shared>,
    UrlPath((org, repo, _rev, dir)): UrlPath<(String, String, String, String)>,
    RawQuery(q): RawQuery,
) -> Response {
    let f = st.lock().unwrap();
    let Some(pages) = f.hf_pages.get(&dir) else { return Json(f.hf.clone()).into_response() };
    let i: usize = q.as_deref().and_then(|q| q.strip_prefix("cursor=")).and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut res = Json(pages[i].clone()).into_response();
    if i + 1 < pages.len() {
        let link = format!("</api/models/{org}/{repo}/tree/main/{dir}?cursor={}>; rel=\"next\"", i + 1);
        res.headers_mut().insert(header::LINK, link.parse().unwrap());
    }
    res
}

async fn server(files: Shared) -> String {
    let app = Router::new()
        .route("/f/{name}", get(serve_file))
        .route("/api/models/{org}/{repo}/tree/{rev}/{dir}", get(hf_tree))
        .route("/{org}/{repo}/resolve/{rev}/{*path}", get(serve_resolve))
        .with_state(files);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn fetcher(base: &str) -> Fetcher {
    Fetcher::with_client(loupe_ai::fetch::http_client().unwrap(), base).with_retry_delay(Duration::from_millis(10))
}

fn quiet() -> Arc<dyn Fn(Progress) + Send + Sync> {
    Arc::new(|_| {})
}

async fn install(f: &Fetcher, m: &Manifest, dir: &Path) -> Result<InstallReport, AiError> {
    f.install(m, dir, quiet(), Arc::new(AtomicBool::new(false))).await
}

// ───────────────────────────── arşiv üretimi ─────────────────────────────

fn le16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// En küçük zip: (ad, içerik, deflate mi). Bayrak 3 (veri tanımlayıcısı) ile
/// yerel başlıkta boyutlar sıfır: okuyucu merkez dizine güvenmeli.
fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, deflate) in entries {
        let packed = if *deflate {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        } else {
            data.to_vec()
        };
        let method = if *deflate { 8 } else { 0 };
        let offset = u32::try_from(out.len()).unwrap();
        le32(&mut out, 0x0403_4b50);
        for x in [20u16, 8, method, 0, 0] {
            le16(&mut out, x);
        }
        for _ in 0..3 {
            le32(&mut out, 0);
        }
        le16(&mut out, u16::try_from(name.len()).unwrap());
        le16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        le32(&mut central, 0x0201_4b50);
        for x in [20u16, 20, 8, method, 0, 0] {
            le16(&mut central, x);
        }
        le32(&mut central, 0);
        le32(&mut central, u32::try_from(packed.len()).unwrap());
        le32(&mut central, u32::try_from(data.len()).unwrap());
        for x in [u16::try_from(name.len()).unwrap(), 0, 0, 0, 0] {
            le16(&mut central, x);
        }
        le32(&mut central, 0);
        le32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let cd_off = u32::try_from(out.len()).unwrap();
    out.extend_from_slice(&central);
    le32(&mut out, 0x0605_4b50);
    for x in [0u16, 0, u16::try_from(entries.len()).unwrap(), u16::try_from(entries.len()).unwrap()] {
        le16(&mut out, x);
    }
    le32(&mut out, u32::try_from(central.len()).unwrap());
    le32(&mut out, cd_off);
    le16(&mut out, 0);
    out
}

fn tgz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, data) in entries {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        tar.extend_from_slice(&h);
        tar.extend_from_slice(data);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.extend_from_slice(&[0u8; 1024]);
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(&tar).unwrap();
    e.finish().unwrap()
}

fn blob(seed: u8, n: usize) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

struct Fixture {
    files: Shared,
    base: String,
    det: Vec<u8>,
    emb: Vec<u8>,
    rt: Vec<u8>,
    clip: Vec<u8>,
    manifest: Manifest,
}

/// Dört kaynak: zip (deflate + saklı üye), tar.gz, sabit özetli düz dosya,
/// Hugging Face'ten çözülen düz dosya.
async fn fixture() -> Fixture {
    let (det, emb, rt, clip, tok) =
        (blob(1, 70_000), blob(2, 300_000), blob(3, 50_000), blob(4, 120_000), b"{\"model\":{}}".to_vec());
    let z = zip(&[("README.txt", b"bilgi", false), ("det_10g.onnx", &det, true), ("w600k_r50.onnx", &emb, false)]);
    let t = tgz(&[("ort/include/x.h", b"//"), ("ort/lib/libonnxruntime.so.1.22.0", &rt)]);
    let files: Shared = Arc::new(Mutex::new(Files::default()));
    {
        let mut f = files.lock().unwrap();
        for (n, b) in [("buffalo.zip", &z), ("ort.tgz", &t), ("clip.onnx", &clip), ("tok.json", &tok)] {
            f.bytes.insert(n.into(), b.clone());
        }
        f.hf = serde_json::json!([
            { "type": "file", "path": "visual/model.onnx", "size": clip.len(),
              "lfs": { "oid": sha256_hex(&clip), "size": clip.len(), "pointerSize": 134 } },
            { "type": "file", "path": "visual/config.json", "size": 10 }
        ]);
    }
    let base = server(files.clone()).await;
    let json = serde_json::json!({
        "version": 1,
        "sources": [
            { "id": "buffalo", "url": format!("{base}/f/buffalo.zip"), "size": z.len(), "sha256": sha256_hex(&z), "archive": "zip" },
            { "id": "ort", "url": format!("{base}/f/ort.tgz"), "size": t.len(), "sha256": sha256_hex(&t), "archive": "tgz" },
            { "id": "clip", "url": format!("{base}/f/clip.onnx"), "hf": { "repo": "org/clip", "revision": "main", "path": "visual/model.onnx" } },
            { "id": "tok", "url": format!("{base}/f/tok.json"), "size": tok.len(), "sha256": sha256_hex(&tok) }
        ],
        "files": [
            { "file": "scrfd_10g.onnx", "role": "face_detect", "source": "buffalo", "member": "det_10g.onnx", "size": det.len(), "sha256": sha256_hex(&det) },
            { "file": "arcface_w600k_r50.onnx", "role": "face_embed", "dim": 512, "source": "buffalo", "member": "w600k_r50.onnx", "size": emb.len(), "sha256": sha256_hex(&emb) },
            { "file": "libonnxruntime.so", "role": "runtime", "source": "ort", "member": "ort/lib/libonnxruntime.so.1.22.0", "size": rt.len(), "sha256": sha256_hex(&rt) },
            { "file": "mclip_image.onnx", "role": "clip_image", "dim": 512, "source": "clip" },
            { "file": "mclip_tokenizer.json", "role": "tokenizer", "source": "tok" }
        ]
    });
    let manifest = Manifest::parse(&json.to_string()).unwrap();
    Fixture { files, base, det, emb, rt, clip, manifest }
}

fn hits(f: &Shared) -> Vec<(String, Option<String>)> {
    f.lock().unwrap().hits.clone()
}

fn states(m: &Manifest, dir: &Path) -> Vec<(String, FileState)> {
    status(m, dir).unwrap().into_iter().map(|s| (s.file, s.state)).collect()
}

// ───────────────────────────── testler ─────────────────────────────

#[test]
fn builtin_manifest_is_valid_and_covers_the_app() {
    let m = Manifest::builtin().unwrap();
    for f in APP_FILES {
        assert!(m.file(f).is_some(), "{f} manifestte yok");
    }
    assert_eq!(m.file("arcface_w600k_r50.onnx").unwrap().dim, Some(512));
    assert_eq!(m.file("mclip_image.onnx").unwrap().dim, Some(512));
    assert_eq!(m.file("mclip_text/model.onnx").unwrap().dim, Some(512));
    assert!(m.file("mclip_text/model.onnx").unwrap().external_data);
    assert_eq!(m.file("libonnxruntime.so").unwrap().role, "runtime");
    for s in &m.sources {
        assert!(s.url.starts_with("https://"), "{}", s.url);
        // GitHub sürümleri bayt bayt sabit; Hugging Face kaynakları LFS özetiyle çözülür.
        if s.url.starts_with("https://github.com/") {
            assert!(s.sha256.is_some() && s.size.is_some(), "{} sabitlenmemiş", s.id);
        } else {
            assert!(s.hf.is_some(), "{}", s.id);
        }
    }
}

#[test]
fn manifest_validation_rejects_unsafe_or_incomplete_entries() {
    let good = Manifest::builtin().unwrap();
    let reject = |edit: &dyn Fn(&mut Manifest)| {
        let mut m = good.clone();
        edit(&mut m);
        let json = serde_json::to_string(&m).unwrap();
        assert!(Manifest::parse(&json).is_err(), "kabul edilmemeliydi: {json:.200}");
    };
    reject(&|m| m.version = 2);
    reject(&|m| m.sources[0].sha256 = Some("1234".into()));
    reject(&|m| m.sources[0].sha256 = Some("zz".repeat(32)));
    reject(&|m| m.sources[0].url = "http://example.com/x.zip".into());
    reject(&|m| m.sources[0].url = "file:///etc/passwd".into());
    reject(&|m| {
        m.sources[0].sha256 = None;
        m.sources[0].size = None;
    });
    reject(&|m| m.files[0].file = "../../.bashrc".into());
    reject(&|m| m.files[0].file = ".partial".into());
    reject(&|m| m.files[1].file = m.files[0].file.clone());
    reject(&|m| m.files[0].source = "yok".into());
    reject(&|m| m.files[0].member = Some("../det.onnx".into()));
    reject(&|m| m.files[0].sha256 = None);
    reject(&|m| m.files[0].size = Some(0));
    reject(&|m| m.sources[1].id = m.sources[0].id.clone());
    // Arşiv olmayan kaynaktan iki dosya çıkamaz.
    reject(&|m| {
        let mut extra = m.files.iter().find(|f| f.source == "mclip-visual").unwrap().clone();
        extra.file = "ikinci.onnx".into();
        m.files.push(extra);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_downloads_verifies_extracts_and_is_idempotent() {
    let fx = fixture().await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("models");
    let seen = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let s2 = seen.clone();
    let progress: Arc<dyn Fn(Progress) + Send + Sync> = Arc::new(move |p| s2.lock().unwrap().push(p.phase));

    let report =
        fetcher(&fx.base).install(&fx.manifest, &dir, progress, Arc::new(AtomicBool::new(false))).await.unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.installed.len(), 5);
    assert_eq!(std::fs::read(dir.join("scrfd_10g.onnx")).unwrap(), fx.det);
    assert_eq!(std::fs::read(dir.join("arcface_w600k_r50.onnx")).unwrap(), fx.emb);
    assert_eq!(std::fs::read(dir.join("libonnxruntime.so")).unwrap(), fx.rt);
    assert_eq!(std::fs::read(dir.join("mclip_image.onnx")).unwrap(), fx.clip);
    assert!(states(&fx.manifest, &dir).iter().all(|(_, s)| *s == FileState::Verified));
    for phase in ["resolve", "download", "verify", "extract", "done"] {
        assert!(seen.lock().unwrap().contains(&phase), "{phase}");
    }
    // Hugging Face özeti kilit dosyasında; yarım dosya kalmadı.
    let lock = Lock::load(&dir);
    assert_eq!(lock.sources["clip"].sha256, sha256_hex(&fx.clip));
    assert!(dir.join(LOCK_FILE).is_file());
    assert_eq!(std::fs::read_dir(dir.join(PARTIAL_DIR)).unwrap().count(), 0);

    // İkinci çalıştırma: her şey doğrulanmış, ağa çıkılmaz.
    let before = hits(&fx.files).len();
    let again = install(&fetcher(&fx.base), &fx.manifest, &dir).await.unwrap();
    assert_eq!((again.installed.len(), again.unchanged.len(), again.downloaded_bytes), (0, 5, 0));
    assert_eq!(hits(&fx.files).len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_download_is_rejected_and_never_installed() {
    let fx = fixture().await;
    let tmp = tempfile::tempdir().unwrap();
    // Sunucu (ya da aradaki biri) içeriği değiştirdi: boyut aynı, bir bayt farklı.
    {
        let mut f = fx.files.lock().unwrap();
        let z = f.bytes.get_mut("buffalo.zip").unwrap();
        let mid = z.len() / 2;
        z[mid] ^= 0xff;
        let t = f.bytes.get_mut("tok.json").unwrap();
        t.push(b' ');
    }
    let report = install(&fetcher(&fx.base), &fx.manifest, tmp.path()).await.unwrap();
    let failed: HashMap<_, _> = report.failed.iter().map(|f| (f.source.as_str(), f.reason.as_str())).collect();
    assert!(failed["buffalo"].contains("doğrulama başarısız"), "{failed:?}");
    assert!(failed["tok"].contains("doğrulama başarısız"), "{failed:?}");
    // Bozuk kaynaktan hiçbir dosya asıl adla görünmez; diğer kaynaklar kuruldu.
    for f in ["scrfd_10g.onnx", "arcface_w600k_r50.onnx", "mclip_tokenizer.json"] {
        assert!(!tmp.path().join(f).exists(), "{f}");
    }
    assert!(tmp.path().join("libonnxruntime.so").is_file());
    assert!(tmp.path().join("mclip_image.onnx").is_file());
    assert_eq!(std::fs::read_dir(tmp.path().join(PARTIAL_DIR)).unwrap().count(), 0, "yarım dosya silindi");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_member_hash_and_tampered_file_on_disk_are_detected() {
    let mut fx = fixture().await;
    let tmp = tempfile::tempdir().unwrap();
    install(&fetcher(&fx.base), &fx.manifest, tmp.path()).await.unwrap();

    // Diskteki model değiştirildi: durum "bozuk", kurulum yeniden indirir.
    let det = tmp.path().join("scrfd_10g.onnx");
    let mut bytes = std::fs::read(&det).unwrap();
    bytes[10] ^= 1;
    std::fs::write(&det, &bytes).unwrap();
    std::fs::write(tmp.path().join("mclip_image.onnx"), b"kisa").unwrap();
    let st: HashMap<_, _> = states(&fx.manifest, tmp.path()).into_iter().collect();
    assert_eq!(st["scrfd_10g.onnx"], FileState::Corrupt);
    assert_eq!(st["mclip_image.onnx"], FileState::Corrupt);
    let report = install(&fetcher(&fx.base), &fx.manifest, tmp.path()).await.unwrap();
    assert!(report.failed.is_empty());
    assert_eq!(std::fs::read(&det).unwrap(), fx.det);
    assert!(states(&fx.manifest, tmp.path()).iter().all(|(_, s)| *s == FileState::Verified));

    // Manifestteki üye özeti yanlış: arşiv doğru olsa bile üye reddedilir.
    let i = fx.manifest.files.iter().position(|f| f.file == "arcface_w600k_r50.onnx").unwrap();
    fx.manifest.files[i].sha256 = Some(sha256_hex(b"baska"));
    let fresh = tempfile::tempdir().unwrap();
    let report = install(&fetcher(&fx.base), &fx.manifest, fresh.path()).await.unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].reason.contains("w600k_r50.onnx"), "{}", report.failed[0].reason);
    assert!(!fresh.path().join("arcface_w600k_r50.onnx").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_download_resumes_with_range() {
    let fx = fixture().await;
    fx.files.lock().unwrap().cut_once.push("buffalo.zip".into());
    let tmp = tempfile::tempdir().unwrap();
    let report = install(&fetcher(&fx.base), &fx.manifest, tmp.path()).await.unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let zip_hits: Vec<_> = hits(&fx.files).into_iter().filter(|(n, _)| n == "buffalo.zip").collect();
    assert_eq!(zip_hits.len(), 2);
    assert_eq!(zip_hits[0].1, None);
    let range = zip_hits[1].1.clone().unwrap();
    assert!(range.starts_with("bytes=") && range != "bytes=0-", "{range}");
    assert_eq!(std::fs::read(tmp.path().join("scrfd_10g.onnx")).unwrap(), fx.det);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_lfs_hugging_face_file_and_cancel_are_reported() {
    let fx = fixture().await;
    fx.files.lock().unwrap().hf = serde_json::json!([{ "type": "file", "path": "visual/model.onnx", "size": 5 }]);
    let tmp = tempfile::tempdir().unwrap();
    let report = install(&fetcher(&fx.base), &fx.manifest, tmp.path()).await.unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].source, "clip");
    assert!(report.failed[0].reason.contains("LFS"), "{}", report.failed[0].reason);

    let cancel = Arc::new(AtomicBool::new(true));
    let fresh = tempfile::tempdir().unwrap();
    let r = fetcher(&fx.base).install(&fx.manifest, fresh.path(), quiet(), cancel.clone()).await;
    assert!(matches!(r, Err(AiError::Cancelled)), "{r:?}");
    assert!(cancel.load(Ordering::Relaxed));
}

// ───────────────────────────── harici ağırlıklı ONNX ─────────────────────────────

fn pb_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn pb_len(field: u64, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    pb_varint(&mut out, (field << 3) | 2);
    pb_varint(&mut out, body.len() as u64);
    out.extend_from_slice(body);
    out
}

fn pb_int(field: u64, v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    pb_varint(&mut out, field << 3);
    pb_varint(&mut out, v);
    out
}

/// `TensorProto`: ad (8), boyutlar (1), `data_location = EXTERNAL` (14) ve
/// `external_data` (13) girdileri.
fn tensor(name: &str, location: &str) -> Vec<u8> {
    let entry = |k: &str, v: &str| [pb_len(1, k.as_bytes()), pb_len(2, v.as_bytes())].concat();
    [
        pb_int(1, 512),
        pb_len(8, name.as_bytes()),
        pb_int(14, 1),
        pb_len(13, &entry("location", location)),
        pb_len(13, &entry("offset", "0")),
    ]
    .concat()
}

/// `ModelProto`: ir_version (1), graph (7) içinde düğüm (1) ve başlatıcılar (5), opset (8).
fn onnx_graph(locations: &[&str]) -> Vec<u8> {
    let mut graph = pb_len(1, b"\x0a\x01x\x12\x01y\x22\x06MatMul");
    for (i, loc) in locations.iter().enumerate() {
        graph.extend(pb_len(5, &tensor(&format!("t{i}"), loc)));
    }
    [pb_int(1, 8), pb_len(7, &graph), pb_len(8, &pb_int(2, 17))].concat()
}

struct TextFixture {
    files: Shared,
    base: String,
    graph: Vec<u8>,
    big: Vec<u8>,
    small: Vec<u8>,
    manifest: Manifest,
}

/// Grafik + iki ağırlık: biri LFS'te (SHA-256), biri düz git dosyası (SHA-1).
/// Dizin listesi iki sayfa.
async fn text_fixture() -> TextFixture {
    let big = blob(7, 400_000);
    let small = blob(8, 2_048);
    // `w_big` iki tensörde: konumlar tekilleştirilir.
    let graph = onnx_graph(&["w_big", "bias.small", "w_big"]);
    let files: Shared = Arc::new(Mutex::new(Files::default()));
    {
        let mut f = files.lock().unwrap();
        f.bytes.insert("hf:textual/model.onnx".into(), graph.clone());
        f.bytes.insert("hf:textual/w_big".into(), big.clone());
        f.bytes.insert("hf:textual/bias.small".into(), small.clone());
        let lfs = |path: &str, b: &[u8]| {
            serde_json::json!({ "type": "file", "path": path, "size": 134,
                "lfs": { "oid": sha256_hex(b), "size": b.len(), "pointerSize": 134 } })
        };
        f.hf_pages.insert(
            "textual".into(),
            vec![
                serde_json::json!([lfs("textual/model.onnx", &graph), { "type": "file", "path": "textual/config.json", "size": 2, "oid": "0".repeat(40) }]),
                serde_json::json!([
                    lfs("textual/w_big", &big),
                    { "type": "file", "path": "textual/bias.small", "size": small.len(), "oid": hex(&git_blob_sha1(&small)) },
                    { "type": "directory", "path": "textual/sub" }
                ]),
            ],
        );
    }
    let base = server(files.clone()).await;
    let json = serde_json::json!({
        "version": 1,
        "sources": [
            { "id": "text", "url": format!("{base}/org/clip/resolve/main/textual/model.onnx"),
              "hf": { "repo": "org/clip", "revision": "main", "path": "textual/model.onnx" } }
        ],
        "files": [
            { "file": "mclip_text/model.onnx", "role": "clip_text", "dim": 512, "source": "text", "external_data": true }
        ]
    });
    let manifest = Manifest::parse(&json.to_string()).unwrap();
    TextFixture { files, base, graph, big, small, manifest }
}

#[test]
fn onnx_external_locations_are_read_from_the_graph() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("m.onnx");
    std::fs::write(&p, onnx_graph(&["a.weight", "onnx__MatMul_1", "a.weight"])).unwrap();
    assert_eq!(onnx_external_locations(&p).unwrap(), ["a.weight", "onnx__MatMul_1"]);
    std::fs::write(&p, onnx_graph(&[])).unwrap();
    assert!(onnx_external_locations(&p).unwrap().is_empty());
    // Grafik dizinin dışına yazdıramaz; kesik dosya hata verir.
    std::fs::write(&p, onnx_graph(&["../../.bashrc"])).unwrap();
    assert!(onnx_external_locations(&p).is_err());
    let g = onnx_graph(&["a"]);
    std::fs::write(&p, &g[..g.len() - 3]).unwrap();
    assert!(onnx_external_locations(&p).is_err());
}

#[test]
fn external_data_requires_hugging_face_and_a_subdirectory() {
    let good = Manifest::builtin().unwrap();
    let i = good.files.iter().position(|f| f.external_data).unwrap();
    for edit in [
        |f: &mut loupe_ai::models::ModelFile| f.file = "mclip_text.onnx".into(),
        |f: &mut loupe_ai::models::ModelFile| f.file = "a/b/c.onnx".into(),
        |f: &mut loupe_ai::models::ModelFile| f.file = "../c/model.onnx".into(),
        |f: &mut loupe_ai::models::ModelFile| f.source = "insightface-buffalo_l".into(),
    ] {
        let mut m = good.clone();
        edit(&mut m.files[i]);
        assert!(Manifest::parse(&serde_json::to_string(&m).unwrap()).is_err(), "{:?}", m.files[i]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_weights_are_installed_next_to_the_graph_and_completed_later() {
    let fx = text_fixture().await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let report = install(&fetcher(&fx.base), &fx.manifest, dir).await.unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let sub = dir.join("mclip_text");
    assert_eq!(std::fs::read(sub.join("model.onnx")).unwrap(), fx.graph);
    assert_eq!(std::fs::read(sub.join("w_big")).unwrap(), fx.big);
    assert_eq!(std::fs::read(sub.join("bias.small")).unwrap(), fx.small);
    let st = status(&fx.manifest, dir).unwrap();
    assert_eq!(st[0].state, FileState::Verified);
    assert_eq!(st[0].size, Some((fx.graph.len() + fx.big.len() + fx.small.len()) as u64));
    let lock = Lock::load(dir);
    let ext = lock.external("text").unwrap();
    assert_eq!(ext.keys().collect::<Vec<_>>(), ["bias.small", "w_big"]);
    assert_eq!(ext["w_big"].sha256.as_deref(), Some(sha256_hex(&fx.big).as_str()));
    assert_eq!(ext["bias.small"].git_sha1.as_deref(), Some(hex(&git_blob_sha1(&fx.small)).as_str()));
    assert_eq!(std::fs::read_dir(dir.join(PARTIAL_DIR)).unwrap().count(), 0);

    // Ağırlığı eksik grafik "ağırlık eksik": kurulum yalnızca eksiği indirir.
    std::fs::remove_file(sub.join("w_big")).unwrap();
    assert_eq!(status(&fx.manifest, dir).unwrap()[0].state, FileState::Incomplete);
    let before = hits(&fx.files);
    let again = install(&fetcher(&fx.base), &fx.manifest, dir).await.unwrap();
    assert!(again.failed.is_empty(), "{:?}", again.failed);
    let new: Vec<_> = hits(&fx.files)[before.len()..].iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(new, ["hf:textual/w_big"]);
    assert_eq!(status(&fx.manifest, dir).unwrap()[0].state, FileState::Verified);

    // Aynı boyda ama değişmiş küçük ağırlık yakalanır ve yenilenir.
    let mut b = fx.small.clone();
    b[0] ^= 1;
    std::fs::write(sub.join("bias.small"), &b).unwrap();
    assert_eq!(status(&fx.manifest, dir).unwrap()[0].state, FileState::Incomplete);
    install(&fetcher(&fx.base), &fx.manifest, dir).await.unwrap();
    assert_eq!(std::fs::read(sub.join("bias.small")).unwrap(), fx.small);

    // Önceki sürümün kilidi (ağırlık listesi yok): grafik yeniden indirilmeden liste çözülür.
    let mut lock = Lock::load(dir);
    lock.sources.get_mut("text").unwrap().external = None;
    lock.save(dir).unwrap();
    assert_eq!(status(&fx.manifest, dir).unwrap()[0].state, FileState::Incomplete);
    let before = hits(&fx.files).len();
    install(&fetcher(&fx.base), &fx.manifest, dir).await.unwrap();
    assert!(hits(&fx.files)[before..].is_empty(), "doğrulanmış dosyalar yeniden indirilmez");
    assert_eq!(status(&fx.manifest, dir).unwrap()[0].state, FileState::Verified);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_or_missing_external_weight_is_reported_and_model_stays_incomplete() {
    let fx = text_fixture().await;
    fx.files.lock().unwrap().bytes.get_mut("hf:textual/bias.small").unwrap()[5] ^= 1;
    let tmp = tempfile::tempdir().unwrap();
    let report = install(&fetcher(&fx.base), &fx.manifest, tmp.path()).await.unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].reason.contains("bias.small"), "{}", report.failed[0].reason);
    assert!(!tmp.path().join("mclip_text/bias.small").exists());
    assert_eq!(status(&fx.manifest, tmp.path()).unwrap()[0].state, FileState::Incomplete);

    // Grafiğin istediği dosya depoda yok.
    let fx = text_fixture().await;
    fx.files.lock().unwrap().hf_pages.get_mut("textual").unwrap()[1] = serde_json::json!([{ "type": "file", "path": "textual/w_big", "size": 1, "lfs": { "oid": sha256_hex(&fx.big), "size": fx.big.len() } }]);
    let fresh = tempfile::tempdir().unwrap();
    let report = install(&fetcher(&fx.base), &fx.manifest, fresh.path()).await.unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].reason.contains("depoda yok"), "{}", report.failed[0].reason);
}
