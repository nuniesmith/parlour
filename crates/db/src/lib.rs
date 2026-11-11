//! Parlour database layer.
//!
//! Owns the sqlx connection pool and runs the embedded migrations
//! from `/migrations` at the repo root (timestamped, append-only,
//! never renumber). Everything database-shaped goes through this
//! crate so the rest of the workspace never touches sqlx directly.
//!
//! No Redis, no second store: Postgres also backs the job queue
//! (see `parlour-jobs`).

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
/// Re-exported so callers can name database errors without
/// depending on sqlx themselves.
pub use sqlx::Error as DbError;
pub use sqlx::PgPool;

/// Name of the environment variable the server reads for the
/// Postgres connection string.
pub const DATABASE_URL_ENV: &str = "PARLOUR_DATABASE_URL";

fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
}

/// Connect to Postgres, waiting until the first connection works.
/// Fails if the database is unreachable.
pub async fn connect(database_url: &str) -> Result<PgPool, DbError> {
    pool_options().connect(database_url).await
}

/// Build a pool without connecting (tests, tooling). Queries will
/// fail until the database is reachable.
pub fn connect_lazy(database_url: &str) -> Result<PgPool, DbError> {
    pool_options().connect_lazy(database_url)
}

/// Run every pending migration from the repo-root `migrations/`
/// folder. Idempotent: sqlx tracks applied migrations itself.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    tracing::info!("running database migrations");
    sqlx::migrate!("../../migrations").run(pool).await?;
    tracing::info!("database migrations up to date");
    Ok(())
}

/// Is the database answering? Used by `/healthz` — it must tell
/// the truth, so this runs a real query, not a cached flag.
pub async fn ping(pool: &PgPool) -> bool {
    sqlx::query("SELECT 1").execute(pool).await.is_ok()
}
