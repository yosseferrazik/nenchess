//! HTTP surface: REST for everything that is not a live game, plus the
//! WebSocket upgrade endpoint.

pub mod auth;
pub mod games;
pub mod health;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use protocol::UserDto;

use crate::bootstrap::AppState;
use crate::error::ApiError;
use crate::modules::identity;
use crate::transport::ws;

/// Authenticated user, attached to request extensions by [`require_auth`].
#[derive(Clone, Debug)]
pub struct AuthUser(pub UserDto);

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/auth/me", get(auth::me))
        .route("/games", post(games::create).get(games::list))
        .route("/games/{id}", get(games::get))
        .route("/games/{id}/join", post(games::join))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    let api = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .merge(protected);

    Router::new()
        .route("/health", get(health::health))
        .nest("/api", api)
        .route("/ws", get(ws::upgrade))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Bearer-token authentication middleware. The WebSocket endpoint
/// authenticates itself through a query parameter instead.
async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = bearer_token(req.headers()).ok_or_else(ApiError::unauthorized)?;
    let user = identity::resolve_session(&state.db, &token)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    req.extensions_mut().insert(AuthUser(user));
    Ok(next.run(req).await)
}

pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

/// `201 Created` helper for handlers.
pub fn created<T: serde::Serialize>(body: T) -> (StatusCode, axum::Json<T>) {
    (StatusCode::CREATED, axum::Json(body))
}
