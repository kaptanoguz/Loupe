use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Şu an, UTC epoch milisaniye (şemadaki tüm zaman damgalarının biçimi).
#[must_use]
pub fn now_ms() -> i64 {
    system_time_ms(SystemTime::now())
}

/// `SystemTime` → epoch ms. 1970 öncesi negatif döner, taşma doygunlaşır.
#[must_use]
pub fn system_time_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => duration_ms(d),
        Err(e) => -duration_ms(e.duration()),
    }
}

/// `SystemTime` → epoch ns (`media_files.mtime_ns`).
#[must_use]
pub fn system_time_ns(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
    }
}

fn duration_ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}
