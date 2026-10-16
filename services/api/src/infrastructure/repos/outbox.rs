//! Transactional outbox. Events destined to other components (the WebSocket
//! hub today, notifications/ratings/analytics later) are written in the same
//! transaction as the state change that produced them, then published by a
//! background dispatcher — publication never depends on an external call
//! succeeding inside a SQL transaction.

use sqlx::PgConnection;
use uuid::Uuid;

pub async fn enqueue(
    conn: &mut PgConnection,
    aggregate_id: Uuid,
    event_type: &str,
    payload: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO outbox_events (aggregate_type, aggregate_id, event_type, payload) \
         VALUES ('game', $1, $2, $3)",
    )
    .bind(aggregate_id)
    .bind(event_type)
    .bind(payload)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
pub struct OutboxRow {
    pub id: i64,
    pub aggregate_id: Uuid,
    pub payload: serde_json::Value,
}

/// Fetch a batch of pending events, locking them (`SKIP LOCKED` so multiple
/// dispatchers never fight over the same row). Must run inside a transaction.
pub async fn fetch_pending(conn: &mut PgConnection) -> Result<Vec<OutboxRow>, sqlx::Error> {
    sqlx::query_as::<_, OutboxRow>(
        "SELECT id, aggregate_id, payload FROM outbox_events \
         WHERE dispatched_at IS NULL ORDER BY id ASC LIMIT 100 FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(conn)
    .await
}

pub async fn mark_dispatched(conn: &mut PgConnection, ids: &[i64]) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("UPDATE outbox_events SET dispatched_at = now() WHERE id = ANY($1)")
        .bind(ids)
        .execute(conn)
        .await?;
    Ok(())
}
