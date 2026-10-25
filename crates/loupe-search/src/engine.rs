//! Hibrit arama: SQL süzgeçleri + CLIP anlamsal sıralama + sözcük eşleşmesi,
//! Reciprocal Rank Fusion ile tek listede.
//!
//! 1. Süzgeçler parametreli SQL'e derlenir ([`crate::compile`]).
//! 2. Metin varsa çok dilli CLIP metin kodlayıcısıyla vektöre çevrilir
//!    (veritabanı kilidi dışında).
//!    - Süzgeç varsa: aday kümesi küçüktür; adayların vektörleriyle kesin
//!      kosinüs hesaplanır (KNN sonrası süzme gibi sonuç kaçırmaz).
//!    - Süzgeç yoksa: `vec_media_clip` KNN (`k = max(4·limit, 200)`).
//! 3. Sözcük sıralayıcısı dosya adı, yer, etkinlik başlığı ve kişi adlarında
//!    sorgu sözcüklerini arar.
//! 4. RRF: `puan = Σ 1 / (60 + sıra)`; iki listede de üstte olan öne geçer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use loupe_ai::{ClipTextEncoder, clip};
use loupe_core::MediaId;
use loupe_db::Pool;
use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::SearchError;
use crate::compile::{HAYSTACK, compile, order_by};
use crate::query::SearchQuery;

/// RRF sabiti (Cormack ve ark., 2009).
pub const RRF_K: f32 = 60.0;
/// Süzgeçli anlamsal adımda bakılan en fazla aday.
pub const MAX_CANDIDATES: usize = 100_000;
/// sqlite-vec KNN sorgusunun `k` üst sınırı.
pub const KNN_MAX: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchHit {
    pub media_id: MediaId,
    /// RRF puanı (metinsiz sorguda 0).
    pub score: f32,
    /// CLIP kosinüs benzerliği.
    pub semantic: Option<f32>,
    /// Sözcük listesindeki sıra (1'den).
    pub keyword_rank: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    /// Süzgeçlerden geçen aday sayısı (metinli sorguda).
    pub candidates: usize,
    pub elapsed: Duration,
}

impl SearchResult {
    #[must_use]
    pub fn ids(&self) -> Vec<MediaId> {
        self.hits.iter().map(|h| h.media_id).collect()
    }
}

pub struct SearchEngine {
    pool: Arc<Pool>,
    /// `None`: AI kapalı; metin yalnızca adlarda sözcük olarak aranır.
    text: Option<Arc<dyn ClipTextEncoder>>,
}

impl std::fmt::Debug for SearchEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let model = self.text.as_ref().map(|t| t.info().name.clone());
        f.debug_struct("SearchEngine").field("text_model", &model).finish_non_exhaustive()
    }
}

