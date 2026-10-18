use axum::extract::State;
use axum::{Extension, Json};
use protocol::ProfileDto;

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
