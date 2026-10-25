//! Gerçek modeller (ONNX Runtime, `ort` özelliği).
//!
//! - **SCRFD** (`det_10g.onnx`, InsightFace): 640×640 letterbox, adımlar
//!   8/16/32, konum başına 2 çapa, mesafe kodlu kutu + 5 nokta, NMS.
//! - **ArcFace** (`w600k_r50.onnx`): 5 noktadan benzerlik dönüşümüyle
//!   112×112 hizalama, `(x − 127.5) / 127.5`, 512 boyut.
//! - **M-CLIP** (XLM-RoBERTa Large + ViT-B/32): görüntü 224 merkez kırpma ve
//!   CLIP ortalama/sapması; metin `tokenizer.json` ile `input_ids` +
//!   `attention_mask`, 512 boyut. Türkçe ve İngilizce aynı uzaydadır.
//!
//! ONNX Runtime paylaşımlı kütüphanesi derlemede indirilmez; çalışma anında
//! `ORT_DYLIB_PATH` ya da [`init_runtime`] ile yüklenir. Her model bir
//! [`ManagedModel`] içindedir: ilk işte yüklenir, `idle_timeout` boyunca
//! kullanılmazsa oturum düşürülür ve RAM/VRAM geri verilir.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use image::imageops::FilterType;
use image::{Rgb, RgbImage};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::AiError;
use crate::embedding::Embedding;
use crate::engine::{BBox, ClipImageEncoder, ClipTextEncoder, DetectedFace, FaceDetector, FaceEmbedder, bbox_pixels};
use crate::jobs::AiEngine;
use crate::lifecycle::{IdleReaper, ManagedModel, Unloadable};
use crate::model::{ModelInfo, ModelTask};

/// ONNX Runtime kütüphanesini belirli bir yoldan yükler (verilmezse `ORT_DYLIB_PATH`).
pub fn init_runtime(dylib: Option<&Path>) -> Result<(), AiError> {
    let builder = match dylib {
        Some(p) => ort::init_from(p.display().to_string()),
        None => ort::init(),
    };
    builder
        .with_name("loupe")
        .commit()
        .map(|_| ())
        .map_err(|e| AiError::ModelLoad { name: "onnxruntime".into(), reason: e.to_string() })
}

/// Tek ONNX oturumu. `Session::run` `&mut` istediği için kilit arkasındadır.
pub struct OnnxSession {
    name: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
    session: Mutex<Session>,
}

impl std::fmt::Debug for OnnxSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxSession").field("name", &self.name).field("inputs", &self.inputs).finish_non_exhaustive()
    }
}

/// Çıkışın şekli ve kopyalanmış verisi.
type Output = (Vec<i64>, Vec<f32>);

impl OnnxSession {
    pub fn load(name: &str, path: &Path, threads: usize) -> Result<Self, AiError> {
        let err = |e: ort::Error| AiError::ModelLoad { name: name.into(), reason: e.to_string() };
        let session = Session::builder()
            .map_err(err)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(err)?
            .with_intra_threads(threads.max(1))
            .map_err(err)?
            .commit_from_file(path)
            .map_err(err)?;
        Ok(Self {
            name: name.into(),
            inputs: session.inputs.iter().map(|i| i.name.clone()).collect(),
            outputs: session.outputs.iter().map(|o| o.name.clone()).collect(),
            session: Mutex::new(session),
        })
    }

    fn infer_err(&self, e: impl std::fmt::Display) -> AiError {
        AiError::Inference { model: self.name.clone(), reason: e.to_string() }
    }

