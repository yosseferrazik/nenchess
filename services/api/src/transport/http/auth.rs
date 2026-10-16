use axum::extract::State;
use axum::{Extension, Json};
use protocol::{AuthResponse, LoginRequest, RegisterRequest, UserDto};

use crate::bootstrap::AppState;
use crate::error::ApiError;
use crate::modules::identity;
use crate::transport::http::{created, AuthUser};

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let response = identity::register(&state.db, &req, state.config.session_ttl_days).await?;
    Ok(created(response))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let response = identity::login(&state.db, &req, state.config.session_ttl_days).await?;
    Ok(Json(response))
}

pub async fn me(Extension(user): Extension<AuthUser>) -> Json<UserDto> {
    Json(user.0)
}
