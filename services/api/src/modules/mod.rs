//! Business feature modules. Each module groups one slice of the product;
//! cross-module use cases and transactions live in `application`.

pub mod games;
pub mod identity;
pub mod matchmaking;
pub mod ratings;
pub mod tournaments;
