//! loupe-media entegrasyon testleri: gerçek EXIF alanlı fikstürler, GeoNames
//! alt kümesi, küçük resim üretimi ve `thumb://` işleyicisi.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use http::{Method, Request, StatusCode};
use image::GenericImageView;
use loupe_core::{Blake3Hash, ColorLabel, TakenAtSource};
use loupe_media::geo::haversine_km;
use loupe_media::thumb::{ImageBackend, SourceKind, ThumbFormat, webp_dimensions};
use loupe_media::{
    GeoIndex, MediaError, Sidecar, SidecarData, ThumbCache, ThumbProtocol, ThumbSize, Thumbnailer, format_dms,
    read_metadata,
};
use tempfile::TempDir;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn hash_file(p: &Path) -> Blake3Hash {
    Blake3Hash(*blake3::hash(&std::fs::read(p).unwrap()).as_bytes())
}

fn thumbnailer(dir: &TempDir) -> Thumbnailer {
    Thumbnailer::new(ThumbCache::new(dir.path().join("thumbs")), Box::new(ImageBackend))
}

// ───────────────────────────────── EXIF ─────────────────────────────────

#[test]
fn reads_camera_exposure_time_and_gps() {
    let m = read_metadata(&fixture("exif/goreme_canon_r6.jpg")).unwrap();
    assert_eq!(m.camera_make.as_deref(), Some("Canon"));
    assert_eq!(m.camera_model.as_deref(), Some("Canon EOS R6"));
    assert_eq!(m.lens_model.as_deref(), Some("RF24-105mm F4 L IS USM"));
    assert_eq!(m.aperture_f, Some(8.0));
    assert_eq!(m.exposure_s, Some(0.004));
    assert_eq!(m.exposure_display().as_deref(), Some("1/250 s"));
    assert_eq!(m.iso, Some(100));
    assert_eq!(m.focal_length_mm, Some(35.0));
    assert_eq!((m.width, m.height, m.orientation), (Some(1200), Some(800), Some(1)));

    // 2025-10-24 06:42:17.25 +03:00 → 03:42:17.25 UTC
    let t = m.taken_at.unwrap();
    assert_eq!(t.utc_ms, 1_761_277_337_250);
    assert_eq!(t.tz_offset_min, Some(180));
    assert_eq!(t.source, TakenAtSource::Exif);

    let g = m.gps.unwrap();
    assert!((g.lat - 38.64312).abs() < 1e-6, "{}", g.lat);
    assert!((g.lon - 34.82897).abs() < 1e-6, "{}", g.lon);
    assert_eq!(g.alt_m, Some(1104.5));
    assert_eq!(format_dms(g.lat, g.lon).unwrap(), "38° 38' 35.2\" N, 034° 49' 44.3\" E");
}

#[test]
fn timezone_from_gps_timestamp_and_southern_western_hemisphere() {
    let m = read_metadata(&fixture("exif/buenos_aires_gps_time.jpg")).unwrap();
    assert_eq!(m.camera_make.as_deref(), Some("NIKON CORPORATION"));
    assert_eq!(m.aperture_f, Some(2.8));
    // Yerel 10:00, GPS UTC 13:00 → −03:00
    let t = m.taken_at.unwrap();
    assert_eq!(t.tz_offset_min, Some(-180));
    assert_eq!(t.utc_ms, 1_740_834_000_000);
    let g = m.gps.unwrap();
    assert!(g.lat < 0.0 && g.lon < 0.0);
    assert_eq!(format_dms(g.lat, g.lon).unwrap(), "34° 36' 13.4\" S, 058° 22' 53.7\" W");
}

#[test]
fn file_without_exif_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("plain.png");
    image::RgbImage::new(40, 30).save(&p).unwrap();
    let m = read_metadata(&p).unwrap();
    assert_eq!((m.width, m.height, m.taken_at, m.gps), (Some(40), Some(30), None, None));
}

// ─────────────────────────────── GeoNames ───────────────────────────────

#[test]
fn offline_reverse_geocoding() {
    let idx = GeoIndex::load_dir(&fixture("geonames")).unwrap();
    assert_eq!(idx.len(), 120);
    assert!(idx.dataset_ver().starts_with("geonames-cities500-"));

    let p = idx.reverse(38.64312, 34.82897).unwrap().unwrap();
    assert_eq!((p.geoname_id, p.name.as_str()), (313_947, "Göreme"));
    assert_eq!((p.country_code.as_str(), p.country.as_str()), ("TR", "Turkey"));
    assert_eq!(p.admin1.as_deref(), Some("Nevşehir"));
    assert!(p.distance_km < 0.1);
    assert_eq!(p.display(), "Göreme, Nevşehir, Turkey");

    let p = idx.reverse(41.0054, 28.9768).unwrap().unwrap();
    assert_eq!((p.name.as_str(), p.admin1.as_deref()), ("Sultanahmet", Some("Istanbul")));

    let p = idx.reverse(-34.60372, -58.38159).unwrap().unwrap();
    assert_eq!((p.name.as_str(), p.country.as_str()), ("Buenos Aires", "Argentina"));

    // Açık deniz: en yakın yerleşim yarıçap dışında.
    assert_eq!(idx.reverse(0.0, -30.0).unwrap(), None);
    assert!(matches!(idx.reverse(95.0, 0.0), Err(MediaError::InvalidCoordinate { .. })));
}

