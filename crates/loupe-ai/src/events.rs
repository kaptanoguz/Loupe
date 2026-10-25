//! Zaman + konum tabanlı otomatik etkinlik algılama.
//!
//! **Gruplama:** medya çekim zamanına göre sıralanır. Ardışık iki fotoğraf
//! arasında `max_gap` (24 sa) boşluk varsa ya da `travel_gap` (3 sa) üstü boşlukla
//! birlikte konum `travel_km` (50 km) ötesine taşındıysa yeni etkinlik başlar.
//! Bir etkinlik `max_span` (10 gün) uzunluğu geçmez. `min_media` altındaki
//! gruplar etkinlik sayılmaz.
//!
//! **Başlık:** yerel saate göre Türkçe ay ve baskın yer: `Ekim 2026 · Göreme`,
//! ay aşımında `Eylül – Ekim 2026`, yıl aşımında `Aralık 2025 – Ocak 2026`. Yer,
//! konumlu fotoğrafların en az %60'ı aynı yerleşimdeyse yerleşim; değilse ortak
//! il, o da yoksa ortak ülkedir.
//!
//! **Kullanıcı koruması:** kilitli (`is_locked`), yeniden adlandırılmış
//! (`title_source = 'user'`), elle oluşturulmuş (`origin = 'manual'`) ya da
//! fotoğrafları elle eklenip çıkarılmış (`event_media.source <> 'auto'`)
//! etkinlik *korumalıdır*: yeniden kümeleme ona ve fotoğraflarına (çıkarılanlar
//! hariç) asla dokunmaz. Korumasız otomatik etkinlikler en çok ortak fotoğrafa
//! sahip yeni grupla eşleşip yerinde güncellenir (kimlikleri sabit kalır);
//! eşleşmeyenler silinir.

use std::collections::{HashMap, HashSet};

use loupe_core::{MediaId, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::AiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(pub i64);

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EventConfig {
    pub max_gap_ms: i64,
    pub travel_gap_ms: i64,
    pub travel_km: f64,
    pub max_span_ms: i64,
    pub min_media: usize,
    /// Başlıkta yerleşim adı için gereken en düşük pay.
    pub locality_share: f64,
}

impl Default for EventConfig {
    fn default() -> Self {
        Self {
            max_gap_ms: DAY_MS,
            travel_gap_ms: 3 * HOUR_MS,
            travel_km: 50.0,
            max_span_ms: 10 * DAY_MS,
            min_media: 3,
            locality_share: 0.6,
        }
    }
}

/// Yeniden kümelemenin özeti.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventReport {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deleted: usize,
    /// Dokunulmayan korumalı etkinlik sayısı.
    pub protected: usize,
    /// Otomatik etkinliklere dağıtılan medya sayısı.
    pub media_in_events: usize,
}

/// Kümelemeye giren medya satırı.
#[derive(Debug, Clone, Copy)]
struct Item {
    id: i64,
    utc: i64,
    tz_min: i32,
    gps: Option<(f64, f64)>,
    location: Option<i64>,
}

#[derive(Debug, Clone)]
struct Place {
    country: String,
    admin1: Option<String>,
    locality: Option<String>,
}

pub use loupe_db::haversine_km;

/// Gün sayısından (1970-01-01 = 0) takvim tarihi (H. Hinnant).
const fn civil_from_days(z: i64) -> (i64, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (y, m as u32)
}

const MONTHS_TR: [&str; 12] =
    ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"];

fn local_year_month(utc: i64, tz_min: i32) -> (i64, u32) {
    civil_from_days((utc + i64::from(tz_min) * 60_000).div_euclid(DAY_MS))
}

fn month_name(m: u32) -> &'static str {
    MONTHS_TR.get(m.saturating_sub(1) as usize).copied().unwrap_or("?")
}

/// `Ekim 2026`, `Eylül – Ekim 2026`, `Aralık 2025 – Ocak 2026`.
#[must_use]
pub fn date_title(start: (i64, u32), end: (i64, u32)) -> String {
    if start == end {
        format!("{} {}", month_name(start.1), start.0)
    } else if start.0 == end.0 {
        format!("{} – {} {}", month_name(start.1), month_name(end.1), end.0)
    } else {
        format!("{} {} – {} {}", month_name(start.1), start.0, month_name(end.1), end.0)
    }
}

