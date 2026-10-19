//! Infrastructure adapters: PostgreSQL, server clock, in-process pub/sub hub,
//! per-game locks, rate limiting, metrics and the transactional outbox
//! dispatcher.

pub mod clock;
pub mod db;
pub mod hub;
pub mod matchmaking_scheduler;
pub mod metrics;
pub mod outbox_dispatcher;
pub mod rate_limit;
pub mod registry;
pub mod repos;
