use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use protocol::{LoginRequest, RegisterRequest, UserDto};
use std::sync::atomic::Ordering;

use crate::bootstrap::AppState;
use crate::error::ApiError;
use crate::modules::identity;
use crate::transport::http::{
    bearer_token, cleared_session_cookie_header, created, session_cookie, session_cookie_header,
    AuthUser,
};

/// Attach the session cookie to an already-formed response.
fn attach_cookie(response: Response, cookie: &str) -> Response {
    let mut response = response;
    match HeaderValue::from_str(cookie) {
        Ok(value) => {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
        Err(err) => tracing::error!(error = %err, "session cookie is not a valid header value"),
    }
    response
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Response, ApiError> {
    let response = identity::register(&state.db, &req, state.config.session_ttl_days).await?;
    state
        .metrics
        .auth_registrations
        .fetch_add(1, Ordering::Relaxed);
    let cookie = session_cookie_header(
        &response.token,
        state.config.session_ttl_days,
        state.config.cookie_secure,
    );
    Ok(attach_cookie(created(response).into_response(), &cookie))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    match identity::login(&state.db, &req, state.config.session_ttl_days).await {
        Ok(response) => {
            state.metrics.auth_logins.fetch_add(1, Ordering::Relaxed);
            let cookie = session_cookie_header(
                &response.token,
                state.config.session_ttl_days,
                state.config.cookie_secure,
            );
            Ok(attach_cookie(Json(response).into_response(), &cookie))
        }
        Err(err) => {
            if err.status == StatusCode::UNAUTHORIZED {
                state
                    .metrics
                    .auth_login_failures
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(err)
        }
    }
}

/// `POST /api/auth/logout` — revoke the session server-side and expire the
/// cookie. Idempotent: an unknown or absent token still clears the cookie.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = bearer_token(&headers).or_else(|| session_cookie(&headers)) {
        if let Err(err) = identity::logout(&state.db, &token).await {
            tracing::warn!(error = %err, "logout failed to revoke session");
        }
    }
    let cookie = cleared_session_cookie_header(state.config.cookie_secure);
    attach_cookie(StatusCode::NO_CONTENT.into_response(), &cookie)
}

pub async fn me(Extension(user): Extension<AuthUser>) -> Json<UserDto> {
    Json(user.0)
}
