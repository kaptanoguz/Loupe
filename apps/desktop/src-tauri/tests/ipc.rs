#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! IPC katmanı: komut adları, argüman biçimi (camelCase) ve hata zarfı
//! (`{code, message}`) arayüzün beklediği gibi mi? Tauri'nin sahte çalışma
//! zamanıyla, WebView açmadan.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use image::{DynamicImage, ImageBuffer, Rgb};
use loupe_app::{AppConfig, AppCore};
use loupe_desktop::{Desktop, wire};
use serde_json::{Value, json};
use tauri::Manager;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;

fn write_jpeg(path: &Path, w: u32, h: u32, seed: u32) {
    let img = DynamicImage::ImageRgb8(ImageBuffer::from_fn(w, h, |x, y| {
        Rgb([((x * seed) % 256) as u8, ((y + seed) % 256) as u8, ((x + y) % 256) as u8])
    }));
    img.save_with_format(path, image::ImageFormat::Jpeg).unwrap();
}

fn call(webview: &tauri::WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Result<Value, Value> {
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|b| b.deserialize::<Value>().unwrap())
}

#[test]
fn commands_round_trip_through_ipc() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("Fotograflar");
    let card = tmp.path().join("kart");
    fs::create_dir_all(&lib_root).unwrap();
    fs::create_dir_all(&card).unwrap();
    write_jpeg(&card.join("A.JPG"), 300, 200, 1);
    write_jpeg(&card.join("B.JPG"), 200, 300, 2);

    let core = AppCore::open(AppConfig::in_dir(tmp.path())).unwrap();
    let app = wire(mock_builder()).build(mock_context(noop_assets())).unwrap();
    app.manage(Desktop { core: core.clone() });
    let w = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

    // Yol verilince klasör penceresi açılmaz.
    let added =
        call(&w, "pick_or_add_library_root", json!({ "path": lib_root, "name": "Fotoğraflar", "managed": true }))
            .unwrap();
    let lib = added["library"]["id"].as_i64().unwrap();
    assert_eq!(added["library"]["state"], "online");
    // Mevcut fotoğraflar için yerinde tarama (klasör boş).
    assert!(added["scan"]["id"].is_i64());

    let session =
        call(&w, "start_import", json!({ "req": { "libraryId": lib, "source": card, "mode": "copy" } })).unwrap();
    let id = session["id"].as_i64().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let s = call(&w, "import_status", json!({ "id": id })).unwrap();
        if s["state"] == "done" {
            assert_eq!(s["doneFiles"], 2);
            break;
        }
        assert!(Instant::now() < deadline, "import bitmedi: {s}");
        std::thread::sleep(Duration::from_millis(20));
    }

    let slice = call(
        &w,
        "get_timeline_slice",
        json!({ "req": { "view": { "kind": "all" }, "grouping": "day", "offset": 0, "limit": 100 } }),
    )
    .unwrap();
    assert_eq!(slice["total"], 2);
    let items = slice["items"].as_array().unwrap();
    let first = items[0]["id"].as_i64().unwrap();
    assert_eq!(items[0]["hash"].as_str().unwrap().len(), 64);
    assert!(items[0]["aspect"].as_f64().unwrap() > 0.0);

    let n = call(&w, "prioritize_viewport", json!({ "ids": [first] })).unwrap();
    assert!(n.as_u64().is_some());

    let culled = call(
        &w,
        "set_rating_label_favorite",
        json!({ "req": { "ids": [first], "rating": 4, "label": "red", "favorite": true } }),
    )
    .unwrap();
    assert_eq!(culled[0]["rating"], 4);
    assert_eq!(culled[0]["label"], "red");
    assert_eq!(culled[0]["favorite"], true);

    let detail = call(&w, "get_media_inspector_detail", json!({ "id": first })).unwrap();
    assert_eq!(detail["rating"], 4);

    let sidebar = call(&w, "get_sidebar", json!({})).unwrap();
    assert_eq!(sidebar["libraries"].as_array().unwrap().len(), 1);

    let status = call(&w, "get_status", json!({})).unwrap();
    assert_eq!(status["totalMedia"], 2);

    // Hata zarfı arayüzde koda göre dallanmak için sabit.
    let err = call(&w, "get_media_inspector_detail", json!({ "id": 999_999 })).unwrap_err();
    assert_eq!(err["code"], "NOT_FOUND");
    assert!(err["message"].as_str().is_some());

    // Geçersiz argüman komuta ulaşmadan reddedilir.
    assert!(call(&w, "set_rating_label_favorite", json!({ "req": { "ids": "x" } })).is_err());

    // ── Adım 3B ──
    // Palet: belirteçler çipe, sorgu akıllı albüme; metinli sorgu "En alakalı" bölümüyle açılır.
    let pal = call(&w, "search_palette", json!({ "req": { "input": "rating:>=4 kişi:Yok" } })).unwrap();
    assert_eq!(pal["total"], 1);
    assert_eq!(pal["chips"][0]["kind"], "rating");
    assert_eq!(pal["chips"][1]["ok"], false);
    assert_eq!(pal["semantic"], false, "AI kapalı: yalnızca sözcük araması");
    let album =
        call(&w, "save_smart_album", json!({ "name": "Dört yıldız", "query": pal["query"], "pinned": true })).unwrap();
    let smart = call(
        &w,
        "get_timeline_slice",
        json!({ "req": { "view": { "kind": "smart_album", "id": album["id"] }, "offset": 0, "limit": 10 } }),
    )
    .unwrap();
    assert_eq!(smart["items"][0]["id"].as_i64(), Some(first));
    let text = call(&w, "search_palette", json!({ "req": { "input": "A.JPG" } })).unwrap();
    let ranked = call(
        &w,
        "get_timeline_slice",
        json!({ "req": { "view": { "kind": "search", "query": text["query"] }, "offset": 0, "limit": 10 } }),
    )
    .unwrap();
    assert_eq!(ranked["sections"][0]["title"], "En alakalı");
    call(&w, "delete_smart_album", json!({ "id": album["id"] })).unwrap();

    let people = call(&w, "list_people", json!({})).unwrap();
    assert_eq!(people["named"].as_array().unwrap().len(), 0);
    assert!(call(&w, "list_events", json!({})).unwrap().is_array());
    let models = call(&w, "models_status", json!({})).unwrap();
    assert_eq!(models["ready"], false);
    assert!(
        models["files"].as_array().unwrap().iter().any(|f| f["file"] == "scrfd_10g.onnx" && f["state"] == "missing")
    );
    assert_eq!(models["job"]["running"], false);
    let ai = call(&w, "ai_status", json!({})).unwrap();
    assert_eq!(ai["mode"], "off");
    assert_eq!(call(&w, "set_resource_mode", json!({ "mode": "quiet" })).unwrap()["resourceMode"], "quiet");
    assert_eq!(call(&w, "set_resource_mode", json!({ "mode": "x" })).unwrap_err()["code"], "INVALID");

    // Wi-Fi: sonuç makinenin ağına bağlı (LAN adresi yoksa koda göre ayrılan hata).
    match call(&w, "pairing_qr", json!({})) {
        Ok(qr) => {
            assert_eq!(qr["ttlSecs"], 60);
            assert!(qr["svg"].as_str().unwrap().contains("<path"), "QR yalnızca rect/path");
            assert!(!qr["addrs"].as_array().unwrap().is_empty());
            call(&w, "cancel_pairing", json!({})).unwrap();
        }
        Err(e) => assert_eq!(e["code"], "INVALID"),
    }
    assert_eq!(call(&w, "list_devices", json!({})).unwrap(), json!([]));
    assert!(call(&w, "set_conflict_policy", json!({ "id": 1, "policy": "sometimes" })).is_err(), "bilinmeyen politika");

    // Bulut ve kurtarma anahtarı.
    let cloud = call(&w, "cloud_settings", json!({})).unwrap();
    assert_eq!(cloud["encryption"]["configured"], false);
    assert_eq!(cloud["secretBackend"], "file");
    let draft = call(&w, "generate_recovery_key", json!({})).unwrap();
    let answers: Vec<Value> = draft["challenge"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| json!({ "group": i, "value": draft["groups"][i.as_u64().unwrap() as usize] }))
        .collect();
    let enc = call(&w, "confirm_recovery_key", json!({ "answers": answers })).unwrap();
    assert_eq!(enc["recipient"], draft["recipient"]);
    assert_eq!(call(&w, "check_recovery_key", json!({ "key": draft["secret"] })).unwrap(), true);
    let nas = tmp.path().join("nas");
    let saved = call(
        &w,
        "save_cloud_target",
        json!({ "input": { "name": "NAS", "provider": "local", "config": { "path": nas }, "bandwidthMbps": 5 } }),
    )
    .unwrap();
    assert_eq!(saved["targets"][0]["encrypted"], true);
    assert_eq!(saved["targets"][0]["bandwidthMbps"], 5.0);

    // Doğrulama: bit-rot raporu.
    let started = call(&w, "verify_library", json!({})).unwrap();
    assert_eq!(started, json!([lib]));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let runs = call(&w, "verify_runs", json!({ "limit": 3 })).unwrap();
        if runs[0]["state"] == "done" {
            assert_eq!(runs[0]["ok"], 2);
            break;
        }
        assert!(Instant::now() < deadline, "{runs}");
        std::thread::sleep(Duration::from_millis(20));
    }

    tauri::async_runtime::block_on(core.shutdown());
}
