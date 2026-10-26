//! Zaman çizelgesi dilimleri.
//!
//! Arayüz justified düzeni için her öğenin en-boy oranını ve bölüm (gün, ay,
//! etkinlik, kişi, yer) sınırlarını ister. Bölümler her dilimde görünümün
//! tamamı için döner (yalnızca anahtar sütunu okunur, 100k satırda birkaç
//! on ms); öğeler `offset..offset+limit` aralığıdır. Kütüphane ne kadar büyük
//! olursa olsun aynı sorgu ve aynı sıralama kullanılır, böylece dilimler üst
//! üste binmez.

use loupe_db::Pool;
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::{AppError, AppResult};

/// En büyük dilim (arayüz 100k'yı ~20 istekte alır).
pub const MAX_SLICE: u32 = 5_000;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum View {
    All,
    Favorites,
    /// Yalnızca videolar.
    Videos,
    Library {
        id: i64,
    },
    /// Etkinlik başlıklarıyla.
    Events,
    /// Adlandırılmış kişilerle (bir fotoğraf birden çok kişide görünebilir).
    People,
    /// Çevrimdışı çözülmüş yerlere göre.
    Places,
    SmartAlbum {
        id: i64,
    },
    /// Arama paletinin sorgusu. Metin varsa sıralama alakaya göredir.
    Search {
        query: Box<loupe_search::SearchQuery>,
    },
    /// Tek kişinin fotoğrafları.
    Person {
        id: i64,
    },
    /// İsimsiz yüz kümesinin fotoğrafları.
    Cluster {
        id: i64,
    },
    /// Tek etkinliğin fotoğrafları (çekim sırasıyla).
    Event {
        id: i64,
    },
}

impl View {
    /// Alaka sıralaması gerektiren (CLIP/sözcük) sorgu.
    pub fn ranked_query(&self, conn: &Connection) -> AppResult<Option<loupe_search::SearchQuery>> {
        let q = match self {
            Self::Search { query } => (**query).clone(),
            Self::SmartAlbum { id } => loupe_search::albums::get(conn, loupe_search::SmartAlbumId(*id))?.query,
            _ => return Ok(None),
        };
        Ok(q.text().is_some().then_some(q))
    }
}

/// Alaka sırasıyla tek bölümün başlığı.
pub const RANKED_TITLE: &str = "En alakalı";

/// Tarih sıralamalarında bölümleme. Ad, boyut ve puan sıralamaları kendi
/// bölümlerini kurar; `None` tek bölümdür.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    #[default]
    Day,
    Month,
    Year,
    None,
}

/// Fotoğraf görünümlerinin sıralama anahtarı.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    /// Çekim tarihi.
    #[default]
    Taken,
    /// Kataloğa eklenme tarihi.
    Added,
    /// Özgün dosya adı.
    Name,
    /// Dosya boyutu.
    Size,
    /// Puan (aynı puanlar çekim tarihine göre).
    Rating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    #[default]
    Desc,
    Asc,
}

impl SortDir {
    const fn sql(self) -> &'static str {
        match self {
            Self::Desc => "DESC",
            Self::Asc => "ASC",
        }
    }
}

/// Görünümün dizilişi: sıralama, yön ve bölümleme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Arrange {
    #[serde(default)]
    pub sort: SortKey,
    #[serde(default)]
    pub dir: SortDir,
    #[serde(default)]
    pub grouping: Grouping,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineRequest {
    pub view: View,
    #[serde(flatten)]
    pub arrange: Arrange,
    #[serde(default)]
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub key: String,
    pub title: String,
    pub subtitle: Option<String>,
    /// Görünümdeki ilk öğenin sırası.
    pub start: u32,
    pub count: u32,
}

