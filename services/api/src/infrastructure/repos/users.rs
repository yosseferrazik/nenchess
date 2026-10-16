//! User, credential, profile, rating and session persistence.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
}

pub async fn insert_user(
    conn: &mut PgConnection,
    id: Uuid,
    username: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO users (id, username) VALUES ($1, $2)")
        .bind(id)
        .bind(username)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn insert_credentials(
    conn: &mut PgConnection,
    user_id: Uuid,
    password_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO user_credentials (user_id, password_hash) VALUES ($1, $2)")
        .bind(user_id)
        .bind(password_hash)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn insert_profile(conn: &mut PgConnection, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO profiles (user_id) VALUES ($1)")
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn insert_rating(conn: &mut PgConnection, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO ratings (user_id) VALUES ($1)")
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn find_by_username(
    conn: &mut PgConnection,
    username: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>("SELECT id, username FROM users WHERE lower(username) = lower($1)")
        .bind(username)
        .fetch_optional(conn)
        .await
}

pub async fn find_by_id(conn: &mut PgConnection, id: Uuid) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>("SELECT id, username FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(conn)
        .await
}

/// Usernames for a set of ids, keyed by id (for view assembly).
pub async fn usernames_for(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, UserRow>("SELECT id, username FROM users WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(conn)
        .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.username)).collect())
}

pub async fn find_password_hash(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_as::<_, (String,)>("SELECT password_hash FROM user_credentials WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(conn)
        .await
        .map(|row| row.map(|(hash,)| hash))
}

pub async fn insert_session(
    conn: &mut PgConnection,
    token: &str,
    user_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO sessions (token, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(token)
        .bind(user_id)
        .bind(expires_at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Resolve a bearer token to its user, ignoring expired sessions.
pub async fn session_user(
    conn: &mut PgConnection,
    token: &str,
    now: DateTime<Utc>,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT u.id, u.username \
         FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token = $1 AND s.expires_at > $2",
    )
    .bind(token)
    .bind(now)
    .fetch_optional(conn)
    .await
}
