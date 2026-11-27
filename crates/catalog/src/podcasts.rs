//! A podcast's feed check (POD2, D32): fetch the feed with a
//! conditional GET, read it (`podcast_feed`), and bring the show's row
//! and its `podcast_episodes` rows in line with it, keeping the
//! refresh state (`last_checked_at`, `last_error`,
//! `consecutive_failures`) and the 24-failures rule.
//!
//! The check itself downloads nothing: every episode the feed lists
//! gets a row with its file columns empty (POD1 reading 3). Episodes
//! no longer in the feed are left alone. After a successful check of a
//! show with `auto_download` on, `refresh_podcasts` downloads the
//! show's new episodes and applies its retention (`podcast_download`,
//! POD3). The rules are written in `docs/AUDIOBOOKS-PODCASTS.md` →
//! "Feed fetch and parse (POD2)" and "Episode download and retention
//! (POD3)".
//!
//! A feed URL can carry a private token (paid feeds put one there), so
//! it is never written to the log, a job's error list or a notice.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use parlour_jobs::cron::CronSchedule;
use serde::Serialize;
use uuid::Uuid;

use crate::podcast_download::{
    apply_retention, download_episode, new_episodes, DownloadOutcome, RetentionOutcome,
};
use crate::podcast_feed::{parse_episode_type, parse_feed_bytes, Feed, FeedEpisode};

/// D32: a show's scheduled check turns itself off (its `auto_download`
/// goes off) after this many failed checks in a row.
pub const MAX_FAILED_CHECKS: i32 = 24;
/// POD1's default schedule, used when a show's own can't be read.
pub const DEFAULT_SCHEDULE: &str = "0 * * * *";
/// A feed larger than this is refused rather than read into memory.
pub const MAX_FEED_BYTES: usize = 64 * 1024 * 1024;
/// How long one feed fetch may take, all told.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest `last_error` kept.
const MAX_ERROR_CHARS: usize = 500;

/// The HTTP client feed checks and episode downloads use: Parlour's
/// User-Agent (no contact address), 15 s to connect, redirects
/// followed. A feed fetch is limited to 60 s (`FETCH_TIMEOUT`, set on
/// the request); a download has its own limits (`podcast_download`).
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(parlour_metadata::books::user_agent())
        .connect_timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// What a fetch brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// 304: the feed hasn't changed since the validators were stored.
    NotModified,
    /// 200: the feed, with the validators to send next time.
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FetchError {
    #[error("could not fetch the feed: {0}")]
    Request(String),
    #[error("the feed answered HTTP {0}")]
    Status(u16),
    #[error("the feed is larger than {} MB", MAX_FEED_BYTES / (1024 * 1024))]
    TooLarge,
}

fn request_error(e: reqwest::Error) -> FetchError {
    // The URL is dropped from the message (it may hold a token).
    let e = e.without_url();
    let detail = if e.is_timeout() {
        "it took too long".to_string()
    } else if e.is_connect() {
        "could not connect".to_string()
    } else {
        e.to_string()
    };
    FetchError::Request(detail)
}

/// GET a feed, sending the stored validators back as `If-None-Match`
/// / `If-Modified-Since`. Only a 200 or a 304 is an answer; anything
/// else is a failure.
pub async fn fetch_feed(
    http: &reqwest::Client,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Fetched, FetchError> {
    use reqwest::header::{
        ACCEPT, CONTENT_LENGTH, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
    };
    let mut req = http.get(url).timeout(FETCH_TIMEOUT).header(
        ACCEPT,
        "application/rss+xml, application/xml;q=0.9, text/xml;q=0.9, */*;q=0.5",
    );
    if let Some(v) = etag.filter(|v| !v.is_empty()) {
        req = req.header(IF_NONE_MATCH, v);
    }
    if let Some(v) = last_modified.filter(|v| !v.is_empty()) {
        req = req.header(IF_MODIFIED_SINCE, v);
    }
    let mut res = req.send().await.map_err(request_error)?;
    let status = res.status().as_u16();
    if status == 304 {
        return Ok(Fetched::NotModified);
    }
    if status != 200 {
        return Err(FetchError::Status(status));
    }
    let header = |name| {
        res.headers()
            .get(name)
            .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let etag = header(ETAG);
    let last_modified = header(LAST_MODIFIED);
    let declared = header(CONTENT_LENGTH).and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > MAX_FEED_BYTES) {
        return Err(FetchError::TooLarge);
    }
    let mut bytes: Vec<u8> = Vec::with_capacity(declared.unwrap_or(0).min(MAX_FEED_BYTES));
    while let Some(chunk) = res.chunk().await.map_err(request_error)? {
        if bytes.len() + chunk.len() > MAX_FEED_BYTES {
            return Err(FetchError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Fetched::Body {
        bytes,
        etag,
        last_modified,
    })
}

/// How one check ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CheckOutcome {
    /// 304: nothing changed.
    NotModified,
    /// 200: the feed was read and stored.
    Updated {
        new_episodes: usize,
        changed_episodes: usize,
    },
    /// The fetch or the feed failed.
    Failed {
        error: String,
        consecutive_failures: i32,
        /// This failure was the 24th in a row and turned the show's
        /// `auto_download` off.
        schedule_turned_off: bool,
    },
}

/// A stored episode, as the matching sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEpisode {
    pub id: Uuid,
    /// Its feed-described values (the guid is the stored one).
    pub feed: FeedEpisode,
}

/// What a feed changes in a show's episode rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpisodePlan {
    /// Feed episodes (by index) that are new.
    pub inserts: Vec<usize>,
    /// Stored episodes whose feed values changed, with the feed
    /// episode (by index) that now describes them.
    pub updates: Vec<(Uuid, usize)>,
}

