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
    assert!(added["scan"].is_null());

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

    tauri::async_runtime::block_on(core.shutdown());
}
