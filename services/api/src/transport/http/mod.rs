//! HTTP surface: REST for everything that is not a live game, plus the
//! WebSocket upgrade endpoint.

pub mod auth;
pub mod csrf;
pub mod games;
pub mod health;
pub mod matchmaking;
pub mod middleware;
pub mod profile;
pub mod tournaments;

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

/// Name of the `HttpOnly` session cookie set at login/registration. The web
/// client never sees or stores the token; API clients keep using
/// `Authorization: Bearer …`, which is immune to CSRF and therefore exempt
/// from the [`csrf`] checks.
pub const SESSION_COOKIE: &str = "nenchess_session";

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/auth/me", get(auth::me))
        .route("/auth/logout", post(auth::logout))
        .route("/games", post(games::create).get(games::list))
        .route("/games/{id}", get(games::get))
        .route("/games/{id}/join", post(games::join))
        .route("/games/{id}/pgn", get(games::pgn))
        .route(
            "/matchmaking",
            post(matchmaking::enqueue)
                .delete(matchmaking::dequeue)
                .get(matchmaking::status),
        )
        .route("/profile", get(profile::get_profile))
        .route("/me/games", get(profile::get_history))
        .route(
            "/tournaments",
            post(tournaments::create).get(tournaments::list),
        )
        .route("/tournaments/{id}", get(tournaments::get))
        .route("/tournaments/{id}/join", post(tournaments::join))
        .route("/tournaments/{id}/start", post(tournaments::start))
        // Inner limit first, `require_auth` around it: authentication must
        // run before the per-user budget can know *which* user to charge.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit_api,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    // Credential endpoints are CPU-expensive (Argon2) and the classic
    // credential-stuffing target: budget them per client IP.
    let auth_public = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit_auth,
        ));

    let api = auth_public.merge(protected);

    Router::new()
        .route("/livez", get(health::livez))
        .route("/readyz", get(health::readyz))
        .route("/health", get(health::health))
        .route("/metrics", get(health::metrics))
        .nest("/api", api)
        .route("/ws", get(ws::upgrade))
        // Registered last = outermost: every response (including CORS
        // preflights and rejections) is counted and carries a request id.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::track_http,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::request_id,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Bearer-or-cookie authentication middleware. The WebSocket endpoint
/// resolves the same session from `?token=` or the cookie instead.
async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = match bearer_token(req.headers()) {
        // Bearer requests are CSRF-immune: no cookie rides along.
        Some(token) => token,
        None => {
            let token = session_cookie(req.headers()).ok_or_else(ApiError::unauthorized)?;
            if !csrf::request_is_safe(req.method(), req.headers(), &state.config.allowed_origins) {
                return Err(ApiError::forbidden("cross-site request rejected"));
            }
            token
        }
    };
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

/// First non-empty `nenchess_session` value from a `Cookie` header.
pub fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{SESSION_COOKIE}=");
    raw.split(';')
        .map(str::trim)
        .filter_map(|pair| pair.strip_prefix(prefix.as_str()))
        .map(str::to_string)
        .find(|value| !value.is_empty())
}

/// `Set-Cookie` value opening a session: `HttpOnly` keeps the token out of
/// page scripts, `SameSite=Lax` blocks cross-site cookie riding, `Secure`
/// is deployment-configurable (dev runs plain HTTP).
pub(crate) fn session_cookie_header(token: &str, ttl_days: i64, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{}",
        ttl_days.max(1) * 86_400,
        if secure { "; Secure" } else { "" }
    )
}

/// `Set-Cookie` value expiring the session cookie (logout).
pub(crate) fn cleared_session_cookie_header(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    )
}

/// `201 Created` helper for handlers.
pub fn created<T: serde::Serialize>(body: T) -> (StatusCode, axum::Json<T>) {
    (StatusCode::CREATED, axum::Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_parses_multi_cookie_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "theme=dark; nenchess_session=abc123; other=1"
                .parse()
                .unwrap(),
        );
        assert_eq!(session_cookie(&headers).as_deref(), Some("abc123"));

        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "nenchess_session=".parse().unwrap());
        assert_eq!(session_cookie(&headers), None);

        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "theme=dark".parse().unwrap());
        assert_eq!(session_cookie(&headers), None);
    }

    #[test]
    fn cookie_headers_carry_the_expected_attributes() {
        let value = session_cookie_header("tok", 14, false);
        assert!(value.starts_with("nenchess_session=tok; "));
        assert!(value.contains("HttpOnly"));
        assert!(value.contains("SameSite=Lax"));
        assert!(value.contains("Path=/"));
        assert!(value.contains("Max-Age=1209600"));
        assert!(!value.contains("Secure"));
        assert!(session_cookie_header("tok", 14, true).contains("; Secure"));

        let cleared = cleared_session_cookie_header(false);
        assert!(cleared.starts_with("nenchess_session=; "));
        assert!(cleared.contains("Max-Age=0"));
    }
}
