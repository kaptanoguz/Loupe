//! `DeterministicMockEngine`: model dosyası indirmeden, milisaniyeler içinde ve
//! her çalıştırmada aynı sonucu veren test motoru. Gerçek piksellere bakar,
//! böylece hat uçtan uca (küçük resim → tespit → vektör → kümeleme → arama)
//! sınanabilir. Anlamsal doğruluğu yoktur; yalnızca aşağıdaki sözleşmeyi sağlar.
//!
//! **Yüz sözleşmesi:** "Yüz", magenta (R>170, G<90, B>170) çerçeveli bir
//! karedir. Kimlik, çerçevenin iç renginden gelir: her kanal 4 düzeye
//! nicelenir (64 kimlik). Aynı iç renk farklı fotoğraflarda kosinüs ≈ 0.9,
//! farklı renkler ≈ 0 verir. Kayıplı WebP/JPEG sıkıştırması sonucu değiştirmez.
//!
//! **CLIP sözleşmesi:** Görüntü vektörü renk kavramlarının (sıcak/turuncu,
//! mavi, yeşil, karanlık, aydınlık) piksel oranlarından oluşur. Metin vektörü
//! Türkçe ve İngilizce anahtar sözcüklerin aynı kavram eksenlerine eşlenmesidir:
//! "gün batımı" ve "sunset" aynı yöne düşer.

use image::RgbImage;

use crate::AiError;
use crate::embedding::Embedding;
use crate::engine::{BBox, ClipImageEncoder, ClipTextEncoder, DetectedFace, FaceDetector, FaceEmbedder, bbox_pixels};
use crate::model::{ModelInfo, ModelTask};

pub const MOCK_DIM: usize = 512;

/// splitmix64: küçük, taşınabilir, tohumlanabilir.
struct Rng(u64);

impl Rng {
    fn new(label: &str) -> Self {
        let h = blake3::hash(label.as_bytes());
        let mut seed = [0u8; 8];
        seed.copy_from_slice(&h.as_bytes()[..8]);
        Self(u64::from_le_bytes(seed))
    }

    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        #[allow(clippy::cast_precision_loss)]
        let u = (z >> 40) as f32 / (1u64 << 24) as f32;
        u * 2.0 - 1.0
    }
}

/// Etiketten türetilen birim vektör (yaklaşık dik eksenler: 512 boyutta rastgele
/// iki vektörün kosinüsü ~±0.05).
fn axis(label: &str) -> Vec<f32> {
    let mut r = Rng::new(label);
    let v: Vec<f32> = (0..MOCK_DIM).map(|_| r.next_f32()).collect();
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::EPSILON);
    v.into_iter().map(|x| x / n).collect()
}

fn add_scaled(acc: &mut [f32], v: &[f32], k: f32) {
    for (a, b) in acc.iter_mut().zip(v) {
        *a += b * k;
    }
}

fn info(task: ModelTask, name: &str, dim: Option<usize>) -> ModelInfo {
    ModelInfo {
        task,
        name: name.into(),
        version: "1".into(),
        dim,
        file_blake3: *blake3::hash(name.as_bytes()).as_bytes(),
    }
}

const fn is_marker(p: [u8; 3]) -> bool {
    p[0] > 170 && p[1] < 90 && p[2] > 170
}

#[derive(Debug, Clone)]
pub struct MockFaceDetector {
    info: ModelInfo,
}

impl Default for MockFaceDetector {
    fn default() -> Self {
        Self { info: info(ModelTask::FaceDetect, "mock-scrfd", None) }
    }
}

