use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use protocol::{ErrorCode, ErrorDto, ServerMessage};
use uuid::Uuid;

use crate::domain::DomainError;

/// Every API failure surfaces as an `ErrorDto` over HTTP and as an `error`
/// frame over WebSocket. The domain never constructs HTTP types; the mapping
/// lives here, at the edge.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    pub code: ErrorCode,
    pub status: StatusCode,
    pub message: String,
    /// `Retry-After` hint for429 responses, in seconds.
    pub retry_after_secs: Option<u64>,
}

impl ApiError {
    pub fn from_code(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            status: status_for(code),
            message: message.into(),
            retry_after_secs: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::BadRequest, message)
    }

    pub fn unauthorized() -> Self {
        Self::from_code(ErrorCode::Unauthorized, "authentication required")
    }

    /// 401 with a specific message (e.g. failed login).
    pub fn unauthorized_with(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::Unauthorized, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::Forbidden, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::Conflict, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::Internal, message)
    }

    /// 429 with a `Retry-After` hint (seconds until the fixed window resets).
    pub fn rate_limited(retry_after_secs: u64) -> Self {
        Self {
            retry_after_secs: Some(retry_after_secs.max(1)),
            ..Self::from_code(
                ErrorCode::RateLimited,
                format!("too many requests, retry in {}s", retry_after_secs.max(1)),
            )
        }
    }

    /// Wire representation used by the WebSocket layer.
    pub fn to_ws_message(&self, command_id: Option<Uuid>) -> ServerMessage {
        ServerMessage::Error {
            command_id,
            code: self.code,
            message: self.message.clone(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::error!(code = ?self.code, message = %self.message, "request failed");
        }
        let retry_after = self
            .retry_after_secs
            .and_then(|secs| HeaderValue::from_str(&secs.to_string()).ok());
        let mut response = (
            self.status,
            Json(ErrorDto {
                code: self.code,
                message: self.message,
            }),
        )
            .into_response();
        if let Some(value) = retry_after {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                ApiError::conflict("resource already exists")
            }
            sqlx::Error::RowNotFound => ApiError::not_found("resource not found"),
            other => {
                tracing::error!(error = %other, "database error");
                ApiError::internal("database error")
            }
        }
    }
}

impl From<DomainError> for ApiError {
    fn from(err: DomainError) -> Self {
        let message = err.to_string();
        let code = match &err {
            DomainError::AwaitingOpponent => ErrorCode::BadRequest,
            DomainError::GameOver => ErrorCode::GameOver,
            DomainError::VersionMismatch { .. } => ErrorCode::VersionConflict,
            DomainError::NotParticipant => ErrorCode::Forbidden,
            DomainError::NotYourTurn => ErrorCode::NotYourTurn,
            DomainError::GameFull => ErrorCode::GameFull,
            DomainError::AlreadySeated => ErrorCode::BadRequest,
            DomainError::IllegalMove(_) => ErrorCode::IllegalMove,
        };
        ApiError::from_code(code, message)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        tracing::error!(error = %err, "serialization error");
        ApiError::internal("serialization error")
    }
}

fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::BadRequest | ErrorCode::IllegalMove => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict
        | ErrorCode::VersionConflict
        | ErrorCode::NotYourTurn
        | ErrorCode::GameOver
        | ErrorCode::GameFull => StatusCode::CONFLICT,
        ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