    /// Tüm çıkışları modeldeki sırayla döndürür.
    fn run(&self, inputs: Vec<(String, ort::session::SessionInputValue<'_>)>) -> Result<Vec<Output>, AiError> {
        let mut s = self.session.lock().map_err(|_| AiError::Poisoned)?;
        let out = s.run(inputs).map_err(|e| self.infer_err(e))?;
        let mut res = Vec::with_capacity(self.outputs.len());
        for name in &self.outputs {
            let v = out.get(name).ok_or_else(|| self.infer_err(format!("çıkış yok: {name}")))?;
            let (shape, data) = v.try_extract_tensor::<f32>().map_err(|e| self.infer_err(e))?;
            res.push((shape.iter().copied().collect(), data.to_vec()));
        }
        Ok(res)
    }

    fn run_image(&self, chw: Vec<f32>, size: (usize, usize)) -> Result<Vec<Output>, AiError> {
        let input = self.inputs.first().cloned().ok_or_else(|| self.infer_err("girdi yok"))?;
        let t = Tensor::from_array(([1usize, 3, size.1, size.0], chw)).map_err(|e| self.infer_err(e))?;
        self.run(vec![(input, t.into())])
    }
}

/// RGB görüntüyü `(x − mean) / std` ile CHW düzlemlerine yazar.
fn to_chw(img: &RgbImage, mean: [f32; 3], std: [f32; 3], scale: f32) -> Vec<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut out = vec![0f32; 3 * w * h];
    for (x, y, p) in img.enumerate_pixels() {
        let i = y as usize * w + x as usize;
        for c in 0..3 {
            out[c * w * h + i] = (f32::from(p.0[c]) * scale - mean[c]) / std[c];
        }
    }
    out
}

// ───────────────────────────── SCRFD ─────────────────────────────

const DET_SIZE: u32 = 640;

#[derive(Debug)]
pub struct Scrfd {
    session: OnnxSession,
    info: ModelInfo,
    pub score_threshold: f32,
    pub nms_threshold: f32,
}

fn nms(mut faces: Vec<DetectedFace>, iou: f32) -> Vec<DetectedFace> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<DetectedFace> = Vec::new();
    for f in faces {
        if keep.iter().all(|k| k.bbox.iou(&f.bbox) < iou) {
            keep.push(f);
        }
    }
    keep
}

impl Scrfd {
    pub fn load(info: ModelInfo, path: &Path, threads: usize) -> Result<Self, AiError> {
        Ok(Self {
            session: OnnxSession::load(&info.name, path, threads)?,
            info,
            score_threshold: 0.5,
            nms_threshold: 0.4,
        })
    }
}

impl FaceDetector for Scrfd {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn detect(&self, img: &RgbImage) -> Result<Vec<DetectedFace>, AiError> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        if w < 1.0 || h < 1.0 {
            return Err(AiError::InvalidInput("boş görüntü".into()));
        }
        // Letterbox: oran korunur, sağ/alt sıfırla doldurulur.
        let scale = (DET_SIZE as f32 / w).min(DET_SIZE as f32 / h);
        let (rw, rh) = (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1));
        let resized = image::imageops::resize(img, rw, rh, FilterType::Triangle);
        let mut canvas = RgbImage::from_pixel(DET_SIZE, DET_SIZE, Rgb([0, 0, 0]));
        image::imageops::replace(&mut canvas, &resized, 0, 0);
        let chw = to_chw(&canvas, [127.5; 3], [128.0; 3], 1.0);
        let out = self.session.run_image(chw, (DET_SIZE as usize, DET_SIZE as usize))?;
        if out.len() < 9 {
            return Err(AiError::Inference {
                model: self.info.name.clone(),
                reason: format!("{} çıkış, 9 bekleniyordu", out.len()),
            });
        }

        let mut faces = Vec::new();
        for (level, stride) in [8usize, 16, 32].into_iter().enumerate() {
            let (scores, boxes, kps) = (&out[level].1, &out[level + 3].1, &out[level + 6].1);
            let grid = DET_SIZE as usize / stride;
            let anchors = grid * grid * 2;
            if scores.len() < anchors || boxes.len() < anchors * 4 || kps.len() < anchors * 10 {
                return Err(AiError::Inference {
                    model: self.info.name.clone(),
                    reason: "beklenmeyen çıkış boyutu".into(),
                });
            }
            let s = stride as f32;
            for a in 0..anchors {
                let score = scores[a];
                if score < self.score_threshold {
                    continue;
                }
                let cell = a / 2;
                let (cx, cy) = ((cell % grid) as f32 * s, (cell / grid) as f32 * s);
                let d = &boxes[a * 4..a * 4 + 4];
                let (x1, y1, x2, y2) = (cx - d[0] * s, cy - d[1] * s, cx + d[2] * s, cy + d[3] * s);
                // Letterbox koordinatından özgün görüntüye, sonra 0..1.
                let n = |x: f32, y: f32| [x / scale / w, y / scale / h];
                let k = &kps[a * 10..a * 10 + 10];
                let mut lm = [[0f32; 2]; 5];
                for (j, p) in lm.iter_mut().enumerate() {
                    *p = n(cx + k[j * 2] * s, cy + k[j * 2 + 1] * s);
                }
                let tl = n(x1, y1);
                let br = n(x2, y2);
                let Some(bbox) = (BBox { x: tl[0], y: tl[1], w: br[0] - tl[0], h: br[1] - tl[1] }).clamped() else {
                    continue;
                };
                let side_px = (bbox.w * w).min(bbox.h * h);
                faces.push(DetectedFace {
                    bbox,
                    score: score.clamp(0.0, 1.0),
                    landmarks: Some(lm),
                    // Küçük yüzlerin vektörü güvenilmez: 64 px altı orantılı düşer.
                    quality: ((side_px / 64.0).min(1.0) * score).clamp(0.0, 1.0),
                });
            }
        }
        Ok(nms(faces, self.nms_threshold))
    }
}

