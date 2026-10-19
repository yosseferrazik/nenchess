//! Domain layer: game aggregate and invariants.
//!
//! Pure logic only — no Axum, no SQLx, no wall-clock access. Time enters the
//! domain as an `i64` (epoch ms) supplied by the application layer.

mod game;

pub use game::{
    accept_draw, apply_move, decline_draw, join, offer_draw, resign, DomainError, DrawOutcome,
    Game, GameResult, GameStatus, MoveOutcome, MoveRecord, ResultReason,
};