/// Do the feed-described values differ? The guid isn't compared: a
/// row keeps the guid it was stored with.
fn differs(a: &FeedEpisode, b: &FeedEpisode) -> bool {
    a.title != b.title
        || a.subtitle != b.subtitle
        || a.description != b.description
        || a.published_at != b.published_at
        || a.duration_ms != b.duration_ms
        || a.season != b.season
        || a.episode_number != b.episode_number
        || a.episode_type != b.episode_type
        || a.enclosure_url != b.enclosure_url
        || a.enclosure_type != b.enclosure_type
        || a.enclosure_length != b.enclosure_length
        || a.chapters_url != b.chapters_url
        || a.transcript_url != b.transcript_url
        || a.transcript_type != b.transcript_type
}

/// Match a feed against the stored episodes. A feed episode is a
/// stored one when its guid matches; failing that (as Audiobookshelf
/// checks) when its enclosure URL matches a stored episode no other
/// feed episode claimed by guid. Everything else is new. Stored
/// episodes the feed no longer lists are not touched.
pub fn plan_episodes(stored: &[StoredEpisode], feed: &[FeedEpisode]) -> EpisodePlan {
    let by_guid: HashMap<&str, usize> = stored
        .iter()
        .enumerate()
        .map(|(i, s)| (s.feed.guid.as_str(), i))
        .collect();
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut matched: Vec<Option<usize>> = vec![None; feed.len()];
    // Pass 1: guids.
    for (fi, ep) in feed.iter().enumerate() {
        if let Some(&si) = by_guid.get(ep.guid.as_str()) {
            if claimed.insert(si) {
                matched[fi] = Some(si);
            }
        }
    }
    // Pass 2: enclosure URLs, among the stored rows still unclaimed.
    let mut by_enclosure: HashMap<&str, usize> = HashMap::new();
    for (si, s) in stored.iter().enumerate() {
        if !claimed.contains(&si) {
            by_enclosure
                .entry(s.feed.enclosure_url.as_str())
                .or_insert(si);
        }
    }
    for (fi, ep) in feed.iter().enumerate() {
        if matched[fi].is_some() || by_guid.contains_key(ep.guid.as_str()) {
            continue;
        }
        if let Some(&si) = by_enclosure.get(ep.enclosure_url.as_str()) {
            if claimed.insert(si) {
                matched[fi] = Some(si);
            }
        }
    }
    let mut plan = EpisodePlan::default();
    for (fi, ep) in feed.iter().enumerate() {
        match matched[fi] {
            Some(si) if differs(&stored[si].feed, ep) => plan.updates.push((stored[si].id, fi)),
            Some(_) => {}
            // A guid a stored row already holds (and some other item
            // claimed it first) is skipped rather than duplicated.
            None if by_guid.contains_key(ep.guid.as_str()) => {}
            None => plan.inserts.push(fi),
        }
    }
    plan
}

/// The show's schedule, or the default when it can't be read.
pub fn schedule_of(expr: &str) -> CronSchedule {
    CronSchedule::parse(expr).unwrap_or_else(|_| {
        CronSchedule::parse(DEFAULT_SCHEDULE).expect("the default schedule parses")
    })
}

/// Shows whose scheduled check is due at `now`: `auto_download` on
/// (the schedule switch, POD1 reading 5) and the schedule has fired
/// since the last check, or there hasn't been one. (id, title).
pub async fn due_podcasts(
    pool: &parlour_db::PgPool,
    now: DateTime<Utc>,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let rows: Vec<(Uuid, String, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, title, check_schedule, last_checked_at FROM podcasts
         WHERE auto_download
         ORDER BY sort_title, id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(id, _, schedule, last)| {
            if CronSchedule::parse(schedule).is_err() {
                tracing::warn!(podcast_id = %id, "unreadable check schedule; using hourly");
            }
            schedule_of(schedule).is_due(*last, now)
        })
        .map(|(id, title, _, _)| (id, title))
        .collect())
}

fn clip(message: &str) -> String {
    if message.chars().count() <= MAX_ERROR_CHARS {
        message.to_string()
    } else {
        let mut s: String = message.chars().take(MAX_ERROR_CHARS - 1).collect();
        s.push('…');
        s
    }
}

