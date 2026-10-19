//! Outbox dispatcher: the post-commit publisher.
//!
//! Periodically takes a locked batch of pending events, publishes each to the
//! in-process hub (live WebSockets) and marks them dispatched. Delivery to
//! in-process subscribers is therefore *at-least-once*: a crash between
//! publish and mark re-delivers, which is why clients apply events
//! idempotently by `version`.

use std::time::Duration;

use protocol::ServerMessage;
use tokio::task::JoinHandle;

use crate::bootstrap::AppState;

pub fn spawn(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(err) = dispatch_once(&state).await {
                tracing::error!(error = %err, "outbox dispatch failed");
            }
            tokio::time::sleep(Duration::from_millis(state.config.outbox_poll_interval_ms)).await;
        }
    })
}

async fn dispatch_once(state: &AppState) -> Result<(), sqlx::Error> {
    let mut tx = state.db.begin().await?;
    let pending = super::repos::outbox::fetch_pending(&mut tx).await?;
    if pending.is_empty() {
        return tx.commit().await;
    }

    let mut ids = Vec::with_capacity(pending.len());
    for row in pending {
        match serde_json::from_value::<ServerMessage>(row.payload.clone()) {
            Ok(msg) => {
                if let Some(game_id) = msg.game_id() {
                    tracing::trace!(event = %row.id, game = %game_id, "publishing outbox event");
                    state.hub.publish(game_id, &msg);
                }
            }
            Err(err) => {
                tracing::error!(event = %row.id, error = %err, "outbox payload is not a ServerMessage");
            }
        }
        ids.push(row.id);
    }
    super::repos::outbox::mark_dispatched(&mut tx, &ids).await?;
    tx.commit().await?;
    state
        .metrics
        .outbox_published
        .fetch_add(ids.len() as u64, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}
