//! Matchmaking queue persistence.
//!
//! A row represents "this player is waiting for an opponent with this time
//! control". Once paired, the row keeps `matched_game_id` so a polling client
//! can be told which game to open; the client consumes it by fetching its
//! status, which deletes the row.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

const ENTRY_COLUMNS: &str =
    "user_id, initial_time_ms, increment_ms, rated, enqueued_at, matched_game_id";

#[derive(Debug, sqlx::FromRow)]
pub struct QueueEntry {
    pub user_id: Uuid,
    pub initial_time_ms: i64,
    pub increment_ms: i64,
    pub rated: bool,
    pub enqueued_at: DateTime<Utc>,
    pub matched_game_id: Option<Uuid>,
}

/// Enter (or re-enter) the queue for a time control. Re-entering replaces any
/// previous entry together with its pending match.
pub async fn enqueue(
    conn: &mut PgConnection,
    user_id: Uuid,
    initial_time_ms: i64,
    increment_ms: i64,
    rated: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO matchmaking_queue (user_id, initial_time_ms, increment_ms, rated, matched_game_id) \
         VALUES ($1,$2,$3,$4,NULL) \
         ON CONFLICT (user_id) DO UPDATE SET \
            initial_time_ms = EXCLUDED.initial_time_ms, \
            increment_ms = EXCLUDED.increment_ms, \
            rated = EXCLUDED.rated, \
            enqueued_at = now(), \
            matched_game_id = NULL",
    )
    .bind(user_id)
    .bind(initial_time_ms)
    .bind(increment_ms)
    .bind(rated)
    .execute(conn)
    .await?;
    Ok(())
}

/// Leave the queue. Returns whether an entry was actually removed.
pub async fn cancel(conn: &mut PgConnection, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let affected = sqlx::query("DELETE FROM matchmaking_queue WHERE user_id = $1")
        .bind(user_id)
        .execute(conn)
        .await?
        .rows_affected();
    Ok(affected > 0)
}

pub async fn status(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> Result<Option<QueueEntry>, sqlx::Error> {
    sqlx::query_as::<_, QueueEntry>(&format!(
        "SELECT {ENTRY_COLUMNS} FROM matchmaking_queue WHERE user_id = $1"
    ))
    .bind(user_id)
    .fetch_optional(conn)
    .await
}

/// Remove a matched entry and return the game it was paired into.
pub async fn take_matched(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_as::<_, (Option<Uuid>,)>(
        "DELETE FROM matchmaking_queue WHERE user_id = $1 AND matched_game_id IS NOT NULL \
         RETURNING matched_game_id",
    )
    .bind(user_id)
    .fetch_optional(conn)
    .await
    .map(|row| row.and_then(|(game_id,)| game_id))
}

/// Oldest unmatched entry that has at least one waiting opponent with the same
/// time control, locked for the duration of the transaction.
///
/// Restricting to *matchable* entries is what keeps a lone player searching for
/// an unusual time control from blocking everyone else in the queue: the
/// scheduler looks past them to the next pair.
pub async fn oldest_matchable(conn: &mut PgConnection) -> Result<Option<QueueEntry>, sqlx::Error> {
    sqlx::query_as::<_, QueueEntry>(&format!(
        "SELECT {ENTRY_COLUMNS} FROM matchmaking_queue a \
         WHERE a.matched_game_id IS NULL \
           AND EXISTS ( \
               SELECT 1 FROM matchmaking_queue b \
               WHERE b.matched_game_id IS NULL \
                 AND b.user_id <> a.user_id \
                 AND b.initial_time_ms = a.initial_time_ms \
                 AND b.increment_ms = a.increment_ms \
                 AND b.rated = a.rated \
           ) \
         ORDER BY a.enqueued_at ASC \
         LIMIT 1 \
         FOR UPDATE OF a SKIP LOCKED"
    ))
    .fetch_optional(conn)
    .await
}

/// The earliest waiting opponent with the same time control, locked.
pub async fn waiting_opponent(
    conn: &mut PgConnection,
    entry: &QueueEntry,
) -> Result<Option<QueueEntry>, sqlx::Error> {
    sqlx::query_as::<_, QueueEntry>(&format!(
        "SELECT {ENTRY_COLUMNS} FROM matchmaking_queue \
         WHERE matched_game_id IS NULL \
           AND user_id <> $1 \
           AND initial_time_ms = $2 \
           AND increment_ms = $3 \
           AND rated = $4 \
         ORDER BY enqueued_at ASC \
         LIMIT 1 \
         FOR UPDATE SKIP LOCKED"
    ))
    .bind(entry.user_id)
    .bind(entry.initial_time_ms)
    .bind(entry.increment_ms)
    .bind(entry.rated)
    .fetch_optional(conn)
    .await
}

/// Record the game an entry was paired into.
pub async fn mark_matched(
    conn: &mut PgConnection,
    user_id: Uuid,
    game_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE matchmaking_queue SET matched_game_id = $2 WHERE user_id = $1")
        .bind(user_id)
        .bind(game_id)
        .execute(conn)
        .await?;
    Ok(())
}