/// Grid hücresinin ihtiyacı olan her şey.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileItem {
    pub id: i64,
    /// BLAKE3 hex: `thumb://sm/{hash}`.
    pub hash: String,
    /// Genişlik / yükseklik (EXIF yönlendirmesi uygulanmış). Boyut bilinmiyorsa 1.5.
    pub aspect: f32,
    pub rating: u8,
    pub label: Option<String>,
    pub favorite: bool,
    /// `none` | `pick` | `reject`
    pub flag: String,
    /// `NEF`, `CR2`, `ARW`… ya da `DNG`; JPEG/HEIC için yok.
    pub raw: Option<String>,
    pub video: bool,
    /// Video süresi (ms); fotoğrafta ya da süre okunamadıysa yok.
    pub duration_ms: Option<i64>,
    /// `local` | `queued` | `synced` | `error` | `offline`
    pub sync: &'static str,
    pub taken_at_utc: i64,
    pub tz_offset_min: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineSlice {
    pub total: u32,
    pub offset: u32,
    pub sections: Vec<Section>,
    pub items: Vec<TileItem>,
}

const LOCAL_SECONDS: &str = "(m.taken_at_utc + COALESCE(m.tz_offset_min, 0) * 60000) / 1000";
/// Eklenme anı UTC ms; bölüm bu makinenin saat dilimiyle.
const ADDED_SECONDS: &str = "m.imported_at / 1000";

/// Görünümün SQL gövdesi: ek JOIN'ler, WHERE, bölüm anahtarı ve sıralama.
struct Plan {
    joins: String,
    where_sql: String,
    params: Vec<Value>,
    key: String,
    /// Bölüm başlığı için ek sütun (etkinlik, kişi, yer adı).
    title: String,
    subtitle: String,
    order: String,
    /// Anahtar bir tarih (`2026-10-24`, `2026-10`, `2026`); başlık ondan üretilir.
    dated: bool,
}

/// Sıralamaya göre bölüm anahtarı, başlık sütunu, tarih mi ve ORDER BY.
fn arrangement(a: Arrange) -> (String, String, bool, String) {
    let dir = a.dir.sql();
    let date_key = |secs: &str, modifier: &str| {
        let fmt = match a.grouping {
            Grouping::Day => "%Y-%m-%d",
            Grouping::Month => "%Y-%m",
            Grouping::Year => "%Y",
            Grouping::None => return ("'all'".to_owned(), "'Tümü'".to_owned(), false),
        };
        (format!("strftime('{fmt}', {secs}, 'unixepoch'{modifier})"), "NULL".to_owned(), true)
    };
    let single = |title: &str| ("'all'".to_owned(), format!("'{title}'"), false);
    let ((key, title, dated), order) = match a.sort {
        SortKey::Taken => (date_key(LOCAL_SECONDS, ""), format!("m.taken_at_utc {dir}, m.id {dir}")),
        SortKey::Added => (date_key(ADDED_SECONDS, ", 'localtime'"), format!("m.imported_at {dir}, m.id {dir}")),
        SortKey::Name => (single("Ada göre"), format!("m.original_filename COLLATE NOCASE {dir}, m.id {dir}")),
        SortKey::Size => (single("Boyuta göre"), format!("m.size_bytes {dir}, m.id {dir}")),
        SortKey::Rating => (
            (
                "'r' || m.rating".to_owned(),
                "CASE m.rating WHEN 0 THEN 'Puansız' ELSE m.rating || ' yıldız' END".to_owned(),
                false,
            ),
            format!("m.rating {dir}, m.taken_at_utc DESC, m.id DESC"),
        ),
    };
    (key, title, dated, order)
}

