//! Canlı aktarım durumu (RFC-001B §2.7 durum çubuğu).
//!
//! Hız son ~5 sn'nin üstel hareketli ortalamasıdır (titremez); ETA kalan bayt
//! bölü hız. "Doğrulandı" yalnızca tam dosya BLAKE3 kontrolünden geçip
//! kataloğa giren dosyaları sayar. Arayüz [`Telemetry::subscribe`] ile
//! `tokio::sync::watch` kanalından okur; her değişiklik yeni bir anlık görüntüdür.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::watch;

/// EMA zaman sabiti.
pub const RATE_WINDOW: Duration = Duration::from_secs(5);
/// Bu aralıktan sık gelen baytlar biriktirilip tek örnek sayılır.
const MIN_SAMPLE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct SyncStatus {
    pub active_uploads: u32,
    /// Açılan oturumların toplam boyutu (devam eden oturumlar için kalan kısım).
    pub bytes_expected: u64,
    pub bytes_received: u64,
    pub bytes_per_sec: f64,
    pub eta_secs: Option<u64>,
    pub verified_files: u64,
    pub already_synced: u64,
    pub skipped: u64,
    pub conflicts: u64,
    pub failed: u64,
}

#[derive(Debug, Default)]
struct Rate {
    last: Option<Instant>,
    pending: u64,
    ema: f64,
    primed: bool,
}

#[derive(Debug)]
pub struct Telemetry {
    tx: watch::Sender<SyncStatus>,
    rate: Mutex<Rate>,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl Telemetry {
    #[must_use]
    pub fn new() -> Self {
        Self { tx: watch::Sender::new(SyncStatus::default()), rate: Mutex::new(Rate::default()) }
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<SyncStatus> {
        self.tx.subscribe()
    }

    #[must_use]
    pub fn snapshot(&self) -> SyncStatus {
        *self.tx.borrow()
    }

    fn update(&self, f: impl FnOnce(&mut SyncStatus)) {
        self.tx.send_modify(|s| {
            f(s);
            let remaining = s.bytes_expected.saturating_sub(s.bytes_received);
            s.eta_secs = if remaining == 0 {
                Some(0)
            } else if s.bytes_per_sec > 0.0 {
                #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some((remaining as f64 / s.bytes_per_sec).ceil() as u64)
            } else {
                None
            };
        });
    }

    /// Yeni ya da devam eden oturum: `remaining` bayt beklenir.
    pub fn upload_started(&self, remaining: u64) {
        self.update(|s| {
            s.active_uploads += 1;
            s.bytes_expected += remaining;
        });
    }

    /// Gelen bayt. `at` testlerde verilir, sunucu `Instant::now()` kullanır.
    pub fn bytes_at(&self, n: u64, at: Instant) {
        let rate = self.rate.lock().ok().and_then(|mut r| {
            r.pending += n;
            let Some(last) = r.last else {
                r.last = Some(at);
                return None;
            };
            let dt = at.saturating_duration_since(last);
            if dt < MIN_SAMPLE {
                return None;
            }
            #[allow(clippy::cast_precision_loss)]
            let inst = r.pending as f64 / dt.as_secs_f64();
            let alpha = 1.0 - (-dt.as_secs_f64() / RATE_WINDOW.as_secs_f64()).exp();
            r.ema = if r.primed { r.ema + alpha * (inst - r.ema) } else { inst };
            r.primed = true;
            r.pending = 0;
            r.last = Some(at);
            Some(r.ema)
        });
        self.update(|s| {
            s.bytes_received += n;
            if let Some(r) = rate {
                s.bytes_per_sec = r;
            }
        });
    }

    pub fn bytes(&self, n: u64) {
        self.bytes_at(n, Instant::now());
    }

    /// Bağlantı koptu ya da parça reddedildi: alınan ama kalıcı olmayan baytlar geri alınır.
    pub fn bytes_discarded(&self, n: u64) {
        self.update(|s| s.bytes_received = s.bytes_received.saturating_sub(n));
    }

    /// Oturumun bu bağlantıdaki payı bitti (tamamlanmadan; telefon sonra devam eder).
    pub fn upload_paused(&self, remaining: u64) {
        self.update(|s| {
            s.active_uploads = s.active_uploads.saturating_sub(1);
            s.bytes_expected = s.bytes_expected.saturating_sub(remaining);
        });
    }

    pub fn verified(&self) {
        self.update(|s| {
            s.active_uploads = s.active_uploads.saturating_sub(1);
            s.verified_files += 1;
        });
    }

    pub fn failed(&self, remaining: u64) {
        self.update(|s| {
            s.active_uploads = s.active_uploads.saturating_sub(1);
            s.bytes_expected = s.bytes_expected.saturating_sub(remaining);
            s.failed += 1;
        });
    }

    pub fn already_synced(&self) {
        self.update(|s| s.already_synced += 1);
    }

    pub fn skipped(&self) {
        self.update(|s| s.skipped += 1);
    }

    pub fn conflict(&self) {
        self.update(|s| s.conflicts += 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_smoothed_and_eta_follows_remaining_bytes() {
        let t = Telemetry::new();
        let rx = t.subscribe();
        t.upload_started(100 * 1024 * 1024);
        let t0 = Instant::now();
        t.bytes_at(0, t0);
        // 10 MB/s sabit akış.
        for i in 1..=50 {
            t.bytes_at(1_000_000, t0 + Duration::from_millis(100 * i));
        }
        let s = *rx.borrow();
        assert_eq!(s.bytes_received, 50_000_000);
        assert!((s.bytes_per_sec - 10_000_000.0).abs() < 1.0, "{}", s.bytes_per_sec);
        let remaining = 100 * 1024 * 1024 - 50_000_000;
        assert_eq!(s.eta_secs, Some(u64::try_from(remaining).unwrap().div_ceil(10_000_000)));

        // Kısa bir duraklama hızı bir anda sıfırlamaz.
        t.bytes_at(1_000_000, t0 + Duration::from_millis(6_000));
        let s = *rx.borrow();
        assert!(s.bytes_per_sec > 5_000_000.0 && s.bytes_per_sec < 10_000_000.0, "{}", s.bytes_per_sec);

        t.verified();
        t.already_synced();
        t.conflict();
        let s = t.snapshot();
        assert_eq!((s.active_uploads, s.verified_files, s.already_synced, s.conflicts), (0, 1, 1, 1));
    }
}
