//! Series and author browsing for audiobooks (Phase 10, AB7, D28;
//! `docs/AUDIOBOOKS-PODCASTS.md`, "Series and authors (AB7)").
//!
//! - `GET /api/v1/series?library_id=&q=` — the series the active
//!   profile can reach, by name, with how many books each has, how
//!   many of them this profile has finished, and a book to take the
//!   cover from
//! - `GET /api/v1/series/{id}` — one series and its books in
//!   sequence order (the book list's shape, AB3)
//! - `GET /api/v1/authors?library_id=&q=` — the people credited as
//!   author on an audiobook the profile can reach, by name, with
//!   their book counts
//! - `GET /api/v1/authors/{id}?library_id=` — one author: their
//!   books by title, and the series those books sit in
//!
//! Series are `series` records joined to books through `book_series`
//! with a sequence on the link (D28). Authors are the `people` that
//! `item_people` links to books with role `author` (AB1): an author
//! page is a query. Everything is scoped to the active profile's
//! library access; a series or author it can't reach reads as 404.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::audiobooks::{active_profile, BookDto, BookRow, BOOKS_SQL};
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::http::AppState;

#[derive(Deserialize)]
struct ListQuery {
    library_id: Option<String>,
    q: Option<String>,
}

fn library_filter(raw: Option<&str>) -> Result<Option<Uuid>, AppError> {
    match raw.filter(|s| !s.is_empty()) {
        Some(s) => Uuid::parse_str(s)
            .map(Some)
            .map_err(|_| AppError::BadRequest("bad library_id".into())),
        None => Ok(None),
    }
}

fn search_text(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn books(
    app: &AppState,
    profile_id: Uuid,
    library_id: Option<Uuid>,
    series_id: Option<Uuid>,
    author_id: Option<Uuid>,
) -> Result<Vec<BookDto>, AppError> {
    let rows = sqlx::query_as::<_, BookRow>(BOOKS_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(library_id)
        .bind(None::<String>)
        .bind("title")
        .bind(series_id)
        .bind(author_id)
        .bind(None::<Vec<Uuid>>)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(rows.into_iter().map(BookDto::from).collect())
}

// --------------------------------------------------------------- series

#[derive(Debug, Serialize, FromRow)]
pub struct SeriesDto {
    pub id: Uuid,
    pub library_id: Uuid,
    pub name: String,
    pub sort_name: String,
    /// Books in the series the profile can reach.
    pub book_count: i64,
    /// Of those, the ones this profile has finished.
    pub finished_count: i64,
    /// The first book in sequence order that has a cover, for the
    /// series' tile.
    pub cover_book_id: Option<Uuid>,
}

/// `$1` profile, `$2` series id or NULL, `$3` library id or NULL,
/// `$4` search text or NULL.
const SERIES_SQL: &str = "
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
    WHERE ($2::uuid IS NULL OR s.id = $2)
      AND ($3::uuid IS NULL OR s.library_id = $3)
      AND ($4::text IS NULL OR s.name ILIKE '%' || $4 || '%')
    GROUP BY s.id
    ORDER BY s.sort_name, s.id";

async fn list_series(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<SeriesDto>>, AppError> {
    let profile_id = active_profile(&user)?;
    let rows = sqlx::query_as::<_, SeriesDto>(SERIES_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(library_filter(q.library_id.as_deref())?)
        .bind(search_text(q.q.as_deref()))
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize)]
struct SeriesDetailDto {
    #[serde(flatten)]
    series: SeriesDto,
    /// In sequence order; each book's `series` entry for this series
    /// carries its sequence.
    books: Vec<BookDto>,
}

async fn get_series(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<SeriesDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let not_found = || AppError::NotFound("no such series".into());
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let series = sqlx::query_as::<_, SeriesDto>(SERIES_SQL)
        .bind(profile_id)
        .bind(Some(id))
        .bind(None::<Uuid>)
        .bind(None::<String>)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?
        .ok_or_else(not_found)?;
    let books = books(&app, profile_id, None, Some(id), None).await?;
    Ok(Json(SeriesDetailDto { series, books }))
}

// -------------------------------------------------------------- authors

#[derive(Debug, Serialize, FromRow)]
pub struct AuthorDto {
    /// The person's id (`people`).
    pub id: Uuid,
    pub name: String,
    /// Audiobooks the profile can reach that credit them as author.
    pub book_count: i64,
}

/// `$1` profile, `$2` person id or NULL, `$3` library id or NULL,
/// `$4` search text or NULL.
const AUTHORS_SQL: &str = "
    SELECT p.id, p.name, COUNT(DISTINCT i.id) AS book_count
    FROM people p
    JOIN item_people ip ON ip.person_id = p.id AND ip.role = 'author'
    JOIN media_items i ON i.id = ip.item_id AND i.kind = 'audiobook'
    JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
    WHERE ($2::uuid IS NULL OR p.id = $2)
      AND ($3::uuid IS NULL OR i.library_id = $3)
      AND ($4::text IS NULL OR p.name ILIKE '%' || $4 || '%')
    GROUP BY p.id
    ORDER BY lower(p.name), p.id";

async fn list_authors(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<AuthorDto>>, AppError> {
    let profile_id = active_profile(&user)?;
    let rows = sqlx::query_as::<_, AuthorDto>(AUTHORS_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(library_filter(q.library_id.as_deref())?)
        .bind(search_text(q.q.as_deref()))
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct AuthorQuery {
    library_id: Option<String>,
}

/// A series one author's books sit in.
#[derive(Debug, Serialize, FromRow)]
struct AuthorSeriesDto {
    id: Uuid,
    library_id: Uuid,
    name: String,
    /// How many of this author's books are in it.
    book_count: i64,
}

#[derive(Debug, Serialize)]
struct AuthorDetailDto {
    #[serde(flatten)]
    author: AuthorDto,
    /// By title.
    books: Vec<BookDto>,
    /// The series those books sit in, by name.
    series: Vec<AuthorSeriesDto>,
}

async fn get_author(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<AuthorQuery>,
) -> Result<Json<AuthorDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let not_found = || AppError::NotFound("no such author".into());
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let library_id = library_filter(q.library_id.as_deref())?;
    let author = sqlx::query_as::<_, AuthorDto>(AUTHORS_SQL)
        .bind(profile_id)
        .bind(Some(id))
        .bind(library_id)
        .bind(None::<String>)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?
        .ok_or_else(not_found)?;
    let books = books(&app, profile_id, library_id, None, Some(id)).await?;
    let series = sqlx::query_as::<_, AuthorSeriesDto>(
        "SELECT s.id, s.library_id, s.name, COUNT(DISTINCT i.id) AS book_count
         FROM series s
         JOIN book_series l ON l.series_id = s.id
         JOIN media_items i ON i.id = l.item_id AND i.kind = 'audiobook'
         JOIN item_people ip ON ip.item_id = i.id AND ip.role = 'author' AND ip.person_id = $2
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE ($3::uuid IS NULL OR i.library_id = $3)
         GROUP BY s.id
         ORDER BY s.sort_name, s.id",
    )
    .bind(profile_id)
    .bind(id)
    .bind(library_id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(AuthorDetailDto {
        author,
        books,
        series,
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/series", get(list_series))
        .route("/api/v1/series/{id}", get(get_series))
        .route("/api/v1/authors", get(list_authors))
        .route("/api/v1/authors/{id}", get(get_author))
}
