//! Audiobook Home shelves (Phase 10, AB8; D29;
//! `docs/AUDIOBOOKS-PODCASTS.md`, "Listening sessions and Home shelves
//! (AB8)").
//!
//! `GET /api/v1/audiobooks/home?library_id=` — Audiobookshelf's book
//! shelves for the active profile, over every audiobooks library it can
//! reach or just the one named:
//!
//! - **Continue Listening** — books in progress (a position past 0),
//!   not finished and not hidden (AB11's `book_progress.hidden`), most
//!   recently updated first;
//! - **Continue Series** — per series where the profile has finished a
//!   book and has none in progress, the first book in sequence order it
//!   hasn't finished; the series it touched most recently first;
//! - **Recently Added** — books added in the last [`RECENT_DAYS`]
//!   days, newest first;
//! - **Recent Series** — series created in the last [`RECENT_DAYS`]
//!   days, newest first;
//! - **Discover** — a random handful of books the profile hasn't
//!   started, where a book in a series is offered only as that series'
//!   first book and only while the profile has started none of it;
//! - **Listen Again** — finished books, the most recently updated
//!   progress first (Audiobookshelf sorts this shelf by the progress
//!   record's update time, not by when the book was finished);
//! - **Newest Authors** — authors whose first book here arrived in the
//!   last [`RECENT_DAYS`] days, newest first.
//!
//! Each shelf holds at most [`PER_SHELF`] entries. Books have the book
//! list's shape (AB3, progress inline), series the series list's and
//! authors the author list's (AB7). Everything is scoped to the
//! profile's library access, as every item query is.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::audiobooks::{active_profile, BookDto, BookRow, BOOKS_SQL};
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::http::AppState;
use crate::series::{AuthorDto, SeriesDto};

/// Entries per shelf (Audiobookshelf's default).
pub const PER_SHELF: i64 = 10;

/// How far back Recently Added, Recent Series and Newest Authors reach
/// (Audiobookshelf's fixed window).
pub const RECENT_DAYS: i32 = 60;

// Sequence order in the queries below is AB7's: the link's sequence
// read as a number (text with no number reads as 0), books without one
// last, then by title.

/// `$1` profile, `$2` library or NULL, `$3` limit.
const CONTINUE_LISTENING_SQL: &str = "
    SELECT i.id
    FROM book_progress bp
    JOIN media_items i ON i.id = bp.item_id AND i.kind = 'audiobook'
    JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
    WHERE bp.profile_id = $1 AND bp.position_ms > 0 AND NOT bp.finished AND NOT bp.hidden
      AND ($2::uuid IS NULL OR i.library_id = $2)
    ORDER BY bp.updated_at DESC, i.id
    LIMIT $3";

/// `$1` profile, `$2` library or NULL, `$3` limit. One row per series:
/// the series, and the book to continue with.
const CONTINUE_SERIES_SQL: &str = "
    WITH links AS (
        SELECT l.series_id, s.name AS series_name, l.item_id, i.sort_title,
               CASE WHEN l.sequence IS NOT NULL
                    THEN COALESCE(substring(l.sequence FROM '^ *(-?[0-9]+([.][0-9]+)?)')::numeric, 0)
               END AS seq,
               COALESCE(bp.finished, false) AS finished,
               COALESCE(bp.position_ms, 0) AS position_ms,
               bp.updated_at
        FROM book_series l
        JOIN series s ON s.id = l.series_id
        JOIN media_items i ON i.id = l.item_id AND i.kind = 'audiobook'
        JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
        LEFT JOIN book_progress bp ON bp.item_id = i.id AND bp.profile_id = $1
        WHERE ($2::uuid IS NULL OR s.library_id = $2)
    ),
    eligible AS (
        SELECT series_id, MAX(updated_at) AS touched
        FROM links
        GROUP BY series_id
        HAVING bool_or(finished)
           AND NOT bool_or(NOT finished AND position_ms > 0)
           AND bool_or(NOT finished)
    ),
    next_book AS (
        SELECT DISTINCT ON (k.series_id) k.series_id, k.series_name, k.item_id, e.touched
        FROM links k
        JOIN eligible e ON e.series_id = k.series_id
        WHERE NOT k.finished
        ORDER BY k.series_id, k.seq NULLS LAST, k.sort_title, k.item_id
    )
    SELECT series_id, series_name, item_id
    FROM next_book
    ORDER BY touched DESC NULLS LAST, series_id
    LIMIT $3";

/// `$1` profile, `$2` library or NULL, `$3` limit, `$4` days back.
const RECENTLY_ADDED_SQL: &str = "
    SELECT i.id
    FROM media_items i
    JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
    WHERE i.kind = 'audiobook' AND ($2::uuid IS NULL OR i.library_id = $2)
      AND i.created_at > now() - make_interval(days => $4)
    ORDER BY i.created_at DESC, i.id
    LIMIT $3";

