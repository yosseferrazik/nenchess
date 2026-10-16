//! End-to-end test harness: spawns the API in-process against a dedicated
//! PostgreSQL test database.
//!
//! Requirements:
//! * A reachable PostgreSQL. Default URL:
//!   `postgres://nenchess:nenchess@localhost:5432/nenchess`, overridable with
//!   `NENCHESS_TEST_DATABASE_URL`.
//! * The test database (`nenchess_test`) is dropped and recreated on every run.

use anyhow::Context;
use nenchess_api::{build_router, build_state, run_migrations, spawn_outbox_dispatcher, Config};

/// Connection URL of the maintenance database used to (re)create the test database.
pub fn admin_url() -> String {
    let url = std::env::var("NENCHESS_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://nenchess:nenchess@localhost:5432/nenchess".to_string());
    match url.rsplit_once('/') {
        Some((base, _)) => format!("{base}/postgres"),
        None => url,
    }
}

/// Connection URL of the dedicated test database.
pub fn test_database_url() -> String {
    let url = std::env::var("NENCHESS_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://nenchess:nenchess@localhost:5432/nenchess".to_string());
    match url.rsplit_once('/') {
        Some((base, _)) => format!("{base}/nenchess_test"),
        None => url,
    }
}

/// Drop + create the test database, run migrations, spawn the API on a random
/// port and start the outbox dispatcher. Returns the base HTTP URL.
pub async fn spawn_server() -> anyhow::Result<String> {
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .context("connecting to PostgreSQL (is it running? try `docker compose up -d postgres`)")?;

    sqlx::query("DROP DATABASE IF EXISTS nenchess_test WITH (FORCE)")
        .execute(&admin)
        .await?;
    sqlx::query("CREATE DATABASE nenchess_test")
        .execute(&admin)
        .await?;
    admin.close().await;

    let config = Config {
        database_url: test_database_url(),
        bind_addr: "127.0.0.1:0".parse()?,
        database_max_connections: 5,
        outbox_poll_interval_ms: 50,
        session_ttl_days: 1,
        rust_log: "info".to_string(),
    };

    let state = build_state(config).await?;
    run_migrations(&state.db).await?;
    spawn_outbox_dispatcher(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let router = build_router(state);
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, router).await {
            eprintln!("server error: {err}");
        }
    });

    Ok(format!("http://{addr}"))
}

/// WebSocket URL for the given base URL and session token.
pub fn ws_url(base: &str, token: &str) -> String {
    base.replacen("http://", "ws://", 1) + &format!("/ws?token={token}")
}
