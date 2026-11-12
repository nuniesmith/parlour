//! Server Dashboard facts (task UI2): `GET /api/v1/admin/server`.
//!
//! One admin-only read that the Dashboard page renders as-is:
//! server name, version, listener port, whether the database
//! answers, the LAN / tailnet / public addresses **as configured**
//! (never derived or guessed — unset stays `null`), and a
//! job-queue snapshot.
//!
//! Restart / power actions are deliberately absent: the process
//! has no supervisor contract it can rely on (the compose file
//! sets no restart policy), so an in-app "restart" would really
//! be "stop". The payload says so via `actions`.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AdminUser;
use crate::config::Config;
use crate::error::AppError;
use crate::http::AppState;

/// What `main` copies out of `Config` for the Dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    pub name: String,
    pub port: u16,
    pub url_lan: Option<String>,
    pub url_tailnet: Option<String>,
    pub url_public: Option<String>,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            name: "Parlour".to_string(),
            port: 8585,
            url_lan: None,
            url_tailnet: None,
            url_public: None,
        }
    }
}

impl ServerSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            name: config.server_name.clone(),
            port: config.bind_addr.port(),
            url_lan: config.url_lan.clone(),
            url_tailnet: config.url_tailnet.clone(),
            url_public: config.url_public.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct AccessUrls {
    lan: Option<String>,
    tailnet: Option<String>,
    public: Option<String>,
}

#[derive(Debug, Serialize)]
struct JobSnapshot {
    queued: i64,
    running: i64,
    /// Jobs that failed in the last 24 hours.
    failed_recent: i64,
    /// Jobs that succeeded in the last 24 hours.
    succeeded_recent: i64,
}

#[derive(Debug, Serialize)]
struct Actions {
    restart: bool,
    shutdown: bool,
}

#[derive(Debug, Serialize)]
struct ServerInfo {
    name: String,
    version: &'static str,
    port: u16,
    /// "ok" or "unreachable" — the same real query `/healthz` runs.
    db: &'static str,
    urls: AccessUrls,
    /// `null` when the database did not answer.
    jobs: Option<JobSnapshot>,
    actions: Actions,
}

async fn job_snapshot(pool: &parlour_db::PgPool) -> Result<JobSnapshot, sqlx::Error> {
    let (queued, running, failed_recent, succeeded_recent): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'queued'),
                count(*) FILTER (WHERE status = 'running'),
                count(*) FILTER (WHERE status = 'failed'
                                 AND finished_at > now() - interval '24 hours'),
                count(*) FILTER (WHERE status = 'succeeded'
                                 AND finished_at > now() - interval '24 hours')
         FROM jobs",
    )
    .fetch_one(pool)
    .await?;
    Ok(JobSnapshot {
        queued,
        running,
        failed_recent,
        succeeded_recent,
    })
}

async fn server_info(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<ServerInfo>, AppError> {
    let db_ok = parlour_db::ping(&state.pool).await;
    let jobs = if db_ok {
        match job_snapshot(&state.pool).await {
            Ok(snapshot) => Some(snapshot),
            Err(e) => {
                tracing::warn!(error = %e, "dashboard: job snapshot failed");
                None
            }
        }
    } else {
        None
    };
    let s = &state.server;
    Ok(Json(ServerInfo {
        name: s.name.clone(),
        version: env!("CARGO_PKG_VERSION"),
        port: s.port,
        db: if db_ok { "ok" } else { "unreachable" },
        urls: AccessUrls {
            lan: s.url_lan.clone(),
            tailnet: s.url_tailnet.clone(),
            public: s.url_public.clone(),
        },
        jobs,
        actions: Actions {
            restart: false,
            shutdown: false,
        },
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/admin/server", get(server_info))
}
