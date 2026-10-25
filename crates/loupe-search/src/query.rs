//! Birleşik arama sorgusu. Akıllı albümler bu yapıyı JSON olarak saklar
//! (`smart_albums.rules`); bilinmeyen alan reddedilir, ham SQL asla saklanmaz.

use serde::{Deserialize, Serialize};

use crate::SearchError;

/// Kişi süzgecinin birleşimi.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    /// Fotoğrafta kişilerin hepsi var.
    #[default]
    All,
    /// En az biri var.
    Any,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonFilter {
    pub ids: Vec<i64>,
    #[serde(default)]
    pub mode: Match,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoBox {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    /// `min_lon > max_lon` ise kutu 180. boylamı aşar.
    pub max_lon: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoRadius {
    pub lat: f64,
    pub lon: f64,
    pub km: f64,
}

/// Yer süzgeci: adlar `locations` hiyerarşisine, kutu ve yarıçap EXIF GPS'e uygulanır.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PlaceFilter {
    pub country_code: Option<String>,
    pub country: Option<String>,
    /// İl.
    pub admin1: Option<String>,
    /// İlçe.
    pub admin2: Option<String>,
    /// Yerleşim (Göreme).
    pub locality: Option<String>,
    pub bbox: Option<GeoBox>,
    pub radius: Option<GeoRadius>,
}

/// Zaman süzgeci. `year`/`month`/`day` fotoğrafın *yerel* saatine göredir
/// (çekildiği yerdeki takvim günü).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TimeFilter {
    /// Dahil, UTC ms.
    pub from_utc: Option<i64>,
    /// Hariç, UTC ms.
    pub to_utc: Option<i64>,
    pub year: Option<i32>,
    pub month: Option<u32>,
    pub day: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    /// Metin varsa RRF puanı, yoksa en yeni önce.
    #[default]
    Relevance,
    TakenDesc,
    TakenAsc,
    RatingDesc,
}

pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 10_000;
const COLOR_LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SearchQuery {
    pub persons: Option<PersonFilter>,
    pub place: Option<PlaceFilter>,
    pub time: Option<TimeFilter>,
    pub event_id: Option<i64>,
    pub min_rating: Option<u8>,
    pub favorite: Option<bool>,
    pub color_labels: Vec<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    /// Doğal dil: CLIP anlamsal arama + dosya/yer/etkinlik/kişi adlarında sözcük araması.
    pub text: Option<String>,
    pub limit: Option<usize>,
    pub sort: SortMode,
}

impl SearchQuery {
    /// Metin dışında süzgeç var mı (anlamsal adım adaylarla sınırlanır).
    #[must_use]
    pub fn has_filters(&self) -> bool {
        self.persons.is_some()
            || self.place.is_some()
            || self.time.is_some()
            || self.event_id.is_some()
            || self.min_rating.is_some()
            || self.favorite.is_some()
            || !self.color_labels.is_empty()
            || self.camera_model.is_some()
            || self.lens_model.is_some()
    }

    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref().map(str::trim).filter(|t| !t.is_empty())
    }

    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }

    /// Değer aralıklarını denetler (SQL'e gitmeden önce).
    pub fn validate(&self) -> Result<(), SearchError> {
        let bad = |m: &str| Err(SearchError::Invalid(m.to_owned()));
        if let Some(p) = &self.persons
            && p.ids.is_empty()
        {
            return bad("kişi süzgeci boş");
        }
        if self.min_rating.is_some_and(|r| r > 5) {
            return bad("puan 0–5 olmalı");
        }
        if let Some(c) = self.color_labels.iter().find(|c| !COLOR_LABELS.contains(&c.as_str())) {
            return Err(SearchError::Invalid(format!("bilinmeyen renk etiketi: {c}")));
        }
        if let Some(t) = &self.time {
            if t.month.is_some_and(|m| !(1..=12).contains(&m)) {
                return bad("ay 1–12 olmalı");
            }
            if t.day.is_some_and(|d| !(1..=31).contains(&d)) {
                return bad("gün 1–31 olmalı");
            }
            if let (Some(a), Some(b)) = (t.from_utc, t.to_utc)
                && a >= b
            {
                return bad("zaman aralığı boş");
            }
        }
        if let Some(p) = &self.place {
            let lat_ok = |l: f64| (-90.0..=90.0).contains(&l);
            let lon_ok = |l: f64| (-180.0..=180.0).contains(&l);
            if let Some(b) = p.bbox
                && !(lat_ok(b.min_lat)
                    && lat_ok(b.max_lat)
                    && lon_ok(b.min_lon)
                    && lon_ok(b.max_lon)
                    && b.min_lat <= b.max_lat)
            {
                return bad("geçersiz koordinat kutusu");
            }
            if let Some(r) = p.radius
                && !(lat_ok(r.lat) && lon_ok(r.lon) && r.km.is_finite() && r.km > 0.0)
            {
                return bad("geçersiz yarıçap");
            }
        }
        Ok(())
    }
}