/// Bir grubun hesaplanmış özeti.
#[derive(Debug, Clone, PartialEq)]
struct Summary {
    title: String,
    start: i64,
    end: i64,
    location: Option<i64>,
    centroid: Option<(f64, f64)>,
    radius_km: Option<f64>,
    cover: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EventDetector {
    pub cfg: EventConfig,
}

fn protected_filter() -> &'static str {
    "(e.is_locked = 1 OR e.title_source = 'user' OR e.origin = 'manual'
      OR EXISTS (SELECT 1 FROM event_media x WHERE x.event_id = e.id AND x.source <> 'auto'))"
}

impl EventDetector {
    #[must_use]
    pub const fn new(cfg: EventConfig) -> Self {
        Self { cfg }
    }

    fn group(&self, items: &[Item]) -> Vec<Vec<Item>> {
        let mut groups: Vec<Vec<Item>> = Vec::new();
        let mut cur: Vec<Item> = Vec::new();
        let mut last_gps: Option<(f64, f64)> = None;
        for &it in items {
            if let (Some(prev), Some(first)) = (cur.last(), cur.first()) {
                let gap = it.utc - prev.utc;
                let moved = match (last_gps, it.gps) {
                    (Some(a), Some(b)) => haversine_km(a, b) > self.cfg.travel_km,
                    _ => false,
                };
                let split = gap > self.cfg.max_gap_ms
                    || (gap > self.cfg.travel_gap_ms && moved)
                    || it.utc - first.utc > self.cfg.max_span_ms;
                if split {
                    groups.push(std::mem::take(&mut cur));
                    last_gps = None;
                }
            }
            if it.gps.is_some() {
                last_gps = it.gps;
            }
            cur.push(it);
        }
        if !cur.is_empty() {
            groups.push(cur);
        }
        groups.retain(|g| g.len() >= self.cfg.min_media);
        groups
    }

    fn summarize(&self, g: &[Item], places: &HashMap<i64, Place>) -> Option<Summary> {
        let (first, last) = (*g.first()?, *g.last()?);
        let date = date_title(local_year_month(first.utc, first.tz_min), local_year_month(last.utc, last.tz_min));

        // Baskın konum (en sık location_id).
        let mut counts: HashMap<i64, usize> = HashMap::new();
        for it in g {
            if let Some(l) = it.location {
                *counts.entry(l).or_default() += 1;
            }
        }
        let located: usize = counts.values().sum();
        let location = counts.iter().max_by_key(|&(id, n)| (*n, std::cmp::Reverse(*id))).map(|(id, _)| *id);

        let place_name = (located > 0).then(|| {
            let ps: Vec<&Place> =
                g.iter().filter_map(|it| it.location.and_then(|l| places.get(&l))).collect::<Vec<_>>();
            let mut loc_counts: HashMap<&str, usize> = HashMap::new();
            for p in &ps {
                if let Some(l) = &p.locality {
                    *loc_counts.entry(l.as_str()).or_default() += 1;
                }
            }
            #[allow(clippy::cast_precision_loss)]
            let top = loc_counts
                .iter()
                .max_by_key(|&(name, n)| (*n, std::cmp::Reverse(*name)))
                .filter(|&(_, n)| *n as f64 / ps.len().max(1) as f64 >= self.cfg.locality_share)
                .map(|(name, _)| (*name).to_owned());
            let common = |f: fn(&Place) -> Option<&str>| {
                let first = ps.first().and_then(|p| f(p))?;
                ps.iter().all(|p| f(p) == Some(first)).then(|| first.to_owned())
            };
            top.or_else(|| common(|p| p.admin1.as_deref())).or_else(|| common(|p| Some(p.country.as_str())))
        });
        let title = match place_name.flatten() {
            Some(p) => format!("{date} · {p}"),
            None => date,
        };

        let pts: Vec<(f64, f64)> = g.iter().filter_map(|it| it.gps).collect();
        #[allow(clippy::cast_precision_loss)]
        let centroid = (!pts.is_empty()).then(|| {
            let n = pts.len() as f64;
            (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n)
        });
        let radius_km = centroid.map(|c| pts.iter().map(|&p| haversine_km(c, p)).fold(0.0, f64::max));
        let cover = g.get(g.len() / 2).map_or(first.id, |it| it.id);
        Some(Summary { title, start: first.utc, end: last.utc, location, centroid, radius_km, cover })
    }

