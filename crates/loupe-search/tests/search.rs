//! loupe-search uçtan uca testleri: gerçek katalog, gerçek sqlite-vec, deterministik mock motor.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use image::{Rgb, RgbImage};
use loupe_ai::faces::FaceClusterer;
use loupe_ai::jobs::{AiEngine, write_tx};
use loupe_ai::{DeterministicMockEngine, PersonId, clip};
use loupe_core::{
    Blake3Hash, ImportMode, LibraryKind, MediaFileType, MediaId, NewMediaFile, Placement, RelPath, TakenAtSource,
};
use loupe_db::models::NewLibrary;
use loupe_db::repo::library;
use loupe_db::repo::location::{self, NewLocation};
use loupe_db::{Pool, PoolConfig, insert_media_batch};
use loupe_search::albums::{self, SmartAlbumId};
use loupe_search::{
    GeoRadius, Match, PersonFilter, PlaceFilter, SearchEngine, SearchError, SearchQuery, SortMode, TimeFilter,
};
use rusqlite::params;
use tempfile::TempDir;

const AYSE: [u8; 3] = [224, 160, 96];
const MEHMET: [u8; 3] = [32, 96, 224];
const SUNSET: [u8; 3] = [240, 140, 40];
const SEA: [u8; 3] = [30, 90, 200];
const FOREST: [u8; 3] = [40, 160, 50];

const T0: i64 = 1_767_225_600_000; // 2026-01-01T00:00:00Z
const DAY: i64 = 86_400_000;

/// Test kütüphanesindeki bir fotoğrafın gerçeği (beklenen sonuçlar bundan hesaplanır).
#[derive(Debug, Clone, Copy)]
struct Photo {
    goreme: bool,
    bg: [u8; 3],
    ayse: bool,
    mehmet: bool,
    rating: u8,
    taken: i64,
}

fn photo(i: usize) -> Photo {
    let bg = [SUNSET, SEA, FOREST][(i / 2) % 3];
    Photo {
        goreme: i.is_multiple_of(2),
        bg,
        ayse: matches!(i % 5, 0 | 2),
        mehmet: matches!(i % 5, 1 | 2),
        rating: ((i / 3) % 6) as u8,
        // 2026 boyunca her fotoğraf ~3 gün arayla; yerel saat 18:00 (UTC+3).
        taken: T0 + (i as i64) * 3 * DAY + 15 * 3_600_000,
    }
}

struct Lib {
    _dir: TempDir,
    pool: Arc<Pool>,
    engine: SearchEngine,
    ids: Vec<MediaId>,
    photos: Vec<Photo>,
    ayse: PersonId,
    mehmet: PersonId,
}

fn scene(p: &Photo) -> RgbImage {
    let mut img = RgbImage::from_pixel(160, 120, Rgb(p.bg));
    let mut faces = Vec::new();
    if p.ayse {
        faces.push((AYSE, 10u32));
    }
    if p.mehmet {
        faces.push((MEHMET, 90u32));
    }
    for (inner, x0) in faces {
        for y in 30..90 {
            for x in x0..x0 + 60 {
                let border = x < x0 + 8 || x >= x0 + 52 || !(38..82).contains(&y);
                img.put_pixel(x, y, Rgb(if border { [230, 30, 220] } else { inner }));
            }
        }
    }
    img
}

