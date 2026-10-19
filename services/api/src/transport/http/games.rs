use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;
use axum::{Extension, Json};
use protocol::{CreateGameRequest, GameView, GamesResponse};
use uuid::Uuid;

use crate::application::export;
use crate::bootstrap::AppState;
use crate::error::ApiError;
use crate::modules::games;
use crate::transport::http::{created, AuthUser};

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<CreateGameRequest>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let view = games::create_game(&state, user.0.id, req).await?;
    Ok(created(view))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthUser>,
) -> Result<Json<GamesResponse>, ApiError> {
    let response = games::list_open_games(&state).await?;
    Ok(Json(response))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> Result<Json<GameView>, ApiError> {
    let view = games::get_game(&state, id).await?;
    Ok(Json(view))
}

pub async fn join(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> Result<Json<GameView>, ApiError> {
    let view = games::join_game(&state, user.0.id, id).await?;
    Ok(Json(view))
}

/// PGN export of any game, replayed from the immutable move records with SAN
/// rendered by the rules kernel. Same visibility as the game itself: any
/// authenticated reader.
pub async fn pgn(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let mut conn = state.db.acquire().await?;
    let body = export::game_pgn(&mut conn, id).await?;
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"game-{id}.pgn\""))
            .expect("uuid filenames are valid header values"),
    );
    Ok(response)
}
