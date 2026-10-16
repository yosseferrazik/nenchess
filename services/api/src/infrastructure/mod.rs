//! Infrastructure adapters: PostgreSQL, server clock, in-process pub/sub hub,
//! per-game locks and the transactional outbox dispatcher.

pub mod clock;
pub mod db;
pub mod hub;
pub mod outbox_dispatcher;
pub mod registry;
pub mod repos;
