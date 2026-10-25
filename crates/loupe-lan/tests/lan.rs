#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Uçtan uca LAN senkronizasyonu: gerçek TLS 1.3 + mTLS, gerçek HTTP (reqwest),
//! gerçek SQLite katalog ve atomik importer. Telefon tarafı bu dosyadaki
//! [`Phone`] ile taklit edilir (CSR üretir, sertifikayı saklar, tus konuşur).

use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use loupe_core::{Blake3Hash, ImportMode, LibraryId, LibraryKind, MediaId};
use loupe_db::models::NewLibrary;
use loupe_db::repo::{library, media};
use loupe_db::{Pool, PoolConfig};
use loupe_lan::server::{PairRequest, PairResponse};
use loupe_lan::store;
use loupe_lan::tus::UploadMeta;
use loupe_lan::{
    AutoApprove, ConflictPolicy, Discovery, LanConfig, LanServer, ManualClock, MemoryDiscovery, PAIRING_TTL,
    PairingPayload, Resolution, ServerIdentity, ServerPin, ServiceAd, client_config,
};
use loupe_storage::{Importer, ImporterConfig, LibraryMarker};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use reqwest::{Client, StatusCode};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tempfile::TempDir;

/// Sertifikalar gerçek saatle doğrulanır (rustls): test zamanı da gerçek saatten başlar.
static NOW: std::sync::LazyLock<i64> = std::sync::LazyLock::new(loupe_core::now_ms);
const MIB: usize = 1024 * 1024;

struct Desk {
    _tmp: TempDir,
    root: PathBuf,
    pool: Arc<Pool>,
    clock: Arc<ManualClock>,
    server: LanServer,
    lib: LibraryId,
}

impl Desk {
    async fn start() -> Self {
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
        let clock = Arc::new(ManualClock::new(*NOW));
        let identity = ServerIdentity::generate("Oğuz-Masaüstü", *NOW).unwrap();
        let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let server = LanServer::start(
            LanConfig::new(bind, lib, "Oğuz-Masaüstü"),
            pool.clone(),
            identity,
            importer,
            clock.clone(),
            Arc::new(AutoApprove),
        )
        .await
        .unwrap();
        Self { _tmp: tmp, root, pool, clock, server, lib }
    }

    fn qr(&self) -> String {
        self.server.pairing_payload(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]).unwrap().to_uri()
    }
}

/// Telefon: anahtarı kendisinde üretir, yalnızca CSR gönderir.
struct Phone {
    key: KeyPair,
    base: String,
    paired: Option<PairResponse>,
}

impl Phone {
    fn from_qr(uri: &str) -> (Self, PairingPayload) {
        let p = PairingPayload::parse(uri).unwrap();
        let base = format!("https://{}:{}", p.addrs[0], p.port);
        (Self { key: KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap(), base, paired: None }, p)
    }

    fn csr(&self) -> String {
        CertificateParams::default().serialize_request(&self.key).unwrap().pem().unwrap()
    }

    fn pairing_client(fp: [u8; 32]) -> Client {
        Client::builder().use_preconfigured_tls(client_config(ServerPin::Leaf(fp), None).unwrap()).build().unwrap()
    }

