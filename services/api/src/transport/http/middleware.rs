//! Cross-cutting HTTP middleware: request ids, metrics and rate limits.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Instant;

use axum::extract::{ConnectInfo, Extension, MatchedPath, Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;
use uuid::Uuid;

use crate::bootstrap::AppState;
use crate::error::ApiError;

use super::AuthUser;

/// Stable per-request id, echoed as `x-request-id` and carried by the span.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Generate (or adopt) the request id, attach a tracing span and echo the
/// id on the response. Runs outermost so even rejections are traceable.
pub async fn request_id(mut req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    req.extensions_mut().insert(RequestId(id.clone()));
    let span = tracing::info_span!("request", request_id = %id);
    let mut response = next.run(req).instrument(span).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Count requests and durations by (method, matched route, status class).
pub async fn track_http(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    // Route *template*, never the raw path: keeps label cardinality bounded.
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let started = Instant::now();
    let response = next.run(req).await;
    state.metrics.record_http(
        method.as_str(),
        &route,
        response.status().as_u16(),
        started.elapsed().as_micros() as u64,
    );
    response
}

/// Per-client-IP budget for `/api/auth/register` and `/api/auth/login`,
/// checked *before* any password hashing happens.
pub async fn rate_limit_auth(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let key = client_key(&state, &req);
    match state.auth_limiter.check(&key) {
        Ok(()) => Ok(next.run(req).await),
        Err(retry_after) => {
            state.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
            Err(ApiError::rate_limited(retry_after))
        }
    }
}

/// Per-user budget for authenticated API calls. Runs *inside*
/// `require_auth`, so the authenticated user is available as a key.
pub async fn rate_limit_api(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    match state.api_limiter.check(&user.0.id.to_string()) {
        Ok(()) => Ok(next.run(req).await),
        Err(retry_after) => {
            state.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
            Err(ApiError::rate_limited(retry_after))
        }
    }
}

/// Identify the client for rate limiting, when the deployment explicitly
/// trusts proxy headers: `X-Real-IP` first (nginx overwrites it with the
/// real peer, so a client cannot spoof it), then the **last**
/// `X-Forwarded-For` hop — `$proxy_add_x_forwarded_for` appends the actual
/// remote address there, while any client-supplied prefix stays attacker
/// controlled. Otherwise the socket peer is authoritative.
fn client_key(state: &AppState, req: &Request) -> String {
    if state.config.trust_proxy_headers {
        if let Some(real_ip) = req
            .headers()
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return real_ip.to_string();
        }
        if let Some(forwarded) = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        {
            if let Some(last_hop) = forwarded
                .rsplit(',')
                .next()
                .map(str::trim)
                .filter(|hop| !hop.is_empty())
            {
                return last_hop.to_string();
            }
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_validation() {
        assert!(valid_request_id("req-01.abc_DEF"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id(&"x".repeat(65)));
        assert!(!valid_request_id("has space"));
        assert!(!valid_request_id("evil\r\nheader"));
    }
}
