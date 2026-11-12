//! Server notices / Alerts (task UI5).
//!
//! A notice is a condition an admin should know about. Notices
//! are *reconciled*, not pushed: every minute `reconcile` looks at
//! the real state — failed jobs, the metadata-review backlog, free
//! space where Parlour writes — raises a notice for each condition
//! that holds and resolves the ones that no longer do. That keeps
//! every producer honest without each failure site having to
//! remember to report itself.
//!
//! Decided with the owner (D16): the disk check covers only the
//! folders Parlour writes to (cache and artwork) — a nearly full
//! media drive is normal — warning below 5 GB free, error below
//! 1 GB. A "restart to apply" notice is supported
//! (`raise_restart_required`) but nothing raises it yet: all
//! configuration is read from the environment at startup, so the
//! running server cannot know it changed.
//!
//! - `GET /api/v1/admin/notices` — active notices this admin has
//!   not dismissed, newest first
//! - `POST /api/v1/admin/notices/{id}/dismiss` — per-admin dismiss

use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;

const GIB: u64 = 1024 * 1024 * 1024;
/// Below this much free space a write folder gets a warning (D16).
pub const DISK_WARNING_BYTES: u64 = 5 * GIB;
/// Below this much it is an error (D16).
pub const DISK_ERROR_BYTES: u64 = GIB;
/// Failed jobs older than this no longer raise a notice.
const JOB_FAILURE_WINDOW_DAYS: i32 = 7;
/// Resolved notices, and old failed-job notices, are deleted after this.
const NOTICE_KEEP_DAYS: i32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// The folders Parlour writes to, with the name the notice uses.
#[derive(Debug, Clone, Default)]
pub struct NoticeSettings {
    pub write_dirs: Vec<(String, PathBuf)>,
}

impl NoticeSettings {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            write_dirs: vec![
                ("cache".to_string(), config.cache_dir.clone()),
                ("artwork".to_string(), config.artwork_dir.clone()),
            ],
        }
    }
}

/// One free-space measurement. `free_bytes` is `None` when the
/// folder could not be measured (missing, or an unsupported
/// platform) — that is not treated as "low".
#[derive(Debug, Clone)]
pub struct DiskReading {
    pub label: String,
    pub path: PathBuf,
    pub free_bytes: Option<u64>,
}

/// Space available to this process on the volume holding `path`.
#[cfg(unix)]
// The statvfs field widths differ between platforms.
#[allow(clippy::unnecessary_cast)]
pub fn free_space(path: &FsPath) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is a valid NUL-terminated string and `stat`
    // points to writable memory of the right size; statvfs fills
    // it in completely when it returns 0.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so `stat` is initialised.
    let stat = unsafe { stat.assume_init() };
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

#[cfg(not(unix))]
pub fn free_space(_path: &FsPath) -> Option<u64> {
    None
}

/// What a free-space figure means, if anything.
pub fn disk_severity(free_bytes: u64) -> Option<Severity> {
    if free_bytes < DISK_ERROR_BYTES {
        Some(Severity::Error)
    } else if free_bytes < DISK_WARNING_BYTES {
        Some(Severity::Warning)
    } else {
        None
    }
}

fn human_size(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    } else {
        format!("{} MB", bytes / (1024 * 1024))
    }
}

/// How a failed job reads in a sentence.
fn failed_job_summary(kind: &str, library: Option<&str>) -> String {
    match (kind, library) {
        ("scan_library", Some(name)) => format!("Scan of {name} failed"),
        ("scan_library", None) => "A library scan failed".to_string(),
        ("fetch_metadata", Some(name)) => format!("Metadata fetch for {name} failed"),
        ("fetch_metadata", None) => "A metadata fetch failed".to_string(),
        ("download_artwork", _) => "An artwork download failed".to_string(),
        ("import_emby", _) => "Emby import failed".to_string(),
        ("purge_activity", _) => "Activity clean-up failed".to_string(),
        (other, _) => format!("A {} job failed", other.replace('_', " ")),
    }
}

