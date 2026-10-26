#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Masaüstü komutlarının uçtan uca testleri: WebView olmadan, gerçek SQLite,
//! gerçek dosya sistemi ve gerçek importer ile.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use image::{DynamicImage, ImageBuffer, Rgb};
use loupe_app::{
    AppConfig, AppCore, Arrange, CullRequest, Grouping, ImportRequest, ImportSessionDto, LibraryPatch, SortDir,
    SortKey, TimelineRequest, View,
};
use loupe_core::{Blake3Hash, MediaId, TakenAtSource};
use loupe_db::repo::{location, media};
use loupe_lan::{Discovery, MemoryDiscovery};
use loupe_media::ThumbSize;
use loupe_media::thumb::ThumbFormat;
use tempfile::TempDir;

/// 2025-10-24 12:00 UTC (Cuma).
const DAY0: u64 = 1_761_307_200;

struct Fx {
    tmp: TempDir,
    core: Arc<AppCore>,
    lib_root: PathBuf,
    card: PathBuf,
}

fn write_jpeg(path: &Path, w: u32, h: u32, seed: u32, day_offset: u64) {
    let img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(w, h, |x, y| {
        Rgb([((x * seed) % 256) as u8, ((y * 3 + seed) % 256) as u8, ((x + y + seed) % 256) as u8])
    }));
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 85).encode_image(&img).unwrap();
    fs::write(path, bytes).unwrap();
    let t = UNIX_EPOCH + Duration::from_secs(DAY0 - day_offset * 86_400);
    fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let lib_root = tmp.path().join("Fotograflar");
        let card = tmp.path().join("EOS_DIGITAL/DCIM/100CANON");
        fs::create_dir_all(&lib_root).unwrap();
        fs::create_dir_all(&card).unwrap();
        let core = AppCore::open(AppConfig::in_dir(tmp.path())).unwrap();
        Self { tmp, core, lib_root, card }
    }

    /// 5 fotoğraf: 3'ü aynı gün, 2'si önceki günler; biri dikey.
    fn shoot_card(&self) {
        write_jpeg(&self.card.join("IMG_0001.JPG"), 300, 200, 1, 0);
        write_jpeg(&self.card.join("IMG_0002.JPG"), 200, 300, 2, 0);
        write_jpeg(&self.card.join("IMG_0003.JPG"), 320, 180, 3, 0);
        write_jpeg(&self.card.join("IMG_0004.JPG"), 300, 200, 4, 1);
        write_jpeg(&self.card.join("IMG_0005.JPG"), 300, 300, 5, 40);
        fs::write(self.card.join("NOTES.TXT"), b"medya degil").unwrap();
    }

    fn add_library(&self) -> i64 {
        let (lib, scan) = self.core.add_library_root(&self.lib_root, Some("Fotoğraflar"), true).unwrap();
        // Boş klasör de yerinde taranır (bulacak bir şey yok).
        let s = wait_done(&self.core, scan.unwrap().id);
        assert_eq!((s.state, s.total_files), ("done", 0));
        assert_eq!(lib.state, "online");
        assert!(lib.managed && lib.primary);
        lib.id
    }

    fn import(&self, lib: i64) -> ImportSessionDto {
        let s = self
            .core
            .start_import(&ImportRequest {
                library_id: lib,
                source: self.card.clone(),
                mode: "copy".into(),
                layout: None,
            })
            .unwrap();
        wait_done(&self.core, s.id)
    }

    fn slice(&self, view: View, grouping: Grouping, offset: u32, limit: u32) -> loupe_app::TimelineSlice {
        self.core
            .timeline_slice(&TimelineRequest {
                view,
                arrange: Arrange { grouping, ..Arrange::default() },
                offset,
                limit,
            })
            .unwrap()
    }

    fn all_ids(&self) -> Vec<i64> {
        self.slice(View::All, Grouping::Day, 0, 100).items.iter().map(|i| i.id).collect()
    }
}

