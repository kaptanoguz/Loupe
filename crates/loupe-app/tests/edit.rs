#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Basit düzenleme uçtan uca: tarif kaydı, ızgaradaki en/boy oranı,
//! `thumb://…?e=` düzenlenmiş küçük resimleri, `.loupe.json`, orijinale dönüş
//! ve JPEG dışa aktarma. Orijinal dosyaya hiç dokunulmaz.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::{Rgb, RgbImage};
use loupe_app::{AiMode, AppConfig, AppCore, Arrange, EditCrop, EditRecipe, ImportRequest, TimelineRequest, View};
use loupe_jobs::{ResourceMode, WorkerConfig};
use tempfile::TempDir;

struct Fx {
    _tmp: TempDir,
    core: Arc<AppCore>,
    lib_root: PathBuf,
}

fn req(uri: &str) -> http::Request<()> {
    http::Request::get(uri).body(()).unwrap()
}

/// 900×600: sol yarı kırmızı, sağ yarı mavi.
fn photo(path: &Path) {
    let img = RgbImage::from_fn(900, 600, |x, _| if x < 450 { Rgb([220, 40, 40]) } else { Rgb([40, 40, 220]) });
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95).encode_image(&img).unwrap();
    fs::write(path, bytes).unwrap();
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let lib_root = tmp.path().join("Fotograflar");
        let card = tmp.path().join("kart");
        fs::create_dir_all(&lib_root).unwrap();
        fs::create_dir_all(&card).unwrap();
        photo(&card.join("IMG_0001.JPG"));
        let mut cfg = AppConfig::in_dir(tmp.path());
        cfg.ai = AiMode::Mock;
        cfg.workers = Some(WorkerConfig {
            workers: 2,
            reserved_immediate: 1,
            idle_wait: Duration::from_millis(50),
            mode: ResourceMode::Full,
            ..WorkerConfig::default()
        });
        let core = AppCore::open(cfg).unwrap();
        let (lib, _) = core.add_library_root(&lib_root, Some("Fotoğraflar"), true).unwrap();
        let s = core
            .start_import(&ImportRequest { library_id: lib.id, source: card, mode: "copy".into(), layout: None })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !matches!(core.import_status(s.id).unwrap().state, "done" | "failed" | "cancelled") {
            assert!(Instant::now() < deadline, "import bitmedi");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self { _tmp: tmp, core, lib_root }
    }

    fn tile(&self) -> loupe_app::TileItem {
        self.core
            .timeline_slice(&TimelineRequest { view: View::All, arrange: Arrange::default(), offset: 0, limit: 10 })
            .unwrap()
            .items
            .remove(0)
    }

    /// İşçiler küçük resimleri üretene dek bekler.
    fn wait_thumbs(&self, hash: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        for size in ["sm", "md", "lg"] {
            while self.core.handle_thumb(&req(&format!("thumb://{size}/{hash}"))).status() != 200 {
                assert!(Instant::now() < deadline, "{size} küçük resmi üretilmedi");
                std::thread::sleep(Duration::from_millis(30));
            }
        }
    }
}