/// `n` gerçek fotoğraf (yüz + CLIP) ve `filler` kadar yalnızca CLIP vektörü olan dolgu.
fn library(n: usize, filler: usize) -> Lib {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(Pool::open(&dir.path().join("catalog.db"), PoolConfig::default()).unwrap());
    let lib = pool
        .write(|c| {
            library::insert(
                c,
                &NewLibrary {
                    uuid: *b"loupe-search-tst",
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
    let loc = |locality: &str, admin1: &str, lat: f64, lon: f64| {
        pool.write(|c| {
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
    };
    let goreme = loc("Göreme", "Nevşehir", 38.6431, 34.8289);
    let istanbul = loc("İstanbul", "İstanbul", 41.0082, 28.9784);

    let photos: Vec<Photo> = (0..n).map(photo).collect();
    let rows: Vec<NewMediaFile> = (0..n + filler)
        .map(|i| {
            let name = if i < n { format!("IMG_{i:05}.JPG") } else { format!("dolgu_{i:05}.jpg") };
            NewMediaFile {
                library_id: lib,
                rel_path: RelPath::new(&format!("2026/{name}")).unwrap(),
                original_filename: name.clone(),
                placement: Placement::Copy,
                import_session_id: None,
                file_type: MediaFileType::Jpeg,
                raw_format: None,
                mime: "image/jpeg".into(),
                size_bytes: 1,
                mtime_ns: 0,
                inode: Some(i as u64 + 1),
                device_id: Some(1),
                blake3: Blake3Hash(*blake3::hash(name.as_bytes()).as_bytes()),
                phash: None,
                width: None,
                height: None,
                taken_at_utc: photos.get(i).map_or(T0 + (i as i64) * 3_600_000, |p| p.taken),
                tz_offset_min: Some(180),
                taken_at_source: TakenAtSource::Exif,
                imported_at: 0,
            }
        })
        .collect();
    let ids: Vec<MediaId> = pool
        .write(|c| insert_media_batch(c, &rows))
        .unwrap()
        .outcomes
        .iter()
        .map(|o| match o {
            loupe_core::CommitOutcome::Inserted(id) | loupe_core::CommitOutcome::Duplicate(id) => *id,
        })
        .collect();

    let engine = AiEngine::from(DeterministicMockEngine::default());
    let model = write_tx(&pool, |c| engine.register(c)).unwrap();
    let fc = FaceClusterer::default();
    let mut work = Vec::new();
    for (i, &m) in ids.iter().enumerate() {
        let (img, place, rating) = match photos.get(i) {
            Some(p) => (scene(p), if p.goreme { goreme } else { istanbul }, p.rating),
            None => (RgbImage::from_pixel(16, 16, Rgb([(i % 200) as u8, 100, 150])), istanbul, 3),
        };
        let faces: Vec<_> = engine
            .detector
            .detect(&img)
            .unwrap()
            .into_iter()
            .map(|f| {
                let e = engine.embedder.embed(&img, &f).unwrap();
                (f, e)
            })
            .collect();
        work.push((m, faces, engine.clip_image.encode_image(&img).unwrap(), place, rating));
    }
    write_tx(&pool, |c| {
        for (m, faces, clip_vec, place, rating) in &work {
            let (lat, lon) = if *place == goreme { (38.6431, 34.8289) } else { (41.0082, 28.9784) };
            c.execute(
                "UPDATE media_files SET location_id = ?2, gps_lat = ?3, gps_lon = ?4, rating = ?5 WHERE id = ?1",
                params![m.0, place, lat, lon, rating],
            )?;
            let f = fc.insert_faces(c, *m, faces, model.detector, model.embedder)?;
            for id in f {
                fc.assign(c, id)?;
            }
            clip::upsert(c, *m, clip_vec)?;
        }
        Ok(())
    })
    .unwrap();

    // Kümeleri adlandır: Ayşe'nin ve Mehmet'in ilk fotoğrafındaki yüzlerin kümeleri.
    let cluster_of = |media: MediaId, x_max: f64| -> i64 {
        pool.read(|c| {
            Ok(c.query_row(
                "SELECT cluster_id FROM faces WHERE media_id = ?1 AND bbox_x < ?2 ORDER BY bbox_x LIMIT 1",
                params![media.0, x_max],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    };
    let first = |pred: fn(&Photo) -> bool| ids[photos.iter().position(pred).unwrap()];
    let ayse_cluster = cluster_of(first(|p| p.ayse), 0.5);
    let mehmet_cluster = cluster_of(first(|p| p.mehmet && !p.ayse), 1.0);
    let ayse = write_tx(&pool, |c| fc.name_cluster(c, loupe_ai::ClusterId(ayse_cluster), "Ayşe")).unwrap();
    let mehmet = write_tx(&pool, |c| fc.name_cluster(c, loupe_ai::ClusterId(mehmet_cluster), "Mehmet")).unwrap();
    let clusters: i64 =
        pool.read(|c| Ok(c.query_row("SELECT count(*) FROM face_clusters", [], |r| r.get(0))?)).unwrap();
    assert_eq!(clusters, 2, "iki kişi, iki küme");

    let engine = SearchEngine::new(Arc::clone(&pool), engine.clip_text);
    Lib { _dir: dir, pool, engine, ids, photos, ayse, mehmet }
}

impl Lib {
    fn expect(&self, pred: impl Fn(&Photo) -> bool) -> BTreeSet<MediaId> {
        self.photos.iter().zip(&self.ids).filter(|(p, _)| pred(p)).map(|(_, m)| *m).collect()
    }
}

fn set(ids: &[MediaId]) -> BTreeSet<MediaId> {
    ids.iter().copied().collect()
}

#[test]
fn goreme_with_ayse_rated_4_plus_closest_to_sunset_in_milliseconds() {
    let lib = library(120, 3000);
    let q = SearchQuery {
        persons: Some(PersonFilter { ids: vec![lib.ayse.0], mode: Match::All }),
        place: Some(PlaceFilter { locality: Some("göreme".into()), ..Default::default() }),
        min_rating: Some(4),
        text: Some("gün batımı".into()),
        ..Default::default()
    };
    // Isınma (hazır ifade önbelleği), sonra ölçüm.
    lib.engine.search(&q).unwrap();
    let r = lib.engine.search(&q).unwrap();
    println!("hibrit arama: {} aday, {} sonuç, {:?}", r.candidates, r.hits.len(), r.elapsed);

    let all = lib.expect(|p| p.goreme && p.ayse && p.rating >= 4);
    let sunsets = lib.expect(|p| p.goreme && p.ayse && p.rating >= 4 && p.bg == SUNSET);
    assert!(!sunsets.is_empty() && sunsets.len() < all.len(), "testin anlamlı olması için karışık küme");
    assert_eq!(set(&r.ids()), all, "süzgeçler tam olarak uygulanmalı");
    assert_eq!(set(&r.ids()[..sunsets.len()]), sunsets, "gün batımları en üstte");
    assert!(r.hits.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(r.hits[0].semantic.unwrap() > 0.8);
    assert!(r.elapsed < Duration::from_millis(250), "{:?}", r.elapsed);

    // İngilizce sorgu aynı sonucu verir (çok dilli metin uzayı).
    let en = lib.engine.search(&SearchQuery { text: Some("sunset".into()), ..q.clone() }).unwrap();
    assert_eq!(set(&en.ids()[..sunsets.len()]), sunsets);
}

#[test]
fn person_modes_time_place_and_sorting() {
    let lib = library(60, 0);
    let both = SearchQuery {
        persons: Some(PersonFilter { ids: vec![lib.ayse.0, lib.mehmet.0], mode: Match::All }),
        limit: Some(1000),
        ..Default::default()
    };
    assert_eq!(set(&lib.engine.search(&both).unwrap().ids()), lib.expect(|p| p.ayse && p.mehmet));
    let any = SearchQuery { persons: Some(PersonFilter { mode: Match::Any, ..both.persons.clone().unwrap() }), ..both };
    assert_eq!(set(&lib.engine.search(&any).unwrap().ids()), lib.expect(|p| p.ayse || p.mehmet));

    // Yerel takvim: Mart 2026 (fotoğraf her 3 günde bir, 18:00 yerel).
    let march = SearchQuery {
        time: Some(TimeFilter { year: Some(2026), month: Some(3), ..Default::default() }),
        sort: SortMode::TakenAsc,
        ..Default::default()
    };
    let r = lib.engine.search(&march).unwrap();
    let start = T0 + (31 + 28) * DAY - 3 * 3_600_000;
    let end = start + 31 * DAY;
    assert_eq!(set(&r.ids()), lib.expect(|p| p.taken >= start && p.taken < end));
    let taken: Vec<i64> =
        r.ids().iter().map(|m| lib.photos[lib.ids.iter().position(|x| x == m).unwrap()].taken).collect();
    assert!(taken.windows(2).all(|w| w[0] <= w[1]));

    // Yarıçap: Göreme'ye 10 km.
    let near = SearchQuery {
        place: Some(PlaceFilter { radius: Some(GeoRadius { lat: 38.65, lon: 34.83, km: 10.0 }), ..Default::default() }),
        limit: Some(1000),
        ..Default::default()
    };
    assert_eq!(set(&lib.engine.search(&near).unwrap().ids()), lib.expect(|p| p.goreme));

    // Puana göre sıralama.
    let by_rating =
        lib.engine.search(&SearchQuery { sort: SortMode::RatingDesc, limit: Some(5), ..Default::default() }).unwrap();
    assert!(by_rating.ids().iter().all(|m| lib.photos[lib.ids.iter().position(|x| x == m).unwrap()].rating == 5));
}

#[test]
fn text_only_uses_knn_and_keywords_match_place_and_person_names() {
    let lib = library(60, 0);
    // Süzgeçsiz anlamsal: KNN yolu.
    let r =
        lib.engine.search(&SearchQuery { text: Some("orman".into()), limit: Some(5), ..Default::default() }).unwrap();
    assert!(r.ids().iter().all(|m| lib.photos[lib.ids.iter().position(|x| x == m).unwrap()].bg == FOREST));

    // Sözcük: "İSTANBUL" Türkçe küçük harfle yer adına, "ayşe" kişi adına eşleşir.
    let ist = lib
        .engine
        .search(&SearchQuery { text: Some("İSTANBUL".into()), limit: Some(1000), ..Default::default() })
        .unwrap();
    let kw: BTreeSet<MediaId> = ist.hits.iter().filter(|h| h.keyword_rank.is_some()).map(|h| h.media_id).collect();
    assert_eq!(kw, lib.expect(|p| !p.goreme));
    let ayse =
        lib.engine.search(&SearchQuery { text: Some("Ayşe".into()), limit: Some(1000), ..Default::default() }).unwrap();
    let kw: BTreeSet<MediaId> = ayse.hits.iter().filter(|h| h.keyword_rank.is_some()).map(|h| h.media_id).collect();
    assert_eq!(kw, lib.expect(|p| p.ayse));

    // Büyük sınır sqlite-vec'in KNN üst sınırını (k ≤ 4096) aşmaz.
    let wide = lib
        .engine
        .search(&SearchQuery { text: Some("orman".into()), limit: Some(5_000), ..Default::default() })
        .unwrap();
    assert!(!wide.hits.is_empty());

    // Model yokken metin yalnızca adlarda aranır.
    let kw_only = SearchEngine::keyword_only(lib.pool.clone());
    assert!(!kw_only.is_semantic());
    let r =
        kw_only.search(&SearchQuery { text: Some("Ayşe".into()), limit: Some(1000), ..Default::default() }).unwrap();
    assert!(r.hits.iter().all(|h| h.semantic.is_none()));
    assert_eq!(r.ids().into_iter().collect::<BTreeSet<_>>(), lib.expect(|p| p.ayse));
}

#[test]
fn smart_album_stores_json_rules_and_runs_dynamically() {
    let lib = library(60, 0);
    let q = SearchQuery {
        place: Some(PlaceFilter { locality: Some("Göreme".into()), ..Default::default() }),
        min_rating: Some(3),
        sort: SortMode::TakenDesc,
        ..Default::default()
    };
    let id = lib.pool.write(|c| Ok(albums::create(c, "Kapadokya favorileri", &q, true))).unwrap().unwrap();
    let before = lib.engine.run_smart_album(id).unwrap();
    assert_eq!(set(&before.ids()), lib.expect(|p| p.goreme && p.rating >= 3));

    // Dinamik: kütüphane değişince albüm de değişir.
    let first = before.ids()[0];
    lib.pool
        .write(|c| {
            c.execute("UPDATE media_files SET rating = 0 WHERE id = ?1", [first.0])?;
            Ok(())
        })
        .unwrap();
    let after = lib.engine.run_smart_album(id).unwrap();
    assert_eq!(after.hits.len(), before.hits.len() - 1);

    // Saklanan kural okunabilir JSON; sıralama sütunu da yazılır.
    let (rules, sort): (String, String) = lib
        .pool
        .read(|c| {
            Ok(c.query_row("SELECT rules, sort_mode FROM smart_albums WHERE id = ?1", [id.0], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?)
        })
        .unwrap();
    assert!(rules.contains("\"locality\":\"Göreme\""), "{rules}");
    assert_eq!(sort, "taken_desc");

    // Bilinmeyen alan, bilinmeyen sürüm ve geçersiz değer reddedilir.
    assert!(matches!(albums::parse_rules(r#"{"min_rating":4,"sql":"DROP TABLE x"}"#, 1), Err(SearchError::Rules(_))));
    assert!(matches!(albums::parse_rules("{}", 2), Err(SearchError::Invalid(_))));
    assert!(matches!(albums::parse_rules(r#"{"min_rating":9}"#, 1), Err(SearchError::Invalid(_))));
    assert!(matches!(lib.engine.run_smart_album(SmartAlbumId(999)), Err(SearchError::NotFound(999))));
}