/// Check one show's feed now, whatever its schedule says. `None` when
/// there is no such show. Only a database error is an `Err`; a feed's
/// own trouble is a `Failed` outcome, recorded on the show.
pub async fn check_podcast(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    podcast_id: Uuid,
) -> Result<Option<CheckOutcome>, sqlx::Error> {
    let row: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT feed_url, etag, last_modified FROM podcasts WHERE id = $1")
            .bind(podcast_id)
            .fetch_optional(pool)
            .await?;
    let Some((feed_url, etag, last_modified)) = row else {
        return Ok(None);
    };
    match fetch_feed(http, &feed_url, etag.as_deref(), last_modified.as_deref()).await {
        Err(e) => record_failure(pool, podcast_id, &e.to_string()).await,
        Ok(Fetched::NotModified) => {
            let done = sqlx::query(
                "UPDATE podcasts SET last_checked_at = now(), last_error = NULL,
                        consecutive_failures = 0
                 WHERE id = $1",
            )
            .bind(podcast_id)
            .execute(pool)
            .await?;
            Ok((done.rows_affected() == 1).then_some(CheckOutcome::NotModified))
        }
        Ok(Fetched::Body {
            bytes,
            etag,
            last_modified,
        }) => {
            let parsed = tokio::task::spawn_blocking(move || parse_feed_bytes(&bytes))
                .await
                .unwrap_or_else(|e| {
                    Err(crate::podcast_feed::FeedError::Xml(format!(
                        "the reader stopped: {e}"
                    )))
                });
            match parsed {
                // The validators are not kept for a feed that couldn't
                // be read: a 304 must never hide a broken feed.
                Err(e) => record_failure(pool, podcast_id, &e.to_string()).await,
                Ok(feed) => {
                    store_feed(
                        pool,
                        podcast_id,
                        &feed,
                        etag.as_deref(),
                        last_modified.as_deref(),
                    )
                    .await
                }
            }
        }
    }
}

/// A failed check: stamp it, keep the reason, count it, and on the
/// 24th in a row turn the show's schedule (`auto_download`) off.
async fn record_failure(
    pool: &parlour_db::PgPool,
    podcast_id: Uuid,
    error: &str,
) -> Result<Option<CheckOutcome>, sqlx::Error> {
    let error = clip(error);
    let row: Option<(i32, bool)> = sqlx::query_as(
        "WITH before AS (
             SELECT id, auto_download FROM podcasts WHERE id = $1 FOR UPDATE
         )
         UPDATE podcasts p SET
             last_checked_at = now(),
             last_error = $2,
             consecutive_failures = p.consecutive_failures + 1,
             auto_download = p.auto_download AND p.consecutive_failures + 1 < $3,
             updated_at = CASE WHEN p.auto_download AND p.consecutive_failures + 1 >= $3
                               THEN now() ELSE p.updated_at END
         FROM before
         WHERE p.id = before.id
         RETURNING p.consecutive_failures, before.auto_download AND NOT p.auto_download",
    )
    .bind(podcast_id)
    .bind(&error)
    .bind(MAX_FAILED_CHECKS)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(count, turned_off)| CheckOutcome::Failed {
        error,
        consecutive_failures: count,
        schedule_turned_off: turned_off,
    }))
}

type EpisodeRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<i64>,
    Option<i32>,
    Option<i32>,
    Option<String>,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A successful 200: the show as the feed describes it, the new
/// validators, the refresh state cleared, and the episode rows.
async fn store_feed(
    pool: &parlour_db::PgPool,
    podcast_id: Uuid,
    feed: &Feed,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Option<CheckOutcome>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let outcome = store_feed_in(&mut tx, podcast_id, feed, etag, last_modified).await?;
    if outcome.is_some() {
        tx.commit().await?;
    }
    Ok(outcome)
}

