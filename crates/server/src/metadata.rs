//! Metadata API + job runners (Milestone 1.4, MD1–MD4).
//!
//! Settings come from the server config: TMDB key
//! (`PARLOUR_TMDB_API_KEY` or `TMDB_API_KEY`), optional TVDB key
//! (MD5), provider base overrides for tests, and the artwork
//! cache directory. Without any key, the metadata endpoints
//! answer 503 — everything else keeps working.
//!
//! Admin surface (MD4):
//! - `POST /admin/libraries/{id}/metadata` — match a library (job)
//! - `POST /admin/items/{id}/metadata/fetch` — re-match one item (job)
//! - `GET  /admin/metadata/review-queue` — needs_review items +
//!   their stored candidates
//! - `GET  /admin/metadata/search` — live provider search for the
//!   fix-match flow
//! - `POST /admin/items/{id}/metadata/apply` — apply a chosen
//!   provider record (manual fix)
//! - `POST /admin/items/{id}/artwork` — cache artwork (job)
//!
//! Users: `GET /items/{id}/artwork/{kind}` serves the cached file.

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use parlour_metadata::{AnyProvider, MediaKind, MetadataProvider};

use crate::auth::{AdminUser, AuthUser};
use crate::config::Config;
use crate::error::AppError;
use crate::http::AppState;

#[derive(Debug, Clone)]
pub struct MetadataSettings {
    pub tmdb_api_key: Option<String>,
    pub tvdb_api_key: Option<String>,
    pub tmdb_base: Option<String>,
    pub tvdb_base: Option<String>,
    pub artwork_dir: PathBuf,
}

impl Default for MetadataSettings {
    fn default() -> Self {
        Self {
            tmdb_api_key: None,
            tvdb_api_key: None,
            tmdb_base: None,
            tvdb_base: None,
            artwork_dir: PathBuf::from("artwork"),
        }
    }
}

impl MetadataSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            tmdb_api_key: config.tmdb_api_key.clone(),
            tvdb_api_key: config.tvdb_api_key.clone(),
            tmdb_base: config.tmdb_base.clone(),
            tvdb_base: config.tvdb_base.clone(),
            artwork_dir: config.artwork_dir.clone(),
        }
    }

    pub fn tmdb(&self) -> Option<parlour_metadata::tmdb::TmdbClient> {
        self.tmdb_api_key.as_ref().map(|key| match &self.tmdb_base {
            Some(base) => parlour_metadata::tmdb::TmdbClient::with_base(key.clone(), base),
            None => parlour_metadata::tmdb::TmdbClient::new(key.clone()),
        })
    }

    pub fn tvdb(&self) -> Option<parlour_metadata::tvdb::TvdbClient> {
        self.tvdb_api_key.as_ref().map(|key| match &self.tvdb_base {
            Some(base) => parlour_metadata::tvdb::TvdbClient::with_base(key.clone(), base),
            None => parlour_metadata::tvdb::TvdbClient::new(key.clone()),
        })
    }

    /// The active provider: TMDB when keyed, else TVDB.
    pub fn provider(&self) -> Option<AnyProvider> {
        if let Some(t) = self.tmdb() {
            return Some(AnyProvider::Tmdb(t));
        }
        self.tvdb().map(AnyProvider::Tvdb)
    }

    /// A named provider (for manual apply against a specific one).
    pub fn provider_named(&self, name: &str) -> Option<AnyProvider> {
        match name {
            "tmdb" => self.tmdb().map(AnyProvider::Tmdb),
            "tvdb" => self.tvdb().map(AnyProvider::Tvdb),
            _ => None,
        }
    }
}

fn parse_id(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound("no such item".into()))
}

fn provider_or_503(state: &AppState) -> Result<AnyProvider, AppError> {
    state
        .metadata
        .provider()
        .ok_or_else(|| AppError::Internal("no metadata provider is configured".into()))
}

// ---------------------------------------------------------------- job runners

pub async fn run_metadata_job_claimed(
    pool: parlour_db::PgPool,
    job: parlour_jobs::Job,
    settings: &MetadataSettings,
) {
    let Some(provider) = settings.provider() else {
        let _ = parlour_jobs::fail(&pool, job.id, "no metadata provider is configured").await;
        return;
    };
    let item_id = job
        .payload
        .get("item_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let result = if let Some(item_id) = item_id {
        parlour_catalog::match_item(&pool, &provider, item_id)
            .await
            .map(|o| serde_json::json!({ "item": o }))
            .map_err(|e| e.to_string())
    } else if let Some(library_id) = job.library_id {
        parlour_catalog::match_library(&pool, &provider, library_id, Some(job.id))
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("metadata job without a library or item".to_string())
    };
    match result {
        Ok(summary) => {
            let _ = parlour_jobs::set_result(&pool, job.id, summary).await;
            let _ = parlour_jobs::complete(&pool, job.id).await;
        }
        Err(e) => {
            let _ = parlour_jobs::fail(&pool, job.id, &e).await;
        }
    }
}

