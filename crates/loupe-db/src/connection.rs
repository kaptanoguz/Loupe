//! Bağlantı açma ve bağlantı başına PRAGMA'lar (RFC-001A §2.2).
//!
//! Migrasyon dosyaları PRAGMA içermez (user_version hariç); bağlantı ayarları
//! yalnızca burada uygulanır, böylece her bağlantı aynı davranır.

use std::path::Path;
use std::time::Duration;

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OpenFlags};

use crate::{DbError, vec_ext};

/// Bağlantının rolü. WAL'da tek yazar vardır; okuyucular salt okunur açılır.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Writer,
    Reader,
}

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub busy_timeout: Duration,
    pub mmap_size: u64,
    /// Negatif: KiB cinsinden (SQLite kuralı). -65536 = 64 MiB.
    pub cache_size: i64,
    pub journal_size_limit: i64,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            busy_timeout: Duration::from_millis(5000),
            mmap_size: 256 * 1024 * 1024,
            cache_size: -65_536,
            journal_size_limit: 64 * 1024 * 1024,
        }
    }
}

pub fn open_connection(path: &Path, role: Role, cfg: &ConnectionConfig) -> Result<Connection, DbError> {
    vec_ext::ensure_registered()?;
    let base = OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
    let flags = match role {
        Role::Writer => base | OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        Role::Reader => base | OpenFlags::SQLITE_OPEN_READ_ONLY,
    };
    let conn = Connection::open_with_flags(path, flags)?;
    apply_pragmas(&conn, role, cfg)?;
    register_functions(&conn)?;
    Ok(conn)
}

fn apply_pragmas(conn: &Connection, role: Role, cfg: &ConnectionConfig) -> Result<(), DbError> {
    conn.busy_timeout(cfg.busy_timeout)?;
    if role == Role::Writer {
        let mode: String = conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(DbError::Validation(format!("journal_mode WAL olmadı: {mode}")));
        }
        conn.pragma_update(None, "journal_size_limit", cfg.journal_size_limit)?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "mmap_size", i64::try_from(cfg.mmap_size).unwrap_or(i64::MAX))?;
    conn.pragma_update(None, "cache_size", cfg.cache_size)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    if role == Role::Reader {
        conn.pragma_update(None, "query_only", "ON")?;
    }
    conn.set_prepared_statement_cache_capacity(64);
    Ok(())
}

/// Türkçe kurallarıyla küçük harf (I → ı, İ → i); SQLite `lower()` yalnızca ASCII çevirir.
#[must_use]
pub fn turkish_lowercase(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            'I' => vec!['ı'],
            'İ' => vec!['i'],
            other => other.to_lowercase().collect(),
        })
        .collect()
}

/// İki koordinat arasındaki büyük çember uzaklığı (km).
#[must_use]
pub fn haversine_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6371.0088 * h.sqrt().min(1.0).asin()
}

/// SQL fonksiyonları (her bağlantıda):
/// - `hamming(a, b)`: iki pHash arasındaki farklı bit sayısı. Şema bu fonksiyona
///   bağlı değildir (indeksler bant ifadeleriyle kurulur); yakın kopya sorgusu kullanır.
/// - `tr_lower(s)`: Türkçe küçük harf (arama).
/// - `haversine_km(lat1, lon1, lat2, lon2)`: yarıçap süzgeci (arama).
fn register_functions(conn: &Connection) -> Result<(), DbError> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_INNOCUOUS;
    conn.create_scalar_function("tr_lower", 1, flags, |ctx| {
        let s: Option<String> = ctx.get(0)?;
        Ok(s.map(|s| turkish_lowercase(&s)))
    })?;
    conn.create_scalar_function("haversine_km", 4, flags, |ctx| {
        let v: [Option<f64>; 4] = [ctx.get(0)?, ctx.get(1)?, ctx.get(2)?, ctx.get(3)?];
        Ok(match v {
            [Some(a), Some(b), Some(c), Some(d)] => Some(haversine_km((a, b), (c, d))),
            _ => None,
        })
    })?;
    conn.create_scalar_function(
        "hamming",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let a: Option<i64> = ctx.get(0)?;
            let b: Option<i64> = ctx.get(1)?;
            Ok(match (a, b) {
                (Some(a), Some(b)) => Some(i64::from((a ^ b).count_ones())),
                _ => None,
            })
        },
    )?;
    Ok(())
}
