//! Exponential backoff (`Retry-After` aware) and circuit breaker for HTTP `429` / `5xx`.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

/// Bounded retry policy for transient HTTP `429` and `5xx` responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    /// Maximum retry attempts after the initial request (`0` disables retries).
    pub max_retries: u32,
    /// Initial exponential backoff delay in milliseconds.
    pub base_delay_ms: u64,
    /// Maximum backoff cap in milliseconds (including `Retry-After` cap).
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay_ms: 50,
            max_delay_ms: 2_000,
        }
    }
}

impl RetryPolicy {
    /// Computes backoff duration for `attempt` (`0`-indexed retry), honoring `Retry-After`.
    #[must_use]
    pub fn delay_for(&self, attempt: u32, retry_after_ms: Option<u64>) -> Duration {
        let shift = attempt.min(10);
        let exp = self.base_delay_ms.saturating_mul(1u64 << shift);
        let chosen = retry_after_ms.map_or(exp, |ra| ra.max(exp));
        Duration::from_millis(chosen.min(self.max_delay_ms))
    }
}

/// Returns `true` when an HTTP status code is transient and eligible for retry / breaker counting.
#[must_use]
pub const fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500..=599)
}

/// Parses an integer-seconds `Retry-After` HTTP header value into milliseconds.
#[must_use]
pub fn parse_retry_after_ms(header_val: &str) -> Option<u64> {
    let secs: u64 = header_val.trim().parse().ok()?;
    Some(secs.saturating_mul(1_000))
}

/// Circuit breaker thresholds protecting against cascading retries to failing upstreams.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CircuitBreakerConfig {
    /// Consecutive `429`/`5xx`/transport failures required to trip the breaker open.
    pub failure_threshold: u32,
    /// Cooldown window in milliseconds before allowing a half-open probe request.
    pub cooldown_ms: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            cooldown_ms: 30_000,
        }
    }
}

#[derive(Debug)]
struct BreakerState {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

/// Thread-safe circuit breaker guarding one `HttpModelProvider` endpoint.
#[derive(Debug)]
pub struct CircuitBreaker {
    cfg: CircuitBreakerConfig,
    state: Mutex<BreakerState>,
}

impl CircuitBreaker {
    /// Creates a closed circuit breaker with the given thresholds.
    #[must_use]
    pub fn new(cfg: CircuitBreakerConfig) -> Self {
        Self {
            cfg,
            state: Mutex::new(BreakerState {
                consecutive_failures: 0,
                open_until: None,
            }),
        }
    }

    /// Returns `Err` if the circuit is currently open and cooldown has not yet elapsed.
    pub fn allow_request(&self) -> Result<(), String> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(until) = guard.open_until {
            let now = Instant::now();
            if now < until {
                let rem = until.saturating_duration_since(now).as_millis();
                return Err(format!("circuit breaker open ({rem}ms remaining)"));
            }
            // Cooldown elapsed: transition to half-open probe.
            guard.open_until = None;
        }
        Ok(())
    }

    /// Resets consecutive failure counts on a successful upstream response.
    pub fn record_success(&self) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.consecutive_failures = 0;
        guard.open_until = None;
    }

    /// Records a transient failure (`429`, `5xx`, or transport error), tripping open if threshold met.
    pub fn record_transient_failure(&self) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.consecutive_failures = guard.consecutive_failures.saturating_add(1);
        if self.cfg.failure_threshold > 0
            && guard.consecutive_failures >= self.cfg.failure_threshold
        {
            guard.open_until = Some(Instant::now() + Duration::from_millis(self.cfg.cooldown_ms));
        }
    }
}