fn wait_done(core: &AppCore, id: i64) -> ImportSessionDto {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let s = core.import_status(id).unwrap();
        if matches!(s.state, "done" | "failed" | "cancelled") {
            return s;
        }
        assert!(Instant::now() < deadline, "import bitmedi: {s:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn import_fills_timeline_slices_with_day_and_month_sections() {
    let fx = Fx::new();
    fx.shoot_card();
    let lib = fx.add_library();

    let s = fx.import(lib);
    assert_eq!(s.state, "done", "{s:?}");
    assert_eq!((s.total_files, s.done_files, s.failed_files), (5, 5, 0));
    assert!(fx.lib_root.join("2025/10/24/IMG_0001.JPG").is_file(), "kütüphane düzeni {{yyyy}}/{{mm}}/{{dd}}");
    assert!(fx.card.join("IMG_0001.JPG").is_file(), "Kopyala kaynağa dokunmaz");

    // Aynı kart ikinci kez: içerik aynı, hiçbir şey kopyalanmaz.
    let again = fx.import(lib);
    assert_eq!((again.done_files, again.skipped_dup), (0, 5));

    // Gün bölümleri, en yeni önce.
    let first = fx.slice(View::All, Grouping::Day, 0, 2);
    assert_eq!(first.total, 5);
    let titles: Vec<(&str, u32, u32)> = first.sections.iter().map(|s| (s.title.as_str(), s.start, s.count)).collect();
    assert_eq!(titles, vec![("24 Ekim 2025", 0, 3), ("23 Ekim 2025", 3, 1), ("14 Eylül 2025", 4, 1)]);
    assert_eq!(first.sections[0].subtitle.as_deref(), Some("Cuma"));
    assert_eq!(first.items.len(), 2);
    let tile = &first.items[0];
    assert_eq!(tile.hash.len(), 64);
    assert_eq!(tile.sync, "local");
    assert!(tile.raw.is_none() && !tile.video);

    // Dilimler üst üste binmez ve sırayı korur.
    let rest = fx.slice(View::All, Grouping::Day, 2, 10);
    assert_eq!(rest.items.len(), 3);
    let mut ids: Vec<i64> = first.items.iter().chain(&rest.items).map(|i| i.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), 5);

    // Ay bölümleri.
    let months = fx.slice(View::All, Grouping::Month, 0, 10);
    let titles: Vec<(&str, u32)> = months.sections.iter().map(|s| (s.title.as_str(), s.count)).collect();
    assert_eq!(titles, vec![("Ekim 2025", 4), ("Eylül 2025", 1)]);

    // Görünüm seçenekleri: yıl bölümü, ada / puana göre sıralama, yön.
    let arranged = |arrange: Arrange| {
        fx.core.timeline_slice(&TimelineRequest { view: View::All, arrange, offset: 0, limit: 10 }).unwrap()
    };
    let years = arranged(Arrange { grouping: Grouping::Year, ..Arrange::default() });
    assert_eq!(years.sections.iter().map(|s| (s.title.as_str(), s.count)).collect::<Vec<_>>(), vec![("2025", 5)]);
    let by_name = arranged(Arrange { sort: SortKey::Name, dir: SortDir::Asc, ..Arrange::default() });
    let names: Vec<String> = by_name
        .items
        .iter()
        .map(|i| fx.core.pool().read(|c| media::get(c, MediaId(i.id))).unwrap().unwrap().original_filename)
        .collect();
    assert_eq!(names, ["IMG_0001.JPG", "IMG_0002.JPG", "IMG_0003.JPG", "IMG_0004.JPG", "IMG_0005.JPG"]);
    assert_eq!(by_name.sections.len(), 1);
    let rated = by_name.items[2].id;
    fx.core.pool().write(|c| Ok(c.execute("UPDATE media_files SET rating = 4 WHERE id = ?1", [rated])?)).unwrap();
    let by_rating = arranged(Arrange { sort: SortKey::Rating, ..Arrange::default() });
    assert_eq!(by_rating.items[0].id, rated);
    let secs: Vec<(&str, u32)> = by_rating.sections.iter().map(|s| (s.title.as_str(), s.count)).collect();
    assert_eq!(secs, vec![("4 yıldız", 1), ("Puansız", 4)]);
    let flat = arranged(Arrange { sort: SortKey::Added, dir: SortDir::Asc, grouping: Grouping::None });
    assert_eq!((flat.sections.len(), flat.total), (1, 5));

    // En-boy oranı: pHash için çözülen boyut; EXIF yönlendirmesi 6 ise yer değiştirir.
    let portrait = rest.items.iter().chain(&first.items).find(|i| (i.aspect - 2.0 / 3.0).abs() < 0.01);
    assert!(portrait.is_some(), "dikey kare 2:3 olmalı");
    let id = MediaId(first.items[0].id);
    fx.core
        .pool()
        .write(|c| {
            media::apply_metadata(
                c,
                id,
                &media::MetadataUpdate {
                    width: Some(400),
                    height: Some(300),
                    orientation: Some(6),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let rotated = fx.slice(View::All, Grouping::Day, 0, 1).items[0].aspect;
    assert!((rotated - 0.75).abs() < 0.001, "{rotated}");

    // İçe aktarma küçük resim + metadata işlerini kuyruğa koydu (+1: açılıştaki etkinlik
    // kümelemesi); görünür olanlar öne alınır.
    let stats = fx.core.queue().stats().unwrap();
    assert_eq!(stats.pending, 16, "{stats:?}");
    let boosted = fx.core.prioritize_viewport(&ids[..2]).unwrap();
    assert_eq!(boosted, 6);
    assert!(fx.core.prioritize_viewport(&vec![1; 3000]).is_err());

    // Kenar çubuğu sayaçları.
    let side = fx.core.sidebar().unwrap();
    assert_eq!(side.libraries.len(), 1);
    assert_eq!(side.libraries[0].media_count, 5);
    assert_eq!(side.views.iter().find(|v| v.key == "all").unwrap().count, 5);

    assert!(
        fx.core
            .timeline_slice(&TimelineRequest { view: View::All, arrange: Arrange::default(), offset: 0, limit: 0 })
            .is_err()
    );
}

#[test]
fn thumb_protocol_serves_cache_bytes_and_queues_missing_sizes() {
    let fx = Fx::new();
    fx.shoot_card();
    let lib = fx.add_library();
    fx.import(lib);
    let item = fx.slice(View::All, Grouping::Day, 0, 1).items.remove(0);
    let hash = Blake3Hash::from_hex(&item.hash).unwrap();

    // Önbellekte `sm` var (gradyan: histogram dolu çıkmalı).
    let img =
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(64, 48, |x, _| Rgb([(x * 4) as u8, 128, 255 - (x * 4) as u8])));
    let mut webp = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .encode(img.as_bytes(), 64, 48, image::ExtendedColorType::Rgb8)
        .unwrap();
    let path = fx.core.thumbs().path(&hash, ThumbSize::Sm, ThumbFormat::Webp);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &webp).unwrap();

    let get = |uri: &str| fx.core.handle_thumb(&http::Request::get(uri).body(()).unwrap());
    let ok = get(&format!("thumb://sm/{}", item.hash));
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.headers()["content-type"], "image/webp");
    assert!(ok.headers()["cache-control"].to_str().unwrap().contains("immutable"));
    assert_eq!(ok.body().as_ref(), webp.as_slice(), "baytlar dosyanın kendisi");
    // WebKitGTK'nın yazdığı biçim de aynı dosyayı verir.
    assert_eq!(get(&format!("thumb://localhost/sm/{}", item.hash)).status(), 200);

    // `lg` yok: 404 ve en acil öncelikle üretim işi.
    let before = fx.core.queue().find(loupe_jobs::JobKind::Thumb, Some(MediaId(item.id)), "lg").unwrap();
    assert!(before.is_none());
    assert_eq!(get(&format!("thumb://lg/{}", item.hash)).status(), 404);
    let job = fx.core.queue().find(loupe_jobs::JobKind::Thumb, Some(MediaId(item.id)), "lg").unwrap().unwrap();
    assert_eq!(job.priority, loupe_jobs::Priority::Immediate.level());

    // Yol geçişi ve bozuk özet reddedilir.
    assert_eq!(get("thumb://sm/../../etc/passwd").status(), 400);
    assert_eq!(get("thumb://xl/abcd").status(), 400);

    // Denetçinin histogramı aynı küçük resimden.
    let d = fx.core.inspector_detail(item.id).unwrap();
    let h = d.histogram.expect("histogram");
    assert_eq!((h.bins, h.source), (64, "sm"));
    assert_eq!(h.luma.iter().sum::<u32>(), 64 * 48);
    assert_eq!(h.green.iter().filter(|&&n| n > 0).count(), 1, "yeşil kanal sabit 128");
}

#[test]
fn rating_label_favorite_go_to_db_and_sidecars_even_after_offline() {
    let fx = Fx::new();
    fx.shoot_card();
    let lib = fx.add_library();
    fx.import(lib);
    let ids = fx.all_ids();

    let res = fx
        .core
        .set_rating_label_favorite(&CullRequest {
            ids: ids[..2].to_vec(),
            rating: Some(4),
            label: Some("red".into()),
            favorite: Some(true),
        })
        .unwrap();
    assert!(res.iter().all(|r| r.sidecar == "written" && r.rating == 4 && r.favorite), "{res:?}");

    let m = fx.core.pool().read(|c| media::get(c, MediaId(ids[0]))).unwrap().unwrap();
    let file = fx.lib_root.join(m.rel_path.as_str());
    let xmp = fs::read_to_string(format!("{}.xmp", file.display())).unwrap();
    assert!(xmp.contains("Rating") && xmp.contains('4') && xmp.contains("Red"), "{xmp}");
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(format!("{}.loupe.json", file.display())).unwrap()).unwrap();
    assert_eq!((json["rating"].as_u64(), json["favorite"].as_bool()), (Some(4), Some(true)));
    let dirty: bool = fx
        .core
        .pool()
        .read(|c| Ok(c.query_row("SELECT sidecar_dirty FROM media_files WHERE id = ?1", [ids[0]], |r| r.get(0))?))
        .unwrap();
    assert!(!dirty);

    // Favoriler görünümü.
    let fav = fx.slice(View::Favorites, Grouping::Day, 0, 10);
    assert_eq!(fav.total, 2);

    // Etiketi kaldır, puanı sıfırla.
    let res = fx
        .core
        .set_rating_label_favorite(&CullRequest {
            ids: vec![ids[0]],
            rating: Some(0),
            label: Some("none".into()),
            favorite: None,
        })
        .unwrap();
    assert_eq!((res[0].rating, res[0].label.as_deref(), res[0].favorite), (0, None, true));

    for bad in [
        CullRequest { ids: vec![ids[0]], rating: Some(6), ..Default::default() },
        CullRequest { ids: vec![ids[0]], label: Some("orange".into()), ..Default::default() },
        CullRequest { ids: vec![ids[0]], ..Default::default() },
        CullRequest { ids: vec![], rating: Some(1), ..Default::default() },
    ] {
        assert!(fx.core.set_rating_label_favorite(&bad).is_err(), "{bad:?}");
    }

    // Disk çıkarıldı: katalog güncellenir, sidecar bekler.
    let parked = fx.tmp.path().join("cikarildi");
    fs::rename(&fx.lib_root, &parked).unwrap();
    let libs = fx.core.refresh_libraries().unwrap();
    assert_eq!(libs[0].state, "offline");
    let tile = fx.slice(View::All, Grouping::Day, 0, 1).items.remove(0);
    assert_eq!(tile.sync, "offline");
    let res = fx
        .core
        .set_rating_label_favorite(&CullRequest { ids: vec![ids[1]], rating: Some(5), ..Default::default() })
        .unwrap();
    assert_eq!(res[0].sidecar, "pending");
    assert!(fx.core.inspector_detail(ids[1]).unwrap().sidecar_dirty);

    // Disk geri geldi: bekleyen sidecar yazılır.
    fs::rename(&parked, &fx.lib_root).unwrap();
    let libs = fx.core.refresh_libraries().unwrap();
    assert_eq!(libs[0].state, "online");
    let d = fx.core.inspector_detail(ids[1]).unwrap();
    assert!(!d.sidecar_dirty);
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(format!("{}.loupe.json", d.path.unwrap())).unwrap()).unwrap();
    assert_eq!(json["rating"].as_u64(), Some(5));
}

