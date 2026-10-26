#![cfg(feature = "ort")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Gerçek modellerle duman testi (elle): `loupe-models install` sonrası
//! indirilen ONNX Runtime ve SCRFD/ArcFace/M-CLIP dosyalarıyla çıkarım.
//!
//! ```text
//! LOUPE_MODELS_DIR=~/.local/share/loupe/models LOUPE_FACE_IMAGE=yuzler.jpg \
//!   cargo test -p loupe-ai --features ort --test real_models -- --ignored --nocapture
//! ```
//!
//! M-CLIP testi yalnızca `LOUPE_MODELS_DIR` ister (metin kodlayıcısının harici
//! ağırlıkları `mclip_text/` altında olmalı).

use std::path::PathBuf;

use loupe_ai::model::{ModelInfo, ModelTask};
use loupe_ai::onnx::{ArcFace, ClipVision, MclipText, Scrfd, init_runtime};
use loupe_ai::{ClipImageEncoder, ClipTextEncoder, FaceDetector, FaceEmbedder};

fn models_dir() -> PathBuf {
    let dir = PathBuf::from(std::env::var("LOUPE_MODELS_DIR").expect("LOUPE_MODELS_DIR"));
    init_runtime(Some(&dir.join("libonnxruntime.so"))).unwrap();
    dir
}

#[test]
#[ignore = "indirilmiş M-CLIP modelleri gerekir"]
fn mclip_text_and_image_share_one_multilingual_space() {
    let dir = models_dir();
    let text_path = dir.join("mclip_text/model.onnx");
    let text = MclipText::load(
        ModelInfo::from_file(ModelTask::ClipText, "mclip_text", "1", Some(512), &text_path).unwrap(),
        &text_path,
        &dir.join("mclip_tokenizer.json"),
        2,
    )
    .unwrap();
    let enc = |s: &str| text.encode_text(s).unwrap();
    let (tr, en, other) = (enc("kırmızı bir araba"), enc("a red car"), enc("karlı dağlarda kayak yapan insanlar"));
    assert_eq!(tr.as_slice().len(), 512);
    let (same, diff) = (tr.cosine(&en), tr.cosine(&other));
    println!("tr~en {same:.3} · tr~başka {diff:.3}");
    assert!(same > diff, "Türkçe ve İngilizce aynı anlam birbirine daha yakın olmalı");

    let image_path = dir.join("mclip_image.onnx");
    let image = ClipVision::load(
        ModelInfo::from_file(ModelTask::ClipImage, "mclip_image", "1", Some(512), &image_path).unwrap(),
        &image_path,
        2,
    )
    .unwrap();
    // Düz kırmızı görüntü "kırmızı" metnine "kar"dan yakın olmalı.
    let red = image::RgbImage::from_pixel(224, 224, image::Rgb([200, 20, 20]));
    let v = image.encode_image(&red).unwrap();
    assert_eq!(v.as_slice().len(), 512);
    let (r, s) = (v.cosine(&enc("kırmızı")), v.cosine(&enc("beyaz kar")));
    println!("kırmızı görüntü ~ 'kırmızı' {r:.3} · ~ 'beyaz kar' {s:.3}");
    assert!(r > s);
}

#[test]
#[ignore = "indirilmiş modeller ve yüz içeren bir fotoğraf gerekir"]
fn scrfd_and_arcface_run_on_downloaded_models() {
    let dir = models_dir();
    let image = image::open(std::env::var("LOUPE_FACE_IMAGE").expect("LOUPE_FACE_IMAGE")).unwrap().to_rgb8();

    let det_path = dir.join("scrfd_10g.onnx");
    let det = Scrfd::load(
        ModelInfo::from_file(ModelTask::FaceDetect, "scrfd_10g", "1", None, &det_path).unwrap(),
        &det_path,
        2,
    )
    .unwrap();
    let faces = det.detect(&image).unwrap();
    println!("yüz: {}", faces.len());
    assert!(!faces.is_empty(), "fotoğrafta yüz bulunmalı");
    for f in &faces {
        assert!(f.score >= 0.5 && f.score <= 1.0);
    }

    let emb_path = dir.join("arcface_w600k_r50.onnx");
    let emb = ArcFace::load(
        ModelInfo::from_file(ModelTask::FaceEmbed, "arcface_w600k_r50", "1", Some(512), &emb_path).unwrap(),
        &emb_path,
        2,
    )
    .unwrap();
    let v = emb.embed(&image, &faces[0]).unwrap();
    let norm: f32 = v.as_slice().iter().map(|x| x * x).sum::<f32>().sqrt();
    assert_eq!(v.as_slice().len(), 512);
    assert!((norm - 1.0).abs() < 1e-3, "{norm}");
    // Aynı yüz kendine en yakın; farklı yüzle benzerlik daha düşük.
    if faces.len() > 1 {
        let w = emb.embed(&image, &faces[1]).unwrap();
        let same = v.cosine(&emb.embed(&image, &faces[0]).unwrap());
        let other = v.cosine(&w);
        println!("aynı {same:.3} · farklı {other:.3}");
        assert!(same > other);
    }
}
