//! Ratings (stage 05): ELO computation and history.
//!
//! Unimplemented in the skeleton. Rules already fixed:
//! * The server computes rating changes — clients never send ELO.
//! * `ratings` rows exist from registration (default 1200).
//! * Every change appends `rating_history` (before/after) in the same
//!   transaction as the game result, driven by the `game.ended` outbox event.