#[test]
fn inspector_formats_exif_coordinates_place_and_backup_state() {
    let fx = Fx::new();
    fx.shoot_card();
    let lib = fx.add_library();
    fx.import(lib);
    let id = MediaId(fx.all_ids()[0]);
    fx.core
        .pool()
        .write(|c| {
            media::apply_metadata(
                c,
                id,
                &media::MetadataUpdate {
                    width: Some(6000),
                    height: Some(4000),
                    taken_at: Some((1_761_309_130_000, Some(180), TakenAtSource::Exif)),
                    gps: Some((38.642_83, 34.828_86, Some(1105.0))),
                    camera_make: Some("FUJIFILM".into()),
                    camera_model: Some("X-T5".into()),
                    lens_model: Some("XF16-55mmF2.8 R LM WR".into()),
                    focal_length_mm: Some(23.0),
                    aperture_f: Some(2.8),
                    exposure_s: Some(0.002),
                    iso: Some(100),
                    ..Default::default()
                },
            )?;
            let loc = location::upsert(
                c,
                &location::NewLocation {
                    geoname_id: Some(315_469),
                    country_code: "TR".into(),
                    country: "Türkiye".into(),
                    admin1: Some("Nevşehir".into()),
                    admin2: Some("Merkez".into()),
                    locality: Some("Göreme".into()),
                    lat: 38.643,
                    lon: 34.829,
                    dataset_ver: "test".into(),
                },
            )?;
            location::assign_to_media(c, id, loc)
        })
        .unwrap();

    let d = fx.core.inspector_detail(id.0).unwrap();
    assert_eq!(d.aperture.as_deref(), Some("f/2.8"));
    assert_eq!(d.exposure.as_deref(), Some("1/500 s"));
    assert_eq!(d.focal_length.as_deref(), Some("23 mm"));
    assert_eq!(d.iso, Some(100));
    assert_eq!(d.megapixels, Some(24.0));
    assert_eq!(d.taken_local, "2025-10-24 15:32:10 +03:00");
    let gps = d.gps.unwrap();
    assert_eq!(gps.dms, "38° 38' 34.2\" N, 034° 49' 43.9\" E");
    assert_eq!(gps.alt_m, Some(1105.0));
    assert_eq!(d.place.unwrap().display, "Göreme, Merkez, Nevşehir, Türkiye");
    assert!(d.library_online && d.path.unwrap().starts_with(fx.lib_root.to_str().unwrap()));
    assert!(d.sync.is_empty());
    assert!(fx.core.inspector_detail(999_999).is_err());

    // Yerler görünümü yere göre gruplar.
    let places = fx.slice(View::Places, Grouping::Day, 0, 10);
    assert_eq!(places.sections.len(), 1);
    assert_eq!(places.sections[0].title, "Göreme");
    assert_eq!(places.sections[0].subtitle.as_deref(), Some("Nevşehir, Türkiye"));
}

