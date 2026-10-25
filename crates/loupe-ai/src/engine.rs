//! Çıkarım arayüzleri. Kuyruk, kümeleme ve arama yalnızca bu trait'leri bilir;
//! gerçek ONNX modelleri (`ort` özelliği) ile deterministik test motoru
//! ([`crate::mock::DeterministicMockEngine`]) aynı yerlere takılır.

use image::RgbImage;

use crate::AiError;
use crate::embedding::Embedding;
use crate::model::ModelInfo;

/// Normalize sınırlayıcı kutu (0..1), yönlendirmesi düzeltilmiş görüntüye göre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BBox {
    /// Kutuyu [0,1] aralığına kırpar; boş kalırsa `None`.
    #[must_use]
    pub fn clamped(self) -> Option<Self> {
        let x0 = self.x.clamp(0.0, 1.0);
        let y0 = self.y.clamp(0.0, 1.0);
        let x1 = (self.x + self.w).clamp(0.0, 1.0);
        let y1 = (self.y + self.h).clamp(0.0, 1.0);
        (x1 > x0 && y1 > y0).then_some(Self { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    #[must_use]
    pub fn iou(&self, o: &Self) -> f32 {
        let ix = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let iy = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        if ix <= 0.0 || iy <= 0.0 {
            return 0.0;
        }
        let inter = ix * iy;
        inter / (self.w * self.h + o.w * o.h - inter)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetectedFace {
    pub bbox: BBox,
    /// Tespit güveni 0..1.
    pub score: f32,
    /// Sol göz, sağ göz, burun, ağız sol, ağız sağ (normalize).
    pub landmarks: Option<[[f32; 2]; 5]>,
    /// Netlik ve boyuta göre 0..1; eşik altı yüzler kümelemeye girmez.
    pub quality: f32,
}

pub trait FaceDetector: Send + Sync {
    fn info(&self) -> &ModelInfo;
    fn detect(&self, image: &RgbImage) -> Result<Vec<DetectedFace>, AiError>;
}

pub trait FaceEmbedder: Send + Sync {
    fn info(&self) -> &ModelInfo;
    /// 512 boyutlu, L2 normalize yüz vektörü.
    fn embed(&self, image: &RgbImage, face: &DetectedFace) -> Result<Embedding, AiError>;
}

pub trait ClipImageEncoder: Send + Sync {
    fn info(&self) -> &ModelInfo;
    fn encode_image(&self, image: &RgbImage) -> Result<Embedding, AiError>;
}

/// Çok dilli metin kodlayıcı (Türkçe ve İngilizce aynı uzaya düşer).
pub trait ClipTextEncoder: Send + Sync {
    fn info(&self) -> &ModelInfo;
    fn encode_text(&self, text: &str) -> Result<Embedding, AiError>;
}

/// Tespit edilen yüzün kutusunu piksel dikdörtgenine çevirir (en az 1 px).
#[must_use]
pub fn bbox_pixels(img: &RgbImage, b: &BBox) -> (u32, u32, u32, u32) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let px = |v: f32, max: f32| (v * max).round().clamp(0.0, max) as u32;
    let x0 = px(b.x, w);
    let y0 = px(b.y, h);
    let x1 = px(b.x + b.w, w).max(x0 + 1).min(img.width());
    let y1 = px(b.y + b.h, h).max(y0 + 1).min(img.height());
    (x0, y0, x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1))
}