    /// Eşleşmeden sonraki her bağlantı: CA sabitli + istemci sertifikası.
    fn mtls(&self) -> Client {
        let r = self.paired.as_ref().unwrap();
        let ca = CertificateDer::from_pem_slice(r.ca_cert_pem.as_bytes()).unwrap();
        let cert = CertificateDer::from_pem_slice(r.client_cert_pem.as_bytes()).unwrap();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.serialize_der()));
        Client::builder()
            .use_preconfigured_tls(client_config(ServerPin::Ca(ca), Some((vec![cert], key))).unwrap())
            .build()
            .unwrap()
    }

    /// Sertifikasız ama sunucuyu doğrulayan istemci.
    fn anonymous(&self) -> Client {
        let r = self.paired.as_ref().unwrap();
        let ca = CertificateDer::from_pem_slice(r.ca_cert_pem.as_bytes()).unwrap();
        Client::builder().use_preconfigured_tls(client_config(ServerPin::Ca(ca), None).unwrap()).build().unwrap()
    }

    async fn pair(&self, payload: &PairingPayload) -> reqwest::Response {
        Self::pairing_client(payload.server_cert_sha256)
            .post(format!("{}/pair", self.base))
            .json(&PairRequest {
                token: payload.token.clone(),
                csr_pem: self.csr(),
                device_name: "Pixel 9".into(),
                platform: "android".into(),
            })
            .send()
            .await
            .unwrap()
    }

    async fn paired(desk: &Desk) -> Self {
        let (mut phone, payload) = Self::from_qr(&desk.qr());
        let res = phone.pair(&payload).await;
        assert_eq!(res.status(), StatusCode::CREATED);
        phone.paired = Some(res.json().await.unwrap());
        phone
    }

    fn device_id(&self) -> i64 {
        self.paired.as_ref().unwrap().device_id
    }

    async fn create(&self, c: &Client, meta: &UploadMeta) -> reqwest::Response {
        c.post(format!("{}/files", self.base))
            .header("Tus-Resumable", "1.0.0")
            .header("Upload-Length", meta.size.to_string())
            .header("Upload-Metadata", meta.to_tus_metadata())
            .send()
            .await
            .unwrap()
    }

    async fn patch(
        &self,
        c: &Client,
        location: &str,
        offset: usize,
        body: impl Into<reqwest::Body>,
    ) -> reqwest::Response {
        c.patch(format!("{}{location}", self.base))
            .header("Tus-Resumable", "1.0.0")
            .header("Content-Type", "application/offset+octet-stream")
            .header("Upload-Offset", offset.to_string())
            .body(body)
            .send()
            .await
            .unwrap()
    }

    /// Tek parçada tam yükleme; oluşan medya kimliği.
    async fn upload_all(&self, c: &Client, meta: &UploadMeta, bytes: &[u8]) -> MediaId {
        let res = self.create(c, meta).await;
        assert_eq!(res.status(), StatusCode::CREATED, "{:?}", res.text().await);
        let loc = header(&res, "location");
        let res = self.patch(c, &loc, 0, bytes.to_vec()).await;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(header(&res, "loupe-sync-status"), "verified");
        MediaId(header(&res, "loupe-media-id").parse().unwrap())
    }
}

fn header(res: &reqwest::Response, name: &str) -> String {
    res.headers().get(name).unwrap_or_else(|| panic!("{name} başlığı yok")).to_str().unwrap().to_owned()
}

/// Gerçek (çözülebilir) JPEG, EOI'den sonra dolguyla tam `size` bayt.
fn photo(seed: u32, size: usize) -> Vec<u8> {
    let img = image::RgbImage::from_fn(128, 96, |x, y| {
        let v = (x.wrapping_mul(seed) ^ y.wrapping_mul(31).wrapping_add(seed)) as u8;
        image::Rgb([v, v.wrapping_add(85), v.wrapping_add(170)])
    });
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    let mut bytes = out.into_inner();
    assert!(bytes.len() < size);
    let mut x = seed.wrapping_mul(2_654_435_761);
    while bytes.len() < size {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        bytes.push(x as u8);
    }
    bytes
}

fn meta(name: &str, asset: &str, bytes: &[u8]) -> UploadMeta {
    UploadMeta {
        filename: name.into(),
        device_asset_id: asset.into(),
        blake3: Blake3Hash(*blake3::hash(bytes).as_bytes()),
        size: bytes.len() as u64,
        modified_at_ms: *NOW - 3_600_000,
        taken_at_ms: Some(*NOW - 7_200_000),
        resolution: None,
    }
}

