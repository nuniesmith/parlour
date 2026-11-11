//! Catalog API + background scanning (Milestone 1.3, C6/C7).
//!
//! Admin: trigger a scan (a `scan_library` job, run immediately
//! and also claimable by the background worker after a restart),
//! and inspect jobs (progress + per-file errors while they run).
//!
//! Users: browse the catalog of a library their ACTIVE profile can
//! access, and read one item with its children, files, streams and
//! subtitle tracks. Access is enforced in SQL with the same
//! `library_access` join as libraries (no access = 404).
//!
//! Background (spawned from `main`, never from the router, so tests
//! stay deterministic):
//! - a worker loop claiming queued scan jobs,
//! - a filesystem watcher (`notify`) over every library path:
//!   events are debounced, then a targeted scan is queued for each
//!   affected library (C7),
//! - a nightly full scan per library as the safety net (C7).

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::AppError;
use crate::http::AppState;

fn parse_id(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound("no such item".into()))
}

// ---------------------------------------------------------------- items

#[derive(Debug, Serialize, FromRow)]
pub struct ItemDto {
    pub id: Uuid,
    pub library_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i32>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub overview: Option<String>,
    pub content_rating: Option<String>,
    pub genres: Vec<String>,
    pub vote_average: Option<f64>,
    pub metadata_status: String,
    pub metadata_provider: Option<String>,
    pub available: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct ItemsQuery {
    kind: Option<String>,
}

/// Top-level items (movies, series) of a library the session's
/// active profile can access.
async fn list_items(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Vec<ItemDto>>, AppError> {
    let Some(profile_id) = user.active_profile_id else {
        return Err(AppError::BadRequest("select a profile first".into()));
    };
    let library_id = parse_id(&id)?;
    let rows = sqlx::query_as::<_, ItemDto>(
        "SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider, i.created_at, i.updated_at
         FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $2
         WHERE i.library_id = $1 AND i.parent_id IS NULL
           AND ($3::text IS NULL OR i.kind = $3)
         ORDER BY i.sort_title",
    )
    .bind(library_id)
    .bind(profile_id)
    .bind(&q.kind)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    // An inaccessible library and an empty one both read as empty
    // here; the library row itself is the 404 boundary (libraries.rs).
    Ok(Json(rows))
}

#[derive(Debug, Serialize, FromRow)]
struct StreamDto {
    stream_index: i32,
    kind: String,
    codec: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    hdr_format: Option<String>,
    language: Option<String>,
    title: Option<String>,
    channels: Option<i32>,
    is_default: bool,
}

#[derive(Debug, Serialize, FromRow)]
struct SubtitleDto {
    stream_index: Option<i32>,
    source: String,
    path: Option<String>,
    format: Option<String>,
    language: Option<String>,
    title: Option<String>,
    is_forced: bool,
    is_default: bool,
    is_sdh: bool,
}

#[derive(Debug, Serialize, FromRow)]
struct FileDto {
    id: Uuid,
    path: String,
    relative_path: String,
    size_bytes: i64,
    container: Option<String>,
    duration_ms: Option<i64>,
    probe_error: Option<String>,
    available: bool,
    #[sqlx(skip)]
    streams: Vec<StreamDto>,
    #[sqlx(skip)]
    subtitles: Vec<SubtitleDto>,
}

#[derive(Debug, Serialize, FromRow)]
struct ImageDto {
    kind: String,
    variant: String,
    path: String,
    source_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct ItemDetailDto {
    #[serde(flatten)]
    item: ItemDto,
    children: Vec<ItemDto>,
    files: Vec<FileDto>,
    images: Vec<ImageDto>,
}

async fn get_item(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ItemDetailDto>, AppError> {
    let Some(profile_id) = user.active_profile_id else {
        return Err(AppError::BadRequest("select a profile first".into()));
    };
    let item_id = parse_id(&id)?;
    let item = sqlx::query_as::<_, ItemDto>(
        "SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider, i.created_at, i.updated_at
         FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $2
         WHERE i.id = $1",
    )
    .bind(item_id)
    .bind(profile_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| AppError::NotFound("no such item".into()))?;

    let children = sqlx::query_as::<_, ItemDto>(
        "SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider, i.created_at, i.updated_at
         FROM media_items i
         WHERE i.parent_id = $1
         ORDER BY i.season_number NULLS LAST, i.episode_number NULLS LAST, i.sort_title",
    )
    .bind(item_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    let mut files = sqlx::query_as::<_, FileDto>(
        "SELECT id, path, relative_path, size_bytes, container, duration_ms,
                probe_error, available
         FROM media_files WHERE item_id = $1 ORDER BY path",
    )
    .bind(item_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    for file in &mut files {
        file.streams = sqlx::query_as::<_, StreamDto>(
            "SELECT stream_index, kind, codec, width, height, hdr_format, language,
                    title, channels, is_default
             FROM media_streams WHERE file_id = $1 ORDER BY stream_index",
        )
        .bind(file.id)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::from)?;
        file.subtitles = sqlx::query_as::<_, SubtitleDto>(
            "SELECT stream_index, source, path, format, language, title,
                    is_forced, is_default, is_sdh
             FROM subtitle_tracks WHERE file_id = $1
             ORDER BY source, stream_index NULLS LAST, path",
        )
        .bind(file.id)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::from)?;
    }

    let images = sqlx::query_as::<_, ImageDto>(
        "SELECT kind, variant, path, source_url FROM images
         WHERE item_id = $1 ORDER BY kind, variant",
    )
    .bind(item_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(Json(ItemDetailDto {
        item,
        children,
        files,
        images,
    }))
}

// ---------------------------------------------------------------- jobs

/// Run one scan job now: claim it, scan, complete/fail it.
/// Idempotent claiming means the background worker and this direct
/// runner can race safely — only one wins the row.
pub async fn run_scan_job(pool: parlour_db::PgPool, job_id: Uuid) {
    match parlour_jobs::claim_by_id(&pool, job_id).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::warn!(%job_id, error = %e, "could not claim scan job");
            return;
        }
    }
    let job = match parlour_jobs::get(&pool, job_id).await {
        Ok(Some(j)) => j,
        _ => return,
    };
    let Some(library_id) = job.library_id else {
        let _ = parlour_jobs::fail(&pool, job_id, "scan job without a library").await;
        return;
    };
    match parlour_catalog::scan_library(&pool, library_id, Some(job_id)).await {
        Ok(report) => {
            tracing::info!(
                %job_id, %library_id, seen = report.seen_files,
                probed = report.probed_files, missing = report.missing_files,
                errors = report.errors.len(), "library scan finished"
            );
            let _ = parlour_jobs::complete(&pool, job_id).await;
        }
        Err(e) => {
            tracing::warn!(%job_id, error = %e, "library scan failed");
            let _ = parlour_jobs::fail(&pool, job_id, &e.to_string()).await;
        }
    }
}

async fn trigger_scan(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let library_id = parse_id(&id)?;
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM libraries WHERE id = $1")
        .bind(library_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::from)?;
    if exists.is_none() {
        return Err(AppError::NotFound("no such library".into()));
    }
    let job_id = parlour_jobs::enqueue(
        &state.pool,
        parlour_jobs::JobKind::ScanLibrary,
        Some(library_id),
        serde_json::json!({ "trigger": "manual" }),
    )
    .await
    .map_err(AppError::from)?;
    let pool = state.pool.clone();
    tokio::spawn(async move { run_scan_job(pool, job_id).await });
    let job = parlour_jobs::get(&state.pool, job_id)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::Internal("job vanished".into()))?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Deserialize)]
struct JobsQuery {
    library_id: Option<String>,
    limit: Option<i64>,
}

async fn list_jobs(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<JobsQuery>,
) -> Result<Json<Vec<parlour_jobs::Job>>, AppError> {
    let library_id = match &q.library_id {
        Some(s) => Some(parse_id(s)?),
        None => None,
    };
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let jobs = parlour_jobs::list_recent(&state.pool, library_id, limit)
        .await
        .map_err(AppError::from)?;
    Ok(Json(jobs))
}

async fn get_job(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Json<parlour_jobs::Job>, AppError> {
    let job_id = parse_id(&id)?;
    let job = parlour_jobs::get(&state.pool, job_id)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::NotFound("no such job".into()))?;
    Ok(Json(job))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/libraries/{id}/items", get(list_items))
        .route("/api/v1/items/{id}", get(get_item))
        .route("/api/v1/admin/libraries/{id}/scan", post(trigger_scan))
        .route("/api/v1/admin/jobs", get(list_jobs))
        .route("/api/v1/admin/jobs/{id}", get(get_job))
}

// ---------------------------------------------------------------- background

async fn library_paths(pool: &parlour_db::PgPool) -> Vec<(Uuid, String)> {
    sqlx::query_as::<_, (Uuid, String)>("SELECT library_id, path FROM library_paths ORDER BY path")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

async fn scan_already_active(pool: &parlour_db::PgPool, library_id: Uuid) -> bool {
    let row: Result<(bool,), _> = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind = 'scan_library'
                        AND library_id = $1 AND status IN ('queued', 'running'))",
    )
    .bind(library_id)
    .fetch_one(pool)
    .await;
    row.map(|(b,)| b).unwrap_or(false)
}