#[test]
fn edits_are_saved_rendered_exported_and_reverted_without_touching_the_original() {
    let fx = Fx::new();
    let tile = fx.tile();
    assert_eq!(tile.edit, 0);
    assert!((tile.aspect - 1.5).abs() < 1e-3);
    fx.wait_thumbs(&tile.hash);
    let original = fx.lib_root.join(fx.core.inspector_detail(tile.id).unwrap().rel_path);
    let before = fs::read(&original).unwrap();

    let empty = fx.core.edit_state(tile.id).unwrap();
    assert_eq!((empty.revision, empty.recipe), (0, EditRecipe::default()));

    // Sol yarıyı kırp, saat yönünde döndür, biraz aydınlat.
    let recipe = EditRecipe {
        quarter: 1,
        crop: Some(EditCrop { x: 0.0, y: 0.0, w: 1.0, h: 0.5 }),
        brightness: 20.0,
        ..EditRecipe::default()
    };
    let saved = fx.core.save_edit(tile.id, recipe).unwrap();
    assert!(saved.revision > 0);
    assert_eq!(saved.recipe, recipe);
    assert_eq!(saved.sidecar, "written");
    // 900×600 → döndürülünce 600×900 → üst yarı: 600×450.
    assert!((saved.aspect - 600.0 / 450.0).abs() < 1e-3, "{}", saved.aspect);

    let tile2 = fx.tile();
    assert_eq!(tile2.edit, saved.revision);
    assert!((tile2.aspect - saved.aspect).abs() < 1e-3);
    assert_eq!(fx.core.edit_state(tile.id).unwrap().recipe, recipe);

    // Düzenlenmiş küçük resim: JPEG, düzenlenmiş oran; ikinci istek önbellekten.
    let uri = format!("thumb://md/{}?e={}", tile.hash, saved.revision);
    let resp = fx.core.handle_thumb(&req(&uri));
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "image/jpeg");
    assert!(resp.headers()["cache-control"].to_str().unwrap().contains("immutable"));
    let img = image::load_from_memory(resp.body()).unwrap().to_rgb8();
    assert_eq!((img.width(), img.height()), (600, 450));
    // Döndürmeden sonra üst yarı = orijinalin sol yarısı (kırmızı).
    let p = img.get_pixel(300, 200).0;
    assert!(p[0] > 200 && p[2] < 100, "{p:?}");
    assert_eq!(fx.core.handle_thumb(&req(&uri)).body(), resp.body());
    // Eski revizyonla istenirse güncel olan verilir ama saklanmaz.
    let stale = fx.core.handle_thumb(&req(&format!("thumb://sm/{}?e={}", tile.hash, saved.revision - 1)));
    assert_eq!(stale.status(), 200);
    assert_eq!(stale.headers()["cache-control"], "no-store");
    let orig = fx.core.handle_thumb(&req(&format!("thumb://orig/{}?e={}", tile.hash, saved.revision)));
    let full = image::load_from_memory(orig.body()).unwrap();
    assert_eq!((full.width(), full.height()), (600, 450));

    // Tarif .loupe.json'da da var.
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(loupe_media::Sidecar::for_media(&original).json_path()).unwrap()).unwrap();
    assert_eq!(json["edits"]["ops"][0]["op"], "rotate");

    // Dışa aktarma: tam çözünürlük, .jpg eklenir.
    let out = fx.lib_root.join("disari");
    let exp = fx.core.export_edit(tile.id, &out, 90).unwrap();
    assert_eq!(exp.path, format!("{}.jpg", out.display()));
    assert_eq!((exp.width, exp.height, exp.low_res), (600, 450, false));
    let written = image::open(&exp.path).unwrap();
    assert_eq!((written.width(), written.height()), (600, 450));
    assert!(fx.core.export_edit(tile.id, &original, 90).is_err(), "orijinalin üzerine yazılmaz");
    let (dir, name) = fx.core.export_suggestion(tile.id).unwrap();
    assert_eq!(dir.as_deref(), original.parent());
    assert_eq!(name, "IMG_0001-edit.jpg");

    // Otomatik iyileştirme yalnızca öneri döndürür.
    let auto = fx.core.auto_tone(tile.id).unwrap();
    assert!(auto.brightness.abs() <= 40.0);
    assert_eq!(fx.core.edit_state(tile.id).unwrap().revision, saved.revision);

    // Orijinale dönüş: ızgara, küçük resim ve sidecar eski hâline.
    let reverted = fx.core.revert_edit(tile.id).unwrap();
    assert_eq!((reverted.revision, reverted.recipe), (0, EditRecipe::default()));
    let tile3 = fx.tile();
    assert_eq!(tile3.edit, 0);
    assert!((tile3.aspect - 1.5).abs() < 1e-3);
    let back = fx.core.handle_thumb(&req(&uri));
    assert_eq!(back.status(), 200);
    assert_eq!(back.headers()["content-type"], "image/webp", "düzenleme yoksa orijinal küçük resim");
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(loupe_media::Sidecar::for_media(&original).json_path()).unwrap()).unwrap();
    assert!(json.get("edits").is_none());

    assert_eq!(fs::read(&original).unwrap(), before, "orijinal dosya değişmemeli");
    assert!(fx.core.save_edit(tile.id, EditRecipe { contrast: f32::NAN, ..EditRecipe::default() }).is_err());
}