pub async fn run_artwork_job_claimed(
    pool: parlour_db::PgPool,
    job: parlour_jobs::Job,
    settings: &MetadataSettings,
) {
    let http = reqwest::Client::new();
    let item_id = job
        .payload
        .get("item_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    let result = if let Some(item_id) = item_id {
        parlour_catalog::download_item_artwork(&pool, &http, &settings.artwork_dir, item_id)
            .await
            .map(|r| serde_json::to_value(r).unwrap_or(serde_json::Value::Null))
            .map_err(|e| e.to_string())
    } else if let Some(library_id) = job.library_id {
        parlour_catalog::download_library_artwork(
            &pool,
            &http,
            &settings.artwork_dir,
            library_id,
            Some(job.id),
        )
        .await
        .map_err(|e| e.to_string())
    } else {
        Err("artwork job without a library or item".to_string())
    };
    match result {
        Ok(summary) => {
            let _ = parlour_jobs::set_result(&pool, job.id, summary).await;
            let _ = parlour_jobs::complete(&pool, job.id).await;
        }
        Err(e) => {
            let _ = parlour_jobs::fail(&pool, job.id, &e).await;
        }
    }
}

/// Enqueue + immediately spawn a runner for a job (the background
/// worker is the backstop if the process dies in between).
async fn enqueue_and_spawn(
    state: &AppState,
    kind: parlour_jobs::JobKind,
    library_id: Option<Uuid>,
    payload: serde_json::Value,
) -> Result<parlour_jobs::Job, AppError> {
    let job_id = parlour_jobs::enqueue(&state.pool, kind, library_id, payload)
        .await
        .map_err(AppError::from)?;
    let pool = state.pool.clone();
    let settings = state.metadata.clone();
    tokio::spawn(async move {
        if parlour_jobs::claim_by_id(&pool, job_id)
            .await
            .unwrap_or(false)
        {
            if let Ok(Some(job)) = parlour_jobs::get(&pool, job_id).await {
                match kind {
                    parlour_jobs::JobKind::FetchMetadata => {
                        run_metadata_job_claimed(pool, job, &settings).await
                    }
                    parlour_jobs::JobKind::DownloadArtwork => {
                        run_artwork_job_claimed(pool, job, &settings).await
                    }
                    _ => {}
                }
            }
        }
    });
    parlour_jobs::get(&state.pool, job_id)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::Internal("job vanished".into()))
}

async fn library_of_item(state: &AppState, item_id: Uuid) -> Result<Uuid, AppError> {
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT library_id FROM media_items WHERE id = $1")
        .bind(item_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::from)?;
    row.map(|(l,)| l)
        .ok_or_else(|| AppError::NotFound("no such item".into()))
}

// ---------------------------------------------------------------- endpoints

