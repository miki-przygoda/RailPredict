//! Database layer.
//!
//! Exposes a single `Db` type alias (`sqlx::PgPool`) and the `connect()` function
//! that creates the pool and runs pending migrations on startup.
//!
//! Sub-modules:
//!   - `static_data`: queries for Tier A read-only tables (stations, timetable_calls, fares)
//!   - `history`:     load/flush for the Tier B delay_history table

pub mod analytics;
pub mod history;
pub mod maintenance;
pub mod operators;
pub mod overview;
pub mod predictions;
pub mod static_data;
pub mod stations;
pub mod synthetic;

use sqlx::postgres::PgPoolOptions;

pub type Db = sqlx::PgPool;

/// Create a connection pool and run all pending migrations.
///
/// `max_connections` is sourced from the `DB_MAX_CONNECTIONS` env var (default 5)
/// via `Config::db_max_connections`, allowing the pool size to be tuned per deployment
/// without recompiling.
/// Migrations live in `../migrations/` relative to the crate root at compile time.
pub async fn connect(database_url: &str, max_connections: u32) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to database: {e}"))?;

    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("Migration failed: {e}"))?;

    tracing::info!("Database connected and migrations applied");
    Ok(pool)
}
