//! POD4 (D31/D32): managing a podcasts library's feeds, as
//! Audiobookshelf lets an admin do. Spec: `docs/AUDIOBOOKS-PODCASTS.md`
//! → "Managing feeds (POD4)". **Admins only** (D31: only admins add
//! shows); everyone else gets the admin surface's 404. Listener-facing
//! podcast pages and the native API are POD5/POD6.
//!
//! - `GET  /api/v1/admin/libraries/{id}/podcasts` — the library's shows.
//! - `POST /api/v1/admin/libraries/{id}/podcasts` `{"feed_url"}` — add
//!   a show: the address is fetched and read at once, so one that isn't
//!   a feed is refused (400, in words that never repeat the address)
//!   and nothing is stored; a feed already in the library is refused
//!   (409). A feed that reads is stored with its first check done.
//! - `POST /api/v1/admin/libraries/{id}/podcasts/opml` (the file as the
//!   body) — add every feed an OPML file names, each the same way;
//!   repeats and feeds already in the library are skipped. Answers what
//!   was added, skipped and refused, by name, never by address.
//! - `GET  /api/v1/admin/libraries/{id}/podcasts/opml` — the library's
//!   shows as an OPML file.
//! - `GET  /api/v1/admin/podcasts/{id}` — a show, its settings and its
//!   episode list (downloaded or not).
//! - `PUT  /api/v1/admin/podcasts/{id}/settings` — D32's four settings;
//!   an unreadable cron schedule is refused (400).
//! - `POST /api/v1/admin/podcasts/{id}/refresh` — "refresh now": queues
//!   POD2's `{"podcast_id"}` job (202). It counts toward the failures.
//! - `POST /api/v1/admin/podcast-episodes/{id}/download` — "download":
//!   queues POD3's `{"episode_id"}` job (202); an episode that already
//!   has a file is refused (409) and the job leaves one alone anyway.
//!
//! A feed address can carry a private token (paid feeds put one there).
//! It is shown only to admins, on the show and in the OPML export, and
//! never in an error, the log, a job or a notice.
//!
//! POD5 (`crate::podcast_api`) is the listener side: the shows a
//! profile reaches through its grants, episode streams and progress.

use std::collections::HashSet;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use parlour_catalog::podcasts::{AddOutcome, ShowSettings};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;

/// How many feeds an OPML import fetches at once.
pub const OPML_IMPORT_PARALLEL: usize = 4;

fn parse_id(id: &str, what: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound(format!("no such {what}")))
}

/// The podcasts library `id` (its name), or why not.
async fn podcasts_library(state: &AppState, id: &str) -> Result<(Uuid, String), AppError> {
    let library_id = parse_id(id, "library")?;
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT name, kind FROM libraries WHERE id = $1")
            .bind(library_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::from)?;
    match row {
        None => Err(AppError::NotFound("no such library".into())),
        Some((_, kind)) if kind != "podcasts" => Err(AppError::BadRequest(
            "only a podcasts library holds podcasts".into(),
        )),
        Some((name, _)) => Ok((library_id, name)),
    }
}

// ------------------------------------------------------------------ list

#[derive(Serialize, sqlx::FromRow)]
struct ShowSummary {
    id: Uuid,
    title: String,
    author: Option<String>,
    episodes: i64,
    downloaded: i64,
    last_checked_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    consecutive_failures: i32,
    auto_download: bool,
    check_schedule: String,
}