fn plan(conn: &Connection, view: &View, arrange: Arrange, ranked: Option<&[i64]>) -> AppResult<Plan> {
    let (key, title, dated, order) = arrangement(arrange);
    let by_date = |where_sql: String, params: Vec<Value>| Plan {
        joins: String::new(),
        where_sql,
        params,
        key: key.clone(),
        title: title.clone(),
        subtitle: "NULL".into(),
        order: order.clone(),
        dated,
    };
    let base = "m.soft_deleted_at IS NULL".to_owned();
    if let Some(ids) = ranked {
        let json = serde_json::to_string(ids).map_err(|e| AppError::Invalid(e.to_string()))?;
        return Ok(Plan {
            joins: "JOIN json_each(?1) r ON r.value = m.id".into(),
            where_sql: base,
            params: vec![Value::Text(json)],
            key: "'r'".into(),
            title: format!("'{RANKED_TITLE}'"),
            subtitle: "NULL".into(),
            order: "r.key".into(),
            dated: false,
        });
    }
    Ok(match view {
        View::All => by_date(base, Vec::new()),
        View::Favorites => by_date(format!("{base} AND m.is_favorite = 1"), Vec::new()),
        View::Videos => by_date(format!("{base} AND m.file_type = 'video'"), Vec::new()),
        View::Library { id } => by_date(format!("{base} AND m.library_id = ?1"), vec![Value::Integer(*id)]),
        // Metinli sorgular `ranked` ile gelir; burada yalnızca süzgeçler.
        View::SmartAlbum { id } => {
            let album = loupe_search::albums::get(conn, loupe_search::SmartAlbumId(*id))?;
            let compiled = loupe_search::compile::compile(&album.query);
            by_date(compiled.where_sql, compiled.params)
        }
        View::Search { query } => {
            query.validate()?;
            let compiled = loupe_search::compile::compile(query);
            by_date(compiled.where_sql, compiled.params)
        }
        View::Person { id } => by_date(
            format!("{base} AND EXISTS (SELECT 1 FROM v_face_person v WHERE v.media_id = m.id AND v.person_id = ?1)"),
            vec![Value::Integer(*id)],
        ),
        View::Cluster { id } => by_date(
            format!(
                "{base} AND EXISTS (SELECT 1 FROM faces f WHERE f.media_id = m.id AND f.cluster_id = ?1
                                     AND f.is_rejected = 0)"
            ),
            vec![Value::Integer(*id)],
        ),
        View::Event { id } => {
            // Etkinlik bir hikâyedir: varsayılan diziliş çekim sırasıyla (eskiden yeniye).
            let (key, title, dated, order) = if arrange == Arrange::default() {
                arrangement(Arrange { dir: SortDir::Asc, ..arrange })
            } else {
                (key.clone(), title.clone(), dated, order.clone())
            };
            Plan {
                joins:
                    "JOIN event_media em ON em.media_id = m.id AND em.source <> 'user_excluded' AND em.event_id = ?1"
                        .into(),
                where_sql: base,
                params: vec![Value::Integer(*id)],
                key,
                title,
                subtitle: "NULL".into(),
                order,
                dated,
            }
        }
        View::Events => Plan {
            joins: "JOIN event_media em ON em.media_id = m.id AND em.source <> 'user_excluded'
                    JOIN events e ON e.id = em.event_id"
                .into(),
            where_sql: base,
            params: Vec::new(),
            key: "'e' || e.id".into(),
            title: "e.title".into(),
            subtitle: "e.start_utc || ':' || e.end_utc".into(),
            order: "e.start_utc DESC, e.id DESC, m.taken_at_utc DESC, m.id DESC".into(),
            dated: false,
        },
        View::People => Plan {
            joins: "JOIN (SELECT DISTINCT media_id, person_id FROM v_face_person WHERE person_id IS NOT NULL) vp
                      ON vp.media_id = m.id
                    JOIN persons p ON p.id = vp.person_id AND p.is_hidden = 0"
                .into(),
            where_sql: base,
            params: Vec::new(),
            key: "'p' || p.id".into(),
            title: "p.name".into(),
            subtitle: "NULL".into(),
            order: "p.name COLLATE NOCASE, p.id, m.taken_at_utc DESC, m.id DESC".into(),
            dated: false,
        },
        View::Places => Plan {
            joins: String::new(),
            where_sql: format!("{base} AND m.location_id IS NOT NULL"),
            params: Vec::new(),
            key: "'l' || l.id".into(),
            title: "COALESCE(l.user_label, l.locality, l.admin2, l.admin1, l.country)".into(),
            subtitle: "COALESCE(l.admin1 || ', ', '') || l.country".into(),
            order: "l.country, l.admin1, COALESCE(l.locality, l.admin2), l.id, m.taken_at_utc DESC, m.id DESC".into(),
            dated: false,
        },
    })
}

impl Plan {
    fn from(&self) -> String {
        format!(
            "FROM media_files m LEFT JOIN locations l ON l.id = m.location_id {} WHERE {}",
            self.joins, self.where_sql
        )
    }
}

const TR_MONTHS: [&str; 12] =
    ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"];
const TR_WEEKDAYS: [&str; 7] = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];

