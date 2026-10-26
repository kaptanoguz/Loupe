#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Uçtan uca kurulum gerektirmeyen telefon aktarımı (RFC-001G): QR adresi →
//! belirteç → gömülü sayfa → tarayıcı oturumu → ön kontrol → parçalı tus
//! yüklemesi (kopma ve HEAD ile devam) → kütüphaneye atomik giriş.
//!
//! Telefon tarayıcısı burada sertifikasız bir reqwest istemcisidir: sertifika
//! uyarısını kabul etmiş tarayıcı gibi yalnızca sunucu sertifikasına güvenir
//! ve kimliğini `Authorization: Bearer` ya da çerezle bildirir.

use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use loupe_core::{ImportMode, LibraryId, LibraryKind, MediaId};
use loupe_db::models::NewLibrary;
use loupe_db::repo::{library, media};
use loupe_db::{Pool, PoolConfig};
use loupe_lan::web::{CSP, PrecheckItem, PrecheckResponse, SESSION_COOKIE, WebPairRequest, WebSession};
use loupe_lan::{
    AutoApprove, LanConfig, LanServer, ManualClock, PairingPayload, SERVER_NAME, ServerIdentity, ServerPin, WebCa,
    client_config,
};
use loupe_storage::{Importer, ImporterConfig, LibraryMarker};
use reqwest::{Client, RequestBuilder, StatusCode};
use tempfile::TempDir;

static NOW: std::sync::LazyLock<i64> = std::sync::LazyLock::new(loupe_core::now_ms);
const KIB: usize = 1024;

struct Desk {
    _tmp: TempDir,
    root: PathBuf,
    pool: Arc<Pool>,
    server: LanServer,
    _lib: LibraryId,
}

impl Desk {
    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    async fn start_with(tune: impl FnOnce(&mut LanConfig)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Fotograflar");
        std::fs::create_dir_all(&root).unwrap();
        let marker = LibraryMarker::new("Fotoğraflar");
        marker.write(&root).unwrap();
        let pool = Arc::new(Pool::open(&tmp.path().join("catalog.db"), PoolConfig::default()).unwrap());
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
        let importer = Importer::open(&root, ImporterConfig::new(lib)).unwrap();
        let mut cfg = LanConfig::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0), lib, "Oğuz-Masaüstü");
        tune(&mut cfg);
        let server = LanServer::start(
            cfg,
            pool.clone(),
            ServerIdentity::generate("Oğuz-Masaüstü", *NOW).unwrap(),
            importer,
            Arc::new(ManualClock::new(*NOW)),
            Arc::new(AutoApprove),
        )
        .await
        .unwrap();
        Self { _tmp: tmp, root, pool, server, _lib: lib }
    }

    fn qr(&self) -> PairingPayload {
        self.server.pairing_payload(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap()
    }
}

/// Telefon tarayıcısı: kamera QR adresini açtı, kullanıcı sertifika uyarısını geçti.
struct Browser {
    http: Client,
    origin: String,
    key: Option<String>,
}

impl Browser {
    /// QR'daki `https://…/m#t=…` adresini açar; parça istek satırına girmez.
    fn open(url: &str, fp: [u8; 32]) -> (Self, String) {
        let (page, fragment) = url.split_once('#').unwrap();
        let origin = page.strip_suffix("/m").unwrap().to_owned();
        let token = fragment.strip_prefix("t=").unwrap().to_owned();
        let http =
            Client::builder().use_preconfigured_tls(client_config(ServerPin::Leaf(fp), None).unwrap()).build().unwrap();
        (Self { http, origin, key: None }, token)
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.auth(self.http.get(format!("{}{path}", self.origin)))
    }

    fn post(&self, path: &str) -> RequestBuilder {
        self.auth(self.http.post(format!("{}{path}", self.origin)))
    }

    fn auth(&self, r: RequestBuilder) -> RequestBuilder {
        match &self.key {
            Some(k) => r.bearer_auth(k),
            None => r,
        }
    }

    async fn pair(&self, token: &str) -> reqwest::Response {
        self.http
            .post(format!("{}/m/session", self.origin))
            .json(&WebPairRequest {
                token: token.into(),
                device_name: "SM-S918B (Chrome)".into(),
                platform: "android".into(),
            })
            .send()
            .await
            .unwrap()
    }

