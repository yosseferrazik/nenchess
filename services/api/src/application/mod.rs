//! Application layer: cross-module use cases — transactions, idempotency,
//! authorization glue and view assembly. Transport calls into here; the
//! domain stays pure.

pub mod moves;
pub mod views;

pub use moves::{submit_move, submit_resign, MoveCommand, ResignCommand};
