//! Emby cutover admin surface (Milestone 1.7).
//!
//! - `GET /api/v1/admin/emby/status` — configured? reachable?
//!   which users does Emby see?
//! - `POST /api/v1/admin/emby/import` — enqueue an import job
//!   for one Emby user → one Parlour profile, dry-run first.
//!
//! The job itself runs in the background worker and lands its
//! ImportReport as the job result (see the Jobs admin tab).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use parlour_catalog::{EmbyClient, EmbySettings};

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;

#[derive(Serialize)]
struct EmbyUserDto {
    id: String,
    name: String,
}

#[derive(Serialize)]
struct EmbyStatus {
    configured: bool,
    reachable: bool,
    error: Option<String>,
    users: Vec<EmbyUserDto>,
}

async fn status(
    State(app): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<EmbyStatus>, AppError> {
    if !app.emby.configured() {
        return Ok(Json(EmbyStatus {
            configured: false,
            reachable: false,
            error: None,
            users: Vec::new(),
        }));
    }
    match EmbyClient::new(&app.emby) {
        Ok(client) => match client.list_users().await {
            Ok(users) => Ok(Json(EmbyStatus {
                configured: true,
                reachable: true,
                error: None,
                users: users
                    .into_iter()
                    .map(|u| EmbyUserDto {
                        id: u.id,
                        name: u.name,
                    })
                    .collect(),
            })),
            Err(e) => Ok(Json(EmbyStatus {
                configured: true,
                reachable: false,
                error: Some(e.to_string()),
                users: Vec::new(),
            })),
        },
        Err(e) => Ok(Json(EmbyStatus {
            configured: true,
            reachable: false,
            error: Some(e.to_string()),
            users: Vec::new(),
        })),
    }
}

#[derive(Serialize, sqlx::FromRow)]
struct ProfileOption {
    id: Uuid,
    name: String,
    username: String,
}

/// Every profile in the household, for the mapping screen (the
/// import maps an Emby user to *any* Parlour profile).
async fn profiles(
    State(app): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<ProfileOption>>, AppError> {
    let rows = sqlx::query_as::<_, ProfileOption>(
        "SELECT p.id, p.name, u.username FROM profiles p
         JOIN users u ON u.id = p.user_id ORDER BY u.username, p.name",
    )
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct ImportBody {
    emby_user_id: String,
    profile_id: Uuid,
    dry_run: Option<bool>,
}

async fn start_import(
    State(app): State<AppState>,
    _admin: AdminUser,
    Json(body): Json<ImportBody>,
) -> Result<(axum::http::StatusCode, Json<parlour_jobs::Job>), AppError> {
    if !app.emby.configured() {
        return Err(AppError::BadRequest(
            "Emby is not configured on the server (PARLOUR_EMBY_URL / PARLOUR_EMBY_API_KEY)".into(),
        ));
    }
    // The target profile must exist (it may belong to any user —
    // mapping Jordan's Emby user to Kayla's profile is the point
    // of the manual mapping screen).
    let exists: Option<(bool,)> = sqlx::query_as("SELECT true FROM profiles WHERE id = $1")
        .bind(body.profile_id)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?;
    if exists.is_none() {
        return Err(AppError::BadRequest("no such profile".into()));
    }
    let job_id = parlour_jobs::enqueue(
        &app.pool,
        parlour_jobs::JobKind::ImportEmby,
        None,
        serde_json::json!({
            "emby_user_id": body.emby_user_id,
            "profile_id": body.profile_id,
            "dry_run": body.dry_run.unwrap_or(true),
        }),
    )
    .await
    .map_err(AppError::from)?;
    let job = parlour_jobs::get(&app.pool, job_id)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::Internal("job vanished after enqueue".into()))?;
    Ok((axum::http::StatusCode::ACCEPTED, Json(job)))
}

/// Worker entry point (called from the catalog background loop).
pub async fn run_emby_job_claimed(
    pool: parlour_db::PgPool,
    job: parlour_jobs::Job,
    settings: &EmbySettings,
) {
    let payload = &job.payload;
    let emby_user_id = payload["emby_user_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let profile_id = payload["profile_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok());
    let dry_run = payload["dry_run"].as_bool().unwrap_or(true);
    let Some(profile_id) = profile_id else {
        let _ = parlour_jobs::fail(&pool, job.id, "import job without a valid profile_id").await;
        return;
    };
    let client = match EmbyClient::new(settings) {
        Ok(c) => c,
        Err(e) => {
            let _ = parlour_jobs::fail(&pool, job.id, &e.to_string()).await;
            return;
        }
    };
    match parlour_catalog::run_import(&pool, &client, &emby_user_id, profile_id, dry_run).await {
        Ok(report) => {
            let _ = parlour_jobs::set_result(
                &pool,
                job.id,
                serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            )
            .await;
            tracing::info!(
                job_id = %job.id, matched = report.matched, unmatched = report.unmatched,
                ambiguous = report.ambiguous, dry_run, "emby import finished"
            );
            let _ = parlour_jobs::complete(&pool, job.id).await;
            crate::activity::record(
                &pool,
                crate::activity::Entry::system(
                    "import.completed",
                    if dry_run {
                        "Emby import dry run finished"
                    } else {
                        "Emby import finished"
                    },
                )
                .profile(profile_id)
                .detail(serde_json::json!({
                    "job_id": job.id,
                    "dry_run": dry_run,
                    "matched": report.matched,
                    "unmatched": report.unmatched,
                    "ambiguous": report.ambiguous,
                })),
            )
            .await;
        }
        Err(e) => {
            let _ = parlour_jobs::fail(&pool, job.id, &e.to_string()).await;
            crate::activity::record(
                &pool,
                crate::activity::Entry::system("import.failed", "Emby import failed")
                    .profile(profile_id)
                    .detail(serde_json::json!({ "job_id": job.id, "dry_run": dry_run })),
            )
            .await;
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/emby/status", get(status))
        .route("/api/v1/admin/emby/profiles", get(profiles))
        .route("/api/v1/admin/emby/import", post(start_import))
}
