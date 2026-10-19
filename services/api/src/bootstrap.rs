//! Bootstrap: wiring, migrations and server startup.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::infrastructure::clock::{ProcessClock, SharedClock};
use crate::infrastructure::db;
use crate::infrastructure::hub::Hub;
use crate::infrastructure::matchmaking_scheduler;
use crate::infrastructure::metrics::Metrics;
use crate::infrastructure::outbox_dispatcher;
use crate::infrastructure::rate_limit::RateLimiter;
use crate::infrastructure::registry::GameRegistry;

/// Shared, cheap-to-clone application context handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub config: Arc<Config>,
    pub clock: SharedClock,
    pub hub: Arc<Hub>,
    pub registry: Arc<GameRegistry>,
    pub metrics: Arc<Metrics>,
    /// Per-IP budget for `/api/auth/register` and `/api/auth/login`.
    pub auth_limiter: Arc<RateLimiter>,
    /// Per-user budget for authenticated API calls.
    pub api_limiter: Arc<RateLimiter>,
}

impl AppState {
    /// Build state with a custom clock (tests) and an existing pool.
    pub fn with_parts(db: sqlx::PgPool, config: Config, clock: SharedClock) -> Self {
        let window = Duration::from_secs(60);
        let auth_limiter = Arc::new(RateLimiter::new(config.rate_limit_auth_per_minute, window));
        let api_limiter = Arc::new(RateLimiter::new(config.rate_limit_api_per_minute, window));
        Self {
            db,
            config: Arc::new(config),
            clock,
            hub: Arc::new(Hub::default()),
            registry: Arc::new(GameRegistry::default()),
            metrics: Arc::new(Metrics::new()),
            auth_limiter,
            api_limiter,
        }
    }
}

/// Connect to PostgreSQL and assemble the application state.
pub async fn build_state(config: Config) -> anyhow::Result<AppState> {
    let db = db::connect(&config).await?;
    let clock: SharedClock = Arc::new(ProcessClock::new());
    Ok(AppState::with_parts(db, config, clock))
}

/// Run embedded migrations (same files used by CI and deployments).
pub async fn run_migrations(db: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations").run(db).await?;
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    crate::transport::http::build_router(state)
}

/// Start the transactional outbox dispatcher (post-commit publication).
pub fn spawn_outbox_dispatcher(state: AppState) -> JoinHandle<()> {
    outbox_dispatcher::spawn(state)
}

/// Start the matchmaking scheduler (single writer for pairings).
pub fn spawn_matchmaking_scheduler(state: AppState) -> JoinHandle<()> {
    matchmaking_scheduler::spawn(state)
}

/// Entry point used by `main`: connect, migrate, dispatch, serve.
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let state = build_state(config).await?;
    run_migrations(&state.db).await?;
    spawn_outbox_dispatcher(state.clone());
    spawn_matchmaking_scheduler(state.clone());

    let listener = TcpListener::bind(state.config.bind_addr).await?;
    let addr = listener.local_addr()?;
    tracing::info!(%addr, "nenchess api listening");
    axum::serve(
        listener,
        build_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
