//! Akıllı albümler: süzgeç [`SearchQuery`] JSON'u olarak `smart_albums.rules`
//! sütununda saklanır ve her açılışta yeniden çalıştırılır. Ham SQL saklanmaz;
//! JSON bilinmeyen alan içeremez (`deny_unknown_fields`), sürümü `rules_version`.

use loupe_core::now_ms;
use rusqlite::{Connection, OptionalExtension, params};

use crate::SearchError;
use crate::engine::{SearchEngine, SearchResult};
use crate::query::{SearchQuery, SortMode};

/// Bu sürümün yazdığı ve okuyabildiği kural biçimi.
pub const RULES_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SmartAlbumId(pub i64);

#[derive(Debug, Clone, PartialEq)]
pub struct SmartAlbum {
    pub id: SmartAlbumId,
    pub name: String,
    pub query: SearchQuery,
    pub is_pinned: bool,
}

const fn sort_column(s: SortMode) -> &'static str {
    match s {
        SortMode::TakenAsc => "taken_asc",
        SortMode::RatingDesc => "rating_desc",
        SortMode::Relevance | SortMode::TakenDesc => "taken_desc",
    }
}

/// Kuralları doğrular ve JSON'a çevirir.
pub fn rules_json(q: &SearchQuery) -> Result<String, SearchError> {
    q.validate()?;
    Ok(serde_json::to_string(q)?)
}

/// Saklanan kuralı ayrıştırır; bilinmeyen sürüm ya da alan hatadır.
pub fn parse_rules(json: &str, version: i64) -> Result<SearchQuery, SearchError> {
    if version != RULES_VERSION {
        return Err(SearchError::Invalid(format!("desteklenmeyen kural sürümü {version}")));
    }
    let q: SearchQuery = serde_json::from_str(json)?;
    q.validate()?;
    Ok(q)
}

pub fn create(conn: &Connection, name: &str, q: &SearchQuery, pinned: bool) -> Result<SmartAlbumId, SearchError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(SearchError::Invalid("albüm adı 1–200 karakter olmalı".into()));
    }
    let rules = rules_json(q)?;
    let limit = q.limit.map(|l| i64::try_from(l).unwrap_or(i64::MAX));
    let id: i64 = conn
        .prepare_cached(
            "INSERT INTO smart_albums (name, rules, rules_version, sort_mode, result_limit, is_pinned, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) RETURNING id",
        )?
        .query_row(params![name, rules, RULES_VERSION, sort_column(q.sort), limit, pinned, now_ms()], |r| r.get(0))?;
    Ok(SmartAlbumId(id))
}

pub fn update_rules(conn: &Connection, id: SmartAlbumId, q: &SearchQuery) -> Result<(), SearchError> {
    let rules = rules_json(q)?;
    let limit = q.limit.map(|l| i64::try_from(l).unwrap_or(i64::MAX));
    let n = conn
        .prepare_cached(
            "UPDATE smart_albums SET rules = ?2, rules_version = ?3, sort_mode = ?4, result_limit = ?5, updated_at = ?6
              WHERE id = ?1",
        )?
        .execute(params![id.0, rules, RULES_VERSION, sort_column(q.sort), limit, now_ms()])?;
    if n == 0 {
        return Err(SearchError::NotFound(id.0));
    }
    Ok(())
}

pub fn get(conn: &Connection, id: SmartAlbumId) -> Result<SmartAlbum, SearchError> {
    let row: Option<(String, String, i64, bool)> = conn
        .prepare_cached("SELECT name, rules, rules_version, is_pinned FROM smart_albums WHERE id = ?1")?
        .query_row([id.0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .optional()?;
    let (name, rules, version, is_pinned) = row.ok_or(SearchError::NotFound(id.0))?;
    Ok(SmartAlbum { id, name, query: parse_rules(&rules, version)?, is_pinned })
}

impl SearchEngine {
    /// Akıllı albümü güncel kütüphane üzerinde çalıştırır.
    pub fn run_smart_album(&self, id: SmartAlbumId) -> Result<SearchResult, SearchError> {
        let album = self.pool().read(|c| Ok(get(c, id)))??;
        self.search(&album.query)
    }
}