/// Sözcük aramasına giren sorgu sözcükleri (Türkçe küçük harf, en az 3 harf).
fn keywords(text: &str) -> Vec<String> {
    let mut out: Vec<String> = loupe_db::turkish_lowercase(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_owned)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

impl SearchEngine {
    #[must_use]
    pub fn new(pool: Arc<Pool>, text: Arc<dyn ClipTextEncoder>) -> Self {
        Self { pool, text: Some(text) }
    }

    /// CLIP modeli yokken: süzgeçler ve sözcük araması çalışır, anlamsal adım atlanır.
    #[must_use]
    pub fn keyword_only(pool: Arc<Pool>) -> Self {
        Self { pool, text: None }
    }

    /// Anlamsal (CLIP) adım açık mı.
    #[must_use]
    pub fn is_semantic(&self) -> bool {
        self.text.is_some()
    }

    #[must_use]
    pub const fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    pub fn search(&self, q: &SearchQuery) -> Result<SearchResult, SearchError> {
        let started = Instant::now();
        q.validate()?;
        let compiled = compile(q);
        let limit = q.limit();

        let Some(text) = q.text() else {
            let sql = format!("{} ORDER BY {} LIMIT {limit}", compiled.select("m.id"), order_by(q.sort));
            let ids: Vec<MediaId> = self.pool.read(|c| {
                Ok(c.prepare_cached(&sql)?
                    .query_map(params_from_iter(compiled.params.iter()), |r| Ok(MediaId(r.get(0)?)))?
                    .collect::<Result<_, _>>()?)
            })?;
            let hits = ids
                .into_iter()
                .map(|media_id| SearchHit { media_id, score: 0.0, semantic: None, keyword_rank: None })
                .collect();
            return Ok(SearchResult { hits, candidates: 0, elapsed: started.elapsed() });
        };

        let query_vec = self.text.as_ref().map(|t| t.encode_text(text)).transpose()?;
        let words = keywords(text);
        let filtered = q.has_filters();
        let pool_k = (limit * 4).max(200);

        let (semantic, keyword, candidates) = self.pool.read(|c| {
            // Anlamsal liste.
            let (semantic, candidates) = if let Some(query_vec) = &query_vec
                && filtered
            {
                let sql = format!("{} LIMIT {MAX_CANDIDATES}", compiled.select("m.id"));
                let ids: Vec<MediaId> = c
                    .prepare_cached(&sql)?
                    .query_map(params_from_iter(compiled.params.iter()), |r| Ok(MediaId(r.get(0)?)))?
                    .collect::<Result<_, _>>()?;
                let n = ids.len();
                (clip::similarities(c, query_vec, &ids).map_err(ai_to_db)?, n)
            } else if let Some(query_vec) = &query_vec {
                let near = clip::knn(c, query_vec, pool_k.min(KNN_MAX)).map_err(ai_to_db)?;
                // Çöpteki medya KNN'den gelebilir: temel süzgeçten geçir.
                let alive: std::collections::HashSet<i64> = if near.is_empty() {
                    std::collections::HashSet::new()
                } else {
                    let marks = vec!["?"; near.len()].join(",");
                    c.prepare(&format!("SELECT id FROM media_files WHERE soft_deleted_at IS NULL AND id IN ({marks})"))?
                        .query_map(params_from_iter(near.iter().map(|(m, _)| m.0)), |r| r.get(0))?
                        .collect::<Result<_, _>>()?
                };
                let near: Vec<_> = near.into_iter().filter(|(m, _)| alive.contains(&m.0)).collect();
                let n = near.len();
                (near, n)
            } else {
                (Vec::new(), 0)
            };

            // Sözcük listesi: eşleşen sözcük sayısına göre.
            let keyword: Vec<MediaId> = if words.is_empty() {
                Vec::new()
            } else {
                let mut params: Vec<Value> = compiled.params.clone();
                let mut terms = Vec::with_capacity(words.len());
                for w in &words {
                    params.push(Value::Text(format!("%{}%", like_escape(w))));
                    terms.push(format!("(hay LIKE ?{} ESCAPE '\\')", params.len()));
                }
                let sql = format!(
                    "SELECT id, {score} AS s FROM ({inner}) WHERE {score} > 0
                      ORDER BY s DESC, taken DESC, id DESC LIMIT {pool_k}",
                    score = terms.join(" + "),
                    inner = compiled.select(&format!("m.id AS id, m.taken_at_utc AS taken, {HAYSTACK} AS hay")),
                );
                c.prepare(&sql)?
                    .query_map(params_from_iter(params.iter()), |r| Ok(MediaId(r.get(0)?)))?
                    .collect::<Result<_, _>>()?
            };
            Ok((semantic, keyword, candidates))
        })?;

        // RRF birleşimi.
        let mut fused: HashMap<MediaId, SearchHit> = HashMap::new();
        #[allow(clippy::cast_precision_loss)]
        let rrf = |rank: usize| 1.0 / (RRF_K + rank as f32);
        for (i, (m, sim)) in semantic.iter().enumerate() {
            let h =
                fused.entry(*m).or_insert(SearchHit { media_id: *m, score: 0.0, semantic: None, keyword_rank: None });
            h.score += rrf(i + 1);
            h.semantic = Some(*sim);
        }
        for (i, m) in keyword.iter().enumerate() {
            let h =
                fused.entry(*m).or_insert(SearchHit { media_id: *m, score: 0.0, semantic: None, keyword_rank: None });
            h.score += rrf(i + 1);
            h.keyword_rank = Some(i + 1);
        }
        let mut hits: Vec<SearchHit> = fused.into_values().collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b.semantic.unwrap_or(-1.0).total_cmp(&a.semantic.unwrap_or(-1.0)))
                .then(a.media_id.0.cmp(&b.media_id.0))
        });
        hits.truncate(limit);
        Ok(SearchResult { hits, candidates, elapsed: started.elapsed() })
    }
}

/// `Pool::read` kapanışı `DbError` ister; AI hatası metniyle taşınır.
fn ai_to_db(e: loupe_ai::AiError) -> loupe_db::DbError {
    match e {
        loupe_ai::AiError::Db(d) => d,
        other => loupe_db::DbError::Validation(other.to_string()),
    }
}
