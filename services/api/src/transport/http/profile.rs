use axum::extract::{Query, State};
use axum::{Extension, Json};
use protocol::{GameHistoryResponse, ProfileDto};
use serde::Deserialize;

use crate::application::export;
use crate::bootstrap::AppState;
use crate::error::ApiError;
use crate::infrastructure::repos::ratings as ratings_repo;
use crate::modules::ratings;
use crate::transport::http::AuthUser;

const CATEGORIES: [&str; 3] = ["bullet", "blitz", "rapid"];

/// The caller's own profile, with a rating row per standard category. Missing
/// rows are created lazily so the response is always complete.
pub async fn get_profile(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<ProfileDto>, ApiError> {
    let mut conn = state.db.acquire().await?;
    ratings_repo::ensure_exists(&mut conn, user.0.id, &CATEGORIES).await?;
    let map = ratings_repo::get_ratings_map(&mut conn, user.0.id).await?;
    Ok(Json(ratings::profile_dto(&user.0, &map)))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_history_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_history_limit() -> i64 {
    export::HISTORY_DEFAULT_LIMIT
}

/// The caller's game history (as white or black), newest first. Waiting games
/// are excluded — they are lobby material, not history. `limit` is clamped
/// server-side; out-of-range or malformed values never fail the request.
pub async fn get_history(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<GameHistoryResponse>, ApiError> {
    let mut conn = state.db.acquire().await?;
    let response = export::game_history(&mut conn, user.0.id, query.limit, query.offset).await?;
    Ok(Json(response))
}
