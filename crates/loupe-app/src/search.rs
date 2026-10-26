//! Cmd/Ctrl+K paleti: serbest metin + süzgeç belirteçleri → [`SearchQuery`].
//!
//! ```text
//! deniz kenarında gün batımı person:Ayşe place:Göreme rating:>=4 year:2026
//! ```
//!
//! - Belirteç `anahtar:değer`; boşluklu değer tırnakla yazılır
//!   (`person:"Ayşe Yılmaz"`). Anahtarların Türkçesi de geçer (`kişi:`,
//!   `yer:`, `etkinlik:`, `puan:`, `renk:`, `kamera:`, `objektif:`, `yıl:`,
//!   `favori:`).
//! - Adlar kimliğe burada çözülür (kişi adı → `persons.id`, yer → hiyerarşinin
//!   eşleşen katmanı, etkinlik başlığı → `events.id`). Çözülemeyen belirteç
//!   sorguya girmez, arayüzde kırmızı çip olarak kalır.
//! - Geri kalan sözcükler doğal dil metnidir: CLIP (model varsa) ve adlarda
//!   sözcük araması, RRF ile tek sırada.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use loupe_db::{Pool, turkish_lowercase};
use loupe_search::{PersonFilter, PlaceFilter, SearchEngine, SearchQuery, TimeFilter};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::timeline::{self, SmartAlbumDto, TileItem, TimelineRequest, View};
use crate::{AppError, AppResult};

