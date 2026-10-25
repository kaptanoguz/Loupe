//! `locations`: çevrimdışı ters coğrafi kodlama sonuçları (GeoNames yerleşimi başına bir satır).

use loupe_core::{MediaId, now_ms};
use rusqlite::{Connection, OptionalExtension, params};

use crate::DbError;

/// Çözücünün bulduğu yer. `geoname_id` aynı olan kayıtlar tek satırda toplanır.
#[derive(Debug, Clone, PartialEq)]
pub struct NewLocation {
    pub geoname_id: Option<i64>,
    pub country_code: String,
    pub country: String,
    pub admin1: Option<String>,
    pub admin2: Option<String>,
    pub locality: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub dataset_ver: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocationId(pub i64);

/// Yerleşimi ekler veya mevcut satırın kimliğini döndürür.
pub fn upsert(conn: &Connection, loc: &NewLocation) -> Result<LocationId, DbError> {
    if let Some(gid) = loc.geoname_id {
        let existing: Option<i64> = conn
            .prepare_cached("SELECT id FROM locations WHERE geoname_id = ?1")?
            .query_row([gid], |r| r.get(0))
            .optional()?;
        if let Some(id) = existing {
            return Ok(LocationId(id));
        }
    }
    let id = conn.prepare_cached(
        "INSERT INTO locations (geoname_id, country_code, country, admin1, admin2, locality, lat, lon, dataset_ver, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id",
    )?
    .query_row(
        params![
            loc.geoname_id,
            loc.country_code,
            loc.country,
            loc.admin1,
            loc.admin2,
            loc.locality,
            loc.lat,
            loc.lon,
            loc.dataset_ver,
            now_ms()
        ],
        |r| r.get(0),
    )?;
    Ok(LocationId(id))
}

pub fn get(conn: &Connection, id: LocationId) -> Result<Option<NewLocation>, DbError> {
    Ok(conn
        .prepare_cached(
            "SELECT geoname_id, country_code, country, admin1, admin2, locality, lat, lon, dataset_ver
               FROM locations WHERE id = ?1",
        )?
        .query_row([id.0], |r| {
            Ok(NewLocation {
                geoname_id: r.get(0)?,
                country_code: r.get(1)?,
                country: r.get(2)?,
                admin1: r.get(3)?,
                admin2: r.get(4)?,
                locality: r.get(5)?,
                lat: r.get(6)?,
                lon: r.get(7)?,
                dataset_ver: r.get(8)?,
            })
        })
        .optional()?)
}

/// Medyayı yere bağlar (çözücü işi `geocode` sonucu).
pub fn assign_to_media(conn: &Connection, media: MediaId, loc: LocationId) -> Result<(), DbError> {
    let n = conn.execute(
        "UPDATE media_files SET location_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![media.0, loc.0, now_ms()],
    )?;
    if n == 0 { Err(DbError::NotFound { entity: "medya", id: media.0 }) } else { Ok(()) }
}