async fn match_library_endpoint(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let _ = provider_or_503(&state)?;
    let library_id = parse_id(&id)?;
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM libraries WHERE id = $1")
        .bind(library_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::from)?;
    if exists.is_none() {
        return Err(AppError::NotFound("no such library".into()));
    }
    let job = enqueue_and_spawn(
        &state,
        parlour_jobs::JobKind::FetchMetadata,
        Some(library_id),
        serde_json::json!({ "trigger": "manual" }),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn match_item_endpoint(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let _ = provider_or_503(&state)?;
    let item_id = parse_id(&id)?;
    let library_id = library_of_item(&state, item_id).await?;
    let job = enqueue_and_spawn(
        &state,
        parlour_jobs::JobKind::FetchMetadata,
        Some(library_id),
        serde_json::json!({ "trigger": "manual", "item_id": item_id }),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn artwork_item_endpoint(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let item_id = parse_id(&id)?;
    let library_id = library_of_item(&state, item_id).await?;
    let job = enqueue_and_spawn(
        &state,
        parlour_jobs::JobKind::DownloadArtwork,
        Some(library_id),
        serde_json::json!({ "trigger": "manual", "item_id": item_id }),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Debug, Serialize, FromRow)]
struct CandidateDto {
    provider: String,
    provider_id: i64,
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    poster_url: Option<String>,
    confidence: f64,
}

#[derive(Debug, Serialize, FromRow)]
struct ReviewRow {
    id: Uuid,
    library_id: Uuid,
    kind: String,
    title: String,
    year: Option<i32>,
    #[sqlx(skip)]
    candidates: Vec<CandidateDto>,
}

async fn review_queue(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<ReviewRow>>, AppError> {
    let mut rows = sqlx::query_as::<_, ReviewRow>(
        "SELECT id, library_id, kind, title, year FROM media_items
         WHERE metadata_status = 'needs_review' ORDER BY title LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    for row in &mut rows {
        row.candidates = sqlx::query_as::<_, CandidateDto>(
            "SELECT provider, provider_id, title, year, overview, poster_url, confidence
             FROM metadata_candidates WHERE item_id = $1 ORDER BY confidence DESC",
        )
        .bind(row.id)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::from)?;
    }
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct SearchQuery {
    kind: String,
    q: String,
    year: Option<i32>,
}

#[derive(Serialize)]
struct ScoredCandidate {
    #[serde(flatten)]
    candidate: parlour_metadata::Candidate,
    confidence: f64,
}

async fn search_endpoint(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<ScoredCandidate>>, AppError> {
    let provider = provider_or_503(&state)?;
    let kind = match q.kind.as_str() {
        "movie" => MediaKind::Movie,
        "show" | "series" => MediaKind::Show,
        _ => return Err(AppError::BadRequest("kind must be movie or show".into())),
    };
    let results = provider
        .search(kind, &q.q, q.year)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut scored: Vec<ScoredCandidate> = results
        .into_iter()
        .map(|c| ScoredCandidate {
            confidence: parlour_metadata::score_candidate(&q.q, q.year, &c),
            candidate: c,
        })
        .collect();
    scored.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(Json(scored))
}

#[derive(Deserialize)]
struct ApplyBody {
    provider_id: i64,
    provider: Option<String>,
}

async fn apply_endpoint(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<ApplyBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let item_id = parse_id(&id)?;
    let provider = match &body.provider {
        Some(name) => state
            .metadata
            .provider_named(name)
            .ok_or_else(|| AppError::BadRequest(format!("provider {name} is not configured")))?,
        None => provider_or_503(&state)?,
    };
    parlour_catalog::apply_manual(&state.pool, &provider, item_id, body.provider_id)
        .await
        .map_err(|e| match e {
            parlour_catalog::MatchError::NotFound => AppError::NotFound("no such item".into()),
            parlour_catalog::MatchError::Skipped(k) => {
                AppError::BadRequest(format!("{k} items do not take metadata directly"))
            }
            other => AppError::Internal(other.to_string()),
        })?;
    // Cache the new artwork right away so the fix is visible.
    let http = reqwest::Client::new();
    let _ = parlour_catalog::download_item_artwork(
        &state.pool,
        &http,
        &state.metadata.artwork_dir,
        item_id,
    )
    .await;
    Ok(Json(serde_json::json!({ "status": "matched" })))
}

/// Serve a cached artwork file for an item the profile can access.
#[derive(Deserialize)]
struct ArtworkQuery {
    v: Option<String>,
}

async fn serve_artwork(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, kind)): Path<(String, String)>,
    Query(q): Query<ArtworkQuery>,
) -> Result<Response, AppError> {
    let Some(profile_id) = user.active_profile_id else {
        return Err(AppError::BadRequest("select a profile first".into()));
    };
    if !matches!(kind.as_str(), "poster" | "backdrop" | "logo" | "still") {
        return Err(AppError::NotFound("no such artwork kind".into()));
    }
    let item_id = parse_id(&id)?;
    let preferred = q.v.as_deref().unwrap_or("original");
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT img.path FROM images img
         JOIN media_items i ON i.id = img.item_id
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $2
         WHERE img.item_id = $1 AND img.kind = $3 AND img.path <> '' AND img.path NOT LIKE 'pending:%'
         ORDER BY CASE img.variant WHEN $4 THEN 0 WHEN 'original' THEN 1 ELSE 2 END
         LIMIT 1",
    )
    .bind(item_id)
    .bind(profile_id)
    .bind(&kind)
    .bind(preferred)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;
    let Some((path,)) = row else {
        return Err(AppError::NotFound("no artwork cached for that item".into()));
    };
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound("artwork file is missing".into()))?;
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime.as_ref())],
        bytes,
    )
        .into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/admin/libraries/{id}/metadata",
            post(match_library_endpoint),
        )
        .route(
            "/api/v1/admin/items/{id}/metadata/fetch",
            post(match_item_endpoint),
        )
        .route(
            "/api/v1/admin/items/{id}/metadata/apply",
            post(apply_endpoint),
        )
        .route(
            "/api/v1/admin/items/{id}/artwork",
            post(artwork_item_endpoint),
        )
        .route("/api/v1/admin/metadata/review-queue", get(review_queue))
        .route("/api/v1/admin/metadata/search", get(search_endpoint))
        .route("/api/v1/items/{id}/artwork/{kind}", get(serve_artwork))
}