/// Metinli aramada sıralanan en çok sonuç.
pub const RANKED_LIMIT: usize = 2_000;
/// Palet önizlemesindeki küçük resim sayısı.
pub const PREVIEW: u32 = 30;
/// Sıralı sonuç önbelleği: aynı arama dilim dilim istenirken CLIP yeniden çalışmaz.
pub const RANK_TTL: Duration = Duration::from_secs(120);
const RANK_SLOTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChipKind {
    Person,
    Place,
    Event,
    Rating,
    Color,
    Camera,
    Lens,
    Year,
    Favorite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chip {
    pub kind: ChipKind,
    /// Kullanıcının yazdığı belirteç (`person:ayşe`).
    pub raw: String,
    /// Çözülmüş hali (`Ayşe Yılmaz`).
    pub label: String,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub kind: ChipKind,
    pub label: String,
    /// Son belirtecin yerine yazılacak metin (`person:"Ayşe Yılmaz"`).
    pub insert: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub query: SearchQuery,
    pub chips: Vec<Chip>,
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteRequest {
    pub input: String,
    #[serde(default)]
    pub preview: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteResult {
    /// `View::Search` ve akıllı albüm için hazır sorgu.
    pub query: SearchQuery,
    pub chips: Vec<Chip>,
    pub suggestions: Vec<Suggestion>,
    pub text: Option<String>,
    /// Sorgu boşsa (yalnızca komut aranıyor) `false`; sonuç yoktur.
    pub searched: bool,
    pub total: u64,
    pub items: Vec<TileItem>,
    /// CLIP anlamsal adımı çalıştı mı (model yoksa yalnızca sözcük araması).
    pub semantic: bool,
    pub elapsed_ms: u64,
}

// ───────────────────────────── ayrıştırma ─────────────────────────────

/// Boşlukla ayrılmış belirteçler; tırnak içi tek belirteçtir.
#[must_use]
pub fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in input.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn key_of(k: &str) -> Option<ChipKind> {
    Some(match turkish_lowercase(k).as_str() {
        "person" | "kişi" | "kisi" | "p" => ChipKind::Person,
        "place" | "yer" | "konum" => ChipKind::Place,
        "event" | "etkinlik" => ChipKind::Event,
        "rating" | "puan" | "yıldız" | "yildiz" => ChipKind::Rating,
        "color" | "colour" | "renk" | "label" | "etiket" => ChipKind::Color,
        "camera" | "kamera" => ChipKind::Camera,
        "lens" | "objektif" => ChipKind::Lens,
        "year" | "yıl" | "yil" | "date" | "tarih" => ChipKind::Year,
        "fav" | "favorite" | "favori" | "is" => ChipKind::Favorite,
        _ => return None,
    })
}

/// Etiket rengi: Türkçe ve İngilizce adlar.
#[must_use]
pub fn color_of(v: &str) -> Option<&'static str> {
    Some(match turkish_lowercase(v).as_str() {
        "red" | "kırmızı" | "kirmizi" => "red",
        "yellow" | "sarı" | "sari" => "yellow",
        "green" | "yeşil" | "yesil" => "green",
        "blue" | "mavi" => "blue",
        "purple" | "mor" => "purple",
        _ => return None,
    })
}

/// `>=4`, `>3`, `4`, `=4`, `4+` → en düşük puan.
#[must_use]
pub fn rating_of(v: &str) -> Option<u8> {
    let v = v.trim();
    let (base, plus) = if let Some(r) = v.strip_prefix(">=").or_else(|| v.strip_prefix('≥')) {
        (r, 0)
    } else if let Some(r) = v.strip_prefix('>') {
        (r, 1)
    } else if let Some(r) = v.strip_prefix('=') {
        (r, 0)
    } else if let Some(r) = v.strip_suffix('+') {
        (r, 0)
    } else {
        (v, 0)
    };
    let n: u8 = base.trim().parse().ok()?;
    let n = n.checked_add(plus)?;
    (n <= 5).then_some(n)
}

/// `2026`, `2026-10` ya da `2024..2026`.
#[must_use]
pub fn year_of(v: &str) -> Option<TimeFilter> {
    let in_range = |y: i32| (1826..=2200).contains(&y);
    if let Some((a, b)) = v.split_once("..") {
        let (a, b): (i32, i32) = (a.parse().ok()?, b.parse().ok()?);
        if !in_range(a) || !in_range(b) || a > b {
            return None;
        }
        let ms = |y: i32| days_from_civil(i64::from(y), 1, 1) * 86_400_000;
        return Some(TimeFilter { from_utc: Some(ms(a)), to_utc: Some(ms(b + 1)), ..TimeFilter::default() });
    }
    let mut parts = v.splitn(2, '-');
    let y: i32 = parts.next()?.parse().ok()?;
    if !in_range(y) {
        return None;
    }
    let month = match parts.next() {
        Some(m) => Some(m.parse::<u32>().ok().filter(|m| (1..=12).contains(m))?),
        None => None,
    };
    Some(TimeFilter { year: Some(y), month, ..TimeFilter::default() })
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn like_prefix(v: &str) -> String {
    format!("{}%", turkish_lowercase(v).replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

fn like_any(v: &str) -> String {
    format!("%{}", like_prefix(v))
}

fn quote(v: &str) -> String {
    if v.chars().any(char::is_whitespace) { format!("\"{v}\"") } else { v.to_owned() }
}

/// Kişi adı → kimlik: önce tam ad, sonra önek (en çok yüzü olan).
fn resolve_person(c: &Connection, v: &str) -> rusqlite::Result<Option<(i64, String)>> {
    let exact = c
        .prepare_cached("SELECT id, name FROM persons WHERE is_hidden = 0 AND tr_lower(name) = ?1 ORDER BY id LIMIT 1")?
        .query_row([turkish_lowercase(v)], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    if exact.is_some() {
        return Ok(exact);
    }
    c.prepare_cached(
        "SELECT id, name FROM persons WHERE is_hidden = 0 AND tr_lower(name) LIKE ?1 ESCAPE '\\'
          ORDER BY face_count DESC, id LIMIT 1",
    )?
    .query_row([like_prefix(v)], |r| Ok((r.get(0)?, r.get(1)?)))
    .optional()
}

const PLACE_LEVELS: [&str; 4] = ["locality", "admin2", "admin1", "country"];

/// Yer adı → hiyerarşinin eşleşen en ince katmanı (yerleşim, ilçe, il, ülke).
fn resolve_place(c: &Connection, v: &str) -> rusqlite::Result<Option<(PlaceFilter, String)>> {
    for exact in [true, false] {
        for col in PLACE_LEVELS {
            let (op, arg) = if exact { ("=", turkish_lowercase(v)) } else { ("LIKE", like_prefix(v)) };
            let sql = format!(
                "SELECT {col} FROM locations WHERE {col} IS NOT NULL AND tr_lower({col}) {op} ?1 ESCAPE '\\'
                  GROUP BY {col} ORDER BY count(*) DESC LIMIT 1"
            );
            let sql = if exact { sql.replace(" ESCAPE '\\'", "") } else { sql };
            if let Some(name) = c.prepare_cached(&sql)?.query_row([&arg], |r| r.get::<_, String>(0)).optional()? {
                let mut f = PlaceFilter::default();
                match col {
                    "locality" => f.locality = Some(name.clone()),
                    "admin2" => f.admin2 = Some(name.clone()),
                    "admin1" => f.admin1 = Some(name.clone()),
                    _ => f.country = Some(name.clone()),
                }
                return Ok(Some((f, name)));
            }
        }
    }
    // Ülke kodu (`TR`).
    if v.len() == 2 {
        let code = v.to_ascii_uppercase();
        let found: Option<String> = c
            .prepare_cached("SELECT country FROM locations WHERE country_code = ?1 LIMIT 1")?
            .query_row([&code], |r| r.get(0))
            .optional()?;
        if let Some(country) = found {
            return Ok(Some((PlaceFilter { country_code: Some(code), ..PlaceFilter::default() }, country)));
        }
    }
    Ok(None)
}

fn resolve_event(c: &Connection, v: &str) -> rusqlite::Result<Option<(i64, String)>> {
    c.prepare_cached(
        "SELECT id, title FROM events WHERE tr_lower(title) LIKE ?1 ESCAPE '\\' AND media_count > 0
          ORDER BY (tr_lower(title) = ?2) DESC, start_utc DESC LIMIT 1",
    )?
    .query_row([like_any(v), turkish_lowercase(v)], |r| Ok((r.get(0)?, r.get(1)?)))
    .optional()
}

/// Kamera/objektif: süzgeç tam eşleşme ister; yazılanı katalogdaki adla değiştir.
fn resolve_model(c: &Connection, col: &str, v: &str) -> rusqlite::Result<Option<String>> {
    c.prepare_cached(&format!(
        "SELECT {col} FROM media_files WHERE {col} IS NOT NULL AND tr_lower({col}) LIKE ?1 ESCAPE '\\'
          GROUP BY {col} ORDER BY (tr_lower({col}) = ?2) DESC, count(*) DESC LIMIT 1"
    ))?
    .query_row([like_any(v), turkish_lowercase(v)], |r| r.get(0))
    .optional()
}

/// Girdiyi sorguya çevirir. `suggest`: son belirteç için tamamlama önerileri.
pub fn parse(c: &Connection, input: &str, suggest: bool) -> AppResult<Parsed> {
    let tokens = tokenize(input);
    let mut q = SearchQuery::default();
    let mut chips = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut persons: Vec<i64> = Vec::new();

    for tok in &tokens {
        let Some((k, v)) = tok.split_once(':') else {
            words.push(tok.clone());
            continue;
        };
        let Some(kind) = key_of(k) else {
            words.push(tok.clone());
            continue;
        };
        let v = v.trim();
        let mut chip = |label: String, ok: bool, error: Option<&str>| {
            chips.push(Chip { kind, raw: tok.clone(), label, ok, error: error.map(str::to_owned) });
        };
        if v.is_empty() && kind != ChipKind::Favorite {
            // Yazılmakta olan belirteç: yalnızca öneri.
            continue;
        }
        match kind {
            ChipKind::Person => match resolve_person(c, v)? {
                Some((id, name)) => {
                    if !persons.contains(&id) {
                        persons.push(id);
                    }
                    chip(name, true, None);
                }
                None => chip(v.to_owned(), false, Some("kişi bulunamadı")),
            },
            ChipKind::Place => match resolve_place(c, v)? {
                Some((f, name)) => {
                    q.place = Some(f);
                    chip(name, true, None);
                }
                None => chip(v.to_owned(), false, Some("yer bulunamadı")),
            },
            ChipKind::Event => match resolve_event(c, v)? {
                Some((id, title)) => {
                    q.event_id = Some(id);
                    chip(title, true, None);
                }
                None => chip(v.to_owned(), false, Some("etkinlik bulunamadı")),
            },
            ChipKind::Rating => match rating_of(v) {
                Some(r) => {
                    q.min_rating = Some(r);
                    chip(format!("≥ {r} yıldız"), true, None);
                }
                None => chip(v.to_owned(), false, Some("puan 0–5; yalnızca ≥ desteklenir")),
            },
            ChipKind::Color => match color_of(v) {
                Some(col) => {
                    if !q.color_labels.iter().any(|x| x == col) {
                        q.color_labels.push(col.to_owned());
                    }
                    chip(col.to_owned(), true, None);
                }
                None => chip(v.to_owned(), false, Some("renk: kırmızı, sarı, yeşil, mavi, mor")),
            },
            ChipKind::Camera => match resolve_model(c, "camera_model", v)? {
                Some(m) => {
                    q.camera_model = Some(m.clone());
                    chip(m, true, None);
                }
                None => chip(v.to_owned(), false, Some("kamera bulunamadı")),
            },
            ChipKind::Lens => match resolve_model(c, "lens_model", v)? {
                Some(m) => {
                    q.lens_model = Some(m.clone());
                    chip(m, true, None);
                }
                None => chip(v.to_owned(), false, Some("objektif bulunamadı")),
            },
            ChipKind::Year => match year_of(v) {
                Some(t) => {
                    q.time = Some(t);
                    chip(v.to_owned(), true, None);
                }
                None => chip(v.to_owned(), false, Some("yıl: 2026, 2026-10 ya da 2024..2026")),
            },
            ChipKind::Favorite => {
                let lv = turkish_lowercase(v);
                if matches!(lv.as_str(), "" | "true" | "evet" | "yes" | "1" | "fav" | "favori" | "favorite") {
                    q.favorite = Some(true);
                    chip("Favoriler".into(), true, None);
                } else {
                    chip(v.to_owned(), false, Some("favori: evet"));
                }
            }
        }
    }
    if !persons.is_empty() {
        q.persons = Some(PersonFilter { ids: persons, mode: loupe_search::Match::All });
    }
    let text = words.join(" ");
    q.text = (!text.trim().is_empty()).then_some(text);

    let suggestions = if suggest { suggestions(c, input, &tokens)? } else { Vec::new() };
    Ok(Parsed { query: q, chips, suggestions })
}

/// Son belirteç için öneriler: `person:ay` → kişiler; düz sözcük → her türden eşleşen ad.
fn suggestions(c: &Connection, input: &str, tokens: &[String]) -> AppResult<Vec<Suggestion>> {
    // Boşlukla bitiyorsa son belirteç tamamlanmıştır.
    if input.ends_with(char::is_whitespace) {
        return Ok(Vec::new());
    }
    let Some(last) = tokens.last() else { return Ok(Vec::new()) };
    let (kind, prefix) = match last.split_once(':') {
        Some((k, v)) => match key_of(k) {
            Some(kind) => (Some(kind), v),
            None => return Ok(Vec::new()),
        },
        None if last.chars().count() >= 2 => (None, last.as_str()),
        None => return Ok(Vec::new()),
    };
    let want = |k: ChipKind| kind.is_none_or(|x| x == k);
    let mut out = Vec::new();
    let pat = like_prefix(prefix);
    let n = if kind.is_some() { 8 } else { 3 };
    if want(ChipKind::Person) {
        let rows: Vec<(String, i64)> = c
            .prepare_cached(
                "SELECT name, face_count FROM persons WHERE is_hidden = 0 AND face_count > 0
                   AND tr_lower(name) LIKE ?1 ESCAPE '\\' ORDER BY face_count DESC LIMIT ?2",
            )?
            .query_map(rusqlite::params![pat, n], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        out.extend(rows.into_iter().map(|(name, faces)| Suggestion {
            kind: ChipKind::Person,
            insert: format!("person:{}", quote(&name)),
            label: name,
            detail: Some(format!("{faces} yüz")),
        }));
    }
    if want(ChipKind::Place) {
        let rows: Vec<(String, i64)> = c
            .prepare_cached(
                "SELECT name, count(*) FROM (
                   SELECT COALESCE(l.locality, l.admin2, l.admin1, l.country) AS name
                     FROM media_files m JOIN locations l ON l.id = m.location_id WHERE m.soft_deleted_at IS NULL)
                  WHERE tr_lower(name) LIKE ?1 ESCAPE '\\' GROUP BY name ORDER BY count(*) DESC LIMIT ?2",
            )?
            .query_map(rusqlite::params![pat, n], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        out.extend(rows.into_iter().map(|(name, count)| Suggestion {
            kind: ChipKind::Place,
            insert: format!("place:{}", quote(&name)),
            label: name,
            detail: Some(format!("{count} fotoğraf")),
        }));
    }
    if want(ChipKind::Event) {
        let rows: Vec<(String, i64)> = c
            .prepare_cached(
                "SELECT title, media_count FROM events WHERE media_count > 0 AND tr_lower(title) LIKE ?1 ESCAPE '\\'
                  ORDER BY start_utc DESC LIMIT ?2",
            )?
            .query_map(rusqlite::params![like_any(prefix), n], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        out.extend(rows.into_iter().map(|(title, count)| Suggestion {
            kind: ChipKind::Event,
            insert: format!("event:{}", quote(&title)),
            label: title,
            detail: Some(format!("{count} fotoğraf")),
        }));
    }
    if kind == Some(ChipKind::Camera) {
        let rows: Vec<(String, i64)> = c
            .prepare_cached(
                "SELECT camera_model, count(*) FROM media_files WHERE camera_model IS NOT NULL
                   AND tr_lower(camera_model) LIKE ?1 ESCAPE '\\' GROUP BY camera_model ORDER BY count(*) DESC LIMIT 8",
            )?
            .query_map([like_any(prefix)], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        out.extend(rows.into_iter().map(|(m, count)| Suggestion {
            kind: ChipKind::Camera,
            insert: format!("camera:{}", quote(&m)),
            label: m,
            detail: Some(format!("{count} fotoğraf")),
        }));
    }
    if kind == Some(ChipKind::Color) {
        for (tr, en) in
            [("kırmızı", "red"), ("sarı", "yellow"), ("yeşil", "green"), ("mavi", "blue"), ("mor", "purple")]
        {
            let p = turkish_lowercase(prefix);
            if tr.starts_with(&p) || en.starts_with(&p) {
                out.push(Suggestion {
                    kind: ChipKind::Color,
                    label: tr.to_owned(),
                    insert: format!("color:{tr}"),
                    detail: None,
                });
            }
        }
    }
    if kind == Some(ChipKind::Rating) {
        for r in (1..=5u8).rev() {
            out.push(Suggestion {
                kind: ChipKind::Rating,
                label: format!("≥ {r} yıldız"),
                insert: format!("rating:>={r}"),
                detail: None,
            });
        }
    }
    Ok(out)
}

// ───────────────────────────── çalıştırma ─────────────────────────────

/// Sıralı arama sonuçlarının küçük önbelleği (sorgu JSON'u → kimlikler).
/// (sorgu JSON'u, zaman, kimlikler)
type RankSlot = (String, Instant, Arc<Vec<i64>>);

#[derive(Debug, Default)]
pub struct RankCache {
    slots: Mutex<Vec<RankSlot>>,
}

impl RankCache {
    fn get(&self, key: &str) -> Option<Arc<Vec<i64>>> {
        let mut s = self.slots.lock().ok()?;
        s.retain(|(_, at, _)| at.elapsed() < RANK_TTL);
        s.iter().find(|(k, _, _)| k == key).map(|(_, _, v)| v.clone())
    }

    fn put(&self, key: String, ids: Arc<Vec<i64>>) {
        if let Ok(mut s) = self.slots.lock() {
            s.retain(|(k, _, _)| *k != key);
            if s.len() >= RANK_SLOTS {
                s.remove(0);
            }
            s.push((key, Instant::now(), ids));
        }
    }

    /// Katalog değişti (ad, puan, yeni medya): önbellek boşalır.
    pub fn clear(&self) {
        if let Ok(mut s) = self.slots.lock() {
            s.clear();
        }
    }
}

/// Metinli sorgunun alaka sırasıyla kimlikleri (önbellekten ya da motordan).
pub fn ranked_ids(engine: &SearchEngine, cache: &RankCache, q: &SearchQuery) -> AppResult<Arc<Vec<i64>>> {
    let mut q = q.clone();
    q.limit = Some(q.limit.unwrap_or(RANKED_LIMIT).min(loupe_search::query::MAX_LIMIT));
    let key = serde_json::to_string(&q).map_err(|e| AppError::Invalid(e.to_string()))?;
    if let Some(ids) = cache.get(&key) {
        return Ok(ids);
    }
    let res = engine.search(&q)?;
    let ids = Arc::new(res.hits.iter().map(|h| h.media_id.0).collect::<Vec<_>>());
    cache.put(key, ids.clone());
    Ok(ids)
}

/// Paletin tek isteği: ayrıştır, öner, ilk sonuçları getir.
pub fn palette(engine: &SearchEngine, cache: &RankCache, req: &PaletteRequest) -> AppResult<PaletteResult> {
    let started = Instant::now();
    if req.input.chars().count() > 500 {
        return Err(AppError::Invalid("arama en çok 500 karakter".into()));
    }
    let parsed = engine.pool().read(|c| parse(c, &req.input, true).map_err(to_db))?;
    let text = parsed.query.text().map(str::to_owned);
    let searched = text.is_some() || parsed.query.has_filters();
    let preview = req.preview.unwrap_or(PREVIEW).clamp(1, 200);
    let (total, items) = if !searched {
        (0, Vec::new())
    } else if text.is_some() {
        let ids = ranked_ids(engine, cache, &parsed.query)?;
        let view = View::Search { query: Box::new(parsed.query.clone()) };
        let req = TimelineRequest { view, arrange: timeline::Arrange::default(), offset: 0, limit: preview };
        let slice = timeline::slice_ranked(engine.pool(), &req, Some(&ids))?;
        (ids.len() as u64, slice.items)
    } else {
        let view = View::Search { query: Box::new(parsed.query.clone()) };
        let req = TimelineRequest { view, arrange: timeline::Arrange::default(), offset: 0, limit: preview };
        let slice = timeline::slice(engine.pool(), &req)?;
        (u64::from(slice.total), slice.items)
    };
    Ok(PaletteResult {
        query: parsed.query,
        chips: parsed.chips,
        suggestions: parsed.suggestions,
        text,
        searched,
        total,
        items,
        semantic: engine.is_semantic(),
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// Etkin aramayı akıllı albüm olarak saklar (kenar çubuğunda görünür).
pub fn save_smart_album(pool: &Pool, name: &str, query: &SearchQuery, pinned: bool) -> AppResult<SmartAlbumDto> {
    if !query.has_filters() && query.text().is_none() {
        return Err(AppError::Invalid("boş arama akıllı albüm olamaz".into()));
    }
    let id = pool.write(|c| loupe_search::albums::create(c, name, query, pinned).map_err(search_to_db))?;
    Ok(SmartAlbumDto { id: id.0, name: name.trim().to_owned(), pinned })
}

pub fn delete_smart_album(pool: &Pool, id: i64) -> AppResult<()> {
    let n = pool.write(|c| Ok(c.execute("DELETE FROM smart_albums WHERE id = ?1", [id])?))?;
    if n == 0 {
        return Err(AppError::NotFound { entity: "akıllı albüm", id });
    }
    Ok(())
}

fn search_to_db(e: loupe_search::SearchError) -> loupe_db::DbError {
    match e {
        loupe_search::SearchError::Db(d) => d,
        other => loupe_db::DbError::Validation(other.to_string()),
    }
}

fn to_db(e: AppError) -> loupe_db::DbError {
    match e {
        AppError::Db(d) => d,
        other => loupe_db::DbError::Validation(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_respect_quotes() {
        assert_eq!(
            tokenize(r#"gün batımı person:"Ayşe Yılmaz"  rating:>=4"#),
            ["gün", "batımı", "person:Ayşe Yılmaz", "rating:>=4"]
        );
    }

    #[test]
    fn rating_color_year() {
        assert_eq!(rating_of(">=4"), Some(4));
        assert_eq!(rating_of(">3"), Some(4));
        assert_eq!(rating_of("5+"), Some(5));
        assert_eq!(rating_of(">5"), None);
        assert_eq!(rating_of("<3"), None);
        assert_eq!(color_of("Kırmızı"), Some("red"));
        assert_eq!(color_of("BLUE"), Some("blue"));
        assert_eq!(color_of("turuncu"), None);
        assert_eq!(year_of("2026").and_then(|t| t.year), Some(2026));
        assert_eq!(year_of("2026-10").and_then(|t| t.month), Some(10));
        let r = year_of("2024..2025").unwrap();
        assert_eq!(r.from_utc, Some(1_704_067_200_000));
        assert_eq!(r.to_utc, Some(1_767_225_600_000));
        assert!(year_of("2026-13").is_none());
        assert!(year_of("abc").is_none());
    }
}