/// `store_feed` inside the caller's transaction (POD4's add stores the
/// new row and its first check together). The caller commits.
async fn store_feed_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    podcast_id: Uuid,
    feed: &Feed,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Option<CheckOutcome>, sqlx::Error> {
    let show = &feed.show;
    // A feed without a title keeps the stored one (the column is
    // required); every other field is as the feed now has it.
    let sort_title = show.title.as_deref().map(crate::parse::sort_title);
    let done = sqlx::query(
        "UPDATE podcasts SET
             title = COALESCE($2, title),
             sort_title = COALESCE($3, sort_title),
             author = $4,
             description = $5,
             language = $6,
             explicit = $7,
             podcast_type = $8,
             genres = $9,
             image_url = $10,
             etag = $11,
             last_modified = $12,
             last_checked_at = now(),
             last_error = NULL,
             consecutive_failures = 0,
             updated_at = CASE
                 WHEN (title, sort_title, author, description, language, explicit,
                       podcast_type, genres, image_url)
                      IS DISTINCT FROM
                      (COALESCE($2, title), COALESCE($3, sort_title), $4, $5, $6, $7,
                       $8, $9, $10)
                 THEN now() ELSE updated_at END
         WHERE id = $1",
    )
    .bind(podcast_id)
    .bind(show.title.as_deref())
    .bind(sort_title.as_deref())
    .bind(show.author.as_deref())
    .bind(show.description.as_deref())
    .bind(show.language.as_deref())
    .bind(show.explicit)
    .bind(show.podcast_type)
    .bind(&show.genres)
    .bind(show.image_url.as_deref())
    .bind(etag)
    .bind(last_modified)
    .execute(&mut **tx)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }

    let rows: Vec<EpisodeRow> = sqlx::query_as(
        "SELECT id, guid, title, subtitle, description, published_at, duration_ms, season,
                episode_number, episode_type, enclosure_url, enclosure_type,
                enclosure_length, chapters_url, transcript_url, transcript_type
         FROM podcast_episodes WHERE podcast_id = $1
         ORDER BY created_at, id
         FOR UPDATE",
    )
    .bind(podcast_id)
    .fetch_all(&mut **tx)
    .await?;
    let stored: Vec<StoredEpisode> = rows
        .into_iter()
        .map(|r| StoredEpisode {
            id: r.0,
            feed: FeedEpisode {
                guid: r.1,
                title: r.2,
                subtitle: r.3,
                description: r.4,
                published_at: r.5,
                duration_ms: r.6,
                season: r.7,
                episode_number: r.8,
                episode_type: r.9.as_deref().and_then(parse_episode_type),
                enclosure_url: r.10,
                enclosure_type: r.11,
                enclosure_length: r.12,
                chapters_url: r.13,
                transcript_url: r.14,
                transcript_type: r.15,
            },
        })
        .collect();
    let plan = plan_episodes(&stored, &feed.episodes);

    let mut inserted = 0usize;
    for &fi in &plan.inserts {
        let e = &feed.episodes[fi];
        let done = sqlx::query(
            "INSERT INTO podcast_episodes
                 (podcast_id, guid, title, subtitle, description, published_at, duration_ms,
                  season, episode_number, episode_type, enclosure_url, enclosure_type,
                  enclosure_length, chapters_url, transcript_url, transcript_type)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
             ON CONFLICT (podcast_id, guid) DO NOTHING",
        )
        .bind(podcast_id)
        .bind(&e.guid)
        .bind(&e.title)
        .bind(e.subtitle.as_deref())
        .bind(e.description.as_deref())
        .bind(e.published_at)
        .bind(e.duration_ms)
        .bind(e.season)
        .bind(e.episode_number)
        .bind(e.episode_type)
        .bind(&e.enclosure_url)
        .bind(e.enclosure_type.as_deref())
        .bind(e.enclosure_length)
        .bind(e.chapters_url.as_deref())
        .bind(e.transcript_url.as_deref())
        .bind(e.transcript_type.as_deref())
        .execute(&mut **tx)
        .await?;
        inserted += done.rows_affected() as usize;
    }
    for &(id, fi) in &plan.updates {
        let e = &feed.episodes[fi];
        sqlx::query(
            "UPDATE podcast_episodes SET
                 title = $2, subtitle = $3, description = $4, published_at = $5,
                 duration_ms = $6, season = $7, episode_number = $8, episode_type = $9,
                 enclosure_url = $10, enclosure_type = $11, enclosure_length = $12,
                 chapters_url = $13, transcript_url = $14, transcript_type = $15,
                 updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(&e.title)
        .bind(e.subtitle.as_deref())
        .bind(e.description.as_deref())
        .bind(e.published_at)
        .bind(e.duration_ms)
        .bind(e.season)
        .bind(e.episode_number)
        .bind(e.episode_type)
        .bind(&e.enclosure_url)
        .bind(e.enclosure_type.as_deref())
        .bind(e.enclosure_length)
        .bind(e.chapters_url.as_deref())
        .bind(e.transcript_url.as_deref())
        .bind(e.transcript_type.as_deref())
        .execute(&mut **tx)
        .await?;
    }
    Ok(Some(CheckOutcome::Updated {
        new_episodes: inserted,
        changed_episodes: plan.updates.len(),
    }))
}

/// What a `refresh_podcasts` job did, for its result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RefreshReport {
    pub checked: usize,
    pub not_modified: usize,
    pub updated: usize,
    pub failed: usize,
    pub new_episodes: usize,
    pub changed_episodes: usize,
    pub schedules_turned_off: usize,
    /// Episodes downloaded (POD3).
    pub downloaded: usize,
    /// Episodes whose download failed (listed on the job).
    pub download_failed: usize,
    /// Episodes asked for by hand that already had a file.
    pub already_downloaded: usize,
    /// Episodes retention took back to "not downloaded".
    pub removed: usize,
    /// Episodes whose file retention couldn't delete (listed on the job).
    pub remove_failed: usize,
}

