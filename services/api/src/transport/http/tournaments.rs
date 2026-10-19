use axum::extract::{Path, State};
use axum::{Extension, Json};
use protocol::{CreateTournamentRequest, TournamentDto, TournamentView, TournamentsResponse};
use uuid::Uuid;

use crate::bootstrap::AppState;
use crate::error::ApiError;
use crate::modules::tournaments;
use crate::transport::http::AuthUser;

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<CreateTournamentRequest>,
) -> Result<(axum::http::StatusCode, Json<TournamentDto>), ApiError> {
    let mut conn = state.db.acquire().await?;
    let t = tournaments::create_tournament(
        &mut conn,
        &req.name,
        req.time_control_initial_ms,
        req.time_control_increment_ms,
        req.rated,
        req.max_participants,
        user.0.id,
    )
    .await?;
    Ok((axum::http::StatusCode::CREATED, Json(t)))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthUser>,
) -> Result<Json<TournamentsResponse>, ApiError> {
    let mut conn = state.db.acquire().await?;
    let ts = tournaments::list_tournaments(&mut conn).await?;
    Ok(Json(ts))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut conn = state.db.acquire().await?;
    let view = tournaments::get_tournament(&mut conn, id).await?;
    Ok(Json(view))
}

pub async fn join(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), ApiError> {
    let mut conn = state.db.acquire().await?;
    tournaments::join_tournament(&mut conn, id, user.0.id).await?;
    Ok((axum::http::StatusCode::OK, Json(serde_json::Value::Null)))
}

pub async fn start(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> Result<Json<TournamentDto>, ApiError> {
    let mut conn = state.db.acquire().await?;
    let now = state.clock.now_ms();
    let t = tournaments::start_tournament(&mut conn, id, user.0.id, now).await?;
    Ok(Json(t))
}