impl FaceDetector for MockFaceDetector {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn detect(&self, img: &RgbImage) -> Result<Vec<DetectedFace>, AiError> {
        let (w, h) = img.dimensions();
        let (wu, hu) = (w as usize, h as usize);
        let mut seen = vec![false; wu * hu];
        let mut faces = Vec::new();
        let mut stack = Vec::new();
        for start in 0..wu * hu {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let (sx, sy) = (start % wu, start / wu);
            if !is_marker(img.get_pixel(sx as u32, sy as u32).0) {
                continue;
            }
            // 4-komşulu taşkın dolgu: çerçevenin sınırlayıcı kutusu yüz kutusudur.
            let (mut x0, mut y0, mut x1, mut y1, mut count) = (sx, sy, sx, sy, 0usize);
            stack.push(start);
            while let Some(i) = stack.pop() {
                count += 1;
                let (x, y) = (i % wu, i / wu);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                let mut visit = |nx: usize, ny: usize| {
                    let j = ny * wu + nx;
                    if !seen[j] {
                        seen[j] = true;
                        if is_marker(img.get_pixel(nx as u32, ny as u32).0) {
                            stack.push(j);
                        }
                    }
                };
                if x > 0 {
                    visit(x - 1, y);
                }
                if x + 1 < wu {
                    visit(x + 1, y);
                }
                if y > 0 {
                    visit(x, y - 1);
                }
                if y + 1 < hu {
                    visit(x, y + 1);
                }
            }
            let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
            if bw.min(bh) < 12 || count * 5 < bw * bh / 2 {
                continue; // gürültü ya da çerçeve olmayan leke
            }
            #[allow(clippy::cast_precision_loss)]
            let (fw, fh) = (w as f32, h as f32);
            #[allow(clippy::cast_precision_loss)]
            let bbox = BBox { x: x0 as f32 / fw, y: y0 as f32 / fh, w: bw as f32 / fw, h: bh as f32 / fh };
            let Some(bbox) = bbox.clamped() else { continue };
            let lm = |dx: f32, dy: f32| [bbox.x + bbox.w * dx, bbox.y + bbox.h * dy];
            #[allow(clippy::cast_precision_loss)]
            let quality = (bw.min(bh) as f32 / 48.0).min(1.0);
            faces.push(DetectedFace {
                bbox,
                score: 0.99,
                landmarks: Some([lm(0.3, 0.38), lm(0.7, 0.38), lm(0.5, 0.55), lm(0.35, 0.75), lm(0.65, 0.75)]),
                quality,
            });
        }
        Ok(faces)
    }
}

#[derive(Debug, Clone)]
pub struct MockFaceEmbedder {
    info: ModelInfo,
}

impl Default for MockFaceEmbedder {
    fn default() -> Self {
        Self { info: info(ModelTask::FaceEmbed, "mock-arcface", Some(MOCK_DIM)) }
    }
}

impl FaceEmbedder for MockFaceEmbedder {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn embed(&self, img: &RgbImage, face: &DetectedFace) -> Result<Embedding, AiError> {
        let (x, y, w, h) = bbox_pixels(img, &face.bbox);
        // Çerçeveyi dışarıda bırak: iç bölgenin ortalama rengi kimliktir.
        let (ix, iy, iw, ih) = (x + w * 3 / 10, y + h * 3 / 10, (w * 4 / 10).max(1), (h * 4 / 10).max(1));
        let mut sum = [0u64; 3];
        let mut n = 0u64;
        for py in iy..(iy + ih).min(img.height()) {
            for px in ix..(ix + iw).min(img.width()) {
                let p = img.get_pixel(px, py).0;
                for c in 0..3 {
                    sum[c] += u64::from(p[c]);
                }
                n += 1;
            }
        }
        if n == 0 {
            return Err(AiError::InvalidInput("yüz bölgesi boş".into()));
        }
        let mean = sum.map(|s| s / n);
        let identity = mean.iter().fold(0u64, |acc, &m| acc * 4 + (m / 64).min(3));
        let mut v = axis(&format!("face-identity-{identity}"));
        // Aynı kişinin farklı fotoğrafları birebir aynı vektörü vermesin.
        let jitter = axis(&format!("face-jitter-{x}-{y}-{w}-{h}-{}x{}-{mean:?}", img.width(), img.height()));
        add_scaled(&mut v, &jitter, 0.35);
        Embedding::normalized(v)
    }
}