fn blake3_of(path: &Path) -> [u8; 32] {
    *blake3::hash(&std::fs::read(path).unwrap()).as_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn qr_pairing_issues_client_cert_and_rejects_replay_and_expired_tokens() {
    let desk = Desk::start().await;

    // Telefon servisi mDNS ile bulur (testte bellek içi ağ) ve CA'yla eşleştirir.
    let net = MemoryDiscovery::default();
    let identity = desk.server.identity();
    net.advertise(&ServiceAd::for_server(
        &identity,
        "Oğuz-Masaüstü",
        desk.server.local_addr().port(),
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
    ))
    .unwrap();

    let qr = desk.qr();
    assert!(qr.starts_with("loupe://pair?v=1&a=127.0.0.1&p="));
    let (mut phone, payload) = Phone::from_qr(&qr);
    assert_eq!(payload.server_cert_sha256, identity.server_cert_sha256());

    // Yanlış parmak izi: TLS el sıkışması başarısız, belirteç hiç gönderilmez.
    let wrong = Phone::pairing_client([0; 32]).post(format!("{}/pair", phone.base)).body("{}").send().await;
    assert!(wrong.is_err(), "sabitlenen parmak izi tutmayan sunucuya bağlanılmamalı");

    let res = phone.pair(&payload).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let paired: PairResponse = res.json().await.unwrap();
    assert_eq!(paired.install_id, identity.install_id());

    // Cihaz kaydı: parmak izi, verilen sertifikanınkiyle aynı.
    let cert = CertificateDer::from_pem_slice(paired.client_cert_pem.as_bytes()).unwrap();
    let stored: Vec<u8> = desk
        .pool
        .read(|c| Ok(c.query_row("SELECT cert_sha256 FROM devices WHERE id = ?1", [paired.device_id], |r| r.get(0))?))
        .unwrap();
    assert_eq!(stored, loupe_lan::identity::sha256(&cert).to_vec());
    phone.paired = Some(paired);

    // Replay: aynı QR ikinci kez → 403, hangi adımın başarısız olduğu söylenmez.
    let (other, _) = Phone::from_qr(&qr);
    let res = other.pair(&payload).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"], "PAIRING_FAILED");
    assert_eq!(body["message"], "");

    // Süresi dolmuş belirteç → 403.
    let fresh = desk.qr();
    let (late, late_payload) = Phone::from_qr(&fresh);
    desk.clock.advance(PAIRING_TTL + Duration::from_secs(1));
    assert_eq!(late.pair(&late_payload).await.status(), StatusCode::FORBIDDEN);

    // Telefon artık mDNS'te kendi CA'sını taşıyan servisi seçer ve yalnızca mTLS ile konuşur.
    let found = net.browse(Duration::from_millis(50)).unwrap();
    let svc = found.iter().find(|s| s.matches_ca(&identity.ca_spki_sha256())).unwrap();
    assert_eq!(svc.port, desk.server.local_addr().port());

    let ok = phone.mtls().request(reqwest::Method::OPTIONS, format!("{}/files", phone.base)).send().await.unwrap();
    assert_eq!(ok.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&ok, "tus-version"), "1.0.0");
    assert!(header(&ok, "tus-extension").contains("checksum"));

    // Sertifikasız istemci eşleşme dışındaki uç noktalara giremez.
    let m = meta("IMG_0001.JPG", "asset-1", b"abc");
    let res = phone.create(&phone.anonymous(), &m).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // İptal: bir sonraki el sıkışma reddedilir (yeni bağlantı).
    desk.server.revoke_device(phone.device_id(), "kayıp telefon").await.unwrap();
    let after = phone.mtls().request(reqwest::Method::OPTIONS, format!("{}/files", phone.base)).send().await;
    let rejected = match after {
        Err(_) => true,
        Ok(r) => r.status() == StatusCode::UNAUTHORIZED,
    };
    assert!(rejected, "iptal edilen cihaz bağlanamamalı");
    desk.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_resumes_after_disconnect_and_lands_verified_in_library() {
    let desk = Desk::start().await;
    let phone = Phone::paired(&desk).await;
    let telemetry = desk.server.telemetry();
    let watch = telemetry.subscribe();

    let bytes = photo(7, 2 * MIB);
    let m = meta("IMG_0412.JPG", "MediaStore:412", &bytes);
    let c = phone.mtls();
    let res = phone.create(&c, &m).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let loc = header(&res, "location");
    assert!(loc.starts_with("/files/") && loc.len() == "/files/".len() + 22, "{loc}");
    assert_eq!(header(&res, "upload-offset"), "0");

    // İlk 1 MiB, parça checksum'ıyla.
    let first = bytes[..MIB].to_vec();
    let sum = STANDARD.encode(blake3::hash(&first).as_bytes());
    let res = c
        .patch(format!("{}{loc}", phone.base))
        .header("Tus-Resumable", "1.0.0")
        .header("Content-Type", "application/offset+octet-stream")
        .header("Upload-Offset", "0")
        .header("Upload-Checksum", format!("blake3 {sum}"))
        .body(first)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&res, "upload-offset"), "1048576");

    // Wi-Fi koptu: ikinci parçanın ortasında akış hata verir, bağlantı düşer.
    let tail = bytes[MIB..MIB + 256 * 1024].to_vec();
    let broken = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(tail)),
        Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "wifi gitti")),
    ]);
    let cut = c
        .patch(format!("{}{loc}", phone.base))
        .header("Tus-Resumable", "1.0.0")
        .header("Content-Type", "application/offset+octet-stream")
        .header("Upload-Offset", "1048576")
        .body(reqwest::Body::wrap_stream(broken))
        .send()
        .await;
    assert!(cut.is_err());
    drop(c);

    // Yeni bağlantı: sunucu yalnızca fsync edilmiş, onaylı baytları bildirir.
    let c = phone.mtls();
    let res = c.head(format!("{}{loc}", phone.base)).header("Tus-Resumable", "1.0.0").send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header(&res, "upload-offset"), "1048576");
    assert_eq!(header(&res, "upload-length"), "2097152");
    assert_eq!(header(&res, "cache-control"), "no-store");

    // Yanlış offset reddedilir.
    let res = phone.patch(&c, &loc, 0, vec![0u8; 10]).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // Kalan 1 MiB → tam dosya BLAKE3 doğrulanır, kütüphaneye girer.
    let res = phone.patch(&c, &loc, MIB, bytes[MIB..].to_vec()).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&res, "upload-offset"), "2097152");
    assert_eq!(header(&res, "loupe-sync-status"), "verified");
    let media_id = MediaId(header(&res, "loupe-media-id").parse().unwrap());

    let row = desk.pool.read(|c| media::get(c, media_id)).unwrap().unwrap();
    assert_eq!(row.blake3.0, *blake3::hash(&bytes).as_bytes());
    assert_eq!(row.size_bytes, 2 * MIB as u64);
    let on_disk = row.rel_path.to_path(&desk.root);
    assert_eq!(blake3_of(&on_disk), row.blake3.0, "kütüphanedeki dosya bit bit aynı");
    assert!(row.rel_path.as_str().ends_with("/IMG_0412.JPG"), "{}", row.rel_path.as_str());
    let (placement, state, asset_media): (String, String, i64) = desk
        .pool
        .read(|c| {
            Ok(c.query_row(
                "SELECT m.placement, t.state, a.media_id FROM media_files m
                   JOIN lan_transfers t ON t.result_media_id = m.id
                   JOIN device_assets a ON a.media_id = m.id AND a.client_asset_id = 'MediaStore:412'
                  WHERE m.id = ?1",
                [media_id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!((placement.as_str(), state.as_str(), asset_media), ("lan", "done", media_id.0));
    let staging = desk.root.join(".loupe/lan-staging");
    assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0, "staging temizlendi");

    // Canlı durum: 2 MiB alındı (kopan parçanın baytları düşüldü), 1 dosya doğrulandı.
    let s = *watch.borrow();
    assert_eq!((s.bytes_received, s.verified_files, s.active_uploads), (2 * MIB as u64, 1, 0));
    assert_eq!(s.eta_secs, Some(0));

    // Telefon aynı varlığı yeniden gönderirse ağdan geçmez.
    let res = phone.create(&c, &m).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header(&res, "loupe-sync-status"), "already-synced");
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["media_id"], media_id.0);
    assert_eq!(telemetry.snapshot().already_synced, 1);

    // Biten oturuma yazılamaz.
    let res = phone.patch(&c, &loc, 2 * MIB, vec![1u8]).await;
    assert_eq!(res.status(), StatusCode::GONE);
    desk.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edited_on_phone_conflict_follows_device_policy() {
    let desk = Desk::start().await;
    let phone = Phone::paired(&desk).await;
    let c = phone.mtls();
    let asset = "PHAsset:0F3A-41";

    let v1 = photo(11, 96 * 1024);
    let m1 = meta("IMG_7001.JPG", asset, &v1);
    let first = phone.upload_all(&c, &m1, &v1).await;

    // Aynı kimlik + aynı BLAKE3: 200 Already Synced.
    let res = phone.create(&c, &m1).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header(&res, "loupe-sync-status"), "already-synced");

    // Telefonda düzenlendi (kırpma): aynı kimlik, farklı BLAKE3. Varsayılan politika "sor".
    let v2 = photo(12, 90 * 1024);
    let mut m2 = meta("IMG_7001.JPG", asset, &v2);
    let res = phone.create(&c, &m2).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(header(&res, "loupe-sync-status"), "conflict");
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["previous_media_id"], first.0);

    // Kullanıcı "ikisini de tut" der: yenisi `_1` ekiyle, ikisi aynı yakın kopya grubunda.
    m2.resolution = Some(Resolution::KeepBoth);
    let second = phone.upload_all(&c, &m2, &v2).await;
    let (a, b) = desk.pool.read(|c| Ok((media::get(c, first)?.unwrap(), media::get(c, second)?.unwrap()))).unwrap();
    assert!(b.rel_path.as_str().ends_with("IMG_7001_1.JPG"), "{}", b.rel_path.as_str());
    let groups: (Option<i64>, Option<i64>, i64) = desk
        .pool
        .read(|c| {
            Ok(c.query_row(
                "SELECT (SELECT near_dup_group_id FROM media_files WHERE id = ?1),
                        (SELECT near_dup_group_id FROM media_files WHERE id = ?2),
                        (SELECT member_count FROM near_duplicate_groups
                          WHERE id = (SELECT near_dup_group_id FROM media_files WHERE id = ?1))",
                [a.id.0, b.id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert!(groups.0.is_some() && groups.0 == groups.1 && groups.2 == 2, "{groups:?}");
    assert!(a.soft_deleted_at.is_none());

    // Politika "yenisini tut": telefon sormadan gönderir, önceki sürüm çöp kutusuna.
    let device = phone.device_id();
    desk.pool.write(|c| Ok(store::set_conflict_policy(c, device, ConflictPolicy::ReplaceWithNewer))).unwrap().unwrap();
    let v3 = photo(13, 80 * 1024);
    let third = phone.upload_all(&c, &meta("IMG_7001.JPG", asset, &v3), &v3).await;
    let (b, a) = desk.pool.read(|c| Ok((media::get(c, second)?.unwrap(), media::get(c, first)?.unwrap()))).unwrap();
    assert!(b.soft_deleted_at.is_some(), "bir önceki sürüm çöp kutusunda (30 gün geri alınabilir)");
    assert!(a.soft_deleted_at.is_none(), "daha eski bağımsız kopyaya dokunulmaz");
    let replaced: i64 = desk
        .pool
        .read(|c| {
            Ok(c.query_row("SELECT replaces_media_id FROM lan_transfers WHERE result_media_id = ?1", [third.0], |r| {
                r.get(0)
            })?)
        })
        .unwrap();
    assert_eq!(replaced, second.0);

    // Politika "atla": aktarılmaz, bu sürüm bir daha sorulmaz.
    desk.pool.write(|c| Ok(store::set_conflict_policy(c, device, ConflictPolicy::Skip))).unwrap().unwrap();
    let v4 = photo(14, 70 * 1024);
    let m4 = meta("IMG_7001.JPG", asset, &v4);
    let res = phone.create(&c, &m4).await;
    assert_eq!((res.status(), header(&res, "loupe-sync-status")), (StatusCode::OK, "skipped".to_owned()));
    let (hash, linked): (Vec<u8>, i64) = desk
        .pool
        .read(|c| {
            Ok(c.query_row(
                "SELECT blake3, media_id FROM device_assets WHERE device_id = ?1 AND client_asset_id = ?2",
                rusqlite::params![device, asset],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!((hash, linked), (m4.blake3.0.to_vec(), third.0));
    desk.pool.write(|c| Ok(store::set_conflict_policy(c, device, ConflictPolicy::AskUser))).unwrap().unwrap();
    let res = phone.create(&c, &m4).await;
    assert_eq!(header(&res, "loupe-sync-status"), "already-synced", "atlanan sürüm yeniden sorulmaz");

    // Manifest: tek istekte have / new / conflict.
    let other = photo(15, 64 * 1024);
    let res = c
        .post(format!("{}/manifest", phone.base))
        .json(&serde_json::json!([
            { "device_asset_id": asset, "blake3": m4.blake3.to_hex() },
            { "device_asset_id": "yeni", "blake3": blake3::hash(&other).to_hex().to_string() },
            { "device_asset_id": asset, "blake3": blake3::hash(&other).to_hex().to_string() },
        ]))
        .send()
        .await
        .unwrap();
    let rows: Vec<serde_json::Value> = res.json().await.unwrap();
    let statuses: Vec<&str> = rows.iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, ["have", "new", "conflict"]);

    let s = desk.server.telemetry().snapshot();
    assert_eq!((s.verified_files, s.conflicts, s.skipped), (3, 1, 1));
    // Üç sürüm katalogda; "yenisini tut" ile değişen çöp kutusunda, zaman çizelgesinde değil.
    let visible = desk.pool.read(|c| media::count(c, desk.lib)).unwrap();
    assert_eq!(visible, 2);
    desk.server.shutdown().await;
}
