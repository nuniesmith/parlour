//! Native audiobook API (Phase 10, AB3 + AB4;
//! `docs/AUDIOBOOKS-PODCASTS.md`).
//!
//! - `GET /api/v1/audiobooks` — books the active profile can reach,
//!   with authors, narrators, length, counts and this profile's
//!   progress inline (`library_id`, `q`, `sort` = title | added |
//!   recent)
//! - `GET /api/v1/audiobooks/{id}` — one book with its tracks
//!   (book-global `start_ms`, tokenised `stream_url`) and chapters
//! - `GET /api/v1/audiobooks/{id}/tracks/{index}/stream` — a track's
//!   audio with HTTP Range, authorised like `/api/v1/stream/{file}`
//!   (stream token or session + library access)
//! - `GET|PUT|DELETE /api/v1/audiobooks/{id}/progress` — this
//!   profile's progress on one book
//! - `GET /api/v1/audiobooks/progress?since=` — every progress row
//!   of this profile changed after `since` (client sync)
//!
//! Progress is per profile, one book-global position (ms from the
//! start of track 0), an explicit `finished` flag and
//! `last_listened_at`; writes are last-write-wins. All of that is
//! provisional until the owner's D29 / D33 (see DECISIONS.md).
//! No access to a book's library reads as 404, like items.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::http::AppState;
use crate::playback::{authorise_media, content_type_for, make_token, serve_range, MaybeUser};

fn parse_id(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound("no such book".into()))
}

fn active_profile(user: &AuthUser) -> Result<Uuid, AppError> {
    user.active_profile_id
        .ok_or_else(|| AppError::BadRequest("select a profile first".into()))
}

/// Clamp a reported position onto the book: never negative (the
/// caller rejects those first), never past the end when the length
/// is known.
pub fn clamp_position(position_ms: i64, duration_ms: Option<i64>) -> i64 {
    let p = position_ms.max(0);
    match duration_ms {
        Some(d) if d > 0 => p.min(d),
        _ => p,
    }
}

/// Book-global start of each track: the sum of the lengths before
/// it (an unknown length counts as 0, as chapters do).
pub fn track_offsets(durations: &[Option<i64>]) -> Vec<i64> {
    let mut out = Vec::with_capacity(durations.len());
    let mut at = 0i64;
    for d in durations {
        out.push(at);
        at += d.unwrap_or(0).max(0);
    }
    out
}

// ------------------------------------------------------------------ DTOs