const CONCEPTS: [&str; 5] = ["warm", "blue", "green", "dark", "bright"];

fn concept_fractions(img: &RgbImage) -> [f32; 5] {
    let mut counts = [0u32; 5];
    let mut total = 0u32;
    for (x, y, p) in img.enumerate_pixels() {
        if x % 2 == 1 || y % 2 == 1 || is_marker(p.0) {
            continue;
        }
        let [r, g, b] = p.0.map(i32::from);
        total += 1;
        let lum = (r * 299 + g * 587 + b * 114) / 1000;
        if r > g && g > b && r - b > 60 && lum > 50 {
            counts[0] += 1;
        }
        if b > r + 30 && b > g {
            counts[1] += 1;
        }
        if g > r + 20 && g > b + 20 {
            counts[2] += 1;
        }
        if lum < 50 {
            counts[3] += 1;
        }
        if lum > 210 {
            counts[4] += 1;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    counts.map(|c| if total == 0 { 0.0 } else { c as f32 / total as f32 })
}

#[derive(Debug, Clone)]
pub struct MockClip {
    image: ModelInfo,
    text: ModelInfo,
}

impl Default for MockClip {
    fn default() -> Self {
        Self {
            image: info(ModelTask::ClipImage, "mock-mclip-image", Some(MOCK_DIM)),
            text: info(ModelTask::ClipText, "mock-mclip-text", Some(MOCK_DIM)),
        }
    }
}

impl MockClip {
    #[must_use]
    pub fn text_info(&self) -> &ModelInfo {
        &self.text
    }
}

impl ClipImageEncoder for MockClip {
    fn info(&self) -> &ModelInfo {
        &self.image
    }

    fn encode_image(&self, img: &RgbImage) -> Result<Embedding, AiError> {
        if img.width() == 0 || img.height() == 0 {
            return Err(AiError::InvalidInput("boş görüntü".into()));
        }
        let mut v = vec![0.0f32; MOCK_DIM];
        for (name, frac) in CONCEPTS.iter().zip(concept_fractions(img)) {
            add_scaled(&mut v, &axis(&format!("clip-concept-{name}")), frac);
        }
        // Görüntüye özgü küçük bileşen: 4×4 ızgaranın kaba renkleri.
        let small = image::imageops::resize(img, 4, 4, image::imageops::FilterType::Triangle);
        let sig: Vec<u8> = small.pixels().flat_map(|p| p.0.map(|c| c / 16)).collect();
        add_scaled(&mut v, &axis(&format!("clip-image-{sig:?}")), 0.25);
        Embedding::normalized(v)
    }
}

pub use loupe_db::turkish_lowercase;

const KEYWORDS: &[(&str, usize)] = &[
    ("gün batımı", 0),
    ("günbatımı", 0),
    ("gün doğumu", 0),
    ("sunset", 0),
    ("sunrise", 0),
    ("golden hour", 0),
    ("turuncu", 0),
    ("orange", 0),
    ("akşam", 0),
    ("deniz", 1),
    ("sea", 1),
    ("ocean", 1),
    ("okyanus", 1),
    ("göl", 1),
    ("lake", 1),
    ("sahil", 1),
    ("beach", 1),
    ("gökyüzü", 1),
    ("sky", 1),
    ("mavi", 1),
    ("blue", 1),
    ("orman", 2),
    ("forest", 2),
    ("ağaç", 2),
    ("tree", 2),
    ("yeşil", 2),
    ("green", 2),
    ("çimen", 2),
    ("park", 2),
    ("gece", 3),
    ("night", 3),
    ("karanlık", 3),
    ("dark", 3),
    ("kar", 4),
    ("snow", 4),
    ("beyaz", 4),
    ("white", 4),
];

impl ClipTextEncoder for MockClip {
    fn info(&self) -> &ModelInfo {
        &self.text
    }

    fn encode_text(&self, text: &str) -> Result<Embedding, AiError> {
        let t = turkish_lowercase(text.trim());
        if t.is_empty() {
            return Err(AiError::InvalidInput("boş sorgu".into()));
        }
        let padded = format!(" {} ", t.split_whitespace().collect::<Vec<_>>().join(" "));
        let mut v = vec![0.0f32; MOCK_DIM];
        let mut hit = [false; 5];
        for (kw, c) in KEYWORDS {
            // Sözcük başı eşleşmesi: "deniz" "denizde"yi de yakalar, "kar" "akar"ı yakalamaz.
            if padded.contains(&format!(" {kw}")) {
                hit[*c] = true;
            }
        }
        for (i, name) in CONCEPTS.iter().enumerate() {
            if hit[i] {
                add_scaled(&mut v, &axis(&format!("clip-concept-{name}")), 1.0);
            }
        }
        add_scaled(&mut v, &axis(&format!("clip-text-{t}")), 0.2);
        Embedding::normalized(v)
    }
}

/// Test ve CI için tüm modellerin deterministik karşılıkları.
#[derive(Debug, Clone, Default)]
pub struct DeterministicMockEngine {
    pub detector: MockFaceDetector,
    pub embedder: MockFaceEmbedder,
    pub clip: MockClip,
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Gri sahne üstünde magenta çerçeveli, iç rengi `inner` olan kare.
    fn scene_with_face(inner: [u8; 3], at: (u32, u32), bg: [u8; 3]) -> RgbImage {
        let mut img = RgbImage::from_pixel(320, 240, Rgb(bg));
        for y in at.1..at.1 + 60 {
            for x in at.0..at.0 + 60 {
                let border = x < at.0 + 8 || x >= at.0 + 52 || y < at.1 + 8 || y >= at.1 + 52;
                img.put_pixel(x, y, Rgb(if border { [230, 30, 220] } else { inner }));
            }
        }
        img
    }

    #[test]
    fn same_identity_close_different_far() {
        let e = DeterministicMockEngine::default();
        let a1 = scene_with_face([224, 160, 96], (20, 30), [120, 120, 120]);
        let a2 = scene_with_face([226, 158, 98], (200, 100), [90, 100, 110]);
        let b = scene_with_face([32, 96, 224], (40, 40), [120, 120, 120]);
        let emb = |img: &RgbImage| {
            let f = e.detector.detect(img).unwrap();
            assert_eq!(f.len(), 1);
            e.embedder.embed(img, &f[0]).unwrap()
        };
        let (va1, va2, vb) = (emb(&a1), emb(&a2), emb(&b));
        assert!(va1.cosine(&va2) > 0.8, "{}", va1.cosine(&va2));
        assert!(va1.cosine(&vb).abs() < 0.3, "{}", va1.cosine(&vb));
        assert_eq!(va1, emb(&a1), "deterministik");
    }

    #[test]
    fn multilingual_text_matches_image_concept() {
        let c = MockClip::default();
        let sunset = RgbImage::from_pixel(64, 64, Rgb([240, 140, 40]));
        let sea = RgbImage::from_pixel(64, 64, Rgb([30, 90, 200]));
        let (vs, vsea) = (c.encode_image(&sunset).unwrap(), c.encode_image(&sea).unwrap());
        let tr = c.encode_text("Gün batımı").unwrap();
        let en = c.encode_text("sunset").unwrap();
        assert!(tr.cosine(&en) > 0.9);
        assert!(tr.cosine(&vs) > tr.cosine(&vsea) + 0.5);
        assert!(c.encode_text("deniz kenarında").unwrap().cosine(&vsea) > 0.8);
        assert_eq!(turkish_lowercase("IŞIK İzmir"), "ışık izmir");
    }
}