/// `2026-10-24` → `24 Ekim 2026` ve `Cumartesi`; `2026-10` → `Ekim 2026`.
#[must_use]
pub fn date_title(key: &str) -> (String, Option<String>) {
    let mut parts = key.split('-');
    let (Some(y), Some(m)) = (parts.next().and_then(|s| s.parse::<i64>().ok()), parts.next()) else {
        return (key.to_owned(), None);
    };
    let Some(month) = m.parse::<usize>().ok().and_then(|m| TR_MONTHS.get(m.wrapping_sub(1))) else {
        return (key.to_owned(), None);
    };
    match parts.next().and_then(|d| d.parse::<u32>().ok()) {
        Some(d) => {
            let days = days_from_civil(y, m.parse().unwrap_or(1), d);
            // 1970-01-01 perşembe (Pazartesi = 0 dizisinde 3).
            let wd = usize::try_from((days + 3).rem_euclid(7)).unwrap_or(0);
            (format!("{d} {month} {y}"), TR_WEEKDAYS.get(wd).map(|s| (*s).to_owned()))
        }
        None => (format!("{month} {y}"), None),
    }
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

/// `start:end` (UTC ms) → `12–15 Ekim 2026`.
pub(crate) fn event_subtitle(raw: &str) -> Option<String> {
    let (a, b) = raw.split_once(':')?;
    let day = |ms: i64| {
        let (y, m, d) = loupe_storage::layout::civil_from_days(ms.div_euclid(86_400_000));
        (y, m, d)
    };
    let (y1, m1, d1) = day(a.parse().ok()?);
    let (y2, m2, d2) = day(b.parse().ok()?);
    let mname = |m: u32| TR_MONTHS.get(m as usize - 1).copied().unwrap_or("");
    Some(if (y1, m1, d1) == (y2, m2, d2) {
        format!("{d1} {} {y1}", mname(m1))
    } else if (y1, m1) == (y2, m2) {
        format!("{d1}–{d2} {} {y1}", mname(m1))
    } else if y1 == y2 {
        format!("{d1} {} – {d2} {} {y1}", mname(m1), mname(m2))
    } else {
        format!("{d1} {} {y1} – {d2} {} {y2}", mname(m1), mname(m2))
    })
}

fn sections(conn: &Connection, p: &Plan) -> AppResult<Vec<Section>> {
    let sql = format!("SELECT {} AS k, {}, {} {} ORDER BY {}", p.key, p.title, p.subtitle, p.from(), p.order);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(p.params.iter()))?;
    let mut out: Vec<Section> = Vec::new();
    let mut index: u32 = 0;
    while let Some(r) = rows.next()? {
        let key: String = r.get(0)?;
        match out.last_mut() {
            Some(s) if s.key == key => s.count += 1,
            _ => {
                let (title, subtitle) = if p.dated {
                    date_title(&key)
                } else {
                    let title: Option<String> = r.get(1)?;
                    let sub: Option<String> = r.get(2)?;
                    let sub = sub.and_then(|s| if key.starts_with('e') { event_subtitle(&s) } else { Some(s) });
                    (title.unwrap_or_else(|| key.clone()), sub)
                };
                out.push(Section { key, title, subtitle, start: index, count: 1 });
            }
        }
        index += 1;
    }
    Ok(out)
}

fn aspect(w: Option<i64>, h: Option<i64>, orientation: Option<i64>) -> f32 {
    match (w, h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            // EXIF 5–8: 90° döndürülmüş; genişlik ve yükseklik yer değiştirir.
            let (w, h) = if matches!(orientation, Some(5..=8)) { (h, w) } else { (w, h) };
            ((w as f64 / h as f64) as f32).clamp(0.1, 10.0)
        }
        _ => 1.5,
    }
}

const ITEM_COLS: &str = "m.id, m.blake3, m.width, m.height, m.orientation, m.rating, m.color_label, m.is_favorite,
    m.flag, m.file_type, m.raw_format, m.taken_at_utc, m.tz_offset_min,
    (SELECT lb.state FROM libraries lb WHERE lb.id = m.library_id),
    (SELECT CASE WHEN count(*) = 0 THEN 'local'
                 WHEN sum(s.state IN ('error','conflict')) > 0 THEN 'error'
                 WHEN sum(s.state <> 'synced') = 0 THEN 'synced'
                 ELSE 'queued' END
       FROM sync_status s WHERE s.media_id = m.id),
    m.duration_ms";

