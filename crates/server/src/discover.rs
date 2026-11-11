//! Home rails + search (Milestone 1.5, W3/W6).
//!
//! `GET /api/v1/home` — Continue Watching (per-profile resume
//! state from `user_data`; rows are written by playback reporting
//! in 1.6, the read side lands now) and Recently Added, both
//! limited to libraries the active profile can access.
//!
//! `GET /api/v1/search?q=` — Postgres full-text over item titles
//! plus person matches (cast) with the items they appear in.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::catalog::ItemDto;
use crate::error::AppError;
use crate::http::AppState;

#[derive(Debug, Serialize, FromRow)]
struct ContinueDto {
    #[sqlx(flatten)]
    #[serde(flatten)]
    item: ItemDto,
    position_ms: i64,
    duration_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct HomeDto {
    continue_watching: Vec<ContinueDto>,
    recently_added: Vec<ItemDto>,
}

async fn home(State(state): State<AppState>, user: AuthUser) -> Result<Json<HomeDto>, AppError> {
    let Some(profile_id) = user.active_profile_id else {
        return Err(AppError::BadRequest("select a profile first".into()));
    };
    let continue_watching = sqlx::query_as::<_, ContinueDto>(
        "SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider,
                EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                        AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_poster,
                i.created_at, i.updated_at, u.position_ms, u.duration_ms
         FROM user_data u
         JOIN media_items i ON i.id = u.item_id
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE u.profile_id = $1 AND NOT u.played AND u.position_ms > 0
         ORDER BY u.last_played_at DESC NULLS LAST, u.updated_at DESC
         LIMIT 24",
    )
    .bind(profile_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    let recently_added = sqlx::query_as::<_, ItemDto>(
        "SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider,
                EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                        AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_poster,
                i.created_at, i.updated_at
         FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE i.parent_id IS NULL
         ORDER BY i.created_at DESC
         LIMIT 24",
    )
    .bind(profile_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    Ok(Json(HomeDto {
        continue_watching,
        recently_added,
    }))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Debug, Serialize, FromRow)]
struct PersonHit {
    id: Uuid,
    name: String,
    #[sqlx(skip)]
    items: Vec<ItemDto>,
}

#[derive(Debug, Serialize)]
struct SearchDto {
    items: Vec<ItemDto>,
    people: Vec<PersonHit>,
}

async fn search(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<SearchQuery>,
) -> Result<Json<SearchDto>, AppError> {
    let Some(profile_id) = user.active_profile_id else {
        return Err(AppError::BadRequest("select a profile first".into()));
    };
    let query = q.q.trim().to_string();
    if query.len() < 2 {
        return Ok(Json(SearchDto {
            items: Vec::new(),
            people: Vec::new(),
        }));
    }
    let like = format!("%{query}%");
    let items = sqlx::query_as::<_, ItemDto>(
        "SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider,
                EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                        AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_poster,
                i.created_at, i.updated_at
         FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE i.parent_id IS NULL
           AND (to_tsvector('english', i.title) @@ plainto_tsquery('english', $2)
                OR i.title ILIKE $3)
         ORDER BY i.sort_title
         LIMIT 60",
    )
    .bind(profile_id)
    .bind(&query)
    .bind(&like)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    let mut people = sqlx::query_as::<_, PersonHit>(
        "SELECT p.id, p.name FROM people p WHERE p.name ILIKE $1 ORDER BY p.name LIMIT 20",
    )
    .bind(&like)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    for person in &mut people {
        person.items = sqlx::query_as::<_, ItemDto>(
            "SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider,
                EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                        AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_poster,
                i.created_at, i.updated_at
             FROM media_items i
             JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
             JOIN item_people ip ON ip.item_id = i.id AND ip.person_id = $2
             WHERE i.parent_id IS NULL
             ORDER BY i.sort_title
             LIMIT 24",
        )
        .bind(profile_id)
        .bind(person.id)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::from)?;
    }

    Ok(Json(SearchDto { items, people }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/home", get(home))
        .route("/api/v1/search", get(search))
}