// ───────────────────────────── ArcFace ─────────────────────────────

/// InsightFace 112×112 hizalama şablonu (sol göz, sağ göz, burun, ağız sol, ağız sağ).
const ARCFACE_TEMPLATE: [[f32; 2]; 5] =
    [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

/// `src → dst` en küçük kareler benzerlik dönüşümü: `x' = a·x − b·y + tx`, `y' = b·x + a·y + ty`.
fn similarity(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> Option<[f32; 4]> {
    let mean = |p: &[[f32; 2]; 5]| {
        let (sx, sy) = p.iter().fold((0.0, 0.0), |(x, y), q| (x + q[0], y + q[1]));
        (sx / 5.0, sy / 5.0)
    };
    let (ms, md) = (mean(src), mean(dst));
    let (mut num_a, mut num_b, mut den) = (0f32, 0f32, 0f32);
    for (s, d) in src.iter().zip(dst) {
        let (xs, ys, xd, yd) = (s[0] - ms.0, s[1] - ms.1, d[0] - md.0, d[1] - md.1);
        num_a += xs * xd + ys * yd;
        num_b += xs * yd - ys * xd;
        den += xs * xs + ys * ys;
    }
    if den < 1e-6 {
        return None;
    }
    let (a, b) = (num_a / den, num_b / den);
    Some([a, b, md.0 - a * ms.0 + b * ms.1, md.1 - b * ms.0 - a * ms.1])
}

fn bilinear(img: &RgbImage, x: f32, y: f32) -> [u8; 3] {
    if x < 0.0 || y < 0.0 || x > (img.width() - 1) as f32 || y > (img.height() - 1) as f32 {
        return [0, 0, 0];
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(img.width() - 1), (y0 + 1).min(img.height() - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let p = |x, y| img.get_pixel(x, y).0.map(f32::from);
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bot = c[i] + (d[i] - c[i]) * fx;
        (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8
    })
}

/// Yüzü 112×112 ArcFace girdisine hizalar (nokta yoksa kutu kırpılır).
#[must_use]
pub fn align_face(img: &RgbImage, face: &DetectedFace) -> RgbImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let t = face.landmarks.and_then(|lm| similarity(&lm.map(|p| [p[0] * w, p[1] * h]), &ARCFACE_TEMPLATE));
    let Some([a, b, tx, ty]) = t else {
        let (x, y, cw, ch) = bbox_pixels(img, &face.bbox);
        let crop = image::imageops::crop_imm(img, x, y, cw, ch).to_image();
        return image::imageops::resize(&crop, 112, 112, FilterType::Triangle);
    };
    let det = a * a + b * b;
    RgbImage::from_fn(112, 112, |u, v| {
        let (du, dv) = (u as f32 - tx, v as f32 - ty);
        Rgb(bilinear(img, (a * du + b * dv) / det, (-b * du + a * dv) / det))
    })
}

#[derive(Debug)]
pub struct ArcFace {
    session: OnnxSession,
    info: ModelInfo,
}

impl ArcFace {
    pub fn load(info: ModelInfo, path: &Path, threads: usize) -> Result<Self, AiError> {
        Ok(Self { session: OnnxSession::load(&info.name, path, threads)?, info })
    }
}

impl FaceEmbedder for ArcFace {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn embed(&self, img: &RgbImage, face: &DetectedFace) -> Result<Embedding, AiError> {
        let aligned = align_face(img, face);
        let out = self.session.run_image(to_chw(&aligned, [127.5; 3], [127.5; 3], 1.0), (112, 112))?;
        let v = out.into_iter().next().map(|o| o.1).unwrap_or_default();
        Embedding::normalized(v)?.expect_dim(loupe_db::FACE_DIM)
    }
}

// ───────────────────────────── M-CLIP ─────────────────────────────

const CLIP_MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const CLIP_STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

#[derive(Debug)]
pub struct ClipVision {
    session: OnnxSession,
    info: ModelInfo,
}

impl ClipVision {
    pub fn load(info: ModelInfo, path: &Path, threads: usize) -> Result<Self, AiError> {
        Ok(Self { session: OnnxSession::load(&info.name, path, threads)?, info })
    }
}

impl ClipImageEncoder for ClipVision {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    fn encode_image(&self, img: &RgbImage) -> Result<Embedding, AiError> {
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Err(AiError::InvalidInput("boş görüntü".into()));
        }
        // Kısa kenar 224, ortadan 224×224.
        let s = 224.0 / w.min(h) as f32;
        let (rw, rh) = (((w as f32 * s).round() as u32).max(224), ((h as f32 * s).round() as u32).max(224));
        let resized = image::imageops::resize(img, rw, rh, FilterType::CatmullRom);
        let crop = image::imageops::crop_imm(&resized, (rw - 224) / 2, (rh - 224) / 2, 224, 224).to_image();
        let out = self.session.run_image(to_chw(&crop, CLIP_MEAN, CLIP_STD, 1.0 / 255.0), (224, 224))?;
        let v = out.into_iter().next().map(|o| o.1).unwrap_or_default();
        Embedding::normalized(v)?.expect_dim(loupe_db::CLIP_DIM)
    }
}

pub struct MclipText {
    session: OnnxSession,
    tokenizer: tokenizers::Tokenizer,
    info: ModelInfo,
    max_tokens: usize,
}

impl std::fmt::Debug for MclipText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MclipText").field("info", &self.info).finish_non_exhaustive()
    }
}