async fn enqueue_scan(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    trigger: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    if scan_already_active(pool, library_id).await {
        return Ok(None);
    }
    parlour_jobs::enqueue(
        pool,
        parlour_jobs::JobKind::ScanLibrary,
        Some(library_id),
        serde_json::json!({ "trigger": trigger }),
    )
    .await
    .map(Some)
}

/// Start the background worker, watcher, and nightly scan.
/// Called once from `main`; deliberately not part of the router so
/// integration tests control scanning explicitly.
pub fn spawn_background(pool: parlour_db::PgPool, metadata: crate::metadata::MetadataSettings) {
    // Worker: claim queued jobs (covers restarts + watcher and
    // nightly enqueues) every few seconds.
    {
        let pool = pool.clone();
        let metadata = metadata.clone();
        tokio::spawn(async move {
            loop {
                let mut claimed = None;
                for kind in [
                    parlour_jobs::JobKind::ScanLibrary,
                    parlour_jobs::JobKind::FetchMetadata,
                    parlour_jobs::JobKind::DownloadArtwork,
                ] {
                    match parlour_jobs::claim_next(&pool, kind).await {
                        Ok(Some(job)) => {
                            claimed = Some(job);
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "job claim failed");
                            tokio::time::sleep(Duration::from_secs(10)).await;
                        }
                    }
                }
                match claimed {
                    Some(job) if job.kind == "fetch_metadata" => {
                        crate::metadata::run_metadata_job_claimed(pool.clone(), job, &metadata)
                            .await
                    }
                    Some(job) if job.kind == "download_artwork" => {
                        crate::metadata::run_artwork_job_claimed(pool.clone(), job, &metadata).await
                    }
                    Some(job) => run_scan_job_claimed(pool.clone(), job).await,
                    None => tokio::time::sleep(Duration::from_secs(3)).await,
                }
            }
        });
    }

    // Nightly safety net (C7): after 07:30 UTC (02:30/03:30 in
    // Toronto), queue one full scan per library per day.
    {
        let pool = pool.clone();
        tokio::spawn(async move {
            loop {
                nightly_tick(&pool).await;
                tokio::time::sleep(Duration::from_secs(900)).await;
            }
        });
    }

    // Filesystem watcher (C7), on a blocking thread: `notify`
    // callbacks feed a channel; events are debounced 3 s, then each
    // affected library gets one targeted scan queued.
    {
        let pool = pool.clone();
        let handle = tokio::runtime::Handle::current();
        std::thread::spawn(move || watcher_loop(pool, handle));
    }
}

