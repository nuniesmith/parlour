//! Home rails + search (Milestone 1.5, W3/W6).
//!
//! `GET /api/v1/home` — Continue Watching (per-profile resume
//! state from `user_data`; rows are written by playback reporting
//! in 1.6, the read side lands now), Next Up, Recently Added, and
//! (UI8) the library tile row plus one Latest rail per library —
//! all limited to libraries the active profile can access.
//!
//! `GET /api/v1/search?q=` — Postgres full-text over item titles
//! plus person matches (cast) with the items they appear in.
//!
//! `/api/v1/search/recent` (UI9) — the active profile's recent
//! searches: `GET` lists them newest first, `POST {query}` records
//! one (the web UI calls it when a search is committed — Enter or
//! opening a result — not per keystroke), `DELETE` clears them all
//! and `DELETE /api/v1/search/recent/{id}` forgets one.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
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

/// One tile in Home's library row (UI8).
#[derive(Debug, Serialize, FromRow)]
struct LibraryTileDto {
    id: Uuid,
    name: String,
    kind: String,
    /// Top-level items (movies / series) in the library.
    item_count: i64,
    /// The newest top-level item with a cached backdrop, whose
    /// backdrop the tile shows; `None` → a plain tile.
    art_item_id: Option<Uuid>,
}

/// A Latest row as fetched: an item plus the library it is in.
#[derive(Debug, FromRow)]
struct LatestRow {
    #[sqlx(flatten)]
    item: ItemDto,
    library_name: String,
    library_kind: String,
}

/// One per-library Latest rail (UI8).
#[derive(Debug, Serialize)]
struct LatestRailDto {
    library_id: Uuid,
    library_name: String,
    library_kind: String,
    items: Vec<ItemDto>,
}

/// Items in a Latest rail, per library.
const LATEST_PER_LIBRARY: i64 = 16;

/// Fold rows already ordered by library, then recency, into one
/// rail per library, keeping that order. Libraries with nothing
/// in them have no rows, so they get no rail.
fn group_latest(rows: Vec<LatestRow>) -> Vec<LatestRailDto> {
    let mut rails: Vec<LatestRailDto> = Vec::new();
    for row in rows {
        match rails.last_mut() {
            Some(rail) if rail.library_id == row.item.library_id => rail.items.push(row.item),
            _ => rails.push(LatestRailDto {
                library_id: row.item.library_id,
                library_name: row.library_name,
                library_kind: row.library_kind,
                items: vec![row.item],
            }),
        }
    }
    rails
}

#[derive(Debug, Serialize)]
struct HomeDto {
    continue_watching: Vec<ContinueDto>,
    recently_added: Vec<ItemDto>,
    next_up: Vec<ItemDto>,
    libraries: Vec<LibraryTileDto>,
    latest: Vec<LatestRailDto>,
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

