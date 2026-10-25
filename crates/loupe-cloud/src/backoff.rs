//! `run_after = şimdi + min(6 sa, 2^deneme × 30 sn) × rastgele(0.5–1.0)`
//! (RFC-001B §3.4). Tam rastgele titreşim, aynı anda düşen işlerin aynı anda
//! yeniden denemesini (fırtına) önler. 12 denemeden sonra iş `dead` olur.

use std::time::Duration;

use rand::Rng;

pub const BASE: Duration = Duration::from_secs(30);
pub const MAX: Duration = Duration::from_secs(6 * 3600);
pub const MAX_ATTEMPTS: u32 = 12;

/// `attempt` 1'den başlar (ilk başarısız denemeden sonra 1).
#[must_use]
pub fn delay(attempt: u32, jitter: f64) -> Duration {
    let exp = BASE.saturating_mul(1u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX));
    exp.min(MAX).mul_f64(jitter.clamp(0.5, 1.0))
}

/// [0.5, 1.0] aralığında titreşim.
#[must_use]
pub fn jitter() -> f64 {
    rand::rng().random_range(0.5..=1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_exponentially_is_capped_and_jittered() {
        assert_eq!(delay(1, 1.0), Duration::from_secs(60));
        assert_eq!(delay(2, 1.0), Duration::from_secs(120));
        assert_eq!(delay(3, 0.5), Duration::from_secs(120));
        assert_eq!(delay(10, 1.0), MAX, "2^10 × 30 sn > 6 sa");
        assert_eq!(delay(40, 1.0), MAX);
        assert_eq!(delay(1, 0.1), Duration::from_secs(30), "titreşim 0.5'in altına inmez");
        for _ in 0..1000 {
            let j = jitter();
            assert!((0.5..=1.0).contains(&j));
            let d = delay(4, j);
            assert!(d >= Duration::from_secs(240) && d <= Duration::from_secs(480));
        }
    }
}
