//! Zaman kaynağı. Belirteç süresi ve oturum ömrü testlerde elle ilerletilir.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Unix zamanı, milisaniye.
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        loupe_core::now_ms()
    }
}

/// Elle kurulan saat.
#[derive(Debug)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    #[must_use]
    pub const fn new(now_ms: i64) -> Self {
        Self(AtomicI64::new(now_ms))
    }

    pub fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }

    pub fn advance(&self, by: Duration) {
        let ms = i64::try_from(by.as_millis()).unwrap_or(i64::MAX);
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