async fn list_shows(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<ShowSummary>>, AppError> {
    let (library_id, _) = podcasts_library(&state, &id).await?;
    let shows: Vec<ShowSummary> = sqlx::query_as(
        "SELECT p.id, p.title, p.author,
                (SELECT count(*) FROM podcast_episodes e WHERE e.podcast_id = p.id) AS episodes,
                (SELECT count(*) FROM podcast_episodes e
                 WHERE e.podcast_id = p.id AND e.file_path IS NOT NULL) AS downloaded,
                p.last_checked_at, p.last_error, p.consecutive_failures, p.auto_download,
                p.check_schedule
         FROM podcasts p WHERE p.library_id = $1
         ORDER BY p.sort_title, p.id",
    )
    .bind(library_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(shows))
}

// ------------------------------------------------------------------- add

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddBody {
    feed_url: String,
}

async fn add_show(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<AddBody>,
) -> Result<(StatusCode, Json<ShowDetail>), AppError> {
    let (library_id, _) = podcasts_library(&state, &id).await?;
    let outcome = parlour_catalog::podcasts::add_podcast(
        &state.pool,
        &state.podcast_http,
        library_id,
        &body.feed_url,
        None,
    )
    .await
    .map_err(AppError::from)?;
    match outcome {
        AddOutcome::Added { id, .. } => {
            tracing::info!(podcast_id = %id, library_id = %library_id, "podcast added");
            crate::podcasts::cache_artwork_soon(
                state.pool.clone(),
                state.podcast_http.clone(),
                state.metadata.artwork_dir.clone(),
                id,
            );
            Ok((StatusCode::CREATED, Json(load_show(&state, id).await?)))
        }
        AddOutcome::Duplicate { title, .. } => Err(AppError::Conflict(format!(
            "This library already has that feed (\u{201c}{title}\u{201d})."
        ))),
        AddOutcome::Refused { error } => Err(AppError::BadRequest(format!(
            "That address can't be added: {error}."
        ))),
    }
}

// ------------------------------------------------------------------ OPML

/// One feed of an OPML import, by name (never by address).
#[derive(Serialize)]
struct ImportLine {
    /// The outline's name, else the feed's own title once read, else
    /// "Feed n in the file".
    name: String,
    /// `added`, `already_in_library`, `repeated_in_file` or `refused`.
    outcome: &'static str,
    /// The show, when it was added or was already there.
    podcast_id: Option<Uuid>,
    /// Why a feed was refused.
    error: Option<String>,
    new_episodes: Option<usize>,
}

#[derive(Serialize)]
struct ImportReport {
    feeds: usize,
    added: usize,
    skipped: usize,
    refused: usize,
    lines: Vec<ImportLine>,
}

async fn import_opml(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<ImportReport>, AppError> {
    let (library_id, _) = podcasts_library(&state, &id).await?;
    let feeds = parlour_catalog::opml::parse_opml(&body)
        .map_err(|e| AppError::BadRequest(format!("That file can't be imported: {e}.")))?;
    let total = feeds.len();
    let label = |i: usize, title: &Option<String>| {
        title
            .clone()
            .unwrap_or_else(|| format!("Feed {} in the file", i + 1))
    };

    // Repeats within the file are skipped before anything is fetched.
    let mut seen: HashSet<String> = HashSet::new();
    let mut lines: Vec<Option<ImportLine>> = Vec::with_capacity(total);
    let mut work: Vec<(usize, String, Option<String>)> = Vec::new();
    for (i, feed) in feeds.into_iter().enumerate() {
        if seen.insert(feed.feed_url.clone()) {
            lines.push(None);
            work.push((i, feed.feed_url, feed.title));
        } else {
            lines.push(Some(ImportLine {
                name: label(i, &feed.title),
                outcome: "repeated_in_file",
                podcast_id: None,
                error: None,
                new_episodes: None,
            }));
        }
    }

    // Each feed is added as an add by URL is, a few at a time.
    let gate = Arc::new(tokio::sync::Semaphore::new(OPML_IMPORT_PARALLEL));
    let mut tasks = tokio::task::JoinSet::new();
    for (i, url, title) in work {
        let pool = state.pool.clone();
        let http = state.podcast_http.clone();
        let gate = gate.clone();
        tasks.spawn(async move {
            let _permit = gate.acquire_owned().await;
            let outcome = parlour_catalog::podcasts::add_podcast(
                &pool,
                &http,
                library_id,
                &url,
                title.as_deref(),
            )
            .await;
            (i, title, outcome)
        });
    }
    let mut db_error = None;
    while let Some(done) = tasks.join_next().await {
        let Ok((i, title, outcome)) = done else {
            continue;
        };
        let line = match outcome {
            Err(e) => {
                db_error = Some(e);
                continue;
            }
            Ok(AddOutcome::Added {
                id,
                title: stored,
                new_episodes,
            }) => {
                crate::podcasts::cache_artwork_soon(
                    state.pool.clone(),
                    state.podcast_http.clone(),
                    state.metadata.artwork_dir.clone(),
                    id,
                );
                ImportLine {
                    name: title.unwrap_or(stored),
                    outcome: "added",
                    podcast_id: Some(id),
                    error: None,
                    new_episodes: Some(new_episodes),
                }
            }
            Ok(AddOutcome::Duplicate { id, title: stored }) => ImportLine {
                name: title.unwrap_or(stored),
                outcome: "already_in_library",
                podcast_id: Some(id),
                error: None,
                new_episodes: None,
            },
            Ok(AddOutcome::Refused { error }) => ImportLine {
                name: label(i, &title),
                outcome: "refused",
                podcast_id: None,
                error: Some(error),
                new_episodes: None,
            },
        };
        lines[i] = Some(line);
    }
    if let Some(e) = db_error {
        return Err(AppError::from(e));
    }
    let lines: Vec<ImportLine> = lines.into_iter().flatten().collect();
    let count = |o: &str| lines.iter().filter(|l| l.outcome == o).count();
    let report = ImportReport {
        feeds: total,
        added: count("added"),
        skipped: count("already_in_library") + count("repeated_in_file"),
        refused: count("refused"),
        lines,
    };
    tracing::info!(library_id = %library_id, feeds = report.feeds, added = report.added,
        skipped = report.skipped, refused = report.refused, "OPML import finished");
    Ok(Json(report))
}

/// A file name made of the library's name: ASCII letters, digits and
/// dashes only, so the header needs no encoding.
pub(crate) fn opml_file_name(library: &str) -> String {
    let mut name = String::new();
    for c in library.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.ends_with('-') && !name.is_empty() {
            name.push('-');
        }
    }
    let name = name.trim_end_matches('-');
    if name.is_empty() {
        "podcasts.opml".to_string()
    } else {
        format!("{name}.opml")
    }
}

async fn export_opml(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let (library_id, name) = podcasts_library(&state, &id).await?;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT title, feed_url FROM podcasts WHERE library_id = $1 ORDER BY sort_title, id",
    )
    .bind(library_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    let shows: Vec<parlour_catalog::opml::OpmlShow> = rows
        .into_iter()
        .map(|(title, feed_url)| parlour_catalog::opml::OpmlShow { title, feed_url })
        .collect();
    let doc = parlour_catalog::opml::write_opml(&format!("Parlour — {name}"), Utc::now(), &shows);
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "text/x-opml; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", opml_file_name(&name)),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        doc,
    )
        .into_response())
}

