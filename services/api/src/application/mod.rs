//! Application layer: cross-module use cases — transactions, idempotency,
//! authorization glue and view assembly. Transport calls into here; the
//! domain stays pure.

pub mod export;
pub mod moves;
pub mod views;

pub use moves::{
    submit_draw, submit_move, submit_resign, DrawAction, DrawCommand, MoveCommand, ResignCommand,
};