/// A job error's label for an episode: the show's and the episode's
/// titles, never a URL.
fn episode_label(show: &str, episode: &str) -> String {
    format!("{show} — {episode}")
}

/// Download one episode (by hand, or as one of a check's new ones),
/// counting it on the report and listing a failure on the job.
async fn download_counted(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    episode_id: Uuid,
    label: &str,
    job_id: Option<Uuid>,
    report: &mut RefreshReport,
) -> Result<(), sqlx::Error> {
    match download_episode(pool, http, episode_id).await? {
        None => {}
        Some(DownloadOutcome::Downloaded { .. }) => report.downloaded += 1,
        Some(DownloadOutcome::AlreadyDownloaded) => report.already_downloaded += 1,
        Some(DownloadOutcome::Failed { error }) => {
            report.download_failed += 1;
            tracing::warn!(episode_id = %episode_id, error = %error,
                "podcast episode download failed");
            if let Some(job) = job_id {
                parlour_jobs::add_error(pool, job, Some(label), &error).await?;
            }
        }
    }
    Ok(())
}

/// After a successful check of a show with `auto_download` on: download
/// its new episodes (`podcast_download::new_episodes`, cut off at the
/// check before this one when nothing is downloaded yet), then apply
/// its retention once.
async fn download_and_retain(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    podcast_id: Uuid,
    previous_check: Option<DateTime<Utc>>,
    job_id: Option<Uuid>,
    report: &mut RefreshReport,
) -> Result<(), sqlx::Error> {
    let show: Option<(String, bool)> =
        sqlx::query_as("SELECT title, auto_download FROM podcasts WHERE id = $1")
            .bind(podcast_id)
            .fetch_optional(pool)
            .await?;
    let Some((show_title, true)) = show else {
        return Ok(());
    };
    for (episode_id, title) in new_episodes(pool, podcast_id, previous_check).await? {
        let label = episode_label(&show_title, &title);
        download_counted(pool, http, episode_id, &label, job_id, report).await?;
    }
    match apply_retention(pool, podcast_id).await? {
        None => {}
        Some(RetentionOutcome::Removed { episode_id, .. }) => {
            report.removed += 1;
            tracing::info!(podcast_id = %podcast_id, episode_id = %episode_id,
                "podcast retention removed the oldest download");
        }
        Some(RetentionOutcome::Failed {
            episode_id,
            title,
            error,
        }) => {
            report.remove_failed += 1;
            tracing::warn!(podcast_id = %podcast_id, episode_id = %episode_id,
                error = %error, "podcast retention could not delete a file");
            if let Some(job) = job_id {
                let label = episode_label(&show_title, &title);
                parlour_jobs::add_error(pool, job, Some(&label), &error).await?;
            }
        }
    }
    Ok(())
}

/// A job naming one episode (POD4's "download"): download it whatever
/// its show's schedule or `auto_download` says. Retention doesn't run
/// (as in Audiobookshelf, only automatic downloads trigger it).
pub async fn download_one(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    episode_id: Uuid,
    job_id: Option<Uuid>,
) -> Result<RefreshReport, sqlx::Error> {
    let mut report = RefreshReport::default();
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT p.title, e.title FROM podcast_episodes e
         JOIN podcasts p ON p.id = e.podcast_id WHERE e.id = $1",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await?;
    let total = i32::from(row.is_some());
    if let Some(job) = job_id {
        parlour_jobs::set_progress(pool, job, 0, total).await?;
    }
    if let Some((show, title)) = row {
        let label = episode_label(&show, &title);
        download_counted(pool, http, episode_id, &label, job_id, &mut report).await?;
        if let Some(job) = job_id {
            parlour_jobs::set_progress(pool, job, 1, 1).await?;
        }
    }
    Ok(report)
}

