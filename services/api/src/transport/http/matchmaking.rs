use axum::extract::State;
use axum::{Extension, Json};
use protocol::{MatchmakingRequest, MatchmakingStatusDto};

use crate::bootstrap::AppState;
use crate::error::ApiError;
use crate::modules::matchmaking;
use crate::transport::http::AuthUser;

pub async fn enqueue(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<MatchmakingRequest>,
) -> Result<Json<MatchmakingStatusDto>, ApiError> {
    let status = matchmaking::enqueue(&state, user.0.id, req).await?;
    Ok(Json(status))
}

pub async fn dequeue(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<MatchmakingStatusDto>, ApiError> {
    let status = matchmaking::dequeue(&state, user.0.id).await?;
    Ok(Json(status))
}

pub async fn status(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<MatchmakingStatusDto>, ApiError> {
    let status = matchmaking::status(&state, user.0.id).await?;
    Ok(Json(status))
}
