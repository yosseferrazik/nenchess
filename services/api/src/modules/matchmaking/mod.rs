//! Matchmaking (stage 05): pairing players by rating and time control.
//!
//! Intentionally unimplemented in the executable skeleton. Planned design:
//! an append-only `matchmaking_queue` table plus a periodic scheduler that
//! pairs entries transactionally; the resulting game row is created through
//! `modules::games::create_game` so invariants stay in one place.