    /// Korumasız otomatik etkinlikleri yeniden üretir. Tek transaction içinde çağrılmalı.
    pub fn recluster(&self, conn: &Connection) -> Result<EventReport, AiError> {
        let mut report = EventReport::default();
        let pf = protected_filter();

        let protected: Vec<i64> = conn
            .prepare(&format!("SELECT e.id FROM events e WHERE {pf}"))?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        report.protected = protected.len();

        // Korumalı etkinliklerin fotoğrafları (çıkarılanlar hariç) havuza girmez.
        let mut items: Vec<Item> = conn
            .prepare(&format!(
                "SELECT m.id, m.taken_at_utc, m.tz_offset_min, m.gps_lat, m.gps_lon, m.location_id
                   FROM media_files m
                  WHERE m.soft_deleted_at IS NULL
                    AND NOT EXISTS (SELECT 1 FROM event_media em JOIN events e ON e.id = em.event_id
                                     WHERE em.media_id = m.id AND em.source <> 'user_excluded' AND {pf})
                  ORDER BY m.taken_at_utc, m.id"
            ))?
            .query_map([], |r| {
                let lat: Option<f64> = r.get(3)?;
                let lon: Option<f64> = r.get(4)?;
                Ok(Item {
                    id: r.get(0)?,
                    utc: r.get(1)?,
                    tz_min: r.get::<_, Option<i32>>(2)?.unwrap_or(0),
                    gps: lat.zip(lon),
                    location: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        items.sort_by_key(|it| (it.utc, it.id));

        let places: HashMap<i64, Place> = conn
            .prepare("SELECT id, country, admin1, locality FROM locations")?
            .query_map([], |r| Ok((r.get(0)?, Place { country: r.get(1)?, admin1: r.get(2)?, locality: r.get(3)? })))?
            .collect::<Result<_, _>>()?;

        let groups = self.group(&items);

        // Mevcut korumasız otomatik etkinlikler ve fotoğrafları.
        let mut old: HashMap<i64, HashSet<i64>> = HashMap::new();
        {
            let mut stmt = conn.prepare(&format!(
                "SELECT e.id, em.media_id FROM events e LEFT JOIN event_media em ON em.event_id = e.id
                  WHERE NOT {pf}"
            ))?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))?;
            for row in rows {
                let (e, m) = row?;
                let set = old.entry(e).or_default();
                if let Some(m) = m {
                    set.insert(m);
                }
            }
        }
        let mut owner: HashMap<i64, i64> = HashMap::new();
        for (&e, ms) in &old {
            for &m in ms {
                owner.insert(m, e);
            }
        }

        // En çok ortak fotoğraf: açgözlü eşleştirme (büyük örtüşme önce).
        let mut pairs: Vec<(usize, usize, i64)> = Vec::new();
        for (gi, g) in groups.iter().enumerate() {
            let mut overlap: HashMap<i64, usize> = HashMap::new();
            for it in g {
                if let Some(&e) = owner.get(&it.id) {
                    *overlap.entry(e).or_default() += 1;
                }
            }
            pairs.extend(overlap.into_iter().map(|(e, n)| (n, gi, e)));
        }
        pairs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut matched_group: HashMap<usize, i64> = HashMap::new();
        let mut used: HashSet<i64> = HashSet::new();
        for (_, gi, e) in pairs {
            if !matched_group.contains_key(&gi) && !used.contains(&e) {
                matched_group.insert(gi, e);
                used.insert(e);
            }
        }

        let params_json = serde_json::to_string(&self.cfg).map_err(|e| AiError::InvalidInput(e.to_string()))?;
        let now = now_ms();

        // Önce eşleşmeyen eski etkinlikler silinir: fotoğrafları yeni sahiplerine serbest kalır.
        for e in old.keys().filter(|e| !used.contains(e)) {
            conn.prepare_cached("DELETE FROM events WHERE id = ?1")?.execute([e])?;
            report.deleted += 1;
        }

        for (gi, g) in groups.iter().enumerate() {
            let Some(s) = self.summarize(g, &places) else { continue };
            let ids: HashSet<i64> = g.iter().map(|it| it.id).collect();
            report.media_in_events += ids.len();
            let (lat, lon) = s.centroid.unzip();
            if let Some(&e) = matched_group.get(&gi) {
                let current = old.get(&e).cloned().unwrap_or_default();
                let changed_media = current != ids;
                let row_changed: bool = conn
                    .prepare_cached(
                        "SELECT NOT (title = ?2 AND start_utc = ?3 AND end_utc = ?4 AND location_id IS ?5
                                     AND centroid_lat IS ?6 AND centroid_lon IS ?7 AND radius_km IS ?8
                                     AND cover_media_id IS ?9)
                           FROM events WHERE id = ?1",
                    )?
                    .query_row(
                        params![e, s.title, s.start, s.end, s.location, lat, lon, s.radius_km, s.cover],
                        |r| r.get(0),
                    )?;
                if !changed_media && !row_changed {
                    report.unchanged += 1;
                    continue;
                }
                for m in current.difference(&ids) {
                    conn.prepare_cached("DELETE FROM event_media WHERE event_id = ?1 AND media_id = ?2")?
                        .execute(params![e, m])?;
                }
                for m in ids.difference(&current) {
                    conn.prepare_cached("DELETE FROM event_media WHERE media_id = ?1 AND source = 'auto'")?
                        .execute([m])?;
                    conn.prepare_cached(
                        "INSERT INTO event_media (event_id, media_id, source) VALUES (?1, ?2, 'auto')",
                    )?
                    .execute(params![e, m])?;
                }
                conn.prepare_cached(
                    "UPDATE events SET title = ?2, start_utc = ?3, end_utc = ?4, location_id = ?5,
                            centroid_lat = ?6, centroid_lon = ?7, radius_km = ?8, cover_media_id = ?9,
                            cluster_params = ?10, updated_at = ?11
                      WHERE id = ?1",
                )?
                .execute(params![
                    e,
                    s.title,
                    s.start,
                    s.end,
                    s.location,
                    lat,
                    lon,
                    s.radius_km,
                    s.cover,
                    params_json,
                    now
                ])?;
                report.updated += 1;
            } else {
                let e: i64 = conn
                    .prepare_cached(
                        "INSERT INTO events (title, start_utc, end_utc, location_id, centroid_lat, centroid_lon,
                                             radius_km, cover_media_id, cluster_params, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10) RETURNING id",
                    )?
                    .query_row(
                        params![s.title, s.start, s.end, s.location, lat, lon, s.radius_km, s.cover, params_json, now],
                        |r| r.get(0),
                    )?;
                for m in &ids {
                    conn.prepare_cached("DELETE FROM event_media WHERE media_id = ?1 AND source = 'auto'")?
                        .execute([m])?;
                    conn.prepare_cached(
                        "INSERT INTO event_media (event_id, media_id, source) VALUES (?1, ?2, 'auto')",
                    )?
                    .execute(params![e, m])?;
                }
                report.created += 1;
            }
        }
        Ok(report)
    }
}

