//! AB6 (D30): matching one audiobook by hand, and an audiobooks
//! library's metadata precedence. Spec: `docs/AUDIOBOOKS-PODCASTS.md`,
//! "Metadata sources, matching and covers (AB6)". Admins only.
//!
//! - `GET  /api/v1/admin/audiobooks/{id}/match/search?provider=&title=&author=`
//!   — search a book provider (`google`, the default, or
//!   `openlibrary`); title and author default to the book's own.
//! - `POST /api/v1/admin/audiobooks/{id}/match` —
//!   `{"provider", "provider_id", "mode": "overwrite" | "fill_gaps",
//!   "cover": true}`: apply one result. The admin chooses the mode;
//!   there is no default.
//! - `DELETE /api/v1/admin/audiobooks/{id}/match` — forget the match;
//!   the next scan merges the book from its local sources again.
//! - `GET | PUT /api/v1/admin/libraries/{id}/metadata-precedence` —
//!   `{"sources": [...]}`, the order a library's books merge their
//!   local sources in, later winning.
//!
//! A library-wide quick match is `POST /admin/libraries/{id}/metadata`
//! on an audiobooks library (`metadata.rs`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use parlour_catalog::book_meta::{
    default_precedence_ids, parse_precedence, stored_precedence, MatchMode, MetadataSource,
};
use parlour_catalog::book_sync::{apply_book_match, forget_book_match, BookMatchError};
use parlour_metadata::books::BookCandidate;

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;

fn parse_id(id: &str, what: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound(format!("no such {what}")))
}

/// The book's title and first author, or 404 when the id isn't a
/// book.
async fn book(state: &AppState, id: Uuid) -> Result<(String, Option<String>), AppError> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT i.title,
                (SELECT p.name FROM item_people ip JOIN people p ON p.id = ip.person_id
                  WHERE ip.item_id = i.id AND ip.role = 'author'
                  ORDER BY ip.sort_order LIMIT 1)
         FROM media_items i WHERE i.id = $1 AND i.kind = 'audiobook'",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;
    row.ok_or_else(|| AppError::NotFound("no such audiobook".into()))
}

fn provider(
    state: &AppState,
    name: Option<&str>,
) -> Result<parlour_metadata::books::BookProvider, AppError> {
    let name = name.unwrap_or("google");
    state.metadata.book_provider(name).ok_or_else(|| {
        AppError::BadRequest(format!(
            "unknown book provider {name:?}; the providers are {}",
            parlour_metadata::books::BOOK_PROVIDERS.join(", ")
        ))
    })
}

#[derive(Deserialize)]
struct SearchQuery {
    provider: Option<String>,
    title: Option<String>,
    author: Option<String>,
}

#[derive(Serialize)]
struct ScoredBook {
    #[serde(flatten)]
    candidate: BookCandidate,
    /// How close the result's title is to the searched title (0–1),
    /// for display; results keep the provider's order.
    confidence: f64,
}

#[derive(Serialize)]
struct SearchResult {
    provider: String,
    title: String,
    author: Option<String>,
    results: Vec<ScoredBook>,
}

