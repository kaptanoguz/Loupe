#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Adım 3B komutları uçtan uca: deterministik sahte AI motoruyla gerçek
//! işçiler (küçük resim → yüz/CLIP → kümeleme), arama paleti, kişiler,
//! etkinlikler, `thumb://orig`, eşleşme QR'ı, bulut ayarları, kurtarma
//! anahtarı ve bit-rot doğrulaması.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use image::{Rgb, RgbImage};
use loupe_app::search::PaletteRequest;
use loupe_app::{
    AiMode, AppConfig, AppCore, Arrange, CloudTargetInput, ImportRequest, ImportSessionDto, TimelineRequest, View,
};
use loupe_jobs::{ResourceMode, WorkerConfig};
use loupe_lan::MemoryDiscovery;
use tempfile::TempDir;

/// 2025-10-24 12:00 UTC.
const DAY0: u64 = 1_761_307_200;
const MAGENTA: [u8; 3] = [230, 30, 220];
const AYSE: [u8; 3] = [224, 160, 96];
const MEHMET: [u8; 3] = [32, 96, 224];

struct Fx {
    tmp: TempDir,
    core: Arc<AppCore>,
    lib_root: PathBuf,
    card: PathBuf,
}

/// Gri sahnede magenta çerçeveli kare: sahte yüz dedektörü bunu yüz sayar,
/// iç rengi kimliktir.
fn scene(bg: [u8; 3], face: Option<([u8; 3], (u32, u32))>) -> RgbImage {
    let mut img = RgbImage::from_pixel(960, 720, Rgb(bg));
    if let Some((inner, (fx, fy))) = face {
        for y in fy..fy + 200 {
            for x in fx..fx + 200 {
                let border = x < fx + 28 || x >= fx + 172 || y < fy + 28 || y >= fy + 172;
                img.put_pixel(x, y, Rgb(if border { MAGENTA } else { inner }));
            }
        }
    }
    img
}

fn write(path: &Path, img: &RgbImage, day_offset: u64) {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95).encode_image(img).unwrap();
    fs::write(path, bytes).unwrap();
    let t = UNIX_EPOCH + Duration::from_secs(DAY0 - day_offset * 3_600);
    fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

impl Fx {
    fn new(workers: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let lib_root = tmp.path().join("Fotograflar");
        let card = tmp.path().join("kart");
        fs::create_dir_all(&lib_root).unwrap();
        fs::create_dir_all(&card).unwrap();
        let mut cfg = AppConfig::in_dir(tmp.path());
        cfg.ai = AiMode::Mock;
        if workers {
            cfg.workers = Some(WorkerConfig {
                workers: 3,
                reserved_immediate: 1,
                idle_wait: Duration::from_millis(50),
                mode: ResourceMode::Full,
                ..WorkerConfig::default()
            });
        }
        let core = AppCore::open(cfg).unwrap();
        Self { tmp, core, lib_root, card }
    }

    /// Ayşe 3, Mehmet 2 fotoğrafta; biri yüzsüz turuncu gün batımı.
    fn shoot(&self) {
        let sea = [30, 90, 200];
        write(&self.card.join("IMG_0001.JPG"), &scene(sea, Some((AYSE, (100, 120)))), 0);
        write(&self.card.join("IMG_0002.JPG"), &scene([120, 120, 120], Some((AYSE, (500, 300)))), 1);
        write(&self.card.join("IMG_0003.JPG"), &scene([90, 100, 110], Some((AYSE, (300, 200)))), 2);
        write(&self.card.join("IMG_0004.JPG"), &scene([40, 160, 60], Some((MEHMET, (200, 100)))), 3);
        write(&self.card.join("IMG_0005.JPG"), &scene([120, 120, 120], Some((MEHMET, (600, 400)))), 4);
        write(&self.card.join("gunbatimi_sahil.jpg"), &scene([240, 140, 40], None), 5);
    }

