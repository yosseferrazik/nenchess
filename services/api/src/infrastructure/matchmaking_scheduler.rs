//! Matchmaking scheduler: the single writer that creates games from the queue.
//!
//! Running it as one background task (rather than inside HTTP handlers) keeps
//! game creation out of the request path and makes the pairing deterministic.
//! The pairing itself uses `FOR UPDATE SKIP LOCKED`, so running more than one
//! of these against the same database is also safe.

use std::time::Duration;

use tokio::task::JoinHandle;

use crate::bootstrap::AppState;
use crate::modules::matchmaking;

pub fn spawn(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match matchmaking::drain_pairs(&state).await {
                Ok(n) if n > 0 => tracing::info!(paired = n, "matchmaking paired games"),
                Ok(_) => {}
                Err(err) => tracing::error!(error = %err, "matchmaking pass failed"),
            }
            tokio::time::sleep(Duration::from_millis(state.config.matchmaking_interval_ms)).await;
        }
    })
}
