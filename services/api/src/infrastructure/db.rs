use crate::config::Config;

/// Build the PostgreSQL pool.
pub async fn connect(config: &Config) -> Result<sqlx::PgPool, sqlx::Error> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(&config.database_url)
        .await
}