#[test]
fn reverse_geocoding_across_the_antimeridian() {
    // 179.8° D'deki nokta için en yakın yerleşim 179.1° B'deki Egvekinot'tur;
    // düz enlem/boylam karşılaştırması 358° fark görüp yanlış yeri seçerdi.
    let idx = GeoIndex::load_dir(&fixture("geonames")).unwrap().with_max_distance_km(100.0);
    let p = idx.reverse(66.0, 179.8).unwrap().unwrap();
    assert_eq!((p.name.as_str(), p.admin1.as_deref(), p.country.as_str()), ("Egvekinot", Some("Chukotka"), "Russia"));
    assert!((p.distance_km - haversine_km(66.0, 179.8, p.lat, p.lon)).abs() < 1e-9);
    assert!(p.distance_km < 61.0);
}

// ───────────────────────────── Küçük resimler ─────────────────────────────

#[test]
fn generates_all_sizes_content_addressed_and_without_upscaling() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.png");
    image::RgbImage::from_fn(2400, 1600, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]))
        .save(&src)
        .unwrap();
    let hash = hash_file(&src);
    let t = thumbnailer(&dir);

    let mut out = t.generate(&src, &hash, None, &[ThumbSize::Sm, ThumbSize::Md, ThumbSize::Lg]).unwrap();
    out.sort_by_key(|g| g.size);
    let dims: Vec<_> = out.iter().map(|g| (g.size, g.width, g.height)).collect();
    assert_eq!(dims, vec![(ThumbSize::Sm, 256, 171), (ThumbSize::Md, 768, 512), (ThumbSize::Lg, 1920, 1280)]);
    for g in &out {
        let hex = hash.to_hex();
        assert_eq!(g.path, dir.path().join("thumbs").join(&hex[..2]).join(format!("{hex}_{}.webp", g.size)));
        let bytes = std::fs::read(&g.path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        assert_eq!(webp_dimensions(&bytes).unwrap(), (g.width, g.height));
        assert_eq!(g.byte_size, bytes.len() as u64);
        assert_eq!((g.format, g.source_kind), (ThumbFormat::Webp, SourceKind::Decode));
    }
    // Geçici dosya kalmadı.
    let shard = out[0].path.parent().unwrap();
    assert_eq!(std::fs::read_dir(shard).unwrap().count(), 3);
    // Önbellekte olan yeniden üretilmez.
    assert!(t.generate(&src, &hash, None, &[ThumbSize::Sm]).unwrap().is_empty());

    // Kaynak 1200×800: lg büyütülmez.
    let small = fixture("exif/goreme_canon_r6.jpg");
    let g = t.generate(&small, &hash_file(&small), Some(1), &[ThumbSize::Lg]).unwrap();
    assert_eq!((g[0].width, g[0].height), (1200, 800));
}

#[test]
fn applies_exif_orientation() {
    let dir = tempfile::tempdir().unwrap();
    let src = fixture("exif/orientation_6.jpg");
    let meta = read_metadata(&src).unwrap();
    assert_eq!((meta.width, meta.height, meta.orientation), (Some(300), Some(200), Some(6)));

    // Hem metadata'dan gelen değerle hem dosyadan okunarak.
    for orientation in [meta.orientation, None] {
        let t = thumbnailer(&dir);
        let hash = Blake3Hash([u8::from(orientation.is_some()); 32]);
        let g = t.generate(&src, &hash, orientation, &[ThumbSize::Md]).unwrap();
        assert_eq!((g[0].width, g[0].height), (200, 300), "dikey olmalı");
        let img = image::load_from_memory(&std::fs::read(&g[0].path).unwrap()).unwrap();
        assert_eq!(img.dimensions(), (200, 300));
        // 90° saat yönü: saklanan sol üst (kırmızı) sağ üste, sağ alt (mavi) sol alta gelir.
        let px = |x, y| img.get_pixel(x, y).0;
        let [r, g_, b, _] = px(190, 10);
        assert!(r > 200 && g_ < 60 && b < 60, "sağ üst kırmızı değil: {:?}", px(190, 10));
        let [r, g_, b, _] = px(10, 290);
        assert!(b > 200 && r < 60 && g_ < 60, "sol alt mavi değil: {:?}", px(10, 290));
        let [r, g_, b, _] = px(10, 10);
        assert!(r > 200 && g_ > 200 && b > 200, "sol üst beyaz değil");
    }
}