// ───────────────────────────── kullanıcı düzenlemeleri ─────────────────────────────

fn require_event(conn: &Connection, e: EventId) -> Result<(), AiError> {
    let found: Option<i64> =
        conn.prepare_cached("SELECT id FROM events WHERE id = ?1")?.query_row([e.0], |r| r.get(0)).optional()?;
    found.map(|_| ()).ok_or(AiError::NotFound { entity: "etkinlik", id: e.0 })
}

/// Etkinliği yeniden adlandırır; artık otomatik kümeleme ona dokunmaz.
pub fn rename(conn: &Connection, e: EventId, title: &str) -> Result<(), AiError> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(AiError::InvalidInput("etkinlik adı 1–200 karakter olmalı".into()));
    }
    require_event(conn, e)?;
    conn.prepare_cached("UPDATE events SET title = ?2, title_source = 'user', updated_at = ?3 WHERE id = ?1")?
        .execute(params![e.0, title, now_ms()])?;
    Ok(())
}

pub fn set_locked(conn: &Connection, e: EventId, locked: bool) -> Result<(), AiError> {
    require_event(conn, e)?;
    conn.prepare_cached("UPDATE events SET is_locked = ?2, updated_at = ?3 WHERE id = ?1")?.execute(params![
        e.0,
        locked,
        now_ms()
    ])?;
    Ok(())
}

