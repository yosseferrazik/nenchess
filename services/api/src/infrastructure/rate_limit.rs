//! In-memory fixed-window rate limiter.
//!
//! Deliberately hand-rolled and per-process: it is a *soft* protection layer
//! (credential-stuffing on `/auth`, command spam on a WebSocket), not a
//! billing-grade quota. A deployment that needs cluster-wide limits should
//! put them at the proxy (nginx `limit_req`) or swap this module for a shared
//! store — the call sites only depend on `check`/`check_at`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Bucket {
    window_start: Instant,
    count: u32,
}

/// Accepts up to `limit` events per key per `window`. A limit of `0`
/// disables the limiter entirely (every check succeeds without tracking).
#[derive(Debug)]
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Count `key` against the current window.
    ///
    /// Returns `Err(retry_after_secs)` when the budget is exhausted.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        self.check_at(key, Instant::now())
    }

    fn check_at(&self, key: &str, now: Instant) -> Result<(), u64> {
        if self.limit == 0 {
            return Ok(());
        }
        let mut buckets = self.buckets.lock().expect("rate limiter lock poisoned");
        // Amortized cleanup: once the map grows past a bound, drop windows
        // that can no longer reject anything.
        if buckets.len() > 10_000 {
            let window = self.window;
            buckets.retain(|_, bucket| now.duration_since(bucket.window_start) < window);
        }
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= self.window {
            bucket.window_start = now;
            bucket.count = 0;
        }
        if bucket.count >= self.limit {
            let elapsed = now.duration_since(bucket.window_start).as_secs();
            return Err(self.window.as_secs().saturating_sub(elapsed).max(1));
        }
        bucket.count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(limit: u32) -> RateLimiter {
        RateLimiter::new(limit, Duration::from_secs(60))
    }

    #[test]
    fn allows_up_to_the_limit_then_rejects() {
        let limiter = limiter(3);
        assert!(limiter.check("ip").is_ok());
        assert!(limiter.check("ip").is_ok());
        assert!(limiter.check("ip").is_ok());
        let retry = limiter.check("ip").expect_err("budget exhausted");
        assert!((1..=60).contains(&retry));
    }

    #[test]
    fn keys_are_isolated() {
        let limiter = limiter(1);
        assert!(limiter.check("a").is_ok());
        assert!(limiter.check("b").is_ok());
        assert!(limiter.check("a").is_err());
        assert!(limiter.check("b").is_err());
    }

    #[test]
    fn zero_disables_the_limiter() {
        let limiter = limiter(0);
        for _ in 0..1_000 {
            assert!(limiter.check("ip").is_ok());
        }
    }

    #[test]
    fn window_reset_restores_the_budget() {
        let limiter = limiter(1);
        let start = Instant::now();
        assert!(limiter.check_at("ip", start).is_ok());
        assert!(limiter
            .check_at("ip", start + Duration::from_secs(59))
            .is_err());
        // Just past the window: fresh budget.
        assert!(limiter
            .check_at("ip", start + Duration::from_secs(60))
            .is_ok());
    }
}