    // P7 Next Up: for each series with a played episode, the
    // first unwatched episode after the latest played one.
    let next_up_rows = sqlx::query_as::<_, ItemDto>(
        "SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider,
                EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                        AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_poster,
                i.created_at, i.updated_at
         FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE i.kind = 'episode'
           AND EXISTS (
             SELECT 1 FROM user_data pu
             JOIN media_items pe ON pe.id = pu.item_id AND pe.kind = 'episode'
             JOIN media_items psn ON psn.id = pe.parent_id
             JOIN media_items isn ON isn.id = i.parent_id
             WHERE pu.profile_id = $1 AND pu.played
               AND isn.parent_id = psn.parent_id
               AND (pe.season_number, pe.episode_number) < (i.season_number, i.episode_number)
           )
           AND NOT EXISTS (
             SELECT 1 FROM user_data u2
             WHERE u2.profile_id = $1 AND u2.item_id = i.id AND u2.played
           )
         ORDER BY i.season_number, i.episode_number
         LIMIT 48",
    )
    .bind(profile_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    // First candidate per season is that season's next episode;
    // one row per series would need the grandparent, so dedupe
    // by season (parent) — good enough for a rail.
    let mut seen = std::collections::HashSet::new();
    let mut next_up: Vec<ItemDto> = Vec::new();
    for row in next_up_rows {
        if let Some(parent) = row.parent_id {
            if seen.insert(parent) {
                next_up.push(row);
            }
        }
        if next_up.len() >= 12 {
            break;
        }
    }

    // UI8 library tile row: every library the profile can see, in
    // the same order as the sidebar.
    let libraries = sqlx::query_as::<_, LibraryTileDto>(
        "SELECT l.id, l.name, l.kind,
                (SELECT count(*) FROM media_items i
                 WHERE i.library_id = l.id AND i.parent_id IS NULL) AS item_count,
                (SELECT i.id FROM media_items i
                 WHERE i.library_id = l.id AND i.parent_id IS NULL
                   AND EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id
                               AND im.kind = 'backdrop' AND im.path <> ''
                               AND im.path NOT LIKE 'pending:%')
                 ORDER BY i.created_at DESC, i.id
                 LIMIT 1) AS art_item_id
         FROM libraries l
         JOIN library_access a ON a.library_id = l.id AND a.profile_id = $1
         ORDER BY l.name",
    )
    .bind(profile_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;

    // UI8 Latest rails: per library, top-level items by their most
    // recent arrival — a series counts as new when any of its
    // seasons or episodes was added, not only when the series row
    // itself was first created.
    let latest_rows = sqlx::query_as::<_, LatestRow>(
        "SELECT r.id, r.library_id, r.parent_id, r.kind, r.title, r.sort_title,
                r.year, r.season_number, r.episode_number, r.overview, r.content_rating,
                r.available, r.genres, r.vote_average, r.metadata_status, r.metadata_provider,
                r.has_poster, r.created_at, r.updated_at, r.library_name, r.library_kind
         FROM (
           SELECT i.id, i.library_id, i.parent_id, i.kind, i.title, i.sort_title,
                  i.year, i.season_number, i.episode_number, i.overview, i.content_rating,
                  i.available, i.genres, i.vote_average, i.metadata_status, i.metadata_provider,
                  EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                          AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_poster,
                  i.created_at, i.updated_at,
                  l.name AS library_name, l.kind AS library_kind,
                  ROW_NUMBER() OVER (
                    PARTITION BY i.library_id
                    ORDER BY GREATEST(i.created_at, COALESCE(d.newest, i.created_at)) DESC, i.id
                  ) AS rn
           FROM media_items i
           JOIN libraries l ON l.id = i.library_id
           JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
           LEFT JOIN LATERAL (
             SELECT max(GREATEST(c.created_at, COALESCE(gc.created_at, c.created_at))) AS newest
             FROM media_items c
             LEFT JOIN media_items gc ON gc.parent_id = c.id
             WHERE c.parent_id = i.id
           ) d ON true
           WHERE i.parent_id IS NULL
         ) r
         WHERE r.rn <= $2
         ORDER BY r.library_name, r.library_id, r.rn",
    )
    .bind(profile_id)
    .bind(LATEST_PER_LIBRARY)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    let latest = group_latest(latest_rows);

    Ok(Json(HomeDto {
        continue_watching,
        recently_added,
        next_up,
        libraries,
        latest,
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

/// Recent searches kept per profile (UI9).
const RECENT_SEARCHES_KEPT: i64 = 10;
/// Longest query worth remembering, in characters.
const RECENT_QUERY_MAX_CHARS: usize = 200;

/// Trim a query for the recent list; reject what search itself
/// would not run (under 2 characters) and anything absurdly long.
fn recent_query(raw: &str) -> Result<String, AppError> {
    let query = raw.trim();
    let chars = query.chars().count();
    if chars < 2 {
        return Err(AppError::BadRequest(
            "a search needs at least 2 characters".into(),
        ));
    }
    if chars > RECENT_QUERY_MAX_CHARS {
        return Err(AppError::BadRequest(format!(
            "a search can be at most {RECENT_QUERY_MAX_CHARS} characters"
        )));
    }
    Ok(query.to_string())
}

#[derive(Debug, Serialize, FromRow)]
struct RecentSearchDto {
    id: Uuid,
    query: String,
    searched_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct RecentSearchBody {
    query: String,
}

fn require_profile(user: &AuthUser) -> Result<Uuid, AppError> {
    user.active_profile_id
        .ok_or_else(|| AppError::BadRequest("select a profile first".into()))
}

async fn fetch_recent(
    state: &AppState,
    profile_id: Uuid,
) -> Result<Vec<RecentSearchDto>, AppError> {
    sqlx::query_as::<_, RecentSearchDto>(
        "SELECT id, query, searched_at FROM recent_searches
         WHERE profile_id = $1
         ORDER BY searched_at DESC, id
         LIMIT $2",
    )
    .bind(profile_id)
    .bind(RECENT_SEARCHES_KEPT)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)
}

async fn list_recent(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<RecentSearchDto>>, AppError> {
    let profile_id = require_profile(&user)?;
    Ok(Json(fetch_recent(&state, profile_id).await?))
}

/// Record a committed search and return the updated list.
async fn add_recent(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<RecentSearchBody>,
) -> Result<Json<Vec<RecentSearchDto>>, AppError> {
    let profile_id = require_profile(&user)?;
    let query = recent_query(&body.query)?;
    let mut tx = state.pool.begin().await.map_err(AppError::from)?;
    sqlx::query(
        "INSERT INTO recent_searches (profile_id, query) VALUES ($1, $2)
         ON CONFLICT (profile_id, lower(query))
         DO UPDATE SET query = EXCLUDED.query, searched_at = now()",
    )
    .bind(profile_id)
    .bind(&query)
    .execute(&mut *tx)
    .await
    .map_err(AppError::from)?;
    sqlx::query(
        "DELETE FROM recent_searches
         WHERE profile_id = $1
           AND id NOT IN (SELECT id FROM recent_searches WHERE profile_id = $1
                          ORDER BY searched_at DESC, id LIMIT $2)",
    )
    .bind(profile_id)
    .bind(RECENT_SEARCHES_KEPT)
    .execute(&mut *tx)
    .await
    .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
    Ok(Json(fetch_recent(&state, profile_id).await?))
}

async fn clear_recent(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<StatusCode, AppError> {
    let profile_id = require_profile(&user)?;
    sqlx::query("DELETE FROM recent_searches WHERE profile_id = $1")
        .bind(profile_id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn forget_recent(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let profile_id = require_profile(&user)?;
    let id = Uuid::parse_str(&id).map_err(|_| AppError::NotFound("no such search".into()))?;
    // Scoped to the active profile: another profile's entry is
    // indistinguishable from a missing one.
    let done = sqlx::query("DELETE FROM recent_searches WHERE id = $1 AND profile_id = $2")
        .bind(id)
        .bind(profile_id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound("no such search".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/home", get(home))
        .route("/api/v1/search", get(search))
        .route(
            "/api/v1/search/recent",
            get(list_recent).post(add_recent).delete(clear_recent),
        )
        .route("/api/v1/search/recent/{id}", delete(forget_recent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn row(library: Uuid, name: &str, title: &str) -> LatestRow {
        LatestRow {
            item: ItemDto {
                id: Uuid::new_v4(),
                library_id: library,
                parent_id: None,
                kind: "movie".into(),
                title: title.into(),
                sort_title: title.into(),
                year: None,
                season_number: None,
                episode_number: None,
                overview: None,
                content_rating: None,
                genres: Vec::new(),
                vote_average: None,
                metadata_status: "matched".into(),
                metadata_provider: None,
                has_poster: false,
                available: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            library_name: name.into(),
            library_kind: "movies".into(),
        }
    }

    #[test]
    fn latest_rows_fold_into_one_rail_per_library_in_order() {
        let films = Uuid::new_v4();
        let shows = Uuid::new_v4();
        let rails = group_latest(vec![
            row(films, "Films", "B"),
            row(films, "Films", "A"),
            row(shows, "Shows", "C"),
        ]);
        assert_eq!(rails.len(), 2);
        assert_eq!(rails[0].library_id, films);
        assert_eq!(rails[0].library_name, "Films");
        let titles: Vec<&str> = rails[0].items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["B", "A"]);
        assert_eq!(rails[1].library_id, shows);
        assert_eq!(rails[1].items.len(), 1);
    }

    #[test]
    fn no_rows_means_no_rails() {
        assert!(group_latest(Vec::new()).is_empty());
    }

    #[test]
    fn recent_query_is_trimmed_and_bounded() {
        assert_eq!(recent_query("  dune  ").unwrap(), "dune");
        assert!(recent_query(" a ").is_err());
        assert!(recent_query("   ").is_err());
        // Counted in characters, not bytes: two accented letters pass.
        assert_eq!(recent_query("été").unwrap(), "été");
        assert!(recent_query("é").is_err());
        assert!(recent_query(&"x".repeat(RECENT_QUERY_MAX_CHARS)).is_ok());
        assert!(recent_query(&"x".repeat(RECENT_QUERY_MAX_CHARS + 1)).is_err());
    }
}
