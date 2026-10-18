//! NenChess API — a modular monolith.
//!
//! Layout (see `docs/architecture`):
//! * `transport`       — Axum: REST, WebSocket, auth extraction, rate limits.
//! * `application`     — use cases: transactions, idempotency, coordination.
//! * `modules`         — feature slices: identity, games, matchmaking, ratings.
//! * `domain`          — game aggregate and invariants (no framework types).
//! * `infrastructure`  — PostgreSQL, clocks, hub, outbox dispatcher.
//!
//! Dependency rule: `domain` knows only `game-core` and `serde`. Everything
//! else points inward.

pub mod application;
pub mod bootstrap;
pub mod config;
pub mod domain;
pub mod error;
pub mod infrastructure;
pub mod modules;
pub mod transport;

pub use bootstrap::{
    build_router, build_state, run_migrations, serve, spawn_matchmaking_scheduler,
    spawn_outbox_dispatcher, AppState,
};
pub use config::Config;