/// `$1` profile, `$2` library or NULL, `$3` limit, `$4` days back. The
/// series list's
/// shape (AB7), newest series first.
const RECENT_SERIES_SQL: &str = "
    SELECT s.id, s.library_id, s.name, s.sort_name,
           COUNT(*) AS book_count,
           COUNT(*) FILTER (WHERE bp.finished) AS finished_count,
           (SELECT l2.item_id FROM book_series l2
            WHERE l2.series_id = s.id
              AND EXISTS (SELECT 1 FROM images im WHERE im.item_id = l2.item_id
                          AND im.kind = 'poster' AND im.path <> ''
                          AND im.path NOT LIKE 'pending:%')
            ORDER BY CASE WHEN l2.sequence IS NOT NULL THEN COALESCE(substring(
                         l2.sequence FROM '^ *(-?[0-9]+([.][0-9]+)?)')::numeric, 0) END
                     NULLS LAST,
                     l2.item_id
            LIMIT 1) AS cover_book_id
    FROM series s
    JOIN library_access a ON a.library_id = s.library_id AND a.profile_id = $1
    JOIN book_series l ON l.series_id = s.id
    JOIN media_items i ON i.id = l.item_id AND i.kind = 'audiobook'
    LEFT JOIN book_progress bp ON bp.item_id = i.id AND bp.profile_id = $1
    WHERE ($2::uuid IS NULL OR s.library_id = $2)
      AND s.created_at > now() - make_interval(days => $4)
    GROUP BY s.id
    ORDER BY s.created_at DESC, s.id
    LIMIT $3";

/// `$1` profile, `$2` library or NULL, `$3` limit. Random each time.
const DISCOVER_SQL: &str = "
    WITH firsts AS (
        SELECT DISTINCT ON (l.series_id) l.series_id, l.item_id
        FROM book_series l
        JOIN media_items i ON i.id = l.item_id AND i.kind = 'audiobook'
        ORDER BY l.series_id,
                 CASE WHEN l.sequence IS NOT NULL
                      THEN COALESCE(substring(l.sequence FROM '^ *(-?[0-9]+([.][0-9]+)?)')::numeric, 0)
                 END NULLS LAST,
                 i.sort_title, i.id
    ),
    started AS (
        SELECT DISTINCT l.series_id
        FROM book_series l
        JOIN book_progress bp ON bp.item_id = l.item_id AND bp.profile_id = $1
        WHERE bp.finished OR bp.position_ms > 0
    )
    SELECT i.id
    FROM media_items i
    JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
    LEFT JOIN book_progress bp ON bp.item_id = i.id AND bp.profile_id = $1
    WHERE i.kind = 'audiobook' AND ($2::uuid IS NULL OR i.library_id = $2)
      AND (bp.item_id IS NULL OR (bp.position_ms = 0 AND NOT bp.finished))
      AND NOT EXISTS (
          SELECT 1 FROM book_series l
          WHERE l.item_id = i.id
            AND (l.series_id IN (SELECT series_id FROM started)
                 OR NOT EXISTS (SELECT 1 FROM firsts f
                                WHERE f.series_id = l.series_id AND f.item_id = i.id)))
    ORDER BY random()
    LIMIT $3";

/// `$1` profile, `$2` library or NULL, `$3` limit.
const LISTEN_AGAIN_SQL: &str = "
    SELECT i.id
    FROM book_progress bp
    JOIN media_items i ON i.id = bp.item_id AND i.kind = 'audiobook'
    JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
    WHERE bp.profile_id = $1 AND bp.finished
      AND ($2::uuid IS NULL OR i.library_id = $2)
    ORDER BY bp.updated_at DESC, i.id
    LIMIT $3";

/// `$1` profile, `$2` library or NULL, `$3` limit, `$4` days back. The
/// author list's shape (AB7); an author is as new as their first book
/// here.
const NEWEST_AUTHORS_SQL: &str = "
    SELECT p.id, p.name, COUNT(DISTINCT i.id) AS book_count
    FROM people p
    JOIN item_people ip ON ip.person_id = p.id AND ip.role = 'author'
    JOIN media_items i ON i.id = ip.item_id AND i.kind = 'audiobook'
    JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
    WHERE ($2::uuid IS NULL OR i.library_id = $2)
    GROUP BY p.id
    HAVING MIN(i.created_at) > now() - make_interval(days => $4)
    ORDER BY MIN(i.created_at) DESC, lower(p.name), p.id
    LIMIT $3";

#[derive(Debug, FromRow)]
struct ContinueSeriesRow {
    series_id: Uuid,
    series_name: String,
    item_id: Uuid,
}

