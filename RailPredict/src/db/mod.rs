//! Database layer.
//!
//! Exposes a single `Db` type alias (`sqlx::PgPool`) and the `connect()` function
//! that creates the pool and runs pending migrations on startup.
//!
//! Sub-modules:
//!   - `static_data`: queries for Tier A read-only tables (stations, timetable_calls, fares)
//!   - `history`:     load/flush for the Tier B delay_history table

pub mod history;
pub mod static_data;

use sqlx::postgres::PgPoolOptions;

pub type Db = sqlx::PgPool;

/// Create a connection pool and run all pending migrations.
///
/// Capped at 10 connections — sufficient for the single-process deployment model.
/// Migrations live in `../migrations/` relative to the crate root at compile time.
pub async fn connect(database_url: &str) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
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
