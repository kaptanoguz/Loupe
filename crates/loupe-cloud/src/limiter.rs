//! Bayt/sn token bucket (RFC-001B §3.4).
//!
//! Hedef başına (`cloud_targets.bandwidth_bps`) ve genel
//! (`cloud.global_bandwidth_bps`) iki kova vardır; bir parça ikisinden de izin
//! almadan gönderilmez. Kova boşsa eksik jeton dolana kadar beklenir; büyük
//! parçalar kova boyunu aşmayacak dilimlerle alınır.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Debug)]
pub struct TokenBucket {
    rate: f64,
    burst: f64,
    state: Mutex<Bucket>,
}

impl TokenBucket {
    /// `bytes_per_sec` > 0; `burst` kovanın alabileceği en fazla bayt (en az 1).
    #[must_use]
    pub fn new(bytes_per_sec: u64, burst: u64) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let (rate, burst) = (bytes_per_sec.max(1) as f64, burst.max(1) as f64);
        Self { rate, burst, state: Mutex::new(Bucket { tokens: burst, last: Instant::now() }) }
    }

    /// Saniyede `bytes_per_sec`, kova bir saniyelik (en az 64 KiB).
    #[must_use]
    pub fn per_second(bytes_per_sec: u64) -> Self {
        Self::new(bytes_per_sec, bytes_per_sec.max(64 * 1024))
    }

    /// `n` jeton alınabildiyse `None`, alınamadıysa beklenecek süre.
    fn try_take(&self, n: f64) -> Option<Duration> {
        let Ok(mut b) = self.state.lock() else { return None };
        let now = Instant::now();
        let dt = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + dt * self.rate).min(self.burst);
        b.last = now;
        if b.tokens >= n {
            b.tokens -= n;
            None
        } else {
            Some(Duration::from_secs_f64((n - b.tokens) / self.rate))
        }
    }

    pub async fn acquire(&self, bytes: u64) {
        #[allow(clippy::cast_precision_loss)]
        let mut left = bytes as f64;
        while left > 0.0 {
            let piece = left.min(self.burst);
            match self.try_take(piece) {
                None => left -= piece,
                Some(wait) => tokio::time::sleep(wait).await,
            }
        }
    }
}

/// Bir yüklemenin geçtiği kovalar (genel + hedef). Boşsa sınırsız.
#[derive(Debug, Clone, Default)]
pub struct Limiter {
    buckets: Vec<Arc<TokenBucket>>,
}

impl Limiter {
    #[must_use]
    pub const fn unlimited() -> Self {
        Self { buckets: Vec::new() }
    }

    #[must_use]
    pub fn with(mut self, bucket: Arc<TokenBucket>) -> Self {
        self.buckets.push(bucket);
        self
    }

    pub async fn acquire(&self, bytes: u64) {
        for b in &self.buckets {
            b.acquire(bytes).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn bucket_enforces_rate_after_burst() {
        let b = TokenBucket::new(1_000_000, 250_000);
        let t0 = Instant::now();
        b.acquire(250_000).await;
        assert_eq!(t0.elapsed(), Duration::ZERO, "kova doluyken bekleme yok");
        b.acquire(1_000_000).await;
        let e = t0.elapsed();
        assert!(e >= Duration::from_millis(999) && e <= Duration::from_millis(1_010), "{e:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn limiter_honours_the_slowest_bucket() {
        let global = Arc::new(TokenBucket::new(2_000_000, 100_000));
        let target = Arc::new(TokenBucket::new(500_000, 100_000));
        let l = Limiter::unlimited().with(global).with(target);
        let t0 = Instant::now();
        for _ in 0..11 {
            l.acquire(100_000).await;
        }
        // İlk 100 KB kovadan, kalan 1 MB hedefin 500 KB/sn hızıyla: ~2 sn.
        let e = t0.elapsed();
        assert!(e >= Duration::from_millis(1_990) && e <= Duration::from_millis(2_100), "{e:?}");
        let free = Limiter::unlimited();
        let t1 = Instant::now();
        free.acquire(u64::MAX).await;
        assert_eq!(t1.elapsed(), Duration::ZERO);
    }
}
