//! Per-category Elo ratings persistence.
use std::collections::HashMap;

use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct RatingRow {
    pub rating: i32,
    pub games_played: i32,
}

/// Read a player's rating map (category → (rating, games_played)).
pub async fn get_ratings_map(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> Result<HashMap<String, (i32, i32)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, i32, i32)>(
        "SELECT category, rating, games_played FROM player_ratings WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(category, rating, games_played)| (category, (rating, games_played)))
        .collect())
}

pub async fn ensure_exists(
    conn: &mut PgConnection,
    user_id: Uuid,
    categories: &[&str],
) -> Result<(), sqlx::Error> {
    for category in categories {
        sqlx::query(
            "INSERT INTO player_ratings (user_id, category) VALUES ($1, $2) \
             ON CONFLICT (user_id, category) DO NOTHING",
        )
        .bind(user_id)
        .bind(*category)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Read a single rating, creating the row (default rating) if it is missing.
pub async fn get_or_create(
    conn: &mut PgConnection,
    user_id: Uuid,
    category: &str,
) -> Result<RatingRow, sqlx::Error> {
    ensure_exists(&mut *conn, user_id, &[category]).await?;
    // Lock the row so two games ending at once for the same player cannot both
    // read the same "before" value and lose an update.
    sqlx::query_as::<_, RatingRow>(
        "SELECT rating, games_played FROM player_ratings \
         WHERE user_id = $1 AND category = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(category)
    .fetch_one(conn)
    .await
}

pub async fn set_rating(
    conn: &mut PgConnection,
    user_id: Uuid,
    category: &str,
    new_rating: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE player_ratings SET rating = $3, games_played = games_played + 1, updated_at = now() \
         WHERE user_id = $1 AND category = $2",
    )
    .bind(user_id)
    .bind(category)
    .bind(new_rating)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn insert_history(
    conn: &mut PgConnection,
    user_id: Uuid,
    game_id: Uuid,
    rating_before: i32,
    rating_after: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO rating_history (user_id, game_id, rating_before, rating_after) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(game_id)
    .bind(rating_before)
    .bind(rating_after)
    .execute(conn)
    .await?;
    Ok(())
}