/// Check each show in turn, recording progress and each failure (by
/// the show's title) on the job when there is one. After a successful
/// check (200 or 304) of a show with `auto_download` on, its new
/// episodes are downloaded and its retention applied once (POD3); each
/// download failure is listed by show and episode title.
pub async fn refresh_podcasts(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    shows: &[(Uuid, String)],
    job_id: Option<Uuid>,
) -> Result<RefreshReport, sqlx::Error> {
    let mut report = RefreshReport::default();
    let total = shows.len() as i32;
    if let Some(job) = job_id {
        parlour_jobs::set_progress(pool, job, 0, total).await?;
    }
    for (i, (id, title)) in shows.iter().enumerate() {
        // The check before this one: the new-episode cutoff when nothing
        // is downloaded yet (POD3).
        let previous: Option<(Option<DateTime<Utc>>,)> =
            sqlx::query_as("SELECT last_checked_at FROM podcasts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let previous_check = previous.and_then(|(at,)| at);
        match check_podcast(pool, http, *id).await? {
            None => {}
            Some(outcome) => {
                report.checked += 1;
                match outcome {
                    CheckOutcome::NotModified => {
                        report.not_modified += 1;
                        download_and_retain(pool, http, *id, previous_check, job_id, &mut report)
                            .await?;
                    }
                    CheckOutcome::Updated {
                        new_episodes,
                        changed_episodes,
                    } => {
                        report.updated += 1;
                        report.new_episodes += new_episodes;
                        report.changed_episodes += changed_episodes;
                        download_and_retain(pool, http, *id, previous_check, job_id, &mut report)
                            .await?;
                    }
                    CheckOutcome::Failed {
                        error,
                        consecutive_failures,
                        schedule_turned_off,
                    } => {
                        report.failed += 1;
                        if schedule_turned_off {
                            report.schedules_turned_off += 1;
                        }
                        tracing::warn!(podcast_id = %id, failures = consecutive_failures,
                            schedule_turned_off, error = %error, "podcast feed check failed");
                        if let Some(job) = job_id {
                            parlour_jobs::add_error(pool, job, Some(title), &error).await?;
                        }
                    }
                }
            }
        }
        if let Some(job) = job_id {
            parlour_jobs::set_progress(pool, job, i as i32 + 1, total).await?;
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------- POD4

/// The longest feed address accepted when a show is added.
pub const MAX_FEED_URL_CHARS: usize = 2048;
/// The largest `max_episodes_to_keep` / `max_new_episodes_per_check`
/// accepted. ABS has no limit; this only keeps a typo from meaning
/// "forever".
pub const MAX_EPISODE_SETTING: i32 = 100_000;
/// A show whose feed gives no title.
pub const UNTITLED: &str = "Untitled podcast";

/// A feed address as an admin typed it, checked: trimmed, `http` or
/// `https`, with a host. It is stored as typed (trimmed). The reason
/// for a refusal never repeats the address (it may hold a token).
pub fn check_feed_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("Enter the feed's address.".into());
    }
    if url.chars().count() > MAX_FEED_URL_CHARS {
        return Err(format!(
            "A feed address is at most {MAX_FEED_URL_CHARS} characters."
        ));
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        "That isn't a web address. A feed's address starts with http:// or https://.".to_string()
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("A feed's address starts with http:// or https://.".into());
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("That address has no host name.".into());
    }
    Ok(url.to_string())
}

/// Fetch and read a feed that isn't stored yet: the feed and its
/// validators, or why it was refused, in words that never repeat the
/// address.
pub async fn read_new_feed(
    http: &reqwest::Client,
    url: &str,
) -> Result<(Feed, Option<String>, Option<String>), String> {
    match fetch_feed(http, url, None, None).await {
        Err(e) => Err(e.to_string()),
        // Nothing was sent to make a 304 an answer.
        Ok(Fetched::NotModified) => Err(FetchError::Status(304).to_string()),
        Ok(Fetched::Body {
            bytes,
            etag,
            last_modified,
        }) => {
            let feed = tokio::task::spawn_blocking(move || parse_feed_bytes(&bytes))
                .await
                .unwrap_or_else(|e| {
                    Err(crate::podcast_feed::FeedError::Xml(format!(
                        "the reader stopped: {e}"
                    )))
                })
                .map_err(|e| e.to_string())?;
            Ok((feed, etag, last_modified))
        }
    }
}

/// How adding a feed to a library ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AddOutcome {
    /// The show was stored with its first check done.
    Added {
        id: Uuid,
        title: String,
        new_episodes: usize,
    },
    /// The library already has this feed address.
    Duplicate { id: Uuid, title: String },
    /// The address isn't a feed Parlour can read: why, without the
    /// address.
    Refused { error: String },
}

async fn show_with_url(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    url: &str,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as("SELECT id, title FROM podcasts WHERE library_id = $1 AND feed_url = $2")
        .bind(library_id)
        .bind(url)
        .fetch_optional(pool)
        .await
}

async fn show_with_itunes_id(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    itunes_id: i64,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, title FROM podcasts WHERE library_id = $1 AND itunes_id = $2
         ORDER BY created_at, id LIMIT 1",
    )
    .bind(library_id)
    .bind(itunes_id)
    .fetch_optional(pool)
    .await
}

