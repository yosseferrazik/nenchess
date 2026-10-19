//! Identity: registration, login, logout and session resolution.
//!
//! Passwords are hashed with Argon2id (`argon2id$<salt>$<hash>`); sessions
//! are opaque random tokens stored server-side with an expiry. Browsers
//! receive the token in an `HttpOnly` `SameSite=Lax` cookie; API clients
//! send it as `Authorization: Bearer …` (WebSocket: `?token=` or cookie).

use argon2::Argon2;
use chrono::{Duration, Utc};
use protocol::{AuthResponse, LoginRequest, RegisterRequest, UserDto};
use sqlx::PgPool;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::error::ApiError;
use crate::infrastructure::repos::users;

/// Register a new user and open a session.
pub async fn register(
    pool: &PgPool,
    req: &RegisterRequest,
    session_ttl_days: i64,
) -> Result<AuthResponse, ApiError> {
    validate_username(&req.username)?;
    validate_password(&req.password)?;

    let user_id = Uuid::new_v4();
    let password_hash = hash_password(&req.password)?;

    let mut tx = pool.begin().await?;
    if users::find_by_username(&mut tx, &req.username)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("username is already taken"));
    }
    users::insert_user(&mut tx, user_id, &req.username).await?;
    users::insert_credentials(&mut tx, user_id, &password_hash).await?;
    users::insert_profile(&mut tx, user_id).await?;
    users::insert_rating(&mut tx, user_id).await?;

    let (token, expires_at_ms) = open_session(&mut tx, user_id, session_ttl_days).await?;
    tx.commit().await?;

    tracing::info!(user = %req.username, "user registered");
    Ok(AuthResponse {
        token,
        user: UserDto {
            id: user_id,
            username: req.username.clone(),
        },
        expires_at_ms,
    })
}

/// Verify credentials and open a session. The error is deliberately identical
/// for unknown users and wrong passwords (no user enumeration).
pub async fn login(
    pool: &PgPool,
    req: &LoginRequest,
    session_ttl_days: i64,
) -> Result<AuthResponse, ApiError> {
    let invalid = || ApiError::unauthorized_with("invalid username or password");

    let mut conn = pool.acquire().await?;
    let user = users::find_by_username(&mut conn, &req.username)
        .await?
        .filter(|u| !u.username.is_empty())
        .ok_or_else(invalid)?;
    let stored = users::find_password_hash(&mut conn, user.id)
        .await?
        .ok_or_else(invalid)?;
    drop(conn);

    if !verify_password(&req.password, &stored) {
        return Err(invalid());
    }

    let mut tx = pool.begin().await?;
    let (token, expires_at_ms) = open_session(&mut tx, user.id, session_ttl_days).await?;
    tx.commit().await?;

    tracing::info!(user = %user.username, "user logged in");
    Ok(AuthResponse {
        token,
        user: UserDto {
            id: user.id,
            username: user.username,
        },
        expires_at_ms,
    })
}

/// Resolve a bearer token to the authenticated user, if it is still valid.
pub async fn resolve_session(pool: &PgPool, token: &str) -> Result<Option<UserDto>, ApiError> {
    let mut conn = pool.acquire().await?;
    let row = users::session_user(&mut conn, token, Utc::now()).await?;
    Ok(row.map(|r| UserDto {
        id: r.id,
        username: r.username,
    }))
}

/// Revoke a session token (logout). Revoking an unknown or expired token is
/// a no-op: the client already lost access.
pub async fn logout(pool: &PgPool, token: &str) -> Result<(), ApiError> {
    let mut conn = pool.acquire().await?;
    users::delete_session(&mut conn, token).await?;
    Ok(())
}

async fn open_session(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    ttl_days: i64,
) -> Result<(String, i64), ApiError> {
    // v4 UUIDs carry 122 random bits — sufficient as opaque session tokens.
    let token = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::days(ttl_days.max(1));
    users::insert_session(conn, &token, user_id, expires_at).await?;
    Ok((token, expires_at.timestamp_millis()))
}

fn validate_username(username: &str) -> Result<(), ApiError> {
    let len = username.chars().count();
    if !(3..=20).contains(&len) {
        return Err(ApiError::bad_request(
            "username must be between 3 and 20 characters",
        ));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(ApiError::bad_request(
            "username may only contain letters, digits and underscore",
        ));
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.len() < 8 {
        return Err(ApiError::bad_request(
            "password must be at least 8 characters",
        ));
    }
    if password.len() > 128 {
        return Err(ApiError::bad_request(
            "password must be at most 128 characters",
        ));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = Uuid::new_v4();
    let mut out = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt.as_bytes(), &mut out)
        .map_err(|err| ApiError::internal(format!("password hashing failed: {err}")))?;
    Ok(format!(
        "argon2id${}${}",
        hex_encode(salt.as_bytes()),
        hex_encode(&out)
    ))
}

fn verify_password(password: &str, stored: &str) -> bool {
    let Some(rest) = stored.strip_prefix("argon2id$") else {
        return false;
    };
    let Some((salt_hex, expected_hex)) = rest.split_once('$') else {
        return false;
    };
    let (Some(salt), Some(expected)) = (hex_decode(salt_hex), hex_decode(expected_hex)) else {
        return false;
    };
    if expected.is_empty() {
        return false;
    }
    let mut out = vec![0u8; expected.len()];
    if Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut out)
        .is_err()
    {
        return false;
    }
    out.as_slice().ct_eq(expected.as_slice()).into()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_roundtrip() {
        let stored = hash_password("correct-horse-battery").expect("hash");
        assert!(stored.starts_with("argon2id$"));
        assert!(verify_password("correct-horse-battery", &stored));
        assert!(!verify_password("wrong-password", &stored));
        assert!(!verify_password("correct-horse-battery", "garbage"));
        assert!(!verify_password("correct-horse-battery", "argon2id$zz$zz"));
    }

    #[test]
    fn username_rules() {
        assert!(validate_username("abc").is_ok());
        assert!(validate_username("Player_01").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username(&"x".repeat(21)).is_err());
        assert!(validate_username("bad name").is_err());
        assert!(validate_username("ñandú").is_err());
    }

    #[test]
    fn password_rules() {
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
        assert!(validate_password(&"x".repeat(129)).is_err());
    }
}