// ------------------------------------------------------------------ show

#[derive(Serialize, sqlx::FromRow)]
struct EpisodeDto {
    id: Uuid,
    title: String,
    published_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    season: Option<i32>,
    episode_number: Option<i32>,
    episode_type: Option<String>,
    enclosure_length: Option<i64>,
    downloaded_at: Option<DateTime<Utc>>,
    file_size_bytes: Option<i64>,
    file_duration_ms: Option<i64>,
    /// A "download" job for it is waiting.
    download_queued: bool,
}

#[derive(Serialize)]
pub struct ShowDetail {
    id: Uuid,
    library_id: Uuid,
    library_name: String,
    title: String,
    sort_title: String,
    author: Option<String>,
    language: Option<String>,
    explicit: bool,
    podcast_type: Option<String>,
    genres: Vec<String>,
    /// Admins only: it may hold a private token.
    feed_url: String,
    folder_path: Option<String>,
    last_checked_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    consecutive_failures: i32,
    /// D32's four settings, and ABS's defaults beside them.
    settings: ShowSettings,
    default: ShowSettings,
    /// A "refresh now" job for it is waiting.
    refresh_queued: bool,
    created_at: DateTime<Utc>,
    episodes: Vec<EpisodeDto>,
}

#[derive(sqlx::FromRow)]
struct ShowRow {
    id: Uuid,
    library_id: Uuid,
    library_name: String,
    title: String,
    sort_title: String,
    author: Option<String>,
    language: Option<String>,
    explicit: bool,
    podcast_type: Option<String>,
    genres: Vec<String>,
    feed_url: String,
    folder_path: Option<String>,
    last_checked_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    consecutive_failures: i32,
    auto_download: bool,
    check_schedule: String,
    max_episodes_to_keep: i32,
    max_new_episodes_per_check: i32,
    refresh_queued: bool,
    created_at: DateTime<Utc>,
}