    /// Sayfadaki `Upload-Metadata` ile aynı alanlar.
    async fn create(&self, name: &str, bytes: &[u8], modified: i64, resolution: Option<&str>) -> reqwest::Response {
        let mut md = vec![
            format!("filename {}", STANDARD.encode(name)),
            format!("device_asset_id {}", STANDARD.encode(format!("web:{name}"))),
            format!("blake3 {}", STANDARD.encode(blake3::hash(bytes).to_hex().as_str())),
            format!("modified_at {}", STANDARD.encode(modified.to_string())),
        ];
        if let Some(r) = resolution {
            md.push(format!("resolution {}", STANDARD.encode(r)));
        }
        self.post("/files")
            .header("Tus-Resumable", "1.0.0")
            .header("Upload-Length", bytes.len().to_string())
            .header("Upload-Metadata", md.join(","))
            .send()
            .await
            .unwrap()
    }

    fn patch(&self, loc: &str, offset: usize, body: impl Into<reqwest::Body>) -> RequestBuilder {
        self.auth(self.http.patch(format!("{}{loc}", self.origin)))
            .header("Tus-Resumable", "1.0.0")
            .header("Content-Type", "application/offset+octet-stream")
            .header("Upload-Offset", offset.to_string())
            .body(body)
    }

    async fn head(&self, loc: &str) -> usize {
        let res = self
            .auth(self.http.head(format!("{}{loc}", self.origin)))
            .header("Tus-Resumable", "1.0.0")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        header(&res, "upload-offset").parse().unwrap()
    }

    async fn precheck(&self, items: &[(&str, i64)]) -> Vec<String> {
        let body: Vec<PrecheckItem> =
            items.iter().map(|(n, m)| PrecheckItem { device_asset_id: format!("web:{n}"), modified_at: *m }).collect();
        let res = self.post("/m/precheck").json(&body).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        res.json::<PrecheckResponse>().await.unwrap().known
    }
}

fn header(res: &reqwest::Response, name: &str) -> String {
    res.headers().get(name).unwrap_or_else(|| panic!("{name} başlığı yok")).to_str().unwrap().to_owned()
}