/// Add a feed to a podcasts library (POD4, D31: the caller has checked
/// that an admin asked and that the library is a podcasts library).
///
/// The address is checked, a duplicate in the library is refused, and
/// the feed is fetched and read: an address that isn't a feed is
/// refused and nothing is stored. A feed that reads is stored together
/// with its first check, in one transaction (the show, its validators,
/// a row per episode, `last_checked_at`), so a new show is never empty
/// and never a row that fails every check. Nothing is downloaded: the
/// back catalogue waits for a download by hand, as in POD3.
/// `fallback_title` names a feed without a title (an OPML outline's
/// text), else it is "Untitled podcast".
///
/// `itunes_id` is Apple's id for the show when it was added from a
/// directory search (POD7). It is stored in `podcasts.itunes_id`, and a
/// show already in the library under that id is a duplicate too, as
/// the search marks it "already added".
pub async fn add_podcast(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    library_id: Uuid,
    feed_url: &str,
    fallback_title: Option<&str>,
    itunes_id: Option<i64>,
) -> Result<AddOutcome, sqlx::Error> {
    let url = match check_feed_url(feed_url) {
        Ok(url) => url,
        Err(error) => return Ok(AddOutcome::Refused { error }),
    };
    if itunes_id.is_some_and(|n| n <= 0) {
        return Ok(AddOutcome::Refused {
            error: "that isn't an Apple podcast id".into(),
        });
    }
    if let Some((id, title)) = show_with_url(pool, library_id, &url).await? {
        return Ok(AddOutcome::Duplicate { id, title });
    }
    if let Some(itunes_id) = itunes_id {
        if let Some((id, title)) = show_with_itunes_id(pool, library_id, itunes_id).await? {
            return Ok(AddOutcome::Duplicate { id, title });
        }
    }
    let (feed, etag, last_modified) = match read_new_feed(http, &url).await {
        Ok(read) => read,
        Err(error) => {
            return Ok(AddOutcome::Refused {
                error: clip(&error),
            })
        }
    };
    let title = feed
        .show
        .title
        .clone()
        .or_else(|| fallback_title.map(str::to_string))
        .unwrap_or_else(|| UNTITLED.to_string());
    let mut tx = pool.begin().await?;
    let row: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO podcasts (library_id, feed_url, title, sort_title, itunes_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (library_id, feed_url) DO NOTHING
         RETURNING id",
    )
    .bind(library_id)
    .bind(&url)
    .bind(&title)
    .bind(crate::parse::sort_title(&title))
    .bind(itunes_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id,)) = row else {
        // Added by someone else while this one was fetching.
        tx.rollback().await?;
        return Ok(match show_with_url(pool, library_id, &url).await? {
            Some((id, title)) => AddOutcome::Duplicate { id, title },
            None => AddOutcome::Refused {
                error: "the show could not be stored; try again".into(),
            },
        });
    };
    let outcome = store_feed_in(
        &mut tx,
        id,
        &feed,
        etag.as_deref(),
        last_modified.as_deref(),
    )
    .await?;
    tx.commit().await?;
    let new_episodes = match outcome {
        Some(CheckOutcome::Updated { new_episodes, .. }) => new_episodes,
        _ => 0,
    };
    Ok(AddOutcome::Added {
        id,
        title,
        new_episodes,
    })
}

/// A show's D32 settings, as an admin edits them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ShowSettings {
    pub auto_download: bool,
    pub check_schedule: String,
    pub max_episodes_to_keep: i32,
    pub max_new_episodes_per_check: i32,
}

impl Default for ShowSettings {
    /// POD1's column defaults, which are ABS's.
    fn default() -> Self {
        ShowSettings {
            auto_download: false,
            check_schedule: DEFAULT_SCHEDULE.to_string(),
            max_episodes_to_keep: 0,
            max_new_episodes_per_check: 3,
        }
    }
}