async fn load_show(state: &AppState, id: Uuid) -> Result<ShowDetail, AppError> {
    let row: Option<ShowRow> = sqlx::query_as(
        "SELECT p.id, p.library_id, l.name AS library_name, p.title, p.sort_title, p.author,
                p.language, p.explicit, p.podcast_type, p.genres, p.feed_url, p.folder_path,
                p.last_checked_at, p.last_error, p.consecutive_failures, p.auto_download,
                p.check_schedule, p.max_episodes_to_keep, p.max_new_episodes_per_check,
                EXISTS (SELECT 1 FROM jobs j
                        WHERE j.kind = 'refresh_podcasts' AND j.status IN ('queued', 'running')
                          AND j.payload->>'podcast_id' = p.id::text) AS refresh_queued,
                p.created_at
         FROM podcasts p JOIN libraries l ON l.id = p.library_id
         WHERE p.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;
    let row = row.ok_or_else(|| AppError::NotFound("no such podcast".into()))?;
    // A serial show lists its episodes oldest first (POD1 reading 12),
    // as podcast apps do; an episodic one newest first.
    let episodes: Vec<EpisodeDto> = sqlx::query_as(
        "SELECT e.id, e.title, e.published_at, e.duration_ms, e.season, e.episode_number,
                e.episode_type, e.enclosure_length, e.downloaded_at, e.file_size_bytes,
                e.file_duration_ms,
                EXISTS (SELECT 1 FROM jobs j
                        WHERE j.kind = 'refresh_podcasts' AND j.status IN ('queued', 'running')
                          AND j.payload->>'episode_id' = e.id::text) AS download_queued
         FROM podcast_episodes e
         WHERE e.podcast_id = $1
         ORDER BY
             CASE WHEN $2 THEN e.published_at END ASC NULLS LAST,
             CASE WHEN NOT $2 THEN e.published_at END DESC NULLS LAST,
             e.guid",
    )
    .bind(id)
    .bind(row.podcast_type.as_deref() == Some("serial"))
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(ShowDetail {
        id: row.id,
        library_id: row.library_id,
        library_name: row.library_name,
        title: row.title,
        sort_title: row.sort_title,
        author: row.author,
        language: row.language,
        explicit: row.explicit,
        podcast_type: row.podcast_type,
        genres: row.genres,
        feed_url: row.feed_url,
        folder_path: row.folder_path,
        last_checked_at: row.last_checked_at,
        last_error: row.last_error,
        consecutive_failures: row.consecutive_failures,
        settings: ShowSettings {
            auto_download: row.auto_download,
            check_schedule: row.check_schedule,
            max_episodes_to_keep: row.max_episodes_to_keep,
            max_new_episodes_per_check: row.max_new_episodes_per_check,
        },
        default: ShowSettings::default(),
        refresh_queued: row.refresh_queued,
        created_at: row.created_at,
        episodes,
    })
}

async fn get_show(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Json<ShowDetail>, AppError> {
    let id = parse_id(&id, "podcast")?;
    Ok(Json(load_show(&state, id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    auto_download: bool,
    check_schedule: String,
    max_episodes_to_keep: i32,
    max_new_episodes_per_check: i32,
}

async fn put_settings(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<SettingsBody>,
) -> Result<Json<ShowDetail>, AppError> {
    let id = parse_id(&id, "podcast")?;
    let settings = ShowSettings {
        auto_download: body.auto_download,
        check_schedule: body.check_schedule,
        max_episodes_to_keep: body.max_episodes_to_keep,
        max_new_episodes_per_check: body.max_new_episodes_per_check,
    }
    .checked(Utc::now())
    .map_err(AppError::BadRequest)?;
    let done = sqlx::query(
        "UPDATE podcasts SET
             auto_download = $2, check_schedule = $3, max_episodes_to_keep = $4,
             max_new_episodes_per_check = $5,
             updated_at = CASE
                 WHEN (auto_download, check_schedule, max_episodes_to_keep,
                       max_new_episodes_per_check) IS DISTINCT FROM ($2, $3, $4, $5)
                 THEN now() ELSE updated_at END
         WHERE id = $1",
    )
    .bind(id)
    .bind(settings.auto_download)
    .bind(&settings.check_schedule)
    .bind(settings.max_episodes_to_keep)
    .bind(settings.max_new_episodes_per_check)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound("no such podcast".into()));
    }
    Ok(Json(load_show(&state, id).await?))
}

// ------------------------------------------------------------------ jobs

/// A waiting `refresh_podcasts` job naming `key` = `value`, if any: a
/// second click returns the first job rather than queueing another.
async fn waiting_job(state: &AppState, key: &str, value: Uuid) -> Result<Option<Uuid>, AppError> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM jobs
         WHERE kind = 'refresh_podcasts' AND status = 'queued' AND payload->>$1 = $2
         ORDER BY created_at LIMIT 1",
    )
    .bind(key)
    .bind(value.to_string())
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(row.map(|(id,)| id))
}

async fn queue(
    state: &AppState,
    key: &str,
    value: Uuid,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let job_id = match waiting_job(state, key, value).await? {
        Some(id) => id,
        None => parlour_jobs::enqueue(
            &state.pool,
            parlour_jobs::JobKind::RefreshPodcasts,
            None,
            serde_json::json!({ key: value }),
        )
        .await
        .map_err(AppError::from)?,
    };
    let job = parlour_jobs::get(&state.pool, job_id)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::Internal("job vanished".into()))?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn refresh_now(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let id = parse_id(&id, "podcast")?;
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM podcasts WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::from)?;
    if exists.is_none() {
        return Err(AppError::NotFound("no such podcast".into()));
    }
    queue(&state, "podcast_id", id).await
}

