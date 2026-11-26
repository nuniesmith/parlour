//! Podcast feed checks on the Postgres queue (POD2, D32).
//!
//! - **The schedule.** Once a minute a tick looks for shows whose
//!   check is due (`auto_download` on and the show's cron schedule
//!   fired since its last check, `parlour_catalog::podcasts::
//!   due_podcasts`) and, when there are some, queues one
//!   `refresh_podcasts` job with `{"trigger": "schedule"}` — unless one
//!   is already waiting, or one started within the hour is still
//!   running.
//! - **The job.** A worker of its own (feeds can be slow, and must
//!   never hold up scans) claims it and checks the shows that are due
//!   when it runs. A job queued with `{"podcast_id": …}` checks that
//!   one show whatever its schedule says (POD4's "refresh now").
//!   A feed's failure is recorded on the show and listed on the job;
//!   only a database error fails the job.
//! - **The notice** for feeds failing repeatedly is reconciled with
//!   the others (`notices.rs`).
//!
//! Nothing is downloaded (POD3). No API, no UI (POD4–POD6).

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The payload marker of a scheduled check.
pub const SCHEDULE: &str = "schedule";

/// Queue a scheduled check if any show is due at `now` and no
/// scheduled-or-manual check is already waiting. A `running` job that
/// started over an hour ago doesn't count (a restart can leave one
/// behind). Returns the new job's id.
pub async fn tick(
    pool: &parlour_db::PgPool,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>, sqlx::Error> {
    let due = parlour_catalog::podcasts::due_podcasts(pool, now).await?;
    if due.is_empty() {
        return Ok(None);
    }
    let (busy,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (
             SELECT 1 FROM jobs
             WHERE kind = 'refresh_podcasts'
               AND (status = 'queued'
                    OR (status = 'running' AND started_at > now() - interval '1 hour'))
         )",
    )
    .fetch_one(pool)
    .await?;
    if busy {
        return Ok(None);
    }
    let id = parlour_jobs::enqueue(
        pool,
        parlour_jobs::JobKind::RefreshPodcasts,
        None,
        serde_json::json!({ "trigger": SCHEDULE }),
    )
    .await?;
    tracing::info!(job_id = %id, due = due.len(), "podcast feed check queued");
    Ok(Some(id))
}

/// Run one claimed `refresh_podcasts` job.
pub async fn run_refresh_job_claimed(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    job: parlour_jobs::Job,
) {
    let shows: Result<Vec<(Uuid, String)>, sqlx::Error> = match job.payload["podcast_id"].as_str() {
        Some(id) => match Uuid::parse_str(id) {
            Ok(id) => {
                sqlx::query_as("SELECT id, title FROM podcasts WHERE id = $1")
                    .bind(id)
                    .fetch_all(pool)
                    .await
            }
            Err(_) => {
                let _ = parlour_jobs::fail(pool, job.id, "the job names no valid show").await;
                return;
            }
        },
        None => parlour_catalog::podcasts::due_podcasts(pool, Utc::now()).await,
    };
    let result = match shows {
        Ok(shows) => {
            parlour_catalog::podcasts::refresh_podcasts(pool, http, &shows, Some(job.id)).await
        }
        Err(e) => Err(e),
    };
    match result {
        Ok(report) => {
            tracing::info!(job_id = %job.id, checked = report.checked,
                failed = report.failed, new_episodes = report.new_episodes,
                "podcast feed check finished");
            let summary = serde_json::to_value(&report).unwrap_or(serde_json::Value::Null);
            let _ = parlour_jobs::set_result(pool, job.id, summary).await;
            let _ = parlour_jobs::complete(pool, job.id).await;
        }
        Err(e) => {
            tracing::warn!(job_id = %job.id, error = %e, "podcast feed check failed");
            let _ = parlour_jobs::fail(pool, job.id, &e.to_string()).await;
        }
    }
}

/// Start the worker and the once-a-minute schedule tick. Called once
/// from `main`; not part of the router, so tests drive both by hand.
pub fn spawn(pool: parlour_db::PgPool) {
    {
        let pool = pool.clone();
        tokio::spawn(async move {
            let http = parlour_catalog::podcasts::http_client();
            loop {
                match parlour_jobs::claim_next(&pool, parlour_jobs::JobKind::RefreshPodcasts).await
                {
                    Ok(Some(job)) => run_refresh_job_claimed(&pool, &http, job).await,
                    Ok(None) => tokio::time::sleep(Duration::from_secs(3)).await,
                    Err(e) => {
                        tracing::warn!(error = %e, "podcast job claim failed");
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }
            }
        });
    }
    tokio::spawn(async move {
        loop {
            if let Err(e) = tick(&pool, Utc::now()).await {
                tracing::warn!(error = %e, "podcast schedule tick failed");
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}
