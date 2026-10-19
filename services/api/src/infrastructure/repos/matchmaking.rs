//! Matchmaking queue persistence.
//!
//! A row represents "this player is waiting for an opponent with this time
//! control". Once paired, the row keeps `matched_game_id` so a polling client
//! can be told which game to open; the client consumes it by fetching its
//! status, which deletes the row.
//!
//! Rated queues are skill-gated: two players may pair only when their rating
//! distance fits a window that starts at [`BASE_RATING_WINDOW`] points, grows
//! with the wait of the earlier-enqueued player of the pair, and caps at
//! [`MAX_RATING_WINDOW`]. Casual queues ignore ratings entirely.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::modules::ratings::{category_sql, DEFAULT_RATING};

const ENTRY_COLUMNS: &str =
    "user_id, initial_time_ms, increment_ms, rated, enqueued_at, matched_game_id";

/// Initial pairing tolerance in rating points for rated queues.
const BASE_RATING_WINDOW: i64 = 150;
/// Points the window gains per second the earlier-enqueued player waits.
const WINDOW_GROWTH_PER_SECOND: i64 = 2;
/// The window never grows past this.
const MAX_RATING_WINDOW: i64 = 1000;

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

/// One player's rating in the given category, defaulting for players whose
/// row does not exist yet (registration seeds them; pairing must not care).
fn rating_sql(user_expr: &str, category_expr: &str) -> String {
    format!(
        "COALESCE((SELECT pr.rating FROM player_ratings pr \
         WHERE pr.user_id = {user_expr} AND pr.category = {category_expr}), {DEFAULT_RATING})"
    )
}

/// The widest rating gap the pair may have *right now*: the base window grown
/// by how long the earlier-enqueued player of the two has waited (never
/// negative), capped at [`MAX_RATING_WINDOW`].
fn window_sql(anchor_enqueued_at: &str, candidate_enqueued_at: &str) -> String {
    format!(
        "LEAST({MAX_RATING_WINDOW}, {BASE_RATING_WINDOW} + {WINDOW_GROWTH_PER_SECOND} \
         * GREATEST(0, EXTRACT(EPOCH FROM \
         GREATEST(now() - {anchor_enqueued_at}, now() - {candidate_enqueued_at}))))"
    )
}

/// Oldest unmatched entry that has at least one waiting opponent with the same
/// time control *and — for rated queues — a rating inside the current window,
/// locked for the duration of the transaction.
///
/// Restricting to *matchable* entries is what keeps a lone player searching for
/// an unusual time control (or one sitting far outside everyone's window) from
/// blocking everyone else in the queue: the scheduler looks past them to the
/// next pair.
pub async fn oldest_matchable(conn: &mut PgConnection) -> Result<Option<QueueEntry>, sqlx::Error> {
    // The anchor's category is not known before the anchor is picked, so the
    // mapping is evaluated in SQL from the same constants `category_for` uses.
    let category = category_sql("a.initial_time_ms", "a.increment_ms");
    let anchor_rating = rating_sql("a.user_id", &category);
    let candidate_rating = rating_sql("b.user_id", &category);
    let window = window_sql("a.enqueued_at", "b.enqueued_at");
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
                 AND (NOT a.rated OR ABS({anchor_rating} - {candidate_rating}) <= {window}) \
           ) \
         ORDER BY a.enqueued_at ASC \
         LIMIT 1 \
         FOR UPDATE OF a SKIP LOCKED"
    ))
    .fetch_optional(conn)
    .await
}

/// The best waiting opponent for `entry`: same time control, inside the
/// current rating window for rated queues, *nearest rating first* (casual
/// queues fall back to pure FIFO), locked.
///
/// `$1` is the rating category of the shared time control.
pub async fn waiting_opponent(
    conn: &mut PgConnection,
    entry: &QueueEntry,
    category: &str,
) -> Result<Option<QueueEntry>, sqlx::Error> {
    let anchor_rating = rating_sql("$2", "$1");
    let candidate_rating = rating_sql("b.user_id", "$1");
    let window = window_sql("$6::timestamptz", "b.enqueued_at");
    sqlx::query_as::<_, QueueEntry>(&format!(
        "SELECT {ENTRY_COLUMNS} FROM matchmaking_queue b \
         WHERE b.matched_game_id IS NULL \
           AND b.user_id <> $2 \
           AND b.initial_time_ms = $3 \
           AND b.increment_ms = $4 \
           AND b.rated = $5 \
           AND (NOT b.rated OR ABS({anchor_rating} - {candidate_rating}) <= {window}) \
         ORDER BY \
           CASE WHEN b.rated THEN ABS({anchor_rating} - {candidate_rating}) ELSE 0 END ASC, \
           b.enqueued_at ASC \
         LIMIT 1 \
         FOR UPDATE SKIP LOCKED"
    ))
    .bind(category)
    .bind(entry.user_id)
    .bind(entry.initial_time_ms)
    .bind(entry.increment_ms)
    .bind(entry.rated)
    .bind(entry.enqueued_at)
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
