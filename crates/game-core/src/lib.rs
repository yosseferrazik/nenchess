//! `game-core` is the chess rules kernel of the platform.
//!
//! It is deliberately free of any transport, database or framework
//! dependency: it can be unit-tested in isolation and reused by any
//! adapter (HTTP, WebSocket, CLI, tests).
//!
//! The underlying move generator is `cozy-chess` (MIT licence). Its types
//! never leak through this API — the engine is an internal adapter that
//! could be swapped without touching callers.

mod clock;
mod notation;
mod rules;
mod types;

pub use clock::Clock;
pub use notation::san;
pub use rules::{
    insufficient_material, legal_moves, normalize, position_hash, starting_fen, status,
    validate_and_apply, DrawKind, GameStatus, MoveError, MoveInput, STARTING_FEN,
};
pub use types::{Color, PieceKind, Square};
