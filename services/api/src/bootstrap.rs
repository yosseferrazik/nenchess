//! Bootstrap: wiring, migrations and server startup.

use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::infrastructure::clock::{ProcessClock, SharedClock};
use crate::infrastructure::db;
use crate::infrastructure::hub::Hub;
use crate::infrastructure::outbox_dispatcher;
use crate::infrastructure::registry::GameRegistry;

/// Shared, cheap-to-clone application context handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub config: Arc<Config>,
    pub clock: SharedClock,
    pub hub: Arc<Hub>,
    pub registry: Arc<GameRegistry>,
}

impl AppState {
    /// Build state with a custom clock (tests) and an existing pool.
    pub fn with_parts(db: sqlx::PgPool, config: Config, clock: SharedClock) -> Self {
        Self {
            db,
            config: Arc::new(config),
            clock,
            hub: Arc::new(Hub::default()),
            registry: Arc::new(GameRegistry::default()),
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

/// Entry point used by `main`: connect, migrate, dispatch, serve.
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let state = build_state(config).await?;
    run_migrations(&state.db).await?;
    spawn_outbox_dispatcher(state.clone());

    let listener = TcpListener::bind(state.config.bind_addr).await?;
    let addr = listener.local_addr()?;
    tracing::info!(%addr, "nenchess api listening");
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
