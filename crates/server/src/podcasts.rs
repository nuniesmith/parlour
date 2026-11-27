//! Podcast feed checks and episode downloads on the Postgres queue
//! (POD2–POD3, D32).
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
//! - **Downloads (POD3).** After a successful check of a show with
//!   `auto_download` on, the same run downloads the show's new
//!   episodes and applies its retention
//!   (`parlour_catalog::podcast_download`), so there is no second
//!   scheduler. A job queued with `{"episode_id": …}` downloads that
//!   one episode whatever its show's settings say (POD4's "download").
//!   A download's failure is listed on the job by show and episode
//!   title; it doesn't fail the job.
//! - **The notice** for feeds failing repeatedly is reconciled with
//!   the others (`notices.rs`).
//! - **Artwork (POD6).** After the shows are checked, each one's
//!   artwork is fetched into the artwork cache when the feed names a
//!   new address (`parlour_catalog::podcast_artwork`); pages never load
//!   the publisher's address.
//!
//! POD4's admin routes (`podcast_admin`) queue the two hooks; the
//! listener API and pages are POD5/POD6.

use std::path::{Path, PathBuf};
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

/// Run one claimed `refresh_podcasts` job. With `artwork_dir`, each
/// show checked also has its artwork brought in line with its feed
/// (POD6, `parlour_catalog::podcast_artwork`); tests that don't look at
/// artwork pass `None`.
pub async fn run_refresh_job_claimed(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    job: parlour_jobs::Job,
    artwork_dir: Option<&Path>,
) {
    if let Some(id) = job.payload.get("episode_id") {
        let Some(id) = id.as_str().and_then(|s| Uuid::parse_str(s).ok()) else {
            let _ = parlour_jobs::fail(pool, job.id, "the job names no valid episode").await;
            return;
        };
        let result = parlour_catalog::podcasts::download_one(pool, http, id, Some(job.id)).await;
        finish(pool, job.id, result, None).await;
        return;
    }
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
    let mut artwork = None;
    let result = match shows {
        Ok(shows) => {
            let result =
                parlour_catalog::podcasts::refresh_podcasts(pool, http, &shows, Some(job.id)).await;
            if let (Ok(_), Some(dir)) = (&result, artwork_dir) {
                artwork = Some(cache_artwork(pool, http, dir, &shows, Some(job.id)).await);
            }
            result
        }
        Err(e) => Err(e),
    };
    finish(pool, job.id, result, artwork).await;
}

/// What the artwork step of a check did, for the job's result.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct ArtworkSummary {
    pub cached: usize,
    pub failed: usize,
}

/// Bring each show's cached artwork in line with its feed (POD6). A
/// failure is listed on the job by the show's title, never by address,
/// and fails nothing.
pub async fn cache_artwork(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    artwork_dir: &Path,
    shows: &[(Uuid, String)],
    job_id: Option<Uuid>,
) -> ArtworkSummary {
    use parlour_catalog::podcast_artwork::{cache_show_artwork, ArtworkOutcome};
    let mut summary = ArtworkSummary::default();
    for (id, title) in shows {
        match cache_show_artwork(pool, http, artwork_dir, *id).await {
            Ok(ArtworkOutcome::Cached) => summary.cached += 1,
            Ok(ArtworkOutcome::Failed(why)) => {
                summary.failed += 1;
                tracing::info!(podcast_id = %id, reason = %why, "podcast artwork not cached");
                if let Some(job) = job_id {
                    let message = format!("{title}: the artwork wasn't cached ({why})");
                    let _ = parlour_jobs::add_error(pool, job, None, &message).await;
                }
            }
            Ok(_) => {}
            Err(e) => {
                summary.failed += 1;
                tracing::warn!(podcast_id = %id, error = %e, "podcast artwork step failed");
            }
        }
    }
    summary
}

/// Cache a newly added show's artwork in the background, so the add
/// answers as soon as the feed is stored (POD6; POD4's add and OPML
/// import).
pub fn cache_artwork_soon(
    pool: parlour_db::PgPool,
    http: reqwest::Client,
    artwork_dir: PathBuf,
    podcast_id: Uuid,
) {
    tokio::spawn(async move {
        let show = [(podcast_id, String::new())];
        cache_artwork(&pool, &http, &artwork_dir, &show, None).await;
    });
}

/// Record a run's summary and finish the job; only a database error
/// fails it.
async fn finish(
    pool: &parlour_db::PgPool,
    job_id: Uuid,
    result: Result<parlour_catalog::podcasts::RefreshReport, sqlx::Error>,
    artwork: Option<ArtworkSummary>,
) {
    match result {
        Ok(report) => {
            tracing::info!(job_id = %job_id, checked = report.checked,
                failed = report.failed, new_episodes = report.new_episodes,
                downloaded = report.downloaded, download_failed = report.download_failed,
                removed = report.removed, "podcast feed check finished");
            let mut summary = serde_json::to_value(&report).unwrap_or(serde_json::Value::Null);
            if let (Some(art), Some(obj)) = (artwork, summary.as_object_mut()) {
                obj.insert(
                    "artwork".into(),
                    serde_json::to_value(art).unwrap_or(serde_json::Value::Null),
                );
            }
            let _ = parlour_jobs::set_result(pool, job_id, summary).await;
            let _ = parlour_jobs::complete(pool, job_id).await;
        }
        Err(e) => {
            tracing::warn!(job_id = %job_id, error = %e, "podcast feed check failed");
            let _ = parlour_jobs::fail(pool, job_id, &e.to_string()).await;
        }
    }
}

/// Start the worker and the once-a-minute schedule tick. Called once
/// from `main`; not part of the router, so tests drive both by hand.
pub fn spawn(pool: parlour_db::PgPool, artwork_dir: PathBuf) {
    {
        let pool = pool.clone();
        tokio::spawn(async move {
            let http = parlour_catalog::podcasts::http_client();
            loop {
                match parlour_jobs::claim_next(&pool, parlour_jobs::JobKind::RefreshPodcasts).await
                {
                    Ok(Some(job)) => {
                        run_refresh_job_claimed(&pool, &http, job, Some(&artwork_dir)).await
                    }
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
