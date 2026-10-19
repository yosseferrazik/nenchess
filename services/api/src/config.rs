use std::net::SocketAddr;

/// Typed application configuration, read once from the environment at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// PostgreSQL connection string. Required.
    pub database_url: String,
    /// Address the HTTP/WebSocket server binds to.
    pub bind_addr: SocketAddr,
    /// Maximum connections in the PostgreSQL pool.
    pub database_max_connections: u32,
    /// How often the transactional outbox dispatcher runs (ms).
    pub outbox_poll_interval_ms: u64,
    /// How often the matchmaking scheduler attempts to pair players (ms).
    pub matchmaking_interval_ms: u64,
    /// Session lifetime in days.
    pub session_ttl_days: i64,
    /// `tracing` filter directives, e.g. `info,nenchess_api=debug`.
    pub rust_log: String,
    /// Login + register requests allowed per minute per client IP (0 disables).
    pub rate_limit_auth_per_minute: u32,
    /// Authenticated API requests per minute per user (0 disables).
    pub rate_limit_api_per_minute: u32,
    /// WebSocket commands per minute per connection (0 disables).
    pub rate_limit_ws_per_minute: u32,
    /// Emit `Secure` on session cookies (enable behind HTTPS).
    pub cookie_secure: bool,
    /// Trust `X-Real-IP`/`X-Forwarded-For` when identifying clients.
    pub trust_proxy_headers: bool,
    /// Origins accepted on cookie-authenticated cross-checks (`Origin`
    /// header), in addition to the request host itself. CSV via
    /// `ALLOWED_ORIGINS`.
    pub allowed_origins: Vec<String>,
}

/// Development origins accepted by default; deployments override with
/// `ALLOWED_ORIGINS`.
const DEFAULT_ALLOWED_ORIGINS: &str =
    "http://localhost:5173,http://localhost:8081,http://127.0.0.1:5173,http://127.0.0.1:8081";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable `{0}`")]
    Missing(&'static str),
    #[error("invalid value for `{key}`: `{value}`")]
    Invalid { key: &'static str, value: String },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| ConfigError::Missing("DATABASE_URL"))?;

        Ok(Self {
            database_url,
            bind_addr: parse("BIND_ADDR", "127.0.0.1:8080")?,
            database_max_connections: parse("DATABASE_MAX_CONNECTIONS", "10")?,
            outbox_poll_interval_ms: parse("OUTBOX_POLL_INTERVAL_MS", "200")?,
            matchmaking_interval_ms: parse("MATCHMAKING_INTERVAL_MS", "1000")?,
            session_ttl_days: parse("SESSION_TTL_DAYS", "14")?,
            rust_log: std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
            rate_limit_auth_per_minute: parse("RATE_LIMIT_AUTH_PER_MINUTE", "10")?,
            rate_limit_api_per_minute: parse("RATE_LIMIT_API_PER_MINUTE", "0")?,
            rate_limit_ws_per_minute: parse("RATE_LIMIT_WS_PER_MINUTE", "120")?,
            cookie_secure: parse("COOKIE_SECURE", "false")?,
            trust_proxy_headers: parse("TRUST_PROXY_HEADERS", "false")?,
            allowed_origins: std::env::var("ALLOWED_ORIGINS")
                .unwrap_or_else(|_| DEFAULT_ALLOWED_ORIGINS.to_string())
                .split(',')
                .map(|origin| origin.trim().to_string())
                .filter(|origin| !origin.is_empty())
                .collect(),
        })
    }
}

fn parse<T: std::str::FromStr>(key: &'static str, default: &str) -> Result<T, ConfigError> {
    let raw = std::env::var(key).unwrap_or_else(|_| default.to_string());
    raw.parse::<T>()
        .map_err(|_| ConfigError::Invalid { key, value: raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_errors_are_readable() {
        let err = ConfigError::Missing("DATABASE_URL");
        assert!(err.to_string().contains("DATABASE_URL"));
        let err = ConfigError::Invalid {
            key: "BIND_ADDR",
            value: "nope".into(),
        };
        assert!(err.to_string().contains("BIND_ADDR"));
    }

    #[test]
    fn default_allowed_origins_parse_as_csv() {
        let origins: Vec<String> = DEFAULT_ALLOWED_ORIGINS
            .split(',')
            .map(|origin| origin.trim().to_string())
            .filter(|origin| !origin.is_empty())
            .collect();
        assert_eq!(origins.len(), 4);
        assert!(origins.contains(&"http://localhost:5173".to_string()));
        assert!(origins.contains(&"http://127.0.0.1:8081".to_string()));
    }
}