/// Raise (or refresh) the notice for `key`. A notice that had
/// been resolved, or whose severity changed, comes back as new:
/// earlier dismissals no longer hide it.
pub async fn raise(
    pool: &parlour_db::PgPool,
    key: &str,
    kind: &str,
    severity: Severity,
    summary: &str,
    detail: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM notice_dismissals d USING notices n
         WHERE n.id = d.notice_id AND n.key = $1
           AND (n.resolved_at IS NOT NULL OR n.severity <> $2)",
    )
    .bind(key)
    .bind(severity.as_str())
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO notices (key, kind, severity, summary, detail)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (key) DO UPDATE SET
            created_at = CASE WHEN notices.resolved_at IS NOT NULL
                                OR notices.severity <> EXCLUDED.severity
                              THEN now() ELSE notices.created_at END,
            kind = EXCLUDED.kind,
            severity = EXCLUDED.severity,
            summary = EXCLUDED.summary,
            detail = EXCLUDED.detail,
            updated_at = now(),
            resolved_at = NULL",
    )
    .bind(key)
    .bind(kind)
    .bind(severity.as_str())
    .bind(summary)
    .bind(detail)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark the condition behind `key` as over.
pub async fn resolve(pool: &parlour_db::PgPool, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE notices SET resolved_at = now() WHERE key = $1 AND resolved_at IS NULL")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// "Restart the server to apply this." Cleared at the next start.
/// Nothing calls this yet (D16): it is here for the first setting
/// that can change while the server runs.
pub async fn raise_restart_required(
    pool: &parlour_db::PgPool,
    reason: &str,
) -> Result<(), sqlx::Error> {
    raise(
        pool,
        "restart_required",
        "restart_required",
        Severity::Warning,
        "Restart the server to finish applying changes",
        serde_json::json!({ "reason": reason }),
    )
    .await
}

/// Bring the notices in line with reality, measuring the disks.
pub async fn reconcile(
    pool: &parlour_db::PgPool,
    settings: &NoticeSettings,
) -> Result<(), sqlx::Error> {
    let disks: Vec<DiskReading> = settings
        .write_dirs
        .iter()
        .map(|(label, path)| DiskReading {
            label: label.clone(),
            path: path.clone(),
            free_bytes: free_space(path),
        })
        .collect();
    reconcile_with(pool, &disks).await
}

/// `reconcile` with the disk measurements supplied (tests).
pub async fn reconcile_with(
    pool: &parlour_db::PgPool,
    disks: &[DiskReading],
) -> Result<(), sqlx::Error> {
    // 1. Failed jobs that have no notice yet.
    let failed: Vec<(Uuid, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT j.id, j.kind, l.name, j.errors -> -1 ->> 'message'
         FROM jobs j
         LEFT JOIN libraries l ON l.id = j.library_id
         WHERE j.status = 'failed'
           AND j.finished_at > now() - make_interval(days => $1)
           AND NOT EXISTS (SELECT 1 FROM notices n WHERE n.key = 'job_failed:' || j.id::text)
         ORDER BY j.finished_at",
    )
    .bind(JOB_FAILURE_WINDOW_DAYS)
    .fetch_all(pool)
    .await?;
    for (job_id, kind, library, message) in failed {
        raise(
            pool,
            &format!("job_failed:{job_id}"),
            "job_failed",
            Severity::Error,
            &failed_job_summary(&kind, library.as_deref()),
            serde_json::json!({ "job_id": job_id, "job_kind": kind, "error": message }),
        )
        .await?;
    }

    // 2. Metadata-review backlog: one rolling notice.
    let (waiting,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM media_items WHERE metadata_status = 'needs_review'")
            .fetch_one(pool)
            .await?;
    if waiting > 0 {
        let summary = if waiting == 1 {
            "1 item needs a metadata match".to_string()
        } else {
            format!("{waiting} items need a metadata match")
        };
        raise(
            pool,
            "metadata_review",
            "metadata_review",
            Severity::Info,
            &summary,
            serde_json::json!({ "count": waiting }),
        )
        .await?;
    } else {
        resolve(pool, "metadata_review").await?;
    }

    // 3. Free space where Parlour writes.
    for disk in disks {
        let key = format!("disk_low:{}", disk.label);
        match disk
            .free_bytes
            .and_then(|free| disk_severity(free).map(|s| (free, s)))
        {
            Some((free, severity)) => {
                raise(
                    pool,
                    &key,
                    "disk_low",
                    severity,
                    &format!(
                        "Low disk space for the {} folder: {} free",
                        disk.label,
                        human_size(free)
                    ),
                    serde_json::json!({
                        "folder": disk.label,
                        "path": disk.path.to_string_lossy(),
                        "free_bytes": free,
                    }),
                )
                .await?;
            }
            None => resolve(pool, &key).await?,
        }
    }

    // 4. Housekeeping.
    sqlx::query(
        "DELETE FROM notices
         WHERE resolved_at < now() - make_interval(days => $1)
            OR (kind = 'job_failed' AND created_at < now() - make_interval(days => $1))",
    )
    .bind(NOTICE_KEEP_DAYS)
    .execute(pool)
    .await?;
    Ok(())
}

/// Start the reconcile loop (once a minute, first pass at once).
/// A restart has just happened, so any "restart required" notice
/// is resolved first.
pub fn spawn(pool: parlour_db::PgPool, settings: NoticeSettings) {
    tokio::spawn(async move {
        if let Err(e) = resolve(&pool, "restart_required").await {
            tracing::warn!(error = %e, "could not clear the restart notice");
        }
        loop {
            if let Err(e) = reconcile(&pool, &settings).await {
                tracing::warn!(error = %e, "notice reconcile failed");
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

#[derive(Debug, Serialize, FromRow)]
struct NoticeDto {
    id: Uuid,
    kind: String,
    severity: String,
    summary: String,
    detail: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

async fn list_notices(
    State(state): State<AppState>,
    admin: AdminUser,
) -> Result<Json<Vec<NoticeDto>>, AppError> {
    let rows = sqlx::query_as::<_, NoticeDto>(
        "SELECT n.id, n.kind, n.severity, n.summary, n.detail, n.created_at, n.updated_at
         FROM notices n
         WHERE n.resolved_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM notice_dismissals d
                           WHERE d.notice_id = n.id AND d.user_id = $1)
         ORDER BY n.created_at DESC, n.id
         LIMIT 50",
    )
    .bind(admin.0.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(rows))
}

async fn dismiss_notice(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::NotFound("no such notice".into()))?;
    let done = sqlx::query(
        "INSERT INTO notice_dismissals (notice_id, user_id)
         SELECT n.id, $2 FROM notices n WHERE n.id = $1
         ON CONFLICT (notice_id, user_id) DO UPDATE SET dismissed_at = now()",
    )
    .bind(id)
    .bind(admin.0.user_id)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound("no such notice".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/notices", get(list_notices))
        .route("/api/v1/admin/notices/{id}/dismiss", post(dismiss_notice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_thresholds_follow_d16() {
        assert_eq!(disk_severity(0), Some(Severity::Error));
        assert_eq!(disk_severity(GIB - 1), Some(Severity::Error));
        assert_eq!(disk_severity(GIB), Some(Severity::Warning));
        assert_eq!(disk_severity(5 * GIB - 1), Some(Severity::Warning));
        assert_eq!(disk_severity(5 * GIB), None);
        assert_eq!(disk_severity(500 * GIB), None);
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(human_size(3 * GIB + GIB / 2), "3.5 GB");
        assert_eq!(human_size(300 * 1024 * 1024), "300 MB");
    }

    #[test]
    fn failed_jobs_read_as_sentences() {
        assert_eq!(
            failed_job_summary("scan_library", Some("Movies")),
            "Scan of Movies failed"
        );
        assert_eq!(
            failed_job_summary("import_emby", None),
            "Emby import failed"
        );
        assert_eq!(
            failed_job_summary("reconcile_requests", None),
            "A reconcile requests job failed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn free_space_measures_a_real_folder_and_not_a_missing_one() {
        assert!(free_space(&std::env::temp_dir()).is_some());
        assert_eq!(
            free_space(FsPath::new("/definitely/not/a/real/parlour/path")),
            None
        );
    }
}
