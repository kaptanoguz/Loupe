//! Etkinlikler görünümü: zaman + konum kümeleri ("Ekim 2026 · Göreme").
//!
//! Kullanıcının dokunduğu her etkinlik kilitlenir (`is_locked = 1`): yeniden
//! adlandırma ve fotoğraf çıkarma otomatik kümelemenin onu bir daha
//! değiştirmemesi demektir. Çıkarılan fotoğraf `user_excluded` olarak kalır.

use loupe_ai::EventId;
use loupe_ai::jobs::write_tx;
use loupe_core::MediaId;
use loupe_db::Pool;
use serde::Serialize;

use crate::timeline::event_subtitle;
use crate::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCard {
    pub id: i64,
    pub title: String,
    /// `12–15 Ekim 2026`
    pub range: String,
    pub start_utc: i64,
    pub end_utc: i64,
    pub count: u64,
    /// Kapak: `thumb://md/{hash}`.
    pub cover_hash: Option<String>,
    pub cover_aspect: f32,
    pub place: Option<String>,
    pub locked: bool,
    /// `auto` | `manual`
    pub origin: String,
}

const CARD_SQL: &str = "SELECT e.id, e.title, e.start_utc, e.end_utc, e.is_locked, e.origin,
       (SELECT count(*) FROM event_media em JOIN media_files m ON m.id = em.media_id
         WHERE em.event_id = e.id AND em.source <> 'user_excluded' AND m.soft_deleted_at IS NULL),
       (SELECT lower(hex(m.blake3)) FROM media_files m WHERE m.id = COALESCE(e.cover_media_id,
          (SELECT em.media_id FROM event_media em JOIN media_files m2 ON m2.id = em.media_id
            WHERE em.event_id = e.id AND em.source <> 'user_excluded' AND m2.soft_deleted_at IS NULL
            ORDER BY m2.rating DESC, m2.is_favorite DESC, m2.taken_at_utc LIMIT 1))),
       COALESCE(l.user_label, l.locality, l.admin2, l.admin1, l.country)
  FROM events e LEFT JOIN locations l ON l.id = e.location_id";

fn card(r: &rusqlite::Row<'_>) -> rusqlite::Result<EventCard> {
    let (start, end): (i64, i64) = (r.get(2)?, r.get(3)?);
    Ok(EventCard {
        id: r.get(0)?,
        title: r.get(1)?,
        range: event_subtitle(&format!("{start}:{end}")).unwrap_or_default(),
        start_utc: start,
        end_utc: end,
        locked: r.get(4)?,
        origin: r.get(5)?,
        count: u64::try_from(r.get::<_, i64>(6)?).unwrap_or(0),
        cover_hash: r.get(7)?,
        cover_aspect: 1.5,
        place: r.get(8)?,
    })
}

/// Etkinlikler, en yeniden eskiye (boş olanlar gizli).
pub fn list(pool: &Pool) -> AppResult<Vec<EventCard>> {
    let sql = format!("{CARD_SQL} ORDER BY e.start_utc DESC, e.id DESC");
    let cards: Vec<EventCard> =
        pool.read(|c| Ok(c.prepare_cached(&sql)?.query_map([], card)?.collect::<Result<_, _>>()?))?;
    Ok(cards.into_iter().filter(|c| c.count > 0).collect())
}

pub fn get(pool: &Pool, id: i64) -> AppResult<EventCard> {
    use rusqlite::OptionalExtension;
    let sql = format!("{CARD_SQL} WHERE e.id = ?1");
    pool.read(|c| Ok(c.prepare_cached(&sql)?.query_row([id], card).optional()?))?
        .ok_or(AppError::NotFound { entity: "etkinlik", id })
}

fn refresh_count(c: &rusqlite::Connection, id: i64) -> Result<(), loupe_ai::AiError> {
    c.execute(
        "UPDATE events SET media_count = (SELECT count(*) FROM event_media em
           WHERE em.event_id = events.id AND em.source <> 'user_excluded') WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// Yeniden adlandırır ve kilitler.
pub fn rename(pool: &Pool, id: i64, title: &str) -> AppResult<EventCard> {
    write_tx(pool, |c| {
        loupe_ai::events::rename(c, EventId(id), title)?;
        loupe_ai::events::set_locked(c, EventId(id), true)
    })?;
    get(pool, id)
}

/// Fotoğrafları etkinlikten kalıcı olarak çıkarır ve etkinliği kilitler.
pub fn exclude(pool: &Pool, id: i64, media: &[i64]) -> AppResult<EventCard> {
    if media.is_empty() || media.len() > 10_000 {
        return Err(AppError::Invalid("1–10000 fotoğraf seçilmeli".into()));
    }
    write_tx(pool, |c| {
        for &m in media {
            loupe_ai::events::exclude_media(c, EventId(id), MediaId(m))?;
        }
        loupe_ai::events::set_locked(c, EventId(id), true)?;
        refresh_count(c, id)
    })?;
    get(pool, id)
}

/// Kilidi kaldırır: bir sonraki kümeleme etkinliği yeniden düzenleyebilir
/// (çıkarılan fotoğraflar yine dışarıda kalır).
pub fn set_locked(pool: &Pool, id: i64, locked: bool) -> AppResult<EventCard> {
    write_tx(pool, |c| loupe_ai::events::set_locked(c, EventId(id), locked))?;
    get(pool, id)
}

/// Seçili fotoğraflardan elle etkinlik.
pub fn create(pool: &Pool, title: &str, media: &[i64]) -> AppResult<EventCard> {
    let ids: Vec<MediaId> = media.iter().copied().map(MediaId).collect();
    let e = write_tx(pool, |c| {
        let e = loupe_ai::events::create_manual(c, title, &ids)?;
        refresh_count(c, e.0)?;
        Ok(e)
    })?;
    get(pool, e.0)
}
