//! Parlour background jobs (Milestone 1.3, task C6).
//!
//! A Postgres-backed job table claimed with
//! `FOR UPDATE SKIP LOCKED`. No Redis until Postgres demonstrably
//! cannot keep up (it will, at household scale).
//!
//! Jobs are how scanning follow-ups, subtitle extraction and
//! downloads, thumbnail/artwork generation, and request
//! reconciliation stay out of request paths. The row itself carries
//! progress and per-file errors so the admin UI can show a running
//! scan truthfully instead of spinning.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

/// A kind of background job. Stored as text (`as_str`) so new kinds
/// do not need a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    ScanLibrary,
    FetchMetadata,
    DownloadArtwork,
    ExtractSubtitles,
    DownloadSubtitles,
    GenerateArtwork,
    ReconcileRequests,
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::ScanLibrary => "scan_library",
            JobKind::FetchMetadata => "fetch_metadata",
            JobKind::DownloadArtwork => "download_artwork",
            JobKind::ExtractSubtitles => "extract_subtitles",
            JobKind::DownloadSubtitles => "download_subtitles",
            JobKind::GenerateArtwork => "generate_artwork",
            JobKind::ReconcileRequests => "reconcile_requests",
        }
    }
}

/// One row of the `jobs` table.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub library_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub status: String,
    pub progress_done: i32,
    pub progress_total: i32,
    pub errors: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Queue a job and return its id.
pub async fn enqueue(
    pool: &parlour_db::PgPool,
    kind: JobKind,
    library_id: Option<Uuid>,
    payload: serde_json::Value,
) -> Result<Uuid, sqlx::Error> {
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO jobs (kind, library_id, payload) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(kind.as_str())
    .bind(library_id)
    .bind(payload)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Atomically claim one specific queued job (used when the caller
/// just enqueued it and wants to run it now). Returns false when
/// someone else already claimed it.
pub async fn claim_by_id(pool: &parlour_db::PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE jobs SET status = 'running', started_at = now(), finished_at = NULL
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Claim the oldest queued job of a kind, if any. SKIP LOCKED means
/// two workers never take the same row.
pub async fn claim_next(
    pool: &parlour_db::PgPool,
    kind: JobKind,
) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query_as::<_, Job>(
        "UPDATE jobs SET status = 'running', started_at = now(), finished_at = NULL
         WHERE id = (
             SELECT id FROM jobs
             WHERE status = 'queued' AND kind = $1
             ORDER BY created_at
             FOR UPDATE SKIP LOCKED
             LIMIT 1
         )
         RETURNING id, kind, library_id, payload, status, progress_done,
                   progress_total, errors, result, created_at, started_at, finished_at",
    )
    .bind(kind.as_str())
    .fetch_optional(pool)
    .await
}

pub async fn set_progress(
    pool: &parlour_db::PgPool,
    id: Uuid,
    done: i32,
    total: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE jobs SET progress_done = $2, progress_total = $3 WHERE id = $1")
        .bind(id)
        .bind(done)
        .bind(total)
        .execute(pool)
        .await?;
    Ok(())
}

/// Append one error entry (`{path?, message}`) to a job's error list.
pub async fn add_error(
    pool: &parlour_db::PgPool,
    id: Uuid,
    path: Option<&str>,
    message: &str,
) -> Result<(), sqlx::Error> {
    let entry = serde_json::json!({ "path": path, "message": message });
    sqlx::query("UPDATE jobs SET errors = errors || $2::jsonb WHERE id = $1")
        .bind(id)
        .bind(serde_json::json!([entry]))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn complete(pool: &parlour_db::PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE jobs SET status = 'succeeded', finished_at = now(),
                progress_done = progress_total
         WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a finished job's JSON summary (counts an admin can read).
pub async fn set_result(
    pool: &parlour_db::PgPool,
    id: Uuid,
    result: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE jobs SET result = $2 WHERE id = $1")
        .bind(id)
        .bind(result)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn fail(pool: &parlour_db::PgPool, id: Uuid, message: &str) -> Result<(), sqlx::Error> {
    add_error(pool, id, None, message).await?;
    sqlx::query("UPDATE jobs SET status = 'failed', finished_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get(pool: &parlour_db::PgPool, id: Uuid) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query_as::<_, Job>(
        "SELECT id, kind, library_id, payload, status, progress_done, progress_total,
                errors, result, created_at, started_at, finished_at
         FROM jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list_recent(
    pool: &parlour_db::PgPool,
    library_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Job>, sqlx::Error> {
    sqlx::query_as::<_, Job>(
        "SELECT id, kind, library_id, payload, status, progress_done, progress_total,
                errors, result, created_at, started_at, finished_at
         FROM jobs
         WHERE ($1::uuid IS NULL OR library_id = $1)
         ORDER BY created_at DESC
         LIMIT $2",
    )
    .bind(library_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Has a job of this kind already been queued/started since `since`?
/// The nightly safety net uses this so it fires once a day, not
/// once per scheduler tick.
pub async fn exists_since(
    pool: &parlour_db::PgPool,
    kind: JobKind,
    library_id: Option<Uuid>,
    since: DateTime<Utc>,
    payload_marker: &str,
) -> Result<bool, sqlx::Error> {
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS (
             SELECT 1 FROM jobs
             WHERE kind = $1 AND created_at >= $3
               AND ($2::uuid IS NULL OR library_id = $2)
               AND payload ->> 'trigger' = $4
         )",
    )
    .bind(kind.as_str())
    .bind(library_id)
    .bind(since)
    .bind(payload_marker)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_kind_strings_are_stable() {
        assert_eq!(JobKind::ScanLibrary.as_str(), "scan_library");
        assert_eq!(JobKind::GenerateArtwork.as_str(), "generate_artwork");
    }
}