#[test]
fn events_people_and_smart_album_views() {
    let fx = Fx::new();
    fx.shoot_card();
    let lib = fx.add_library();
    fx.import(lib);
    let ids = fx.all_ids();
    fx.core
        .pool()
        .write(|c| {
            c.execute(
                "INSERT INTO events (id, title, start_utc, end_utc, media_count, created_at, updated_at)
                 VALUES (1, 'Ekim 2025 · Göreme', 1761220800000, 1761307200000, 2, 0, 0)",
                [],
            )?;
            for id in &ids[..2] {
                c.execute("INSERT INTO event_media (event_id, media_id) VALUES (1, ?1)", [id])?;
            }
            Ok(())
        })
        .unwrap();
    let ev = fx.slice(View::Events, Grouping::Day, 0, 10);
    assert_eq!(ev.total, 2);
    assert_eq!(ev.sections[0].title, "Ekim 2025 · Göreme");
    assert_eq!(ev.sections[0].subtitle.as_deref(), Some("23–24 Ekim 2025"));
    assert_eq!(fx.slice(View::People, Grouping::Day, 0, 10).total, 0);

    fx.core
        .set_rating_label_favorite(&CullRequest { ids: vec![ids[4]], rating: Some(5), ..Default::default() })
        .unwrap();
    let album = fx
        .core
        .pool()
        .write(|c| {
            loupe_search::albums::create(
                c,
                "Beş yıldız",
                &loupe_search::SearchQuery { min_rating: Some(5), ..Default::default() },
                true,
            )
            .map_err(|e| loupe_db::DbError::Validation(e.to_string()))
        })
        .unwrap();
    let smart = fx.slice(View::SmartAlbum { id: album.0 }, Grouping::Day, 0, 10);
    assert_eq!(smart.items.iter().map(|i| i.id).collect::<Vec<_>>(), vec![ids[4]]);
    let side = fx.core.sidebar().unwrap();
    assert_eq!(side.smart_albums[0].name, "Beş yıldız");
    assert_eq!(side.views.iter().find(|v| v.key == "events").unwrap().count, 1);
    assert!(
        fx.core
            .timeline_slice(&TimelineRequest {
                view: View::SmartAlbum { id: 42 },
                arrange: Arrange::default(),
                offset: 0,
                limit: 5
            })
            .is_err()
    );
}

