#![cfg(feature = "ort")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Gerçek modellerle duman testi (elle): `loupe-models install` sonrası
//! indirilen ONNX Runtime ve SCRFD/ArcFace dosyalarıyla çıkarım.
//!
//! ```text
//! LOUPE_MODELS_DIR=~/.local/share/loupe/models LOUPE_FACE_IMAGE=yuzler.jpg \
//!   cargo test -p loupe-ai --features ort --test real_models -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use loupe_ai::model::{ModelInfo, ModelTask};
use loupe_ai::onnx::{ArcFace, Scrfd, init_runtime};
use loupe_ai::{FaceDetector, FaceEmbedder};

#[test]
#[ignore = "indirilmiş modeller ve yüz içeren bir fotoğraf gerekir"]
fn scrfd_and_arcface_run_on_downloaded_models() {
    let dir = PathBuf::from(std::env::var("LOUPE_MODELS_DIR").expect("LOUPE_MODELS_DIR"));
    let image = image::open(std::env::var("LOUPE_FACE_IMAGE").expect("LOUPE_FACE_IMAGE")).unwrap().to_rgb8();
    init_runtime(Some(&dir.join("libonnxruntime.so"))).unwrap();

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