#[derive(Debug, Serialize, Default)]
pub struct ProgressDto {
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub finished: bool,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_listened_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct BookRow {
    id: Uuid,
    library_id: Uuid,
    title: String,
    sort_title: String,
    year: Option<i32>,
    overview: Option<String>,
    genres: Vec<String>,
    available: bool,
    created_at: DateTime<Utc>,
    has_cover: bool,
    authors: Vec<String>,
    narrators: Vec<String>,
    duration_ms: i64,
    track_count: i64,
    chapter_count: i64,
    p_position_ms: Option<i64>,
    p_duration_ms: Option<i64>,
    p_finished: Option<bool>,
    p_finished_at: Option<DateTime<Utc>>,
    p_last_listened_at: Option<DateTime<Utc>>,
    p_updated_at: Option<DateTime<Utc>>,
    subtitle: Option<String>,
    publisher: Option<String>,
    isbn: Option<String>,
    asin: Option<String>,
    language: Option<String>,
    google_books_id: Option<String>,
    open_library_id: Option<String>,
    match_provider: Option<String>,
    match_provider_id: Option<String>,
    match_mode: Option<String>,
    matched_at: Option<DateTime<Utc>>,
}

/// What an admin's match left on a book (AB6).
#[derive(Debug, Serialize)]
pub struct BookMatchDto {
    pub provider: String,
    pub provider_id: String,
    /// `overwrite` or `fill_gaps`.
    pub mode: String,
    pub matched_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct BookDto {
    pub id: Uuid,
    pub library_id: Uuid,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub genres: Vec<String>,
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    pub duration_ms: i64,
    pub track_count: i64,
    pub chapter_count: i64,
    pub has_cover: bool,
    pub available: bool,
    pub created_at: DateTime<Utc>,
    pub progress: ProgressDto,
    /// AB6: the book-only fields, merged from the book's sources and
    /// any match.
    pub subtitle: Option<String>,
    pub publisher: Option<String>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub language: Option<String>,
    pub google_books_id: Option<String>,
    pub open_library_id: Option<String>,
    /// AB6: the match on record, if an admin matched the book.
    #[serde(rename = "match")]
    pub matched: Option<BookMatchDto>,
}

impl From<BookRow> for BookDto {
    fn from(r: BookRow) -> Self {
        let progress = ProgressDto {
            position_ms: r.p_position_ms.unwrap_or(0),
            duration_ms: r.p_duration_ms,
            finished: r.p_finished.unwrap_or(false),
            finished_at: r.p_finished_at,
            last_listened_at: r.p_last_listened_at,
            updated_at: r.p_updated_at,
        };
        BookDto {
            id: r.id,
            library_id: r.library_id,
            title: r.title,
            sort_title: r.sort_title,
            year: r.year,
            overview: r.overview,
            genres: r.genres,
            authors: r.authors,
            narrators: r.narrators,
            duration_ms: r.duration_ms,
            track_count: r.track_count,
            chapter_count: r.chapter_count,
            has_cover: r.has_cover,
            available: r.available,
            created_at: r.created_at,
            progress,
            subtitle: r.subtitle,
            publisher: r.publisher,
            isbn: r.isbn,
            asin: r.asin,
            language: r.language,
            google_books_id: r.google_books_id,
            open_library_id: r.open_library_id,
            matched: match (
                r.match_provider,
                r.match_provider_id,
                r.match_mode,
                r.matched_at,
            ) {
                (Some(provider), Some(provider_id), Some(mode), Some(matched_at)) => {
                    Some(BookMatchDto {
                        provider,
                        provider_id,
                        mode,
                        matched_at,
                    })
                }
                _ => None,
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct TrackDto {
    index: i32,
    file_id: Uuid,
    title: String,
    start_ms: i64,
    duration_ms: Option<i64>,
    container: Option<String>,
    codec: Option<String>,
    content_type: &'static str,
    size_bytes: i64,
    stream_url: String,
}

#[derive(Debug, Serialize, FromRow)]
struct ChapterDto {
    #[sqlx(rename = "chapter_index")]
    index: i32,
    title: String,
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Serialize)]
struct BookDetailDto {
    #[serde(flatten)]
    book: BookDto,
    tracks: Vec<TrackDto>,
    chapters: Vec<ChapterDto>,
}

// --------------------------------------------------------------- reads

/// The one book query, shared by list and detail: `$1` profile,
/// `$2` book id or NULL, `$3` library id or NULL, `$4` search text
/// or NULL, `$5` sort.
const BOOKS_SQL: &str = "
    SELECT i.id, i.library_id, i.title, i.sort_title, i.year, i.overview, i.genres,
           i.available, i.created_at,
           EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                   AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_cover,
           COALESCE((SELECT array_agg(p.name ORDER BY ip.sort_order, p.name)
                     FROM item_people ip JOIN people p ON p.id = ip.person_id
                     WHERE ip.item_id = i.id AND ip.role = 'author'), '{}') AS authors,
           COALESCE((SELECT array_agg(p.name ORDER BY ip.sort_order, p.name)
                     FROM item_people ip JOIN people p ON p.id = ip.person_id
                     WHERE ip.item_id = i.id AND ip.role = 'narrator'), '{}') AS narrators,
           COALESCE((SELECT SUM(f.duration_ms) FROM media_files f
                     WHERE f.item_id = i.id AND f.available), 0)::bigint AS duration_ms,
           (SELECT COUNT(*) FROM media_files f
            WHERE f.item_id = i.id AND f.available) AS track_count,
           (SELECT COUNT(*) FROM book_chapters c WHERE c.item_id = i.id) AS chapter_count,
           bp.position_ms AS p_position_ms, bp.duration_ms AS p_duration_ms,
           bp.finished AS p_finished, bp.finished_at AS p_finished_at,
           bp.last_listened_at AS p_last_listened_at, bp.updated_at AS p_updated_at,
           bm.subtitle, bm.publisher, bm.isbn, bm.asin, bm.language,
           bm.google_books_id, bm.open_library_id,
           bs.provider AS match_provider, bs.provider_id AS match_provider_id,
           bs.mode AS match_mode, bs.matched_at
    FROM media_items i
    JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
    LEFT JOIN book_progress bp ON bp.item_id = i.id AND bp.profile_id = $1
    LEFT JOIN book_metadata bm ON bm.item_id = i.id
    LEFT JOIN book_sidecars bs ON bs.item_id = i.id
    WHERE i.kind = 'audiobook'
      AND ($2::uuid IS NULL OR i.id = $2)
      AND ($3::uuid IS NULL OR i.library_id = $3)
      AND ($4::text IS NULL
           OR i.title ILIKE '%' || $4 || '%'
           OR EXISTS (SELECT 1 FROM item_people ip JOIN people p ON p.id = ip.person_id
                      WHERE ip.item_id = i.id AND ip.role IN ('author', 'narrator')
                        AND p.name ILIKE '%' || $4 || '%'))
    ORDER BY
      CASE WHEN $5::text = 'added' THEN i.created_at END DESC,
      CASE WHEN $5::text = 'recent' THEN bp.last_listened_at END DESC NULLS LAST,
      i.sort_title, i.id";

async fn load_book(app: &AppState, profile_id: Uuid, book_id: Uuid) -> Result<BookDto, AppError> {
    sqlx::query_as::<_, BookRow>(BOOKS_SQL)
        .bind(profile_id)
        .bind(Some(book_id))
        .bind(None::<Uuid>)
        .bind(None::<String>)
        .bind("title")
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?
        .map(BookDto::from)
        .ok_or_else(|| AppError::NotFound("no such book".into()))
}

#[derive(Deserialize)]
struct ListQuery {
    library_id: Option<String>,
    q: Option<String>,
    sort: Option<String>,
}

async fn list_books(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<BookDto>>, AppError> {
    let profile_id = active_profile(&user)?;
    let library_id = match q.library_id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => {
            Some(Uuid::parse_str(s).map_err(|_| AppError::BadRequest("bad library_id".into()))?)
        }
        None => None,
    };
    let sort = q.sort.as_deref().unwrap_or("title");
    if !matches!(sort, "title" | "added" | "recent") {
        return Err(AppError::BadRequest(
            "sort must be title, added or recent".into(),
        ));
    }
    let search =
        q.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    let rows = sqlx::query_as::<_, BookRow>(BOOKS_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(library_id)
        .bind(search)
        .bind(sort)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(Json(rows.into_iter().map(BookDto::from).collect()))
}

#[derive(FromRow)]
struct TrackRow {
    id: Uuid,
    track_index: i32,
    relative_path: String,
    duration_ms: Option<i64>,
    size_bytes: i64,
    container: Option<String>,
    path: String,
    codec: Option<String>,
}

async fn load_tracks(app: &AppState, book_id: Uuid) -> Result<Vec<TrackRow>, AppError> {
    sqlx::query_as::<_, TrackRow>(
        "SELECT f.id, f.track_index, f.relative_path, f.duration_ms, f.size_bytes,
                f.container, f.path,
                (SELECT s.codec FROM media_streams s
                 WHERE s.file_id = f.id AND s.kind = 'audio'
                 ORDER BY s.is_default DESC, s.stream_index LIMIT 1) AS codec
         FROM media_files f
         WHERE f.item_id = $1 AND f.available AND f.track_index IS NOT NULL
         ORDER BY f.track_index",
    )
    .bind(book_id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)
}

fn track_title(rel_path: &str) -> String {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    name.rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(name)
        .to_string()
}

async fn get_book(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<BookDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let book_id = parse_id(&id)?;
    let book = load_book(&app, profile_id, book_id).await?;
    let rows = load_tracks(&app, book_id).await?;
    let offsets = track_offsets(&rows.iter().map(|r| r.duration_ms).collect::<Vec<_>>());
    let tracks = rows
        .into_iter()
        .zip(offsets)
        .map(|(r, start_ms)| TrackDto {
            index: r.track_index,
            file_id: r.id,
            title: track_title(&r.relative_path),
            start_ms,
            duration_ms: r.duration_ms,
            content_type: content_type_for(r.container.as_deref(), &r.path),
            container: r.container,
            codec: r.codec,
            size_bytes: r.size_bytes,
            stream_url: format!(
                "/api/v1/stream/{}?token={}",
                r.id,
                make_token(&app.playback, r.id, profile_id)
            ),
        })
        .collect();
    let chapters = sqlx::query_as::<_, ChapterDto>(
        "SELECT chapter_index, title, start_ms, end_ms FROM book_chapters
         WHERE item_id = $1 ORDER BY chapter_index",
    )
    .bind(book_id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(BookDetailDto {
        book,
        tracks,
        chapters,
    }))
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// A track's audio by (book, index), with Range. Authorised exactly
/// like `/api/v1/stream/{file_id}`: a stream token for that file, or
/// a session whose active profile can reach the library.
async fn stream_track(
    State(app): State<AppState>,
    headers: HeaderMap,
    user: MaybeUser,
    Path((id, index)): Path<(String, i32)>,
    Query(q): Query<TokenQuery>,
) -> Result<Response, AppError> {
    let book_id = parse_id(&id)?;
    let row: Option<(Uuid, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT f.id, f.path, f.size_bytes, f.container
         FROM media_files f JOIN media_items i ON i.id = f.item_id
         WHERE f.item_id = $1 AND f.track_index = $2 AND f.available
           AND i.kind = 'audiobook'",
    )
    .bind(book_id)
    .bind(index)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    // Authorise before saying whether the track exists.
    let Some((file_id, path, size, container)) = row else {
        if user.0.is_none() && q.token.is_none() {
            return Err(AppError::Unauthorized("media access denied".into()));
        }
        return Err(AppError::NotFound("no such track".into()));
    };
    authorise_media(&app, file_id, q.token.as_deref(), user.0.as_ref()).await?;
    serve_range(
        &path,
        size,
        &headers,
        content_type_for(container.as_deref(), &path),
    )
    .await
}

// ------------------------------------------------------------ progress

#[derive(FromRow)]
struct ProgressRow {
    position_ms: i64,
    duration_ms: Option<i64>,
    finished: bool,
    finished_at: Option<DateTime<Utc>>,
    last_listened_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

impl From<ProgressRow> for ProgressDto {
    fn from(r: ProgressRow) -> Self {
        ProgressDto {
            position_ms: r.position_ms,
            duration_ms: r.duration_ms,
            finished: r.finished,
            finished_at: r.finished_at,
            last_listened_at: r.last_listened_at,
            updated_at: Some(r.updated_at),
        }
    }
}

async fn get_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ProgressDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let book = load_book(&app, profile_id, parse_id(&id)?).await?;
    Ok(Json(book.progress))
}

#[derive(Deserialize)]
struct ProgressBody {
    position_ms: Option<i64>,
    finished: Option<bool>,
}

/// Write this profile's progress. `position_ms` moves the position
/// (clamped to the book) and stamps `last_listened_at`; `finished`
/// sets or clears the flag (and `finished_at`). Either may be sent
/// alone; whatever is left out keeps its stored value.
async fn put_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ProgressBody>,
) -> Result<Json<ProgressDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let book = load_book(&app, profile_id, parse_id(&id)?).await?;
    if body.position_ms.is_none() && body.finished.is_none() {
        return Err(AppError::BadRequest(
            "send position_ms, finished, or both".into(),
        ));
    }
    if body.position_ms.is_some_and(|p| p < 0) {
        return Err(AppError::BadRequest(
            "position_ms must not be negative".into(),
        ));
    }
    let duration = Some(book.duration_ms).filter(|d| *d > 0);
    let position = body.position_ms.map(|p| clamp_position(p, duration));
    let row = sqlx::query_as::<_, ProgressRow>(
        "INSERT INTO book_progress
             (profile_id, item_id, position_ms, duration_ms, finished, finished_at,
              last_listened_at, updated_at)
         VALUES ($1, $2, COALESCE($3::bigint, 0), $4, COALESCE($5::boolean, false),
                 CASE WHEN $5::boolean THEN now() END,
                 CASE WHEN $3::bigint IS NOT NULL THEN now() END, now())
         ON CONFLICT (profile_id, item_id) DO UPDATE SET
             position_ms = COALESCE($3::bigint, book_progress.position_ms),
             duration_ms = COALESCE($4, book_progress.duration_ms),
             finished = COALESCE($5::boolean, book_progress.finished),
             finished_at = CASE
                 WHEN $5::boolean IS TRUE AND book_progress.finished THEN book_progress.finished_at
                 WHEN $5::boolean IS TRUE THEN now()
                 WHEN $5::boolean IS FALSE THEN NULL
                 ELSE book_progress.finished_at END,
             last_listened_at = CASE WHEN $3::bigint IS NOT NULL THEN now()
                                     ELSE book_progress.last_listened_at END,
             updated_at = now()
         RETURNING position_ms, duration_ms, finished, finished_at, last_listened_at,
                   updated_at",
    )
    .bind(profile_id)
    .bind(book.id)
    .bind(position)
    .bind(duration)
    .bind(body.finished)
    .fetch_one(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(row.into()))
}

async fn delete_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let profile_id = active_profile(&user)?;
    let book = load_book(&app, profile_id, parse_id(&id)?).await?;
    sqlx::query("DELETE FROM book_progress WHERE profile_id = $1 AND item_id = $2")
        .bind(profile_id)
        .bind(book.id)
        .execute(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SinceQuery {
    since: Option<DateTime<Utc>>,
}

#[derive(Serialize, FromRow)]
struct ProgressSyncRow {
    item_id: Uuid,
    position_ms: i64,
    duration_ms: Option<i64>,
    finished: bool,
    finished_at: Option<DateTime<Utc>>,
    last_listened_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct ProgressSyncDto {
    /// Pass this back as `since` next time. Taken before the read, so
    /// a write landing during the read is sent again, never missed.
    server_time: DateTime<Utc>,
    progress: Vec<ProgressSyncRow>,
}

async fn sync_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<SinceQuery>,
) -> Result<Json<ProgressSyncDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let (server_time,): (DateTime<Utc>,) = sqlx::query_as("SELECT now()")
        .fetch_one(&app.pool)
        .await
        .map_err(AppError::from)?;
    let progress = sqlx::query_as::<_, ProgressSyncRow>(
        "SELECT bp.item_id, bp.position_ms, bp.duration_ms, bp.finished, bp.finished_at,
                bp.last_listened_at, bp.updated_at
         FROM book_progress bp
         JOIN media_items i ON i.id = bp.item_id
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE bp.profile_id = $1 AND ($2::timestamptz IS NULL OR bp.updated_at > $2)
         ORDER BY bp.updated_at, bp.item_id",
    )
    .bind(profile_id)
    .bind(q.since)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(ProgressSyncDto {
        server_time,
        progress,
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/audiobooks", get(list_books))
        .route("/api/v1/audiobooks/progress", get(sync_progress))
        .route("/api/v1/audiobooks/{id}", get(get_book))
        .route(
            "/api/v1/audiobooks/{id}/progress",
            get(get_progress).put(put_progress).delete(delete_progress),
        )
        .route(
            "/api/v1/audiobooks/{id}/tracks/{index}/stream",
            get(stream_track),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_clamp_onto_the_book() {
        assert_eq!(clamp_position(-5, Some(1000)), 0);
        assert_eq!(clamp_position(500, Some(1000)), 500);
        assert_eq!(clamp_position(5000, Some(1000)), 1000);
        // Unknown or zero length: no upper bound to clamp to.
        assert_eq!(clamp_position(5000, None), 5000);
        assert_eq!(clamp_position(5000, Some(0)), 5000);
    }

    #[test]
    fn track_offsets_accumulate_and_unknown_counts_as_zero() {
        assert_eq!(
            track_offsets(&[Some(3000), None, Some(2000), Some(1)]),
            vec![0, 3000, 3000, 5000]
        );
        assert!(track_offsets(&[]).is_empty());
    }

    #[test]
    fn track_titles_are_file_stems() {
        assert_eq!(track_title("Author/Book/01 - Opening.mp3"), "01 - Opening");
        assert_eq!(track_title("Single.m4b"), "Single");
        assert_eq!(track_title("noext"), "noext");
    }
}
