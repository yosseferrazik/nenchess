use std::time::Duration;

use axum::extract::State;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::bootstrap::AppState;
use crate::error::ApiError;

/// Readiness budget for `/readyz`: a database that takes longer than this to
/// answer `SELECT 1` is not ready to serve traffic.
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);

/// Liveness: the process is up and the HTTP stack answers. Never touches
/// the database — a dead database must not get the process killed.
pub async fn livez() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "alive" }))
}

/// Readiness: the database answers a trivial query within a short budget.
pub async fn readyz(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    match tokio::time::timeout(
        READINESS_TIMEOUT,
        sqlx::query("SELECT 1").execute(&state.db),
    )
    .await
    {
        Ok(Ok(_)) => Ok(Json(serde_json::json!({ "status": "ready" }))),
        Ok(Err(err)) => Err(ApiError::from(err)),
        Err(_elapsed) => Err(ApiError::internal("database readiness probe timed out")),
    }
}

/// Legacy liveness + database probe (kept for existing deployments; prefer
/// `/livez` + `/readyz`).
pub async fn health(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "watched_games": state.hub.watched_games().len(),
    })))
}

/// Prometheus text exposition of process metrics (`/metrics`).
pub async fn metrics(State(state): State<AppState>) -> Response {
    let mut response = state.metrics.render().into_response();
    match HeaderValue::from_str("text/plain; version=0.0.4; charset=utf-8") {
        Ok(value) => {
            response.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        Err(err) => tracing::error!(error = %err, "metrics content type is invalid"),
    }
    response
}