/// IFD0'ında `JPEGInterchangeFormat` ile gömülü önizleme taşıyan en küçük NEF benzeri dosya.
fn synthetic_nef(preview: &[u8]) -> Vec<u8> {
    let mut f = b"II*\0".to_vec();
    f.extend_from_slice(&8u32.to_le_bytes());
    f.extend_from_slice(&2u16.to_le_bytes());
    let data_off = 8 + 2 + 2 * 12 + 4;
    for (tag, val) in [(0x0201u16, data_off), (0x0202, u32::try_from(preview.len()).unwrap())] {
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&4u16.to_le_bytes());
        f.extend_from_slice(&1u32.to_le_bytes());
        f.extend_from_slice(&val.to_le_bytes());
    }
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(preview);
    f
}

#[test]
fn raw_thumbnail_comes_from_embedded_preview() {
    let dir = tempfile::tempdir().unwrap();
    let nef = dir.path().join("DSC_0001.NEF");
    std::fs::write(&nef, synthetic_nef(&std::fs::read(fixture("exif/goreme_canon_r6.jpg")).unwrap())).unwrap();
    let t = thumbnailer(&dir);
    let g = t.generate(&nef, &hash_file(&nef), None, &[ThumbSize::Md]).unwrap();
    assert_eq!((g[0].width, g[0].height, g[0].source_kind), (768, 512, SourceKind::EmbeddedPreview));

    let bare = dir.path().join("EMPTY.NEF");
    std::fs::write(&bare, b"II*\0\x08\0\0\0\0\0\0\0\0\0").unwrap();
    let err = t.generate(&bare, &hash_file(&bare), None, &[ThumbSize::Sm]).unwrap_err();
    assert!(matches!(err, MediaError::NoEmbeddedPreview(_)) && err.is_permanent());
}

#[test]
fn corrupt_images_fail_permanently_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let t = thumbnailer(&dir);
    let garbage = dir.path().join("garbage.jpg");
    std::fs::write(&garbage, b"\xFF\xD8\xFF\xE0 bu bir jpeg degil").unwrap();
    // Başlığın ortasında kesilmiş dosya. (Tarama verisinin ortasında kesilen JPEG
    // ise çözücü tarafından gri doldurularak gösterilir; bu kasıtlıdır.)
    let truncated = dir.path().join("truncated.jpg");
    let full = std::fs::read(fixture("exif/goreme_canon_r6.jpg")).unwrap();
    std::fs::write(&truncated, &full[..200]).unwrap();
    let heic = dir.path().join("x.heic");
    std::fs::write(&heic, b"....ftypheic").unwrap();

    for p in [&garbage, &truncated, &heic] {
        let err = t.generate(p, &hash_file(p), None, &[ThumbSize::Sm]).unwrap_err();
        assert!(err.is_permanent(), "{p:?}: {err}");
    }
    // Eksik dosya kalıcı değildir (disk takılınca yeniden denenir).
    let err = t.generate(&dir.path().join("yok.jpg"), &Blake3Hash([1; 32]), None, &[ThumbSize::Sm]).unwrap_err();
    assert!(!err.is_permanent());
}

// ─────────────────────────────── thumb:// ───────────────────────────────

#[test]
fn thumb_protocol_serves_raw_webp_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let src = fixture("exif/goreme_canon_r6.jpg");
    let hash = hash_file(&src);
    let t = thumbnailer(&dir);
    let g = t.generate(&src, &hash, None, &[ThumbSize::Sm]).unwrap();

    let misses = Arc::new(Mutex::new(Vec::new()));
    let m = Arc::clone(&misses);
    let proto = ThumbProtocol::new(t.cache().clone()).with_miss_hook(move |h, s| m.lock().unwrap().push((h, s)));
    let get = |uri: String| proto.handle(&Request::builder().uri(uri).body(()).unwrap());

    let uri = ThumbProtocol::uri_for(ThumbSize::Sm, &hash);
    assert_eq!(uri, format!("thumb://sm/{}", hash.to_hex()));
    let resp = get(uri.clone());
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "image/webp");
    assert_eq!(resp.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(resp.body().as_ref(), std::fs::read(&g[0].path).unwrap().as_slice());
    assert_eq!(webp_dimensions(resp.body()).unwrap(), (256, 171));
    let etag = resp.headers()["etag"].clone();

    // Linux WebKit ve Windows WebView2 biçimleri aynı baytı döndürür.
    for alt in
        [format!("thumb://localhost/sm/{}", hash.to_hex()), format!("http://thumb.localhost/sm/{}", hash.to_hex())]
    {
        assert_eq!(get(alt).body(), resp.body());
    }

    let cond = Request::builder().uri(&uri).header("if-none-match", etag).body(()).unwrap();
    let r304 = proto.handle(&cond);
    assert_eq!(r304.status(), StatusCode::NOT_MODIFIED);
    assert!(r304.body().is_empty());

    let head = proto.handle(&Request::builder().method(Method::HEAD).uri(&uri).body(()).unwrap());
    assert_eq!(head.status(), StatusCode::OK);
    assert!(head.body().is_empty());
    assert_eq!(head.headers()["content-length"], resp.body().len().to_string().as_str());

    // md henüz üretilmedi → 404 ve kancaya bildirim.
    let miss = get(ThumbProtocol::uri_for(ThumbSize::Md, &hash));
    assert_eq!(miss.status(), StatusCode::NOT_FOUND);
    assert_eq!(miss.headers()["cache-control"], "no-store");
    assert_eq!(misses.lock().unwrap().as_slice(), &[(hash, ThumbSize::Md)]);

    assert_eq!(get("thumb://xl/abc".into()).status(), StatusCode::BAD_REQUEST);
    let post = proto.handle(&Request::builder().method(Method::POST).uri(&uri).body(()).unwrap());
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
}