/// Gerçek (çözülebilir) JPEG, sonu dolguyla tam `size` bayt.
fn photo(seed: u32, size: usize) -> Vec<u8> {
    let img = image::RgbImage::from_fn(96, 64, |x, y| {
        let v = (x.wrapping_mul(seed) ^ y.wrapping_mul(29).wrapping_add(seed)) as u8;
        image::Rgb([v, v.wrapping_add(60), v.wrapping_add(120)])
    });
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    let mut bytes = out.into_inner();
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    while bytes.len() < size {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        bytes.push(x as u8);
    }
    bytes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn qr_url_opens_page_pairs_browser_and_tus_upload_lands_in_library() {
    let desk = Desk::start().await;
    let qr = desk.qr();
    let url = qr.web_url().unwrap();
    let port = desk.server.local_addr().port();
    assert!(url.starts_with(&format!("https://127.0.0.1:{port}/m#t=")), "{url}");
    let (mut phone, token) = Browser::open(&url, qr.server_cert_sha256);
    assert_eq!(token, qr.token);

    // 1. Sayfa ve kaynakları: kimlik istemez, sıkı CSP ile gelir.
    let page = phone.get("/m").send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(header(&page, "content-type"), "text/html; charset=utf-8");
    assert_eq!(header(&page, "content-security-policy"), CSP);
    assert_eq!(header(&page, "x-frame-options"), "DENY");
    let html = page.text().await.unwrap();
    assert!(html.contains("Fotoğraf / Video Seç"));
    assert!(html.contains(r#"accept="image/*,video/*""#));
    assert!(html.contains(r#"src="/m/app.js""#));
    for (path, ctype) in [
        ("/m/app.js", "text/javascript"),
        ("/m/core.js", "text/javascript"),
        ("/m/blake3.js", "text/javascript"),
        ("/m/app.css", "text/css"),
        ("/m/icon.svg", "image/svg+xml"),
    ] {
        let r = phone.get(path).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{path}");
        assert!(header(&r, "content-type").starts_with(ctype), "{path}");
    }
    let root = phone.get("/").send().await.unwrap();
    assert_eq!(root.url().path(), "/m", "kök adres sayfaya yönlenir");

    // 2. Eşleşmeden önce veri uçları kapalı.
    assert_eq!(phone.get("/m/session").send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let res = phone.post("/m/precheck").json(&Vec::<PrecheckItem>::new()).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(phone.create("x.jpg", b"x", 1, None).await.status(), StatusCode::UNAUTHORIZED);

    // 3. Belirteç → oturum anahtarı (gövdede ve HttpOnly çerezde).
    let res = phone.pair(&token).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let cookie = header(&res, "set-cookie");
    for attr in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(cookie.contains(attr), "{cookie}");
    }
    let session: WebSession = res.json().await.unwrap();
    let key = session.session_key.clone().unwrap();
    assert!(cookie.starts_with(&format!("{SESSION_COOKIE}={key};")));
    assert_eq!(session.server_name, "Oğuz-Masaüstü");

    // Belirteç tek kullanımlık: ikinci tarayıcı aynı QR ile eşleşemez.
    let (other, _) = Browser::open(&url, qr.server_cert_sha256);
    assert_eq!(other.pair(&token).await.status(), StatusCode::FORBIDDEN);

    // Anahtar DB'de saklanmaz; yalnızca parmak izi ve tür.
    let (kind, platform, fp): (String, String, Vec<u8>) = desk
        .pool
        .read(|c| {
            Ok(c.query_row(
                "SELECT kind, platform, cert_sha256 FROM devices WHERE id = ?1",
                [session.device_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!((kind.as_str(), platform.as_str()), ("web", "android"));
    assert_eq!(fp, loupe_lan::web::session_fingerprint(&key).to_vec());

    // Çerez tek başına da yeterli (localStorage silinmiş tarayıcı).
    let res = phone.get("/m/session").header("Cookie", format!("{SESSION_COOKIE}={key}")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    phone.key = Some(key);
    let who: WebSession = phone.get("/m/session").send().await.unwrap().json().await.unwrap();
    assert_eq!((who.device_id, who.session_key), (session.device_id, None));

    // 4. Ön kontrol: hiçbir şey bilinmiyor.
    let modified = *NOW - 86_400_000;
    let a = photo(3, 600 * KIB);
    assert!(phone.precheck(&[("IMG_20251025_101500.jpg", modified)]).await.is_empty());

    // 5. tus: 256 KiB parçalar; ikinci parçanın ortasında Wi-Fi kopar.
    let res = phone.create("IMG_20251025_101500.jpg", &a, modified, None).await;
    assert_eq!(res.status(), StatusCode::CREATED, "{:?}", res.text().await);
    let loc = header(&res, "location");
    assert_eq!(header(&res, "upload-offset"), "0");
    let chunk = 256 * KIB;
    let res = phone.patch(&loc, 0, a[..chunk].to_vec()).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&res, "upload-offset"), chunk.to_string());

    let broken = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(a[chunk..chunk + 100 * KIB].to_vec())),
        Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "wifi gitti")),
    ]);
    assert!(phone.patch(&loc, chunk, reqwest::Body::wrap_stream(broken)).send().await.is_err());

    // Sayfa HEAD ile kalınan yeri sorar: yalnızca onaylı baytlar.
    let mut offset = phone.head(&loc).await;
    assert_eq!(offset, chunk);
    let mut last = None;
    while offset < a.len() {
        let end = (offset + chunk).min(a.len());
        let res = phone.patch(&loc, offset, a[offset..end].to_vec()).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        offset = header(&res, "upload-offset").parse().unwrap();
        last = Some(res);
    }
    let last = last.unwrap();
    assert_eq!(header(&last, "loupe-sync-status"), "verified");
    let media_id = MediaId(header(&last, "loupe-media-id").parse().unwrap());

    // 6. Kütüphanede: bit bit aynı, LAN yerleşimi, cihaz varlığı kaydı.
    let row = desk.pool.read(|c| media::get(c, media_id)).unwrap().unwrap();
    assert_eq!(row.blake3.0, *blake3::hash(&a).as_bytes());
    let on_disk = std::fs::read(row.rel_path.to_path(&desk.root)).unwrap();
    assert_eq!(on_disk, a);
    assert!(row.rel_path.as_str().ends_with("/IMG_20251025_101500.jpg"), "{}", row.rel_path.as_str());

    // 7. Aynı dosya yeniden seçilirse özet bile hesaplanmadan atlanır.
    let known = phone.precheck(&[("IMG_20251025_101500.jpg", modified), ("IMG_20251025_101501.jpg", modified)]).await;
    assert_eq!(known, vec!["web:IMG_20251025_101500.jpg".to_owned()]);
    // Değişiklik zamanı farklıysa ön kontrol bilmez; sunucu özetle "zaten var" der.
    assert!(phone.precheck(&[("IMG_20251025_101500.jpg", modified + 1)]).await.is_empty());
    let res = phone.create("IMG_20251025_101500.jpg", &a, modified + 1, None).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header(&res, "loupe-sync-status"), "already-synced");

    // 8. Telefonda düzenlenmiş (aynı ad, farklı içerik): politika "sor" → 409,
    // sayfa kullanıcıya sorar ve kararı `resolution` ile yeniden gönderir.
    let edited = photo(4, 300 * KIB);
    let res = phone.create("IMG_20251025_101500.jpg", &edited, modified + 5, None).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(header(&res, "loupe-sync-status"), "conflict");
    let res = phone.create("IMG_20251025_101500.jpg", &edited, modified + 5, Some("keep_both")).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let loc = header(&res, "location");
    let res = phone.patch(&loc, 0, edited.clone()).send().await.unwrap();
    assert_eq!(header(&res, "loupe-sync-status"), "verified");
    let kept: i64 = desk
        .pool
        .read(|c| Ok(c.query_row("SELECT count(*) FROM media_files WHERE soft_deleted_at IS NULL", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(kept, 2, "ikisi de tutuldu");

    // 9. İptal: masaüstü erişimi kaldırınca aynı anahtar reddedilir.
    desk.server.revoke_device(session.device_id, "kayıp telefon").await.unwrap();
    assert_eq!(phone.get("/m/session").send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    assert_eq!(phone.create("IMG_2.jpg", b"y", 1, None).await.status(), StatusCode::UNAUTHORIZED);
    desk.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_token_burns_the_qr_and_is_rate_limited() {
    let desk = Desk::start().await;
    let qr = desk.qr();
    let (phone, token) = Browser::open(&qr.web_url().unwrap(), qr.server_cert_sha256);

    let res = phone.pair("yanlis-belirtec-yanlis-belirtec").await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!((body["error"].as_str(), body["message"].as_str()), (Some("PAIRING_FAILED"), Some("")));
    // Belirteç her denemede tüketilir: doğru belirteç de artık geçmez.
    assert_eq!(phone.pair(&token).await.status(), StatusCode::FORBIDDEN);

    // IP başına dakikada 5 deneme.
    for _ in 0..3 {
        assert_eq!(phone.pair("x").await.status(), StatusCode::FORBIDDEN);
    }
    assert_eq!(phone.pair("x").await.status(), StatusCode::TOO_MANY_REQUESTS);

    // Uydurma anahtar ve aşırı uzun anahtar.
    let mut fake = Browser::open(&qr.web_url().unwrap(), qr.server_cert_sha256).0;
    fake.key = Some(loupe_lan::web::new_session_key());
    assert_eq!(fake.get("/m/session").send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    fake.key = Some("a".repeat(10_000));
    assert_eq!(fake.get("/m/session").send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    desk.server.shutdown().await;
}

#[test]
fn web_url_prefers_private_ipv4_and_brackets_ipv6() {
    let mut p = PairingPayload {
        addrs: vec!["fd00::5".parse().unwrap(), "192.168.1.20".parse().unwrap()],
        port: 47_320,
        ca_spki_sha256: [1; 32],
        server_cert_sha256: [2; 32],
        token: "abc_DEF-123".into(),
        expires_at_s: 1,
        name: "Masaüstü".into(),
    };
    assert_eq!(p.web_url().unwrap(), "https://192.168.1.20:47320/m#t=abc_DEF-123");
    p.addrs = vec!["fd00::5".parse().unwrap()];
    assert_eq!(p.web_url().unwrap(), "https://[fd00::5]:47320/m#t=abc_DEF-123");
    p.addrs.clear();
    assert_eq!(p.web_url(), None);
}

/// Elle (ya da Playwright ile) gerçek tarayıcıda deneme: sunucuyu açar, QR
/// adresini yazar ve `LOUPE_SERVE_SECS` saniye (varsayılan 120) bekler.
/// `cargo test -p loupe-lan --test mobile -- --ignored --nocapture serve_for_browser`
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "elle çalıştırılır: gerçek tarayıcı için sunucu açar"]
async fn serve_for_browser() {
    let desk = Desk::start().await;
    let url = desk.qr().web_url().unwrap();
    println!("QR_URL {url}");
    println!("LIBRARY {}", desk.root.display());
    let secs = std::env::var("LOUPE_SERVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    let n: i64 = desk.pool.read(|c| Ok(c.query_row("SELECT count(*) FROM media_files", [], |r| r.get(0))?)).unwrap();
    println!("MEDIA_COUNT {n}");
    desk.server.shutdown().await;
}

/// iPhone Kestirmesi: profil kurulmuş telefon web CA'ya güvenir ve sunucuya
/// `<makine>.local` adıyla, sertifika uyarısı olmadan bağlanır. Uygulama
/// istemcisi SNI = `loupe.local` ile eşleşme CA'sının sertifikasını almaya devam eder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shortcut_uploads_whole_files_over_profile_trusted_tls() {
    const HOST: &str = "oguz-mint.local";
    let ca = Arc::new(WebCa::generate(*NOW).unwrap());
    let ca_for_cfg = ca.clone();
    let desk = Desk::start_with(move |cfg| {
        cfg.web_ca = Some(ca_for_cfg);
        cfg.web_hosts = vec![HOST.to_owned()];
    })
    .await;
    let addr = desk.server.local_addr();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.cert().clone()).unwrap();
    let mut tls = rustls::ClientConfig::builder_with_provider(loupe_lan::tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let iphone = Client::builder().use_preconfigured_tls(tls).no_proxy().resolve(HOST, addr).build().unwrap();
    let origin = format!("https://{HOST}:{}", addr.port());

    // Profil herkese açıktır (gizli bilgi içermez) ve CA'yı taşır.
    let res = iphone.get(format!("{origin}/m/loupe.mobileconfig")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header(&res, "content-type"), "application/x-apple-aspen-config");
    assert!(res.text().await.unwrap().contains(&STANDARD.encode(ca.cert().as_ref())));

    // Eşleşmiş tarayıcı Kestirme için ayrı anahtar alır.
    let qr = desk.qr();
    let session: WebSession = iphone
        .post(format!("{origin}/m/session"))
        .json(&WebPairRequest {
            token: qr.token.clone(),
            device_name: "iPhone (Safari)".into(),
            platform: "ios".into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let browser_key = session.session_key.unwrap();
    assert_eq!(iphone.post(format!("{origin}/m/shortcut")).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let sc: serde_json::Value = iphone
        .post(format!("{origin}/m/shortcut"))
        .bearer_auth(&browser_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sc["path"], "/s/upload");
    assert_eq!(sc["host"], HOST);
    let key = sc["key"].as_str().unwrap().to_owned();
    assert_ne!(key, browser_key);

    // Gövde = dosyanın kendisi; ad yoksa içerikten türetilir.
    let jpeg = photo(9, 200 * KIB);
    let upload = |body: Vec<u8>, name: Option<&str>| {
        let mut r = iphone.post(format!("{origin}/s/upload")).bearer_auth(&key).body(body);
        if let Some(n) = name {
            r = r.header("x-loupe-name", n);
        }
        r.send()
    };
    let res = upload(jpeg.clone(), None).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let saved: serde_json::Value = res.json().await.unwrap();
    assert_eq!(saved["status"], "saved");
    let id = MediaId(saved["media_id"].as_i64().unwrap());
    let m = desk.pool.read(|c| media::get(c, id)).unwrap().unwrap();
    assert!(
        m.original_filename.starts_with("IMG_") && m.original_filename.ends_with(".jpg"),
        "{}",
        m.original_filename
    );
    assert!(desk.root.join(m.rel_path.as_str()).is_file());

    // Albüm yeniden gönderilirse aynı içerik atlanır.
    let again: serde_json::Value = upload(jpeg, Some("IMG_0001.JPG")).await.unwrap().json().await.unwrap();
    assert_eq!((again["status"].as_str(), again["media_id"].as_i64()), (Some("duplicate"), Some(id.0)));
    let named: serde_json::Value =
        upload(photo(10, 50 * KIB), Some("IMG_0002.HEIC.jpg")).await.unwrap().json().await.unwrap();
    let m = desk.pool.read(|c| media::get(c, MediaId(named["media_id"].as_i64().unwrap()))).unwrap().unwrap();
    assert_eq!(m.original_filename, "IMG_0002.HEIC.jpg");

    // Anahtarsız ya da boş gövde reddedilir.
    let res = iphone.post(format!("{origin}/s/upload")).body(vec![1u8, 2, 3]).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(upload(Vec::new(), None).await.unwrap().status(), StatusCode::BAD_REQUEST);

    // Uygulama istemcisi (SNI = loupe.local) ve IP ile gelen QR tarayıcısı
    // eşleşme CA'sının sertifikasını almaya devam eder.
    let app = Client::builder()
        .use_preconfigured_tls(client_config(ServerPin::Leaf(qr.server_cert_sha256), None).unwrap())
        .no_proxy()
        .resolve(SERVER_NAME, addr)
        .build()
        .unwrap();
    for url in [format!("https://{SERVER_NAME}:{}/m", addr.port()), format!("https://127.0.0.1:{}/m", addr.port())] {
        assert_eq!(app.get(&url).send().await.unwrap().status(), StatusCode::OK, "{url}");
    }
    desk.server.shutdown().await;
}