    fn import(&self) -> (i64, ImportSessionDto) {
        let (lib, _) = self.core.add_library_root(&self.lib_root, Some("Fotoğraflar"), true).unwrap();
        let s = self
            .core
            .start_import(&ImportRequest { library_id: lib.id, source: self.card.clone(), mode: "copy".into() })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let st = self.core.import_status(s.id).unwrap();
            if matches!(st.state, "done" | "failed" | "cancelled") {
                assert_eq!(st.state, "done", "{st:?}");
                return (lib.id, st);
            }
            assert!(Instant::now() < deadline, "import bitmedi: {st:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn slice(&self, view: View) -> loupe_app::TimelineSlice {
        self.core.timeline_slice(&TimelineRequest { view, arrange: Arrange::default(), offset: 0, limit: 100 }).unwrap()
    }

    fn wait_ai(&self) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let s = self.core.ai_status().unwrap();
            if s.faces_done == 6 && s.clip_done == 6 && s.pending.faces == 0 && s.pending.clip == 0 {
                return;
            }
            assert!(Instant::now() < deadline, "AI işleri bitmedi: {s:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn palette(&self, input: &str) -> loupe_app::PaletteResult {
        self.core.search_palette(&PaletteRequest { input: input.into(), preview: None }).unwrap()
    }
}

#[test]
fn ai_people_events_and_palette_end_to_end() {
    let fx = Fx::new(true);
    fx.shoot();
    let (_lib, st) = fx.import();
    assert_eq!(st.done_files, 6);
    fx.wait_ai();

    let ai = fx.core.ai_status().unwrap();
    assert_eq!((ai.mode, ai.semantic, ai.images), ("mock", true, 6));

    // ── Kişiler: iki isimsiz küme (3 + 2 yüz), kapaklar kare kırpım için bbox taşır.
    let people = fx.core.people().unwrap();
    assert!(people.named.is_empty());
    let sizes: Vec<u64> = people.unnamed.iter().map(|c| c.face_count).collect();
    assert_eq!(sizes, [3, 2], "{people:?}");
    let (a, b) = (people.unnamed[0].cluster_id.unwrap(), people.unnamed[1].cluster_id.unwrap());
    let cover = people.unnamed[0].cover.as_ref().unwrap();
    assert!(cover.bbox.w > 0.1 && cover.bbox.w < 0.5, "{cover:?}");
    assert_eq!(cover.hash.len(), 64);

    let ayse = fx.core.name_cluster(a, "Ayşe").unwrap();
    assert_eq!(fx.slice(View::Person { id: ayse }).total, 3);
    assert_eq!(fx.slice(View::Cluster { id: b }).total, 2);

    // Birleştir: Mehmet'in kümesi Ayşe'ye (yanlış kümeleme senaryosu), sonra bir yüzü ayır ve geri taşı.
    fx.core.merge_clusters(a, &[b]).unwrap();
    let people = fx.core.people().unwrap();
    assert_eq!(people.named.len(), 1);
    assert_eq!((people.named[0].face_count, people.named[0].photo_count), (5, 5));
    assert!(people.unnamed.is_empty());
    let faces = fx.core.person_faces(Some(ayse), None, 100).unwrap();
    assert_eq!(faces.len(), 5);
    let lone = fx.core.detach_face(faces[0].face_id).unwrap();
    assert_eq!(fx.core.people().unwrap().named[0].face_count, 4);
    assert_eq!(fx.core.people().unwrap().unnamed[0].cluster_id, Some(lone));
    fx.core.move_face(faces[0].face_id, a).unwrap();
    assert_eq!(fx.core.people().unwrap().named[0].face_count, 5);
    fx.core.reject_face(faces[1].face_id).unwrap();
    assert_eq!(fx.core.people().unwrap().named[0].face_count, 4);
    assert_eq!(fx.core.rename_person(ayse, "Ayşe Yılmaz").unwrap(), ayse);

    // ── Palet: belirteçler kimliğe çözülür, bilinmeyen kırmızı çip olur.
    let r = fx.palette("person:ayş");
    assert!(r.suggestions.iter().any(|s| s.insert == "person:\"Ayşe Yılmaz\""), "{:?}", r.suggestions);
    let r = fx.palette(r#"person:"ayşe yılmaz" rating:>=0 "#);
    assert!(r.chips.iter().all(|c| c.ok), "{:?}", r.chips);
    assert_eq!(r.total, 4);
    assert!(r.searched && r.text.is_none());
    let r = fx.palette("person:Zeynep color:turuncu year:2025");
    assert_eq!(r.chips.iter().filter(|c| !c.ok).count(), 2, "{:?}", r.chips);
    assert_eq!(r.query.time.and_then(|t| t.year), Some(2025));
    assert_eq!(r.total, 6);
    assert!(!fx.palette("").searched);

    // Doğal dil: CLIP (sahte) turuncu sahneyi gün batımı sayar; dosya adı da sözcükle eşleşir.
    let r = fx.palette("gün batımı");
    assert!(r.semantic);
    assert!(r.total >= 1);
    let sunset_id = r.items[0].id;
    let detail = fx.core.inspector_detail(sunset_id).unwrap();
    assert!(detail.original_filename.starts_with("gunbatimi"), "{}", detail.original_filename);

    // Akıllı albüm: metinli sorgu kenar çubuğunda; açıldığında tek "En alakalı" bölümü.
    let album = fx.core.save_smart_album("Gün batımları", &r.query, true).unwrap();
    assert!(fx.core.sidebar().unwrap().smart_albums.iter().any(|s| s.id == album.id && s.pinned));
    let smart = fx.slice(View::SmartAlbum { id: album.id });
    assert_eq!(smart.sections.len(), 1);
    assert_eq!(smart.sections[0].title, "En alakalı");
    assert_eq!(smart.items[0].id, sunset_id);
    let search = fx.slice(View::Search { query: Box::new(r.query.clone()) });
    assert_eq!(search.items[0].id, sunset_id);
    assert!(fx.core.save_smart_album("Boş", &loupe_app::SearchQuery::default(), false).is_err());
    fx.core.delete_smart_album(album.id).unwrap();

    // ── Etkinlikler: altı saatlik aralıkla çekilmiş fotoğraflar tek etkinlik.
    let events = fx.core.recluster_events().unwrap();
    assert!(!events.is_empty());
    let ev = &events[0];
    assert!(!ev.locked);
    let renamed = fx.core.rename_event(ev.id, "Ekim 2025 · Göreme").unwrap();
    assert!(renamed.locked && renamed.title == "Ekim 2025 · Göreme");
    let in_event = fx.slice(View::Event { id: ev.id });
    let before = renamed.count;
    let after = fx.core.exclude_from_event(ev.id, &[in_event.items[0].id]).unwrap();
    assert_eq!(after.count, before - 1);
    assert_eq!(fx.slice(View::Event { id: ev.id }).total as u64, before - 1);
    // Yeniden kümeleme kilitli etkinliğe dokunmaz, çıkarılan fotoğraf geri gelmez.
    let again = fx.core.recluster_events().unwrap();
    let kept = again.iter().find(|e| e.id == ev.id).unwrap();
    assert_eq!((kept.title.as_str(), kept.count), ("Ekim 2025 · Göreme", before - 1));
    assert!(fx.core.rename_event(ev.id, "  ").is_err());

    // ── Kaynaklar: duraklat/sürdür ve mod kalıcıdır.
    assert!(fx.core.set_paused(true).unwrap().paused);
    assert_eq!(fx.core.set_resource_mode("quiet").unwrap().resource_mode, "quiet");
    assert!(fx.core.set_resource_mode("turbo").is_err());
    assert!(!fx.core.set_paused(false).unwrap().paused);
}

#[test]
fn thumb_orig_serves_originals_rotated_copies_and_errors() {
    let fx = Fx::new(false);
    write(&fx.card.join("A.JPG"), &scene([200, 40, 40], None), 0);
    fx.import();
    let tile = fx.slice(View::All).items.remove(0);
    let req = |uri: String, method: &str| http::Request::builder().method(method).uri(uri).body(()).unwrap();

    let path = fx.lib_root.join(fx.core.inspector_detail(tile.id).unwrap().rel_path);
    let original = fs::read(&path).unwrap();
    let resp = fx.core.handle_thumb(&req(format!("thumb://orig/{}", tile.hash), "GET"));
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "image/jpeg");
    assert_eq!(resp.body().as_ref(), original.as_slice(), "yönü 1: dosyanın kendisi");
    let head = fx.core.handle_thumb(&req(format!("http://thumb.localhost/orig/{}", tile.hash), "HEAD"));
    assert_eq!(head.status(), 200);
    assert!(head.body().is_empty());

    // EXIF 6 (90° saat yönü): döndürülmüş JPEG, önbellekten tekrar.
    fx.core
        .pool()
        .write(|c| Ok(c.execute("UPDATE media_files SET orientation = 6 WHERE id = ?1", [tile.id])?))
        .unwrap();
    let rotated = fx.core.handle_thumb(&req(format!("thumb://orig/{}", tile.hash), "GET"));
    assert_eq!(rotated.status(), 200);
    let img = image::load_from_memory(rotated.body()).unwrap();
    assert_eq!((img.width(), img.height()), (720, 960));
    let cached: Vec<_> = walk(&fx.tmp.path().join("cache/full"));
    assert_eq!(cached.len(), 1);
    fs::remove_file(&path).unwrap();
    assert_eq!(fx.core.handle_thumb(&req(format!("thumb://orig/{}", tile.hash), "GET")).status(), 200);

    let etag = rotated.headers()["etag"].to_str().unwrap().to_owned();
    let not_modified = http::Request::builder()
        .uri(format!("thumb://orig/{}", tile.hash))
        .header("if-none-match", etag)
        .body(())
        .unwrap();
    assert_eq!(fx.core.handle_thumb(&not_modified).status(), 304);
    assert_eq!(fx.core.handle_thumb(&req(format!("thumb://orig/{}", "ab".repeat(32)), "GET")).status(), 404);
    assert_eq!(fx.core.handle_thumb(&req(format!("thumb://orig/{}", tile.hash), "POST")).status(), 405);
    // Küçük resim istekleri eskisi gibi.
    assert_ne!(fx.core.handle_thumb(&req(format!("thumb://sm/{}", tile.hash), "GET")).status(), 400);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn verify_library_reports_bit_rot() {
    let fx = Fx::new(false);
    fx.shoot();
    let (lib, _) = fx.import();
    let wait = || {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let runs = fx.core.verify_runs(5).unwrap();
            if runs.first().is_some_and(|r| r.state != "running") {
                return runs;
            }
            assert!(Instant::now() < deadline, "{runs:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    assert_eq!(fx.core.verify_library(None).unwrap(), [lib]);
    let runs = wait();
    assert_eq!((runs[0].state.as_str(), runs[0].checked, runs[0].ok), ("done", 6, 6));
    assert_eq!(runs[0].library, "Fotoğraflar");

    // Bir dosyada tek bit çevir.
    let tile = fx.slice(View::All).items.remove(0);
    let path = fx.lib_root.join(fx.core.inspector_detail(tile.id).unwrap().rel_path);
    let mut bytes = fs::read(&path).unwrap();
    bytes[2_000] ^= 0x01;
    fs::write(&path, bytes).unwrap();
    fx.core.verify_library(Some(lib)).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let runs = wait();
    assert_eq!((runs[0].ok, runs[0].mismatched), (5, 1));
    assert!(fx.core.verify_library(Some(9_999)).is_err());
    assert_eq!(fx.core.cancel_verify(None).unwrap(), 0);
}

fn local_target(path: &Path, encrypted: bool) -> CloudTargetInput {
    CloudTargetInput {
        name: "NAS".into(),
        provider: "local".into(),
        config: serde_json::json!({ "path": path.display().to_string() }),
        user: None,
        secret: None,
        key_prefix: "loupe/".into(),
        encrypted,
        bandwidth_mbps: Some(12.5),
        enabled: true,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_qr_devices_cloud_settings_and_recovery_key() {
    let fx = Fx::new(false);
    fx.shoot();
    fx.import();

    // LAN kapalıyken QR yok.
    assert!(fx.core.pairing_qr().await.is_err());
    fx.core.start_network("127.0.0.1:0".parse().unwrap(), Some(Arc::new(MemoryDiscovery::default()))).await.unwrap();
    let qr = fx.core.pairing_qr().await.unwrap();
    assert!(qr.svg.starts_with("<svg") && qr.svg.contains("<path d=\"M"));
    assert!(!qr.svg.contains("<circle") && !qr.svg.contains("radius"), "QR yalnızca kare modül");
    assert_eq!(qr.ttl_secs, 60);
    // Kamera açabilsin: https adresi, belirteç parça kısmında.
    let token = |uri: &str| uri.split_once("/m#t=").unwrap().1.to_owned();
    assert!(qr.uri.starts_with("https://") && qr.uri.contains(&format!(":{}/m#t=", qr.port)), "{}", qr.uri);
    assert_eq!(qr.fingerprint.split(':').count(), 32);
    let now = loupe_core::now_ms();
    assert!(qr.expires_at_ms > now && qr.expires_at_ms <= now + 61_000);
    // Yenisi öncekini geçersiz kılar (her açılışta yeni belirteç).
    let qr2 = fx.core.pairing_qr().await.unwrap();
    assert_ne!(token(&qr2.uri), token(&qr.uri));
    fx.core.cancel_pairing().await;

    // Cihazlar: eşleşmiş bir telefon (doğrudan katalogda), politika ve iptal.
    fx.core
        .pool()
        .write(|c| {
            c.execute(
                "INSERT INTO devices (id, name, platform, cert_sha256, spki_sha256, cert_serial, cert_not_after, paired_at)
                 VALUES (7, 'Pixel 9', 'android', zeroblob(32), zeroblob(32), x'01', 0, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let devices = fx.core.set_conflict_policy(7, loupe_lan::ConflictPolicy::KeepBoth).unwrap();
    assert_eq!(devices[0].conflict_policy, loupe_lan::ConflictPolicy::KeepBoth);
    let json = serde_json::to_value(&devices[0]).unwrap();
    assert_eq!(json["conflictPolicy"], "keep_both");
    assert_eq!(json["kind"], "app");
    let devices = fx.core.revoke_device(7).await.unwrap();
    assert!(devices[0].revoked);
    assert_eq!(fx.core.status().await.unwrap().lan.paired_devices, 0);

    // Bulut: şifreli hedef kurtarma anahtarı olmadan eklenemez.
    let nas = fx.tmp.path().join("nas");
    fs::create_dir_all(&nas).unwrap();
    assert!(fx.core.save_cloud_target(None, &local_target(&nas, true)).is_err());
    assert!(!fx.core.cloud_settings().unwrap().encryption.configured);

    // Kurtarma anahtarı: üret → dışa aktar (0600) → yanlış/eksik onay reddedilir → doğru onay.
    let draft = fx.core.generate_recovery_key().unwrap();
    assert!(draft.recipient.starts_with("age1") && draft.challenge.len() == 3 && !draft.replaces);
    let export = fx.tmp.path().join("kurtarma.txt");
    fx.core.export_recovery_key(&export).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(&export).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(fs::read_to_string(&export).unwrap().contains(&draft.secret));
    let wrong: Vec<(usize, String)> = draft.challenge.iter().map(|&i| (i, "QQQQ".to_owned())).collect();
    assert!(fx.core.confirm_recovery_key(&wrong).is_err());
    assert!(fx.core.confirm_recovery_key(&[(draft.challenge[0], draft.groups[draft.challenge[0]].clone())]).is_err());
    let right: Vec<(usize, String)> = draft.challenge.iter().map(|&i| (i, draft.groups[i].to_lowercase())).collect();
    let enc = fx.core.confirm_recovery_key(&right).unwrap();
    assert!(enc.configured && enc.device_key);
    assert_eq!(enc.recipient.as_deref(), Some(draft.recipient.as_str()));
    // Onaydan sonra gizli anahtar bellekte yok: tekrar dışa aktarılamaz, yalnızca doğrulanır.
    assert!(fx.core.export_recovery_key(&fx.tmp.path().join("ikinci.txt")).is_err());
    let spaced = format!("{}\n{}", draft.prefix, draft.groups.join(" "));
    assert!(fx.core.check_recovery_key(&spaced).unwrap());
    assert!(!fx.core.check_recovery_key(&loupe_cloud::RecoveryKey::generate().secret()).unwrap());
    assert!(fx.core.check_recovery_key("AGE-SECRET-KEY-1XYZ").is_err());

    // Şifreli yerel hedef + S3 hedefi; sırlar dönmez, bant genişliği MB/s.
    let s = fx.core.save_cloud_target(None, &local_target(&nas, true)).unwrap();
    assert_eq!(s.targets.len(), 1);
    assert!(s.targets[0].encrypted);
    assert_eq!(s.targets[0].bandwidth_mbps, Some(12.5));
    let b2 = CloudTargetInput {
        name: "B2 Arşiv".into(),
        provider: "b2".into(),
        config: serde_json::json!({ "endpoint": "https://s3.eu-central-003.backblazeb2.com", "bucket": "aile" }),
        user: Some("0031b2key".into()),
        secret: Some("çok-gizli".into()),
        key_prefix: "loupe/".into(),
        encrypted: true,
        bandwidth_mbps: None,
        enabled: false,
    };
    let s = fx.core.save_cloud_target(None, &b2).unwrap();
    let t = s.targets.iter().find(|t| t.provider == "b2").unwrap();
    assert!(t.has_credentials && !t.enabled);
    assert_eq!(t.user.as_deref(), Some("0031b2key"));
    assert!(!serde_json::to_string(&s).unwrap().contains("çok-gizli"));
    assert_eq!(s.secret_backend, "file");
    let s = fx.core.set_global_bandwidth(Some(2.5)).unwrap();
    assert_eq!(s.global_bandwidth_mbps, Some(2.5));
    assert!(fx.core.set_global_bandwidth(Some(-1.0)).is_err());

    // Eksik S3 ayarı ağa çıkmadan reddedilir.
    let mut broken = b2.clone();
    broken.name = "Bozuk".into();
    broken.config = serde_json::json!({ "bucket": "x" });
    assert!(fx.core.save_cloud_target(None, &broken).is_err());

    // Güncelleme kimlik bilgisini korur; silme sırrı da siler.
    let id = t.id;
    let mut upd = b2.clone();
    upd.user = None;
    upd.secret = None;
    upd.bandwidth_mbps = Some(1.0);
    let s = fx.core.save_cloud_target(Some(id), &upd).unwrap();
    let t = s.targets.iter().find(|t| t.id == id).unwrap();
    assert!(t.has_credentials);
    assert_eq!(t.bandwidth_mbps, Some(1.0));
    let secrets_dir = fx.tmp.path().join("data/secrets");
    let before = fs::read_dir(&secrets_dir).unwrap().count();
    fx.core.delete_cloud_target(id).unwrap();
    assert_eq!(fs::read_dir(&secrets_dir).unwrap().count(), before - 1);

    // Şifreli yerel hedef gerçekten yedekler (cihaz anahtarı + kurtarma alıcısı).
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let st = fx.core.network().cloud_status().unwrap();
        let nas = st.iter().find(|t| t.name == "NAS").unwrap();
        if nas.synced == 6 {
            assert!(nas.encrypted);
            break;
        }
        assert!(Instant::now() < deadline, "{st:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    fx.core.shutdown().await;
}