// ──────────────────────────────── Sidecar ────────────────────────────────

#[test]
fn sidecars_round_trip_without_touching_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("IMG_0001.CR2");
    std::fs::write(&raw, b"orijinal RAW baytlari").unwrap();
    let before = hash_file(&raw);

    let sc = Sidecar::for_media(&raw);
    assert_eq!(sc.read().unwrap(), None);
    let data = SidecarData { rating: Some(4), color_label: Some(ColorLabel::Blue), tags: vec!["Kapadokya".into()] };
    sc.write(&data).unwrap();
    assert_eq!(sc.xmp_path(), dir.path().join("IMG_0001.CR2.xmp"));
    assert_eq!(sc.read().unwrap(), Some(data.clone()));
    assert_eq!(hash_file(&raw), before, "orijinal dosya değişmemeli");

    // JSON'daki bilinmeyen alanlar korunur.
    let json_path = sc.json_path();
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    v["edits"] = serde_json::json!([{ "op": "crop", "rect": [0, 0, 10, 10] }]);
    std::fs::write(&json_path, serde_json::to_vec(&v).unwrap()).unwrap();
    sc.write(&SidecarData { rating: Some(1), ..data }).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    assert_eq!(v["rating"], 1);
    assert_eq!(v["edits"][0]["op"], "crop");
}

#[test]
fn lightroom_style_xmp_is_found_and_updated_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("DSC_0412.NEF");
    std::fs::write(&raw, b"x").unwrap();
    let lr = dir.path().join("DSC_0412.xmp");
    std::fs::write(
        &lr,
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="3" crs:Exposure2012="+0.35"/>
</rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    let sc = Sidecar::for_media(&raw);
    assert_eq!(sc.read().unwrap().unwrap().rating, Some(3));
    assert_eq!(sc.xmp_path(), lr);
    sc.write(&SidecarData { rating: Some(5), color_label: None, tags: vec![] }).unwrap();
    let text = std::fs::read_to_string(&lr).unwrap();
    assert!(text.contains("crs:Exposure2012=\"+0.35\""), "{text}");
    assert!(!dir.path().join("DSC_0412.NEF.xmp").exists());
    assert_eq!(sc.read().unwrap().unwrap().rating, Some(5));
}

/// `vips` özelliği: `vipsthumbnail` kuruluysa (CI'da kurulur) aynı sözleşmeyi sağlamalı.
#[cfg(feature = "vips")]
#[test]
fn vips_backend_matches_contract() {
    use loupe_media::thumb::{ThumbnailBackend, VipsBackend};
    let Some(vips) = VipsBackend::detect() else {
        eprintln!("vipsthumbnail bulunamadı; test atlandı");
        return;
    };
    let out = vips
        .render(&fixture("exif/orientation_6.jpg"), &[ThumbSize::Md, ThumbSize::Sm], None, ThumbFormat::Webp)
        .unwrap();
    let dims: Vec<_> = out.iter().map(|t| (t.size, t.width, t.height)).collect();
    assert_eq!(dims, vec![(ThumbSize::Md, 200, 300), (ThumbSize::Sm, 171, 256)]);
    assert_eq!(webp_dimensions(&out[1].bytes).unwrap(), (171, 256));

    let dir = tempfile::tempdir().unwrap();
    let garbage = dir.path().join("garbage.jpg");
    std::fs::write(&garbage, b"not an image").unwrap();
    assert!(vips.render(&garbage, &[ThumbSize::Sm], None, ThumbFormat::Webp).unwrap_err().is_permanent());
}
