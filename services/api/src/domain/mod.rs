//! Domain layer: game aggregate and invariants.
//!
//! Pure logic only — no Axum, no SQLx, no wall-clock access. Time enters the
//! domain as an `i64` (epoch ms) supplied by the application layer.

mod game;

pub use game::{
    apply_move, join, resign, DomainError, Game, GameResult, GameStatus, MoveOutcome, MoveRecord,
    ResultReason,
};