impl ShowSettings {
    /// What an admin sent, checked: the schedule must be a five-field
    /// cron expression that fires (POD2 reading 4: one that can't be
    /// read is refused here, not stored and later run hourly), and the
    /// two limits 0 or more. The schedule comes back with its white
    /// space folded.
    pub fn checked(&self, now: DateTime<Utc>) -> Result<ShowSettings, String> {
        let schedule = self
            .check_schedule
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let cron = CronSchedule::parse(&schedule)
            .map_err(|e| format!("The check schedule can't be read: {e}."))?;
        if cron.next_after(now).is_none() {
            return Err("The check schedule never fires.".into());
        }
        for (name, value) in [
            ("Episodes to keep", self.max_episodes_to_keep),
            ("New episodes per check", self.max_new_episodes_per_check),
        ] {
            if !(0..=MAX_EPISODE_SETTING).contains(&value) {
                return Err(format!(
                    "{name} must be 0 to {MAX_EPISODE_SETTING} (0 means no limit)."
                ));
            }
        }
        Ok(ShowSettings {
            check_schedule: schedule,
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(guid: &str, enclosure: &str, title: &str) -> FeedEpisode {
        FeedEpisode {
            guid: guid.into(),
            enclosure_url: enclosure.into(),
            title: title.into(),
            ..FeedEpisode::default()
        }
    }

    fn stored(n: u128, guid: &str, enclosure: &str, title: &str) -> StoredEpisode {
        StoredEpisode {
            id: Uuid::from_u128(n),
            feed: ep(guid, enclosure, title),
        }
    }

    #[test]
    fn new_unchanged_and_changed_episodes() {
        let rows = [stored(1, "g1", "u1", "One"), stored(2, "g2", "u2", "Two")];
        let feed = [
            ep("g3", "u3", "Three"),
            ep("g2", "u2", "Two (fixed)"),
            ep("g1", "u1", "One"),
        ];
        assert_eq!(
            plan_episodes(&rows, &feed),
            EpisodePlan {
                inserts: vec![0],
                updates: vec![(Uuid::from_u128(2), 1)],
            }
        );
    }

    #[test]
    fn an_enclosure_match_stands_in_for_a_changed_guid() {
        // The publisher re-issued guids; the audio is the same file.
        let rows = [stored(1, "old-guid", "u1", "One")];
        let feed = [ep("new-guid", "u1", "One")];
        assert_eq!(plan_episodes(&rows, &feed), EpisodePlan::default());
        // An item without a guid had its URL stored as the guid.
        let rows = [stored(1, "u1", "u1", "One")];
        let feed = [ep("real-guid", "u1", "One")];
        assert_eq!(plan_episodes(&rows, &feed), EpisodePlan::default());
    }

    #[test]
    fn a_guid_match_wins_over_an_enclosure_match() {
        // Item A carries row 2's enclosure but row 1's guid; item B
        // has row 2's guid. Each row goes to its guid.
        let rows = [stored(1, "g1", "u1", "One"), stored(2, "g2", "u2", "Two")];
        let feed = [ep("g1", "u2", "One"), ep("g2", "u9", "Two")];
        let plan = plan_episodes(&rows, &feed);
        assert!(plan.inserts.is_empty());
        assert_eq!(
            plan.updates,
            vec![(Uuid::from_u128(1), 0), (Uuid::from_u128(2), 1)]
        );
    }

    #[test]
    fn one_row_is_matched_once() {
        // Two new items share an old row's enclosure: the first takes
        // the row, the second is a new episode.
        let rows = [stored(1, "g1", "u1", "One")];
        let feed = [ep("a", "u1", "One"), ep("b", "u1", "One again")];
        assert_eq!(
            plan_episodes(&rows, &feed),
            EpisodePlan {
                inserts: vec![1],
                updates: vec![],
            }
        );
    }

    #[test]
    fn episodes_gone_from_the_feed_are_left_alone() {
        let rows = [stored(1, "g1", "u1", "One"), stored(2, "g2", "u2", "Two")];
        let feed = [ep("g2", "u2", "Two")];
        assert_eq!(plan_episodes(&rows, &feed), EpisodePlan::default());
        assert_eq!(
            plan_episodes(&[], &[ep("g1", "u1", "One")]).inserts,
            vec![0]
        );
    }

    #[test]
    fn an_unreadable_schedule_falls_back_to_hourly() {
        assert_eq!(schedule_of("not cron"), schedule_of(DEFAULT_SCHEDULE));
        assert_ne!(schedule_of("*/5 * * * *"), schedule_of(DEFAULT_SCHEDULE));
    }

    #[test]
    fn long_errors_are_clipped() {
        let long = "x".repeat(2000);
        assert_eq!(clip(&long).chars().count(), MAX_ERROR_CHARS);
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn feed_urls_are_checked_without_echoing_them() {
        assert_eq!(
            check_feed_url("  https://feeds.example/show.xml?token=fake-feed-token \n"),
            Ok("https://feeds.example/show.xml?token=fake-feed-token".into())
        );
        assert_eq!(
            check_feed_url("http://10.0.0.5:8080/f"),
            Ok("http://10.0.0.5:8080/f".into())
        );
        for bad in [
            "",
            "   ",
            "feeds.example/x?token=fake-feed-token",
            "ftp://feeds.example/x?token=fake-feed-token",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
        ] {
            let err = check_feed_url(bad).unwrap_err();
            assert!(
                !err.contains("fake-feed-token") && !err.contains("passwd"),
                "{bad}: {err}"
            );
        }
        let long = format!("https://a.example/{}", "x".repeat(MAX_FEED_URL_CHARS));
        assert!(check_feed_url(&long).is_err());
    }

    #[test]
    fn show_settings_refuse_an_unreadable_schedule() {
        let now = Utc::now();
        let ok = ShowSettings {
            auto_download: true,
            check_schedule: "  30   */6 * * mon-fri ".into(),
            max_episodes_to_keep: 5,
            max_new_episodes_per_check: 0,
        };
        assert_eq!(
            ok.checked(now).unwrap().check_schedule,
            "30 */6 * * mon-fri"
        );
        assert_eq!(
            ShowSettings::default().checked(now),
            Ok(ShowSettings::default())
        );
        for bad in [
            "",
            "hourly",
            "@hourly",
            "0 * * *",
            "0 0 * * * *",
            "61 * * * *",
            "0 25 * * *",
            "0 0 31 2 *",
        ] {
            let s = ShowSettings {
                check_schedule: bad.into(),
                ..ShowSettings::default()
            };
            assert!(s.checked(now).is_err(), "{bad:?} was accepted");
        }
        for (keep, new) in [(-1, 3), (0, -1), (MAX_EPISODE_SETTING + 1, 3)] {
            let s = ShowSettings {
                max_episodes_to_keep: keep,
                max_new_episodes_per_check: new,
                ..ShowSettings::default()
            };
            assert!(s.checked(now).is_err());
        }
    }
}
