//! Server clock.
//!
//! While the process is alive, time advances monotonically from a wall-clock
//! anchor captured at startup, so NTP steps cannot make a clock jump
//! backwards or forwards mid-game. After a restart the persisted wall-clock
//! reference (`turn_started_at`) takes over again.

use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;

/// Time source used by every timestamp-sensitive decision (move receipt,
/// expiry races, session expiry).
pub trait ServerClock: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// Monotonic-within-process clock anchored to wall time at startup.
#[derive(Debug)]
pub struct ProcessClock {
    wall_base_ms: i64,
    mono_base: Instant,
}

impl ProcessClock {
    pub fn new() -> Self {
        Self {
            wall_base_ms: Utc::now().timestamp_millis(),
            mono_base: Instant::now(),
        }
    }
}

impl Default for ProcessClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerClock for ProcessClock {
    fn now_ms(&self) -> i64 {
        self.wall_base_ms + self.mono_base.elapsed().as_millis() as i64
    }
}

pub type SharedClock = Arc<dyn ServerClock>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances_monotonically() {
        let clock = ProcessClock::new();
        let a = clock.now_ms();
        let b = clock.now_ms();
        assert!(b >= a);
        assert!(a > 0);
    }
}