/// Fotoğrafı etkinlikten kalıcı olarak çıkarır (yeniden kümeleme geri koymaz).
pub fn exclude_media(conn: &Connection, e: EventId, m: MediaId) -> Result<(), AiError> {
    require_event(conn, e)?;
    conn.prepare_cached(
        "INSERT INTO event_media (event_id, media_id, source) VALUES (?1, ?2, 'user_excluded')
         ON CONFLICT (event_id, media_id) DO UPDATE SET source = 'user_excluded'",
    )?
    .execute(params![e.0, m.0])?;
    refresh_bounds(conn, e)
}

/// Fotoğrafı etkinliğe elle ekler; başka otomatik etkinlikteyse oradan alınır.
pub fn add_media(conn: &Connection, e: EventId, m: MediaId) -> Result<(), AiError> {
    require_event(conn, e)?;
    conn.prepare_cached("DELETE FROM event_media WHERE media_id = ?1 AND source = 'auto' AND event_id <> ?2")?
        .execute(params![m.0, e.0])?;
    conn.prepare_cached(
        "INSERT INTO event_media (event_id, media_id, source) VALUES (?1, ?2, 'user_added')
         ON CONFLICT (event_id, media_id) DO UPDATE SET source = 'user_added'",
    )?
    .execute(params![e.0, m.0])?;
    refresh_bounds(conn, e)
}

/// Elle etkinlik oluşturur (`origin = 'manual'`, baştan korumalı).
pub fn create_manual(conn: &Connection, title: &str, media: &[MediaId]) -> Result<EventId, AiError> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(AiError::InvalidInput("etkinlik adı 1–200 karakter olmalı".into()));
    }
    let now = now_ms();
    let id: i64 = conn
        .prepare_cached(
            "INSERT INTO events (title, title_source, origin, start_utc, end_utc, created_at, updated_at)
             VALUES (?1, 'user', 'manual', ?2, ?2, ?2, ?2) RETURNING id",
        )?
        .query_row(params![title, now], |r| r.get(0))?;
    let e = EventId(id);
    for &m in media {
        add_media(conn, e, m)?;
    }
    refresh_bounds(conn, e)?;
    Ok(e)
}

/// Başlangıç/bitişi etkinlikte kalan fotoğraflara göre yeniden hesaplar (başlığa dokunmaz).
fn refresh_bounds(conn: &Connection, e: EventId) -> Result<(), AiError> {
    conn.prepare_cached(
        "UPDATE events SET
            start_utc = COALESCE((SELECT min(m.taken_at_utc) FROM event_media em JOIN media_files m ON m.id = em.media_id
                                   WHERE em.event_id = ?1 AND em.source <> 'user_excluded'), start_utc),
            end_utc   = COALESCE((SELECT max(m.taken_at_utc) FROM event_media em JOIN media_files m ON m.id = em.media_id
                                   WHERE em.event_id = ?1 AND em.source <> 'user_excluded'), end_utc),
            updated_at = ?2
          WHERE id = ?1",
    )?
    .execute(params![e.0, now_ms()])?;
    Ok(())
}

/// Bir etkinliğin (çıkarılmamış) fotoğrafları, çekim sırasıyla.
pub fn event_media(conn: &Connection, e: EventId) -> Result<Vec<MediaId>, AiError> {
    Ok(conn
        .prepare_cached(
            "SELECT em.media_id FROM event_media em JOIN media_files m ON m.id = em.media_id
              WHERE em.event_id = ?1 AND em.source <> 'user_excluded' ORDER BY m.taken_at_utc, m.id",
        )?
        .query_map([e.0], |r| Ok(MediaId(r.get(0)?)))?
        .collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turkish_titles_and_distance() {
        assert_eq!(date_title((2026, 10), (2026, 10)), "Ekim 2026");
        assert_eq!(date_title((2026, 9), (2026, 10)), "Eylül – Ekim 2026");
        assert_eq!(date_title((2025, 12), (2026, 1)), "Aralık 2025 – Ocak 2026");
        // 2026-10-24T23:30Z, UTC+3 → 25 Ekim; 2026-10-31T22:00Z, UTC+3 → 1 Kasım.
        assert_eq!(local_year_month(1_792_884_600_000, 180), (2026, 10));
        assert_eq!(local_year_month(1_793_484_000_000, 180), (2026, 11));
        let d = haversine_km((38.6431, 34.8289), (41.0082, 28.9784));
        assert!((d - 565.0).abs() < 15.0, "{d}");
    }
}
