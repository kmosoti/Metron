//! The clock port.
//!
//! Wall-clock readings are informational. The core defines only the port and
//! a manual clock for deterministic tests; the system clock lives in
//! `metron-adapters`, because reading real time is an effect.

use std::sync::atomic::{AtomicU64, Ordering};

/// Source of wall-clock timestamps.
pub trait Clock: Send + Sync {
    /// Nanoseconds since the Unix epoch.
    fn now_nanos(&self) -> u64;
}

/// A clock that only moves when told to.
#[derive(Debug, Default)]
pub struct ManualClock {
    nanos: AtomicU64,
}

impl ManualClock {
    /// Creates a clock reading `nanos`.
    #[must_use]
    pub fn new(nanos: u64) -> Self {
        Self {
            nanos: AtomicU64::new(nanos),
        }
    }

    /// Advances the clock by `delta` nanoseconds.
    pub fn advance(&self, delta: u64) {
        self.nanos.fetch_add(delta, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_nanos(&self) -> u64 {
        self.nanos.load(Ordering::SeqCst)
    }
}

impl<C: Clock + ?Sized> Clock for Box<C> {
    fn now_nanos(&self) -> u64 {
        (**self).now_nanos()
    }
}