fn items(conn: &Connection, p: &Plan, offset: u32, limit: u32) -> AppResult<Vec<TileItem>> {
    let n = p.params.len();
    let sql = format!("SELECT {ITEM_COLS} {} ORDER BY {} LIMIT ?{} OFFSET ?{}", p.from(), p.order, n + 1, n + 2);
    let mut params = p.params.clone();
    params.push(Value::Integer(i64::from(limit)));
    params.push(Value::Integer(i64::from(offset)));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        let blake: Vec<u8> = r.get(1)?;
        let file_type: String = r.get(9)?;
        let raw: Option<String> = r.get(10)?;
        let lib_state: Option<String> = r.get(13)?;
        let sync: String = r.get(14)?;
        Ok(TileItem {
            id: r.get(0)?,
            hash: blake.iter().map(|b| format!("{b:02x}")).collect(),
            aspect: aspect(r.get(2)?, r.get(3)?, r.get(4)?),
            rating: r.get(5)?,
            label: r.get(6)?,
            favorite: r.get(7)?,
            flag: r.get(8)?,
            raw: raw.or_else(|| (file_type == "dng").then(|| "DNG".to_owned())),
            video: file_type == "video",
            duration_ms: r.get(15)?,
            sync: if lib_state.as_deref() != Some("online") {
                "offline"
            } else {
                match sync.as_str() {
                    "synced" => "synced",
                    "queued" => "queued",
                    "error" => "error",
                    _ => "local",
                }
            },
            taken_at_utc: r.get(11)?,
            tz_offset_min: r.get(12)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Görünümün `offset..offset+limit` dilimi ve tüm bölümleri.
pub fn slice(pool: &Pool, req: &TimelineRequest) -> AppResult<TimelineSlice> {
    slice_ranked(pool, req, None)
}

/// `ranked`: aramanın alaka sırasıyla medya kimlikleri (tek bölüm, "En alakalı").
pub fn slice_ranked(pool: &Pool, req: &TimelineRequest, ranked: Option<&[i64]>) -> AppResult<TimelineSlice> {
    if req.limit == 0 || req.limit > MAX_SLICE {
        return Err(AppError::Invalid(format!("dilim boyu 1–{MAX_SLICE} olmalı: {}", req.limit)));
    }
    pool.read(|c| {
        let p = plan(c, &req.view, req.arrange, ranked).map_err(to_db)?;
        let secs = sections(c, &p).map_err(to_db)?;
        let total = secs.last().map_or(0, |s| s.start + s.count);
        let items = items(c, &p, req.offset, req.limit).map_err(to_db)?;
        Ok(TimelineSlice { total, offset: req.offset, sections: secs, items })
    })
    .map_err(AppError::from)
}

fn to_db(e: AppError) -> loupe_db::DbError {
    match e {
        AppError::Db(d) => d,
        other => loupe_db::DbError::Validation(other.to_string()),
    }
}

/// Bir görünümdeki medya sayısı (kenar çubuğu sayaçları).
pub fn count(pool: &Pool, view: &View) -> AppResult<u64> {
    pool.read(|c| {
        let p = plan(c, view, Arrange::default(), None).map_err(to_db)?;
        let distinct = if matches!(view, View::People) { "DISTINCT m.id" } else { "*" };
        let n: i64 =
            c.query_row(&format!("SELECT count({distinct}) {}", p.from()), params_from_iter(p.params.iter()), |r| {
                r.get(0)
            })?;
        Ok(u64::try_from(n).unwrap_or(0))
    })
    .map_err(AppError::from)
}

/// Kenar çubuğu: akıllı albümler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartAlbumDto {
    pub id: i64,
    pub name: String,
    pub pinned: bool,
}

pub fn smart_albums(pool: &Pool) -> AppResult<Vec<SmartAlbumDto>> {
    Ok(pool.read(|c| {
        Ok(c.prepare_cached(
            "SELECT id, name, is_pinned FROM smart_albums ORDER BY is_pinned DESC, name COLLATE NOCASE",
        )?
        .query_map([], |r| Ok(SmartAlbumDto { id: r.get(0)?, name: r.get(1)?, pinned: r.get(2)? }))?
        .collect::<Result<_, _>>()?)
    })?)
}