impl MclipText {
    pub fn load(info: ModelInfo, model: &Path, tokenizer: &Path, threads: usize) -> Result<Self, AiError> {
        let tokenizer = tokenizers::Tokenizer::from_file(tokenizer)
            .map_err(|e| AiError::ModelLoad { name: info.name.clone(), reason: e.to_string() })?;
        Ok(Self { session: OnnxSession::load(&info.name, model, threads)?, tokenizer, info, max_tokens: 128 })
    }
}

impl ClipTextEncoder for MclipText {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn encode_text(&self, text: &str) -> Result<Embedding, AiError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(AiError::InvalidInput("boş sorgu".into()));
        }
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| AiError::Inference { model: self.info.name.clone(), reason: e.to_string() })?;
        let n = enc.get_ids().len().min(self.max_tokens);
        let ids: Vec<i64> = enc.get_ids()[..n].iter().map(|&i| i64::from(i)).collect();
        let mask: Vec<i64> = enc.get_attention_mask()[..n].iter().map(|&i| i64::from(i)).collect();
        let err = |e: ort::Error| AiError::Inference { model: self.info.name.clone(), reason: e.to_string() };
        let ids = Tensor::from_array(([1usize, n], ids)).map_err(err)?;
        let mask = Tensor::from_array(([1usize, n], mask)).map_err(err)?;
        let names = &self.session.inputs;
        let (ni, nm) = match (names.first(), names.get(1)) {
            (Some(a), Some(b)) => (a.clone(), b.clone()),
            _ => {
                return Err(AiError::Inference {
                    model: self.info.name.clone(),
                    reason: "iki girdi bekleniyordu".into(),
                });
            }
        };
        let out = self.session.run(vec![(ni, ids.into()), (nm, mask.into())])?;
        let v = out.into_iter().next().map(|o| o.1).unwrap_or_default();
        Embedding::normalized(v)?.expect_dim(loupe_db::CLIP_DIM)
    }
}

// ───────────────────────────── motor kurulumu ─────────────────────────────

