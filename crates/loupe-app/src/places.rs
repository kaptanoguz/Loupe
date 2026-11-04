//! Yerler haritası: fotoğrafı olan her yer için bir işaret.
//!
//! Harita arayüzde çizilir; burada yalnızca konum, ad ve fotoğraf sayısı
//! okunur. Kapak en yeni fotoğraftır, işaretin üzerine gelince gösterilir.

use loupe_db::Pool;
use serde::Serialize;

use crate::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacePin {
    pub id: i64,
    pub lat: f64,
    pub lon: f64,
    pub title: String,
    pub subtitle: Option<String>,
    pub count: u64,
    /// En yeni fotoğrafın BLAKE3 hex'i (küçük resim için).
    pub cover: Option<String>,
}

/// Fotoğraf sayısına göre azalan; kalabalık yerler üstte çizilir diye
/// arayüz bu sırayı tersine çevirir.
pub fn pins(pool: &Pool) -> AppResult<Vec<PlacePin>> {
    Ok(pool.read(|c| {
        let mut stmt = c.prepare(
            "SELECT l.id, l.lat, l.lon,
                    COALESCE(l.user_label, l.locality, l.admin2, l.admin1, l.country),
                    COALESCE(l.admin1 || ', ', '') || l.country,
                    count(*),
                    (SELECT m2.blake3 FROM media_files m2
                      WHERE m2.location_id = l.id AND m2.soft_deleted_at IS NULL
                      ORDER BY m2.taken_at_utc DESC, m2.id DESC LIMIT 1)
               FROM media_files m JOIN locations l ON l.id = m.location_id
              WHERE m.soft_deleted_at IS NULL
              GROUP BY l.id
              ORDER BY count(*) DESC, l.id",
        )?;
        let rows = stmt.query_map([], |r| {
            let cover: Option<Vec<u8>> = r.get(6)?;
            Ok(PlacePin {
                id: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
                title: r.get(3)?,
                subtitle: r.get(4)?,
                count: u64::try_from(r.get::<_, i64>(5)?).unwrap_or(0),
                cover: cover.map(|b| b.iter().map(|x| format!("{x:02x}")).collect()),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    })?)
}
