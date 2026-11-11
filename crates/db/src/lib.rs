//! Parlour database layer (placeholder).
//!
//! Planned: a sqlx `PgPool` created in the server binary and passed
//! down, plus `sqlx::migrate!()` over the SQL files in `/migrations`
//! at the repo root (timestamped, append-only, never renumbered).
//!
//! No Redis, no second store: Postgres also backs the job queue
//! (see `parlour-jobs`).

/// Name of the environment variable the server reads for the
/// Postgres connection string.
pub const DATABASE_URL_ENV: &str = "PARLOUR_DATABASE_URL";