/// Model dosyaları (ör. `~/.local/share/loupe/models/`).
#[derive(Debug, Clone)]
pub struct OnnxModelPaths {
    pub scrfd: PathBuf,
    pub arcface: PathBuf,
    pub clip_image: PathBuf,
    pub clip_text: PathBuf,
    pub tokenizer: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct OnnxConfig {
    pub idle_timeout: Duration,
    pub intra_threads: usize,
}

impl Default for OnnxConfig {
    fn default() -> Self {
        Self { idle_timeout: Duration::from_secs(120), intra_threads: 2 }
    }
}

/// Dört modeli tembel yüklenen ve boşta boşaltılan [`AiEngine`] olarak kurar.
/// Modeller burada yüklenmez; yalnızca dosya özetleri hesaplanır (kimlik).
pub fn engine(paths: &OnnxModelPaths, cfg: OnnxConfig, reaper: Option<&IdleReaper>) -> Result<AiEngine, AiError> {
    let threads = cfg.intra_threads;
    let i_det = ModelInfo::from_file(ModelTask::FaceDetect, "scrfd_10g", "1", None, &paths.scrfd)?;
    let i_emb =
        ModelInfo::from_file(ModelTask::FaceEmbed, "arcface_w600k_r50", "1", Some(loupe_db::FACE_DIM), &paths.arcface)?;
    let i_img = ModelInfo::from_file(
        ModelTask::ClipImage,
        "mclip_vit_b32_image",
        "1",
        Some(loupe_db::CLIP_DIM),
        &paths.clip_image,
    )?;
    let i_txt = ModelInfo::from_file(
        ModelTask::ClipText,
        "mclip_xlmr_large_vit_b32_text",
        "1",
        Some(loupe_db::CLIP_DIM),
        &paths.clip_text,
    )?;

    let (p, i) = (paths.scrfd.clone(), i_det.clone());
    let det = Arc::new(ManagedModel::new(i_det, cfg.idle_timeout, move || Scrfd::load(i.clone(), &p, threads)));
    let (p, i) = (paths.arcface.clone(), i_emb.clone());
    let emb = Arc::new(ManagedModel::new(i_emb, cfg.idle_timeout, move || ArcFace::load(i.clone(), &p, threads)));
    let (p, i) = (paths.clip_image.clone(), i_img.clone());
    let img = Arc::new(ManagedModel::new(i_img, cfg.idle_timeout, move || ClipVision::load(i.clone(), &p, threads)));
    let (p, t, i) = (paths.clip_text.clone(), paths.tokenizer.clone(), i_txt.clone());
    let txt = Arc::new(ManagedModel::new(i_txt, cfg.idle_timeout, move || MclipText::load(i.clone(), &p, &t, threads)));

    if let Some(r) = reaper {
        r.watch(&(Arc::clone(&det) as Arc<dyn Unloadable>));
        r.watch(&(Arc::clone(&emb) as Arc<dyn Unloadable>));
        r.watch(&(Arc::clone(&img) as Arc<dyn Unloadable>));
        r.watch(&(Arc::clone(&txt) as Arc<dyn Unloadable>));
    }
    Ok(AiEngine { detector: det, embedder: emb, clip_image: img, clip_text: txt })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_transform_recovers_scale_rotation_translation() {
        // dst = 2·R(90°)·src + (10, 5)
        let src = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.5, 0.5]];
        let dst = src.map(|p| [-2.0 * p[1] + 10.0, 2.0 * p[0] + 5.0]);
        let [a, b, tx, ty] = similarity(&src, &dst).unwrap();
        assert!(
            (a - 0.0).abs() < 1e-4 && (b - 2.0).abs() < 1e-4 && (tx - 10.0).abs() < 1e-4 && (ty - 5.0).abs() < 1e-4
        );
    }

    #[test]
    fn nms_keeps_best_of_overlapping() {
        let f = |x: f32, s: f32| DetectedFace {
            bbox: BBox { x, y: 0.1, w: 0.2, h: 0.2 },
            score: s,
            landmarks: None,
            quality: 1.0,
        };
        let kept = nms(vec![f(0.1, 0.7), f(0.11, 0.9), f(0.6, 0.8)], 0.4);
        assert_eq!(kept.iter().map(|k| k.score).collect::<Vec<_>>(), [0.9, 0.8]);
    }
}