/// A Continue Series entry: the series and the book to go on with.
#[derive(Debug, Serialize)]
pub struct ContinueSeriesDto {
    pub series_id: Uuid,
    pub series_name: String,
    pub book: BookDto,
}

#[derive(Debug, Serialize)]
pub struct ShelvesDto {
    pub continue_listening: Vec<BookDto>,
    pub continue_series: Vec<ContinueSeriesDto>,
    pub recently_added: Vec<BookDto>,
    pub recent_series: Vec<SeriesDto>,
    pub discover: Vec<BookDto>,
    pub listen_again: Vec<BookDto>,
    pub newest_authors: Vec<AuthorDto>,
}

/// Put books back in the order a shelf's ids came in; ids with no book
/// (none should go missing between the two reads) are skipped.
pub fn in_order(ids: &[Uuid], books: &mut HashMap<Uuid, BookDto>) -> Vec<BookDto> {
    ids.iter().filter_map(|id| books.remove(id)).collect()
}

async fn ids(
    app: &AppState,
    sql: &'static str,
    profile_id: Uuid,
    library_id: Option<Uuid>,
    days: Option<i32>,
) -> Result<Vec<Uuid>, AppError> {
    let mut query = sqlx::query_as::<_, (Uuid,)>(sql)
        .bind(profile_id)
        .bind(library_id)
        .bind(PER_SHELF);
    if let Some(days) = days {
        query = query.bind(days);
    }
    let rows = query.fetch_all(&app.pool).await.map_err(AppError::from)?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[derive(Deserialize)]
struct HomeQuery {
    library_id: Option<String>,
}

async fn home(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<HomeQuery>,
) -> Result<Json<ShelvesDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let library_id = match q.library_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => {
            Some(Uuid::parse_str(s).map_err(|_| AppError::BadRequest("bad library_id".into()))?)
        }
        None => None,
    };

    let continue_listening =
        ids(&app, CONTINUE_LISTENING_SQL, profile_id, library_id, None).await?;
    let continue_series = sqlx::query_as::<_, ContinueSeriesRow>(CONTINUE_SERIES_SQL)
        .bind(profile_id)
        .bind(library_id)
        .bind(PER_SHELF)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    let recently_added = ids(
        &app,
        RECENTLY_ADDED_SQL,
        profile_id,
        library_id,
        Some(RECENT_DAYS),
    )
    .await?;
    let discover = ids(&app, DISCOVER_SQL, profile_id, library_id, None).await?;
    let listen_again = ids(&app, LISTEN_AGAIN_SQL, profile_id, library_id, None).await?;
    let recent_series = sqlx::query_as::<_, SeriesDto>(RECENT_SERIES_SQL)
        .bind(profile_id)
        .bind(library_id)
        .bind(PER_SHELF)
        .bind(RECENT_DAYS)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    let newest_authors = sqlx::query_as::<_, AuthorDto>(NEWEST_AUTHORS_SQL)
        .bind(profile_id)
        .bind(library_id)
        .bind(PER_SHELF)
        .bind(RECENT_DAYS)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;

    // Every book any shelf shows, read once in the book list's shape.
    let mut wanted: Vec<Uuid> = continue_listening
        .iter()
        .chain(&recently_added)
        .chain(&discover)
        .chain(&listen_again)
        .copied()
        .chain(continue_series.iter().map(|r| r.item_id))
        .collect();
    wanted.sort();
    wanted.dedup();
    let rows = sqlx::query_as::<_, BookRow>(BOOKS_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .bind(None::<String>)
        .bind("title")
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .bind(Some(wanted))
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    let books: HashMap<Uuid, BookDto> = rows
        .into_iter()
        .map(BookDto::from)
        .map(|b| (b.id, b))
        .collect();
    // A book can sit on several shelves, so each shelf takes copies.
    let take = |ids: &[Uuid]| -> Vec<BookDto> {
        let mut copy: HashMap<Uuid, BookDto> = ids
            .iter()
            .filter_map(|id| books.get(id).map(|b| (*id, b.clone())))
            .collect();
        in_order(ids, &mut copy)
    };
    let continue_series = continue_series
        .into_iter()
        .filter_map(|r| {
            books.get(&r.item_id).map(|b| ContinueSeriesDto {
                series_id: r.series_id,
                series_name: r.series_name,
                book: b.clone(),
            })
        })
        .collect();
    Ok(Json(ShelvesDto {
        continue_listening: take(&continue_listening),
        continue_series,
        recently_added: take(&recently_added),
        recent_series,
        discover: take(&discover),
        listen_again: take(&listen_again),
        newest_authors,
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/audiobooks/home", get(home))
}
