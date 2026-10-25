//! Loupe birleşik arama.
//!
//! Tek bir [`SearchQuery`] kişi (hepsi/herhangi biri), yer (ülke/il/ilçe/
//! yerleşim, koordinat kutusu, yarıçap), zaman (aralık, yerel yıl/ay/gün),
//! etkinlik, puan, favori, renk etiketi, kamera/objektif ve doğal dil metnini
//! birleştirir. Metin çok dilli CLIP ile anlamsal, adlar üzerinde sözcük
//! olarak aranır; iki sıralama RRF ile birleşir ([`engine`]). Akıllı albümler
//! aynı sorguyu JSON olarak saklar ([`albums`]).

pub mod albums;
pub mod compile;
pub mod engine;
pub mod query;

pub use albums::{SmartAlbum, SmartAlbumId};
pub use engine::{SearchEngine, SearchHit, SearchResult};
pub use query::{GeoBox, GeoRadius, Match, PersonFilter, PlaceFilter, SearchQuery, SortMode, TimeFilter};

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Db(#[from] loupe_db::DbError),

    #[error(transparent)]
    Ai(#[from] loupe_ai::AiError),

    #[error("geçersiz sorgu: {0}")]
    Invalid(String),

    #[error("akıllı albüm kuralı okunamadı: {0}")]
    Rules(#[from] serde_json::Error),

    #[error("akıllı albüm {0} bulunamadı")]
    NotFound(i64),
}

impl From<rusqlite::Error> for SearchError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(loupe_db::DbError::from(e))
    }
}