async fn search(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<SearchResult>, AppError> {
    let item_id = parse_id(&id, "audiobook")?;
    let (book_title, book_author) = book(&state, item_id).await?;
    let provider = provider(&state, q.provider.as_deref())?;
    let title = q
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or(book_title);
    let author = match q.author {
        Some(a) => Some(a.trim().to_string()).filter(|a| !a.is_empty()),
        None => book_author,
    };
    let found = provider
        .search(&title, author.as_deref())
        .await
        .map_err(|e| AppError::Internal(format!("{} search failed: {e}", provider.name())))?;
    let results = found
        .into_iter()
        .map(|c| ScoredBook {
            confidence: parlour_metadata::title_similarity(&title, &c.title),
            candidate: c,
        })
        .collect();
    Ok(Json(SearchResult {
        provider: provider.name().to_string(),
        title,
        author,
        results,
    }))
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyBody {
    provider: String,
    provider_id: String,
    /// `overwrite` or `fill_gaps`: required, the admin's choice (D30).
    mode: String,
    /// Take the provider's cover (with `fill_gaps`, only when the book
    /// has none). Default true.
    #[serde(default = "default_true")]
    cover: bool,
}

async fn apply(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<ApplyBody>,
) -> Result<Json<parlour_catalog::book_sync::BookMatchReport>, AppError> {
    let item_id = parse_id(&id, "audiobook")?;
    book(&state, item_id).await?;
    let mode = MatchMode::parse(&body.mode).ok_or_else(|| {
        AppError::BadRequest("mode must be \"overwrite\" or \"fill_gaps\"".into())
    })?;
    let provider = provider(&state, Some(&body.provider))?;
    let candidate = provider
        .get_book(body.provider_id.trim())
        .await
        .map_err(|e| match e {
            parlour_metadata::ProviderError::Other(m) => AppError::BadRequest(m),
            parlour_metadata::ProviderError::Status(404) => AppError::NotFound(format!(
                "{} has no book {}",
                provider.name(),
                body.provider_id
            )),
            other => AppError::Internal(format!("{} lookup failed: {other}", provider.name())),
        })?;
    let report = apply_book_match(
        &state.pool,
        &parlour_metadata::books::http_client(),
        &state.metadata.artwork_dir,
        item_id,
        &candidate,
        mode,
        body.cover,
        true,
    )
    .await
    .map_err(|e| match e {
        BookMatchError::NotFound => AppError::NotFound("no such audiobook".into()),
        BookMatchError::Db(e) => AppError::Internal(e.to_string()),
    })?;
    for e in &report.written.errors {
        tracing::warn!(%item_id, error = %e, "book match: a write beside the book failed");
    }
    Ok(Json(report))
}

#[derive(Serialize)]
struct Forgotten {
    forgotten: bool,
}

async fn forget(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Json<Forgotten>, AppError> {
    let item_id = parse_id(&id, "audiobook")?;
    book(&state, item_id).await?;
    let forgotten = forget_book_match(&state.pool, item_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(Forgotten { forgotten }))
}

// ---------------------------------------------------------- precedence

#[derive(Serialize)]
struct Precedence {
    library_id: Uuid,
    /// The library's order, first to last; later sources win.
    sources: Vec<String>,
    /// D30's default (Audiobookshelf's order, Parlour's sidecar last).
    default: Vec<String>,
    /// Every source id there is.
    available: Vec<String>,
}

async fn audiobooks_library(state: &AppState, id: Uuid) -> Result<Vec<String>, AppError> {
    let row: Option<(String, Vec<String>)> =
        sqlx::query_as("SELECT kind, metadata_precedence FROM libraries WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::from)?;
    match row {
        None => Err(AppError::NotFound("no such library".into())),
        Some((kind, _)) if kind != "audiobooks" => Err(AppError::BadRequest(
            "only an audiobooks library has a metadata precedence".into(),
        )),
        Some((_, list)) => Ok(list),
    }
}

fn precedence(library_id: Uuid, stored: &[String]) -> Precedence {
    Precedence {
        library_id,
        sources: stored_precedence(stored)
            .iter()
            .map(|s| s.id().to_string())
            .collect(),
        default: default_precedence_ids(),
        available: MetadataSource::ALL
            .iter()
            .map(|s| s.id().to_string())
            .collect(),
    }
}

async fn get_precedence(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Json<Precedence>, AppError> {
    let library_id = parse_id(&id, "library")?;
    let stored = audiobooks_library(&state, library_id).await?;
    Ok(Json(precedence(library_id, &stored)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrecedenceBody {
    sources: Vec<String>,
}

/// Set a library's precedence. A change takes effect at the next scan,
/// which merges every book of the library again (the order is part of
/// each book's sources fingerprint).
async fn put_precedence(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<PrecedenceBody>,
) -> Result<(StatusCode, Json<Precedence>), AppError> {
    let library_id = parse_id(&id, "library")?;
    audiobooks_library(&state, library_id).await?;
    let list = parse_precedence(&body.sources).map_err(AppError::BadRequest)?;
    let ids: Vec<String> = list.iter().map(|s| s.id().to_string()).collect();
    sqlx::query("UPDATE libraries SET metadata_precedence = $2 WHERE id = $1")
        .bind(library_id)
        .bind(&ids)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    Ok((StatusCode::OK, Json(precedence(library_id, &ids))))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/audiobooks/{id}/match/search", get(search))
        .route(
            "/api/v1/admin/audiobooks/{id}/match",
            post(apply).delete(forget),
        )
        .route(
            "/api/v1/admin/libraries/{id}/metadata-precedence",
            get(get_precedence).put(put_precedence),
        )
}
