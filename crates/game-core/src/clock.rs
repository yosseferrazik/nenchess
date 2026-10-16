use serde::{Deserialize, Serialize};

use crate::types::Color;

/// Server-side game clock.
///
/// The server never trusts a client clock. It persists the remaining time for
/// each side, the increment, the side to move and the instant the current turn
/// started; elapsed time is derived from the server clock only.
///
/// All timestamps are epoch milliseconds on the server clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    /// Remaining time for white at the start of the current turn (ms).
    pub white_ms: i64,
    /// Remaining time for black at the start of the current turn (ms).
    pub black_ms: i64,
    /// Increment added to the mover after each move (ms).
    pub increment_ms: i64,
    /// Side whose clock is running.
    pub turn: Color,
    /// Instant the current turn started; the running clock counts from here.
    pub turn_started_ms: i64,
}

impl Clock {
    pub fn new(initial_ms: i64, increment_ms: i64, turn: Color, now_ms: i64) -> Self {
        Self {
            white_ms: initial_ms,
            black_ms: initial_ms,
            increment_ms,
            turn,
            turn_started_ms: now_ms,
        }
    }

    /// Remaining time at the start of the current turn for a colour.
    pub fn rest_ms(&self, color: Color) -> i64 {
        match color {
            Color::White => self.white_ms,
            Color::Black => self.black_ms,
        }
    }

    /// Milliseconds consumed by the running clock at `now_ms`.
    pub fn elapsed_ms(&self, now_ms: i64) -> i64 {
        (now_ms - self.turn_started_ms).max(0)
    }

    /// Instant (epoch ms) at which the running side's flag falls.
    ///
    /// Deterministic policy: the flag falls when `now >= turn_started + rest`.
    /// A move is legal only if the server receives it strictly before that
    /// instant, so a move racing with the expiry is resolved by the single
    /// server clock timestamp taken when the command is processed.
    pub fn flag_falls_at_ms(&self) -> i64 {
        self.turn_started_ms + self.rest_ms(self.turn)
    }

    /// Whether the running side's flag has already fallen at `now_ms`.
    pub fn flag_fallen(&self, now_ms: i64) -> bool {
        now_ms >= self.flag_falls_at_ms()
    }

    /// Remaining time for `color` at `now_ms` (accounts for the running clock).
    pub fn remaining_ms_at(&self, color: Color, now_ms: i64) -> i64 {
        if color == self.turn {
            (self.rest_ms(color) - self.elapsed_ms(now_ms)).max(0)
        } else {
            self.rest_ms(color)
        }
    }

    /// Apply a completed move: charge the mover, add the increment and switch
    /// turns. Callers must have checked [`Clock::flag_fallen`] first.
    pub fn apply_move(&mut self, mover: Color, now_ms: i64) {
        // Fischer increment: the mover's clock is charged for the elapsed time
        // and then credited the increment (never below zero).
        let charged = (self.rest_ms(mover) - self.elapsed_ms(now_ms)).max(0) + self.increment_ms;
        match mover {
            Color::White => self.white_ms = charged,
            Color::Black => self.black_ms = charged,
        }
        self.turn = mover.opposite();
        self.turn_started_ms = now_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_000_000;

    #[test]
    fn counts_down_running_side_only() {
        let mut clock = Clock::new(60_000, 0, Color::White, T0);
        let now = T0 + 5_000;
        assert_eq!(clock.remaining_ms_at(Color::White, now), 55_000);
        assert_eq!(clock.remaining_ms_at(Color::Black, now), 60_000);

        clock.apply_move(Color::White, now);
        assert_eq!(clock.turn, Color::Black);
        assert_eq!(clock.turn_started_ms, now);
        // Black's clock starts running from the same instant.
        assert_eq!(clock.remaining_ms_at(Color::Black, now + 1_000), 59_000);
    }

    #[test]
    fn applies_increment() {
        let mut clock = Clock::new(10_000, 2_000, Color::White, T0);
        clock.apply_move(Color::White, T0 + 3_000);
        // 10s - 3s + 2s increment = 9s.
        assert_eq!(clock.white_ms, 9_000);
    }

    #[test]
    fn flag_falls_deterministically() {
        let clock = Clock::new(1_000, 0, Color::White, T0);
        assert!(!clock.flag_fallen(T0 + 999));
        assert!(clock.flag_fallen(T0 + 1_000));
        assert!(clock.flag_fallen(T0 + 1_001));
        assert_eq!(clock.flag_falls_at_ms(), T0 + 1_000);
    }

    #[test]
    fn never_goes_negative() {
        let mut clock = Clock::new(1_000, 0, Color::White, T0);
        clock.apply_move(Color::White, T0 + 5_000);
        assert_eq!(clock.white_ms, 0);
    }
}