async fn run_scan_job_claimed(pool: parlour_db::PgPool, job: parlour_jobs::Job) {
    let Some(library_id) = job.library_id else {
        let _ = parlour_jobs::fail(&pool, job.id, "scan job without a library").await;
        return;
    };
    match parlour_catalog::scan_library(&pool, library_id, Some(job.id)).await {
        Ok(report) => {
            tracing::info!(job_id = %job.id, %library_id, seen = report.seen_files,
                "library scan finished");
            let _ = parlour_jobs::complete(&pool, job.id).await;
        }
        Err(e) => {
            let _ = parlour_jobs::fail(&pool, job.id, &e.to_string()).await;
        }
    }
}

async fn nightly_tick(pool: &parlour_db::PgPool) {
    let now = Utc::now();
    if now.hour() < 7 || (now.hour() == 7 && now.minute() < 30) {
        return;
    }
    let start_of_day = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let libraries: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM libraries ORDER BY name")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for (library_id,) in libraries {
        match parlour_jobs::exists_since(
            pool,
            parlour_jobs::JobKind::ScanLibrary,
            Some(library_id),
            start_of_day,
            "nightly",
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                let _ = enqueue_scan(pool, library_id, "nightly").await;
            }
            Err(e) => tracing::warn!(error = %e, "nightly scan check failed"),
        }
    }
}

fn watcher_loop(pool: parlour_db::PgPool, handle: tokio::runtime::Handle) {
    use notify::{RecursiveMode, Watcher};
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    let mut watcher =
        match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                for p in event.paths {
                    let _ = tx.send(p);
                }
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "could not start filesystem watcher");
                return;
            }
        };

    let mut watched: HashSet<String> = HashSet::new();
    let mut last_reload = std::time::Instant::now() - Duration::from_secs(120);
    let mut pending: HashSet<PathBuf> = HashSet::new();

    loop {
        // (Re)load the watch set when it may have changed.
        if last_reload.elapsed() > Duration::from_secs(60) {
            last_reload = std::time::Instant::now();
            let paths = handle.block_on(library_paths(&pool));
            let wanted: HashSet<String> = paths.iter().map(|(_, p)| p.clone()).collect();
            if wanted != watched {
                for p in &wanted {
                    let _ = watcher.watch(std::path::Path::new(p), RecursiveMode::Recursive);
                }
                for p in watched.difference(&wanted) {
                    let _ = watcher.unwatch(std::path::Path::new(p));
                }
                watched = wanted;
            }
        }

        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(p) => {
                pending.insert(p);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }

        // Debounce: act after 3 s without new events.
        if !pending.is_empty() {
            match rx.recv_timeout(Duration::from_secs(3)) {
                Ok(p) => {
                    pending.insert(p);
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
            let changed: Vec<PathBuf> = pending.drain().collect();
            handle.block_on(async {
                let paths = library_paths(&pool).await;
                let mut affected: HashSet<Uuid> = HashSet::new();
                for (library_id, root) in &paths {
                    if changed
                        .iter()
                        .any(|p| p.starts_with(std::path::Path::new(root)))
                    {
                        affected.insert(*library_id);
                    }
                }
                for library_id in affected {
                    match enqueue_scan(&pool, library_id, "watcher").await {
                        Ok(Some(job)) => {
                            tracing::info!(%library_id, %job, "watcher queued a rescan")
                        }
                        Ok(None) => {}
                        Err(e) => tracing::warn!(error = %e, "watcher enqueue failed"),
                    }
                }
            });
        }
    }
}