#[test]
fn reference_library_is_indexed_in_place_and_watcher_events_apply() {
    let fx = Fx::new();
    fx.shoot_card();
    let (lib, scan) = fx.core.add_library_root(&fx.card, None, false).unwrap();
    assert!(!lib.managed && !lib.primary);
    let s = wait_done(&fx.core, scan.unwrap().id);
    assert_eq!((s.mode, s.done_files), ("reference", 5));
    let before: Vec<_> = fs::read_dir(&fx.card).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(before.iter().any(|n| n == ".loupe"));
    let m = fx.core.pool().read(|c| media::get(c, MediaId(fx.all_ids()[0]))).unwrap().unwrap();
    assert_eq!(m.placement, loupe_core::Placement::Reference);
    assert!(fx.card.join(m.rel_path.as_str()).is_file(), "dosya yerinde kalır");

    // Yönetilmeyen kütüphaneye içe aktarma yapılamaz.
    let err = fx
        .core
        .start_import(&ImportRequest {
            library_id: lib.id,
            source: fx.tmp.path().to_path_buf(),
            mode: "copy".into(),
            layout: None,
        })
        .unwrap_err();
    assert_eq!(err.code(), "LIBRARY_OFFLINE");

    // İzleyici olayları (inotify olmadan doğrudan uygulanır).
    let libs = loupe_app::library::Libraries::new(fx.core.pool().clone(), Default::default());
    libs.refresh().unwrap();
    let lid = loupe_core::LibraryId(lib.id);
    write_jpeg(&fx.card.join("IMG_0100.JPG"), 120, 80, 9, 0);
    let fresh = libs.apply_watch_event(lid, &loupe_storage::WatchEvent::Created(fx.card.join("IMG_0100.JPG"))).unwrap();
    assert_eq!(fresh.len(), 1);
    fs::rename(fx.card.join("IMG_0100.JPG"), fx.card.join("IMG_0101.JPG")).unwrap();
    libs.apply_watch_event(
        lid,
        &loupe_storage::WatchEvent::Renamed { from: fx.card.join("IMG_0100.JPG"), to: fx.card.join("IMG_0101.JPG") },
    )
    .unwrap();
    let moved = fx.core.pool().read(|c| media::get(c, fresh[0])).unwrap().unwrap();
    assert_eq!(moved.rel_path.as_str(), "IMG_0101.JPG");
    fs::remove_file(fx.card.join("IMG_0101.JPG")).unwrap();
    libs.apply_watch_event(lid, &loupe_storage::WatchEvent::Removed(fx.card.join("IMG_0101.JPG"))).unwrap();
    let gone = fx.core.pool().read(|c| media::get(c, fresh[0])).unwrap().unwrap();
    assert_eq!(gone.verify_state, loupe_core::VerifyState::Missing);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_starts_lan_advertises_and_backs_up_to_local_target() {
    let fx = Fx::new();
    fx.shoot_card();
    let lib = fx.add_library();
    fx.import(lib);

    let backup = fx.tmp.path().join("nas");
    fs::create_dir_all(&backup).unwrap();
    fx.core
        .pool()
        .write(|c| {
            for (name, provider, cfg, secret) in [
                ("NAS", "local", serde_json::json!({ "path": backup.display().to_string() }), ""),
                (
                    "B2",
                    "b2",
                    serde_json::json!({ "endpoint": "https://s3.example", "bucket": "b", "region": "r" }),
                    "b2-key",
                ),
            ] {
                loupe_cloud::targets::insert(
                    c,
                    &loupe_cloud::targets::NewTarget {
                        name,
                        provider,
                        config: cfg,
                        secret_ref: secret,
                        key_prefix: "loupe/",
                        age_recipient: None,
                        bandwidth_bps: None,
                    },
                    0,
                )
                .map_err(|e| loupe_db::DbError::Validation(e.to_string()))?;
            }
            Ok(())
        })
        .unwrap();

    let net = MemoryDiscovery::default();
    fx.core.start_network("127.0.0.1:0".parse().unwrap(), Some(Arc::new(net.clone()))).await.unwrap();
    let st = fx.core.status().await.unwrap();
    assert_eq!(st.lan.state, "listening");
    assert_eq!(st.total_media, 5);
    let port = st.lan.port.unwrap();
    let ads = net.browse(Duration::from_millis(20)).unwrap();
    assert_eq!(ads.len(), 1);
    assert_eq!(ads[0].port, port);

    // Yerel hedef kimlik bilgisi istemez: her şey yedeklenir; B2'nin anahtarı yok.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let cloud = fx.core.network().cloud_status().unwrap();
        let nas = cloud.iter().find(|t| t.name == "NAS").unwrap();
        let b2 = cloud.iter().find(|t| t.name == "B2").unwrap();
        assert_eq!(b2.state, "needs_setup");
        if nas.synced == 5 {
            assert_eq!(nas.pending, 0);
            break;
        }
        assert!(Instant::now() < deadline, "{cloud:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let tile = fx.slice(View::All, Grouping::Day, 0, 1).items.remove(0);
    assert_eq!(tile.sync, "synced");
    let d = fx.core.inspector_detail(tile.id).unwrap();
    assert_eq!(d.sync.len(), 1);
    assert_eq!((d.sync[0].state.as_str(), d.sync[0].encryption.as_str()), ("synced", "none"));
    assert!(d.sync[0].last_verified_at.is_some());

    // LAN kimliği kalıcı: aynı depodan aynı CA.
    let store = loupe_app::FileSecretStore::new(fx.tmp.path().join("data/secrets"));
    let a = loupe_app::network::load_or_create_identity(&store, "x").unwrap();
    let b = loupe_app::network::load_or_create_identity(&store, "y").unwrap();
    assert_eq!(a.ca_spki_sha256(), b.ca_spki_sha256());

    fx.core.shutdown().await;
    assert!(net.browse(Duration::from_millis(10)).unwrap().is_empty(), "kapanışta duyuru geri çekilir");
}

#[test]
fn managed_library_with_existing_photos_is_indexed_in_place() {
    let fx = Fx::new();
    let sub = fx.lib_root.join("2019/Tatil");
    fs::create_dir_all(&sub).unwrap();
    write_jpeg(&fx.lib_root.join("IMG_A.JPG"), 200, 150, 11, 0);
    write_jpeg(&sub.join("IMG_B.JPG"), 150, 200, 12, 3);
    fs::write(sub.join("notlar.txt"), b"medya degil").unwrap();

    let (lib, scan) = fx.core.add_library_root(&fx.lib_root, Some("Resimler"), true).unwrap();
    assert!(lib.managed && lib.primary, "içe aktarma hedefi olarak kalır");
    let s = wait_done(&fx.core, scan.expect("yönetilen kütüphane de taranır").id);
    assert_eq!((s.state, s.done_files), ("done", 2), "{s:?}");
    assert_eq!(fx.all_ids().len(), 2);
    for id in fx.all_ids() {
        let m = fx.core.pool().read(|c| media::get(c, MediaId(id))).unwrap().unwrap();
        assert_eq!(m.placement, loupe_core::Placement::Reference, "mevcut dosya taşınmaz");
        assert!(fx.lib_root.join(m.rel_path.as_str()).is_file());
    }

    // Önceki sürümde eklenip hiç taranmamış kütüphane: açılışta bir kez taranır.
    let dir = fx.tmp.path().to_path_buf();
    fx.core.pool().write(|c| Ok(c.execute_batch("DELETE FROM import_sessions;")?)).unwrap();
    drop(fx.core);
    // Uygulama kapalıyken klasöre gelen fotoğraf (izleyici görmedi).
    write_jpeg(&sub.join("IMG_C.JPG"), 160, 120, 13, 5);
    let core = AppCore::open(AppConfig::in_dir(&dir)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let n: i64 =
            core.pool().read(|c| Ok(c.query_row("SELECT count(*) FROM media_files", [], |r| r.get(0))?)).unwrap();
        if n == 3 {
            break;
        }
        assert!(Instant::now() < deadline, "açılış taraması bitmedi ({n})");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Taranmış kütüphane bir daha açılışta taranmaz.
    let sessions = |c: &AppCore| -> i64 {
        c.pool().read(|c| Ok(c.query_row("SELECT count(*) FROM import_sessions", [], |r| r.get(0))?)).unwrap()
    };
    let before = sessions(&core);
    drop(core);
    let core = AppCore::open(AppConfig::in_dir(&dir)).unwrap();
    assert_eq!(sessions(&core), before);
}

/// Klasörü Loupe'den kaldırmak kataloğu temizler, diskteki dosyalara dokunmaz.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_library_forgets_its_photos_but_keeps_files() {
    let fx = Fx::new();
    write_jpeg(&fx.lib_root.join("IMG_A.JPG"), 200, 150, 21, 0);
    write_jpeg(&fx.lib_root.join("IMG_B.JPG"), 150, 200, 22, 1);
    let other = fx.tmp.path().join("Diger");
    fs::create_dir_all(&other).unwrap();
    write_jpeg(&other.join("IMG_C.JPG"), 120, 90, 23, 2);
    let (keep, scan) = fx.core.add_library_root(&other, Some("Diğer"), false).unwrap();
    wait_done(&fx.core, scan.unwrap().id);
    let (lib, scan) = fx.core.add_library_root(&fx.lib_root, Some("Fotoğraflar"), true).unwrap();
    wait_done(&fx.core, scan.unwrap().id);
    assert_eq!(fx.all_ids().len(), 3);

    let (removed, lan_stopped) = fx.core.remove_library(lib.id).await.unwrap();
    assert_eq!((removed, lan_stopped), (2, false));
    let libs = fx.core.libraries().unwrap();
    assert_eq!(libs.iter().map(|l| l.id).collect::<Vec<_>>(), vec![keep.id]);
    assert_eq!(fx.all_ids().len(), 1, "diğer kütüphane yerinde");
    assert!(fx.lib_root.join("IMG_A.JPG").is_file() && fx.lib_root.join("IMG_B.JPG").is_file());
    assert!(fx.core.remove_library(lib.id).await.is_err(), "olmayan kütüphane");
}

/// Sahte Çöp Kutusu: dosyaları bir klasöre taşır, adı `KILITLI` olanı taşıyamaz.
struct FakeTrash(PathBuf);

impl loupe_app::trash::Trasher for FakeTrash {
    fn trash(&self, paths: &[PathBuf]) -> loupe_app::AppResult<String> {
        for p in paths {
            if !p.to_string_lossy().contains("KILITLI") {
                fs::rename(p, self.0.join(p.file_name().unwrap())).unwrap();
            }
        }
        Ok("gio: çöp dizini oluşturulamıyor".into())
    }
}

#[test]
fn trashing_photos_moves_files_and_sidecars_then_forgets_them() {
    let fx = Fx::new();
    write_jpeg(&fx.lib_root.join("IMG_A.JPG"), 200, 150, 31, 0);
    write_jpeg(&fx.lib_root.join("IMG_B.JPG"), 150, 200, 32, 1);
    write_jpeg(&fx.lib_root.join("KILITLI.JPG"), 120, 90, 33, 2);
    fs::write(fx.lib_root.join("IMG_A.JPG.loupe.json"), b"{}").unwrap();
    let (_, scan) = fx.core.add_library_root(&fx.lib_root, Some("Fotoğraflar"), true).unwrap();
    wait_done(&fx.core, scan.unwrap().id);
    fs::write(fx.lib_root.join("KILITLI.JPG.loupe.json"), b"{}").unwrap();
    let by_name = |name: &str| {
        fx.all_ids()
            .into_iter()
            .find(|&id| fx.core.pool().read(|c| media::get(c, MediaId(id))).unwrap().unwrap().original_filename == name)
            .unwrap()
    };
    let (a, b, locked) = (by_name("IMG_A.JPG"), by_name("IMG_B.JPG"), by_name("KILITLI.JPG"));
    let bin = fx.tmp.path().join("Cop");
    fs::create_dir_all(&bin).unwrap();
    // B diskten başka yoldan silinmiş: yalnızca kaydı kalkar.
    fs::remove_file(fx.lib_root.join("IMG_B.JPG")).unwrap();

    let out = fx.core.trash_media_with(&[a, b, locked, 999_999], &FakeTrash(bin.clone())).unwrap();
    assert_eq!(out.trashed, 3);
    assert!(out.failed.is_empty(), "{:?}", out.failed);
    assert!(bin.join("IMG_A.JPG").is_file() && bin.join("IMG_A.JPG.loupe.json").is_file(), "sidecar birlikte");
    // Sistem Çöp Kutusu'nun kabul etmediği dosya aynı diskte `.loupe/trash/<gün>/`e gider.
    assert_eq!(out.loupe_trash, 1);
    let day = PathBuf::from(out.loupe_trash_dir.unwrap());
    assert!(day.starts_with(fx.lib_root.join(".loupe/trash")));
    assert!(day.join("KILITLI.JPG").is_file() && day.join("KILITLI.JPG.loupe.json").is_file());
    assert!(!fx.lib_root.join("KILITLI.JPG").exists());
    assert!(fx.all_ids().is_empty());
}

#[test]
fn import_layout_and_library_settings() {
    let fx = Fx::new();
    fx.shoot_card();
    let lib = fx.add_library();

    // Aktarıma özel düzen: kütüphanenin varsayılanı değişmez.
    let s = fx
        .core
        .start_import(&ImportRequest {
            library_id: lib,
            source: fx.card.clone(),
            mode: "copy".into(),
            layout: Some("iPhone/{yyyy}/{mm}".into()),
        })
        .unwrap();
    assert_eq!(wait_done(&fx.core, s.id).done_files, 5);
    assert!(fx.lib_root.join("iPhone/2025/10/IMG_0001.JPG").is_file());
    assert!(fx.lib_root.join("iPhone/2025/09/IMG_0005.JPG").is_file());
    for bad in ["", ".gizli/{yyyy}", "../{yyyy}", "/mutlak", "{yyyy}//{mm}", "{hh}"] {
        let r = fx.core.start_import(&ImportRequest {
            library_id: lib,
            source: fx.card.clone(),
            mode: "copy".into(),
            layout: Some(bad.into()),
        });
        // Boş düzen = kütüphanenin varsayılanı.
        assert_eq!(r.is_err(), !bad.is_empty(), "{bad}");
        if let Ok(s) = r {
            wait_done(&fx.core, s.id);
        }
    }

    // Kütüphane ayarları.
    let patch = LibraryPatch {
        name: Some("Arşiv".into()),
        layout_template: Some("{yyyy}".into()),
        default_import_mode: Some("move".into()),
        primary: Some(true),
    };
    let dto = fx.core.update_library(lib, &patch).unwrap();
    assert_eq!((dto.name.as_str(), dto.layout_template.as_str(), dto.default_import_mode), ("Arşiv", "{yyyy}", "move"));
    assert!(dto.primary);
    let bad = LibraryPatch { layout_template: Some("../x".into()), ..LibraryPatch::default() };
    assert!(fx.core.update_library(lib, &bad).is_err());
    assert!(fx.core.update_library(lib, &LibraryPatch { name: Some(" ".into()), ..LibraryPatch::default() }).is_err());

    // Yeni varsayılan düzen hemen geçerli.
    write_jpeg(&fx.card.join("IMG_0099.JPG"), 64, 48, 99, 0);
    let s = fx
        .core
        .start_import(&ImportRequest { library_id: lib, source: fx.card.clone(), mode: "copy".into(), layout: None })
        .unwrap();
    wait_done(&fx.core, s.id);
    assert!(fx.lib_root.join("2025/IMG_0099.JPG").is_file());
}
