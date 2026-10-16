use axum::extract::State;
use axum::Json;

use crate::bootstrap::AppState;
use crate::error::ApiError;

/// Liveness + database readiness probe.
pub async fn health(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "watched_games": state.hub.watched_games().len(),
    })))
}