async fn download_episode(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let id = parse_id(&id, "episode")?;
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT file_path IS NOT NULL FROM podcast_episodes WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::from)?;
    match row {
        None => Err(AppError::NotFound("no such episode".into())),
        Some((true,)) => Err(AppError::Conflict(
            "This episode is already downloaded.".into(),
        )),
        Some((false,)) => queue(&state, "episode_id", id).await,
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/admin/libraries/{id}/podcasts",
            get(list_shows).post(add_show),
        )
        .route(
            "/api/v1/admin/libraries/{id}/podcasts/opml",
            get(export_opml).post(import_opml),
        )
        .route("/api/v1/admin/podcasts/{id}", get(get_show))
        .route("/api/v1/admin/podcasts/{id}/settings", put(put_settings))
        .route("/api/v1/admin/podcasts/{id}/refresh", post(refresh_now))
        .route(
            "/api/v1/admin/podcast-episodes/{id}/download",
            post(download_episode),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opml_file_names_are_plain_ascii() {
        assert_eq!(opml_file_name("Podcasts"), "Podcasts.opml");
        assert_eq!(
            opml_file_name("  Kids' shows — 2026 "),
            "Kids-shows-2026.opml"
        );
        assert_eq!(opml_file_name("ポッドキャスト"), "podcasts.opml");
        assert_eq!(opml_file_name("\"x\";y"), "x-y.opml");
    }
}
