//! Native audiobook API (Phase 10, AB3 + AB4;
//! `docs/AUDIOBOOKS-PODCASTS.md`).
//!
//! - `GET /api/v1/audiobooks` — books the active profile can reach,
//!   with authors, narrators, length, counts and this profile's
//!   progress inline (`library_id`, `q`, `sort` = title | added |
//!   recent)
//! - `GET /api/v1/audiobooks/{id}` — one book with its tracks
//!   (book-global `start_ms`, tokenised `stream_url`) and chapters.
//!   AB10: with `?client=<profile>` each track is decided against that
//!   client's audio profile (`parlour_playback::decide_audio`): a track
//!   it can't play gets the AAC fallback
//!   (`/api/v1/stream/{file}/aac.m4a`) as its `stream_url`, and a
//!   `plan` saying what and why
//! - `GET /api/v1/audiobooks/{id}/tracks/{index}/stream` — a track's
//!   audio with HTTP Range, authorised like `/api/v1/stream/{file}`
//!   (stream token or session + library access)
//! - `GET|PUT|DELETE /api/v1/audiobooks/{id}/progress` — this
//!   profile's progress on one book
//! - `GET /api/v1/audiobooks/progress?since=` — every progress row
//!   of this profile changed after `since` (client sync)
//!
//! Progress is per profile, one book-global position (ms from the
//! start of track 0), a `finished` flag and `last_listened_at`; writes
//! are last-write-wins (D29/D33). AB11 aligned the rules with
//! Audiobookshelf: the library's auto-finish rule, un-finishing by
//! hand resets the position, a hide-from-Continue-Listening flag that
//! the next change clears, and the client's own update time stored
//! (`apply_write`).
//! No access to a book's library reads as 404, like items.
//!
//! AB7 adds each book's `series` (with its sequence) and
//! `author_refs` (author-page ids); the series and author pages
//! themselves are `crate::series`.

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

pub(crate) fn active_profile(user: &AuthUser) -> Result<Uuid, AppError> {
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

/// An audiobooks library's auto-finish rule (AB11, D29): a book is
/// finished when less than `remaining_ms` of it is left, or — when
/// `percent` is set — once the position reaches that share of the
/// book, which then replaces the time rule (Audiobookshelf's library
/// settings; 10 s and off by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishRule {
    pub remaining_ms: i64,
    pub percent: Option<i32>,
}

impl Default for FinishRule {
    fn default() -> Self {
        FinishRule {
            remaining_ms: i64::from(crate::book_settings::DEFAULT_FINISH_REMAINING_SECONDS) * 1000,
            percent: None,
        }
    }
}

/// Whether a position has reached the rule's finishing point, as
/// Audiobookshelf compares: less than `remaining_ms` left (strictly),
/// or with a percentage, a position strictly past that share of the
/// book. A book of unknown length never finishes by itself; a time
/// rule of 0 is off.
pub fn reaches_finish(position_ms: i64, duration_ms: Option<i64>, rule: FinishRule) -> bool {
    let Some(d) = duration_ms.filter(|d| *d > 0) else {
        return false;
    };
    match rule.percent {
        Some(p) => position_ms.saturating_mul(100) > d.saturating_mul(i64::from(p)),
        None => rule.remaining_ms > 0 && d - position_ms < rule.remaining_ms,
    }
}

/// What is stored for a profile and book before a write (zeros when
/// there is no row yet).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoredProgress {
    pub position_ms: i64,
    pub finished: bool,
    pub hidden: bool,
}

/// One progress write, as a client sent it (the position already
/// clamped onto the book).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressWrite {
    pub position_ms: Option<i64>,
    pub finished: Option<bool>,
    pub hidden: Option<bool>,
}

/// What `finished_at` does on a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishedAt {
    Keep,
    Now,
    Clear,
}

/// The row a write leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextProgress {
    pub position_ms: i64,
    pub finished: bool,
    pub finished_at: FinishedAt,
    pub hidden: bool,
    /// The write sent a position, so it counts as listening
    /// (`last_listened_at`).
    pub listened: bool,
}

/// Audiobookshelf's progress rules (AB11, D29), applied to one write:
///
/// - **un-finishing by hand** (`finished: false` on a finished book)
///   puts the position back to 0, and a position sent with it is
///   ignored;
/// - **finishing by hand** (`finished: true`) finishes the book and
///   leaves the position where it is (or where the write puts it);
/// - otherwise, when the write sends a position and the book's length
///   is known, the library's rule decides: reaching the finishing
///   point finishes the book, and a finished book moved back before it
///   is unfinished again; a write without a position leaves
///   `finished` alone;
/// - `hidden` (hide from Continue Listening) takes the value the write
///   sends; otherwise a change of the position clears it, and a change
///   of `finished` alone does not.
pub fn apply_write(
    stored: StoredProgress,
    write: ProgressWrite,
    duration_ms: Option<i64>,
    rule: FinishRule,
) -> NextProgress {
    let mut position = write.position_ms.unwrap_or(stored.position_ms);
    let mut listened = write.position_ms.is_some();
    let finished = match write.finished {
        Some(false) if stored.finished => {
            position = 0;
            listened = false;
            false
        }
        Some(f) => f,
        None => match write.position_ms {
            Some(_) if duration_ms.is_some_and(|d| d > 0) => {
                if reaches_finish(position, duration_ms, rule) {
                    true
                } else {
                    // A finished book moved back before the point is
                    // unfinished; an unmoved one stays as it was.
                    stored.finished && position == stored.position_ms
                }
            }
            _ => stored.finished,
        },
    };
    let finished_at = match (stored.finished, finished) {
        (false, true) => FinishedAt::Now,
        (_, false) => FinishedAt::Clear,
        (true, true) => FinishedAt::Keep,
    };
    let moved = position != stored.position_ms;
    let hidden = match write.hidden {
        Some(true) => true,
        Some(false) => false,
        None => stored.hidden && !moved,
    };
    NextProgress {
        position_ms: position,
        finished,
        finished_at,
        hidden,
        listened,
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

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProgressDto {
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub finished: bool,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_listened_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    /// AB11: hidden from Continue Listening; cleared by the next write
    /// that changes the position or the finished flag.
    pub hidden: bool,
    /// AB11: when the last writer says it made the update (its own
    /// clock; the server's when it sent none). `updated_at` stays the
    /// server's time.
    pub client_updated_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
pub(crate) struct BookRow {
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
    p_hidden: Option<bool>,
    p_client_updated_at: Option<DateTime<Utc>>,
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
    author_refs: sqlx::types::Json<Vec<AuthorRefDto>>,
    series: sqlx::types::Json<Vec<BookSeriesDto>>,
}

/// An author of a book, with the id of their author page (AB7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorRefDto {
    pub id: Uuid,
    pub name: String,
}

/// A series a book sits in, and its place there (AB7, D28).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookSeriesDto {
    pub id: Uuid,
    pub name: String,
    pub sequence: Option<String>,
}

/// What an admin's match left on a book (AB6).
#[derive(Debug, Clone, Serialize)]
pub struct BookMatchDto {
    pub provider: String,
    pub provider_id: String,
    /// `overwrite` or `fill_gaps`.
    pub mode: String,
    pub matched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
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
    /// AB7: `authors` with their author-page ids, in the same order.
    pub author_refs: Vec<AuthorRefDto>,
    /// AB7: the series the book sits in, in the book's own order.
    pub series: Vec<BookSeriesDto>,
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
            hidden: r.p_hidden.unwrap_or(false),
            client_updated_at: r.p_client_updated_at,
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
            author_refs: r.author_refs.0,
            series: r.series.0,
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
    /// The type of what `stream_url` serves: the file's own, or
    /// `audio/mp4` for the AAC fallback (AB10).
    content_type: &'static str,
    size_bytes: i64,
    stream_url: String,
    /// AB10: the decision for the profile the request named
    /// (`?client=`); `null` when it named none, and `stream_url` is
    /// then the original file, as before AB10.
    plan: Option<TrackPlanDto>,
}

/// AB10: one track's stream decision, in playback-info's words.
#[derive(Debug, Serialize)]
struct TrackPlanDto {
    /// `DirectPlay` | `Remux` (AAC copied into MP4) | `Transcode` (to AAC)
    action: String,
    /// `Direct` | `ContainerUnsupported` | `CodecUnsupported`
    reason: String,
    /// `direct_play` | `copy` | `transcode`
    mode: &'static str,
    /// `file` (the original) or `aac` (the fallback copy)
    delivery: &'static str,
    /// The profile the decision was made for.
    client: &'static str,
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

/// The one book query, shared by list and detail and by the series
/// and author pages (AB7): `$1` profile, `$2` book id or NULL, `$3`
/// library id or NULL, `$4` search text or NULL, `$5` sort, `$6`
/// series id or NULL (only that series' books, in sequence order),
/// `$7` author (person) id or NULL (only that author's books), `$8`
/// a list of book ids or NULL (only those; AB8's shelves).
///
/// Sequence order mirrors Audiobookshelf's: the sequence read as a
/// number (its leading number; text with none reads as 0), books
/// without a sequence last, then by title.
pub(crate) const BOOKS_SQL: &str = "
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
           bp.hidden AS p_hidden, bp.client_updated_at AS p_client_updated_at,
           bm.subtitle, bm.publisher, bm.isbn, bm.asin, bm.language,
           bm.google_books_id, bm.open_library_id,
           bs.provider AS match_provider, bs.provider_id AS match_provider_id,
           bs.mode AS match_mode, bs.matched_at,
           COALESCE((SELECT json_agg(json_build_object('id', p.id, 'name', p.name)
                                     ORDER BY ip.sort_order, p.name)
                     FROM item_people ip JOIN people p ON p.id = ip.person_id
                     WHERE ip.item_id = i.id AND ip.role = 'author'), '[]'::json) AS author_refs,
           COALESCE((SELECT json_agg(json_build_object('id', s.id, 'name', s.name,
                                                       'sequence', l.sequence)
                                     ORDER BY l.sort_order)
                     FROM book_series l JOIN series s ON s.id = l.series_id
                     WHERE l.item_id = i.id), '[]'::json) AS series
    FROM media_items i
    JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
    LEFT JOIN book_progress bp ON bp.item_id = i.id AND bp.profile_id = $1
    LEFT JOIN book_metadata bm ON bm.item_id = i.id
    LEFT JOIN book_sidecars bs ON bs.item_id = i.id
    LEFT JOIN book_series sl ON sl.item_id = i.id AND sl.series_id = $6
    WHERE i.kind = 'audiobook'
      AND ($2::uuid IS NULL OR i.id = $2)
      AND ($3::uuid IS NULL OR i.library_id = $3)
      AND ($4::text IS NULL
           OR i.title ILIKE '%' || $4 || '%'
           OR EXISTS (SELECT 1 FROM item_people ip JOIN people p ON p.id = ip.person_id
                      WHERE ip.item_id = i.id AND ip.role IN ('author', 'narrator')
                        AND p.name ILIKE '%' || $4 || '%')
           OR EXISTS (SELECT 1 FROM book_series l JOIN series s ON s.id = l.series_id
                      WHERE l.item_id = i.id AND s.name ILIKE '%' || $4 || '%'))
      AND ($6::uuid IS NULL OR sl.series_id IS NOT NULL)
      AND ($7::uuid IS NULL
           OR EXISTS (SELECT 1 FROM item_people ip
                      WHERE ip.item_id = i.id AND ip.person_id = $7 AND ip.role = 'author'))
      AND ($8::uuid[] IS NULL OR i.id = ANY($8))
    ORDER BY
      CASE WHEN $6::uuid IS NOT NULL AND sl.sequence IS NOT NULL
           THEN COALESCE(substring(sl.sequence FROM '^ *(-?[0-9]+([.][0-9]+)?)')::numeric, 0)
      END NULLS LAST,
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
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .bind(None::<Vec<Uuid>>)
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
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .bind(None::<Vec<Uuid>>)
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
    channels: Option<i32>,
}

async fn load_tracks(app: &AppState, book_id: Uuid) -> Result<Vec<TrackRow>, AppError> {
    sqlx::query_as::<_, TrackRow>(
        "SELECT f.id, f.track_index, f.relative_path, f.duration_ms, f.size_bytes,
                f.container, f.path,
                (SELECT s.codec FROM media_streams s
                 WHERE s.file_id = f.id AND s.kind = 'audio'
                 ORDER BY s.is_default DESC, s.stream_index LIMIT 1) AS codec,
                (SELECT s.channels FROM media_streams s
                 WHERE s.file_id = f.id AND s.kind = 'audio'
                 ORDER BY s.is_default DESC, s.stream_index LIMIT 1) AS channels
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

#[derive(Deserialize)]
struct BookQuery {
    /// AB10: the client's capability profile (`web`, `android`,
    /// `androidtv` / `firetv`, `ios`, `tvos`). Without one, every
    /// track's `stream_url` is the original file (AB3).
    client: Option<String>,
    /// As for video's playback-info; recorded with the plan only.
    network: Option<String>,
}

/// How far each action goes, for the book's overall plan: the
/// furthest any track needs.
fn action_rank(action: parlour_core::PlaybackAction) -> u8 {
    match action {
        parlour_core::PlaybackAction::DirectPlay => 0,
        parlour_core::PlaybackAction::Remux => 1,
        parlour_core::PlaybackAction::Transcode => 2,
    }
}

async fn get_book(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<BookQuery>,
) -> Result<Json<BookDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let book_id = parse_id(&id)?;
    // An unknown profile name is refused rather than guessed at: the
    // profile decides what plays, so a typo must not quietly pick one.
    let client = match q.client.as_deref() {
        None => None,
        Some(name) => Some(crate::playback::parse_client(Some(name)).ok_or_else(|| {
            AppError::BadRequest(format!(
                "unknown client profile {name:?} (web, android, androidtv, firetv, ios, tvos)"
            ))
        })?),
    };
    let book = load_book(&app, profile_id, book_id).await?;
    let rows = load_tracks(&app, book_id).await?;
    let offsets = track_offsets(&rows.iter().map(|r| r.duration_ms).collect::<Vec<_>>());
    let resume_ms = if book.progress.finished {
        0
    } else {
        book.progress.position_ms
    };
    // The track the player will start in: the last one starting at or
    // before the resume point.
    let resume_track = offsets.iter().rposition(|start| *start <= resume_ms);

    let mut tracks = Vec::with_capacity(rows.len());
    let mut book_plan: Option<crate::playback::StreamPlan> = None;
    let mut book_rank = 0;
    for (i, (r, start_ms)) in rows.into_iter().zip(offsets).enumerate() {
        let decision = client.map(|c| {
            (
                c,
                parlour_playback::decide_audio(r.container.as_deref(), r.codec.as_deref(), c),
            )
        });
        let fallback =
            decision.is_some_and(|(_, d)| d.action != parlour_core::PlaybackAction::DirectPlay);
        let token = make_token(&app.playback, r.id, profile_id);
        let (stream_url, content_type, delivery) = if fallback {
            (
                format!("/api/v1/stream/{}/aac.m4a?token={token}", r.id),
                "audio/mp4",
                "aac",
            )
        } else {
            (
                format!("/api/v1/stream/{}?token={token}", r.id),
                content_type_for(r.container.as_deref(), &r.path),
                "file",
            )
        };
        if fallback && Some(i) == resume_track {
            crate::playback::prepare_audio_fallback(&app, r.id).await;
        }
        if let Some((c, d)) = decision {
            let rank = action_rank(d.action);
            if book_plan.is_none() || rank > book_rank {
                book_rank = rank;
                let bitrate_kbps = match r.duration_ms {
                    Some(ms) if ms > 0 => Some(r.size_bytes * 8 / ms),
                    _ => None,
                };
                book_plan = Some(crate::playback::StreamPlan::for_book(
                    d,
                    delivery,
                    c,
                    crate::playback::parse_network(q.network.as_deref()),
                    r.container.clone(),
                    bitrate_kbps,
                    r.codec.clone(),
                    r.channels,
                ));
            }
        }
        tracks.push(TrackDto {
            index: r.track_index,
            file_id: r.id,
            title: track_title(&r.relative_path),
            start_ms,
            duration_ms: r.duration_ms,
            content_type,
            container: r.container,
            codec: r.codec,
            size_bytes: r.size_bytes,
            stream_url,
            plan: decision.map(|(c, d)| TrackPlanDto {
                action: format!("{:?}", d.action),
                reason: format!("{:?}", d.reason),
                mode: crate::playback::stream_mode(d.action),
                delivery,
                client: crate::playback::client_name(c),
            }),
        });
    }
    if let Some(plan) = book_plan {
        crate::playback::remember_book_plan(&app.playback, profile_id, book_id, plan);
    }
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
    hidden: bool,
    client_updated_at: Option<DateTime<Utc>>,
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
            hidden: r.hidden,
            client_updated_at: r.client_updated_at,
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
    /// AB11: hide from (true) or return to (false) Continue Listening.
    hidden: Option<bool>,
    /// AB11: when the client made this update, by its own clock.
    client_updated_at: Option<DateTime<Utc>>,
}

/// Write this profile's progress (AB4, aligned with Audiobookshelf in
/// AB11). `position_ms` moves the position (clamped to the book) and
/// stamps `last_listened_at`; `finished` sets or clears the flag (and
/// `finished_at`); `hidden` hides the book from Continue Listening.
/// Any of them may be sent alone; whatever is left out keeps its
/// stored value, except as [`apply_write`] has it: a position at the
/// library's finishing point finishes the book, un-finishing by hand
/// puts the position back to 0, and a change clears `hidden`.
/// `client_updated_at` is stored as sent (the server's time when
/// absent). The last write to arrive wins.
async fn put_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ProgressBody>,
) -> Result<Json<ProgressDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let book = load_book(&app, profile_id, parse_id(&id)?).await?;
    if body.position_ms.is_none() && body.finished.is_none() && body.hidden.is_none() {
        return Err(AppError::BadRequest(
            "send position_ms, finished or hidden".into(),
        ));
    }
    if body.position_ms.is_some_and(|p| p < 0) {
        return Err(AppError::BadRequest(
            "position_ms must not be negative".into(),
        ));
    }
    let duration = Some(book.duration_ms).filter(|d| *d > 0);
    let write = ProgressWrite {
        position_ms: body.position_ms.map(|p| clamp_position(p, duration)),
        finished: body.finished,
        hidden: body.hidden,
    };

    let mut tx = app.pool.begin().await.map_err(AppError::from)?;
    let (remaining_s, percent): (i32, Option<i32>) = sqlx::query_as(
        "SELECT finish_remaining_seconds, finish_percent FROM libraries WHERE id = $1",
    )
    .bind(book.library_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::from)?;
    let rule = FinishRule {
        remaining_ms: i64::from(remaining_s) * 1000,
        percent,
    };
    let stored: Option<(i64, bool, bool)> = sqlx::query_as(
        "SELECT position_ms, finished, hidden FROM book_progress
         WHERE profile_id = $1 AND item_id = $2 FOR UPDATE",
    )
    .bind(profile_id)
    .bind(book.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::from)?;
    let stored = stored
        .map(|(position_ms, finished, hidden)| StoredProgress {
            position_ms,
            finished,
            hidden,
        })
        .unwrap_or_default();
    let next = apply_write(stored, write, duration, rule);
    let finished_at = match next.finished_at {
        FinishedAt::Keep => "keep",
        FinishedAt::Now => "now",
        FinishedAt::Clear => "clear",
    };
    let row = sqlx::query_as::<_, ProgressRow>(
        "INSERT INTO book_progress
             (profile_id, item_id, position_ms, duration_ms, finished, finished_at,
              last_listened_at, updated_at, hidden, client_updated_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 = 'now' THEN now() END,
                 CASE WHEN $7 THEN now() END, now(), $8, COALESCE($9, now()))
         ON CONFLICT (profile_id, item_id) DO UPDATE SET
             position_ms = $3,
             duration_ms = COALESCE($4, book_progress.duration_ms),
             finished = $5,
             finished_at = CASE $6 WHEN 'now' THEN now()
                                   WHEN 'clear' THEN NULL
                                   ELSE book_progress.finished_at END,
             last_listened_at = CASE WHEN $7 THEN now()
                                     ELSE book_progress.last_listened_at END,
             updated_at = now(),
             hidden = $8,
             client_updated_at = COALESCE($9, now())
         RETURNING position_ms, duration_ms, finished, finished_at, last_listened_at,
                   updated_at, hidden, client_updated_at",
    )
    .bind(profile_id)
    .bind(book.id)
    .bind(next.position_ms)
    .bind(duration)
    .bind(next.finished)
    .bind(finished_at)
    .bind(next.listened)
    .bind(next.hidden)
    .bind(body.client_updated_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
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
    hidden: bool,
    client_updated_at: Option<DateTime<Utc>>,
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
                bp.last_listened_at, bp.updated_at, bp.hidden, bp.client_updated_at
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

    const TEN_S: FinishRule = FinishRule {
        remaining_ms: 10_000,
        percent: None,
    };

    #[test]
    fn the_finishing_point_is_audiobookshelfs() {
        let d = Some(3_600_000);
        // Less than 10 s left finishes; exactly 10 s doesn't.
        assert!(reaches_finish(3_590_001, d, TEN_S));
        assert!(!reaches_finish(3_590_000, d, TEN_S));
        assert!(reaches_finish(3_600_000, d, TEN_S));
        // Unknown length, or the time rule off: never by itself.
        assert!(!reaches_finish(3_600_000, None, TEN_S));
        assert!(!reaches_finish(3_600_000, Some(0), TEN_S));
        let off = FinishRule {
            remaining_ms: 0,
            percent: None,
        };
        assert!(!reaches_finish(3_600_000, d, off));
        // A percentage replaces the time rule, and must be passed.
        let pct = FinishRule {
            remaining_ms: 10_000,
            percent: Some(95),
        };
        assert!(!reaches_finish(950_000, Some(1_000_000), pct));
        assert!(reaches_finish(950_001, Some(1_000_000), pct));
        assert!(
            !reaches_finish(
                995_000,
                Some(1_000_000),
                FinishRule {
                    percent: Some(100),
                    ..pct
                }
            ),
            "at 100 % the point can't be passed"
        );
        let fine = FinishRule {
            remaining_ms: 10_000,
            percent: Some(99),
        };
        assert!(
            !reaches_finish(3_559_000, d, fine),
            "the percentage, not the 10 s, decides"
        );
    }

    fn stored(position_ms: i64, finished: bool, hidden: bool) -> StoredProgress {
        StoredProgress {
            position_ms,
            finished,
            hidden,
        }
    }

    fn write(
        position_ms: Option<i64>,
        finished: Option<bool>,
        hidden: Option<bool>,
    ) -> ProgressWrite {
        ProgressWrite {
            position_ms,
            finished,
            hidden,
        }
    }

    #[test]
    fn writes_follow_audiobookshelfs_progress_rules() {
        let d = Some(100_000);
        let apply = |s, w| apply_write(s, w, d, TEN_S);

        // A plain position write.
        let n = apply(stored(0, false, false), write(Some(5_000), None, None));
        assert_eq!(
            (n.position_ms, n.finished, n.finished_at),
            (5_000, false, FinishedAt::Clear)
        );
        assert!(n.listened);

        // Reaching the point finishes, once.
        let n = apply(stored(5_000, false, false), write(Some(95_000), None, None));
        assert_eq!((n.finished, n.finished_at), (true, FinishedAt::Now));
        let n = apply(stored(95_000, true, false), write(Some(99_000), None, None));
        assert_eq!((n.finished, n.finished_at), (true, FinishedAt::Keep));

        // Moved back before the point: unfinished again, position kept.
        let n = apply(stored(95_000, true, false), write(Some(1_000), None, None));
        assert_eq!(
            (n.position_ms, n.finished, n.finished_at),
            (1_000, false, FinishedAt::Clear)
        );
        // Not moved: stays finished, even before the point.
        let n = apply(stored(1_000, true, false), write(Some(1_000), None, None));
        assert!(n.finished);
        // Unknown length: positions never change `finished`.
        let n = apply_write(
            stored(1_000, true, false),
            write(Some(2), None, None),
            None,
            TEN_S,
        );
        assert!(n.finished);
        let n = apply_write(
            stored(0, false, false),
            write(Some(9), None, None),
            None,
            TEN_S,
        );
        assert!(!n.finished);

        // By hand: finishing keeps the position; un-finishing resets
        // it to 0 and ignores a position sent with it.
        let n = apply(stored(40_000, false, false), write(None, Some(true), None));
        assert_eq!(
            (n.position_ms, n.finished, n.finished_at),
            (40_000, true, FinishedAt::Now)
        );
        assert!(!n.listened, "marking is not listening");
        let n = apply(
            stored(40_000, true, false),
            write(Some(30_000), Some(false), None),
        );
        assert_eq!(
            (n.position_ms, n.finished, n.finished_at),
            (0, false, FinishedAt::Clear)
        );
        assert!(!n.listened);
        // Un-finishing an unfinished book changes nothing else.
        let n = apply(stored(40_000, false, false), write(None, Some(false), None));
        assert_eq!((n.position_ms, n.finished), (40_000, false));
        // An explicit finished wins over the rule both ways.
        let n = apply(
            stored(0, false, false),
            write(Some(99_000), Some(false), None),
        );
        assert!(!n.finished);
        let n = apply(
            stored(0, false, false),
            write(Some(5_000), Some(true), None),
        );
        assert_eq!((n.position_ms, n.finished), (5_000, true));
    }

    #[test]
    fn the_hide_flag_clears_when_the_position_next_moves() {
        let d = Some(100_000);
        let apply = |s, w| apply_write(s, w, d, TEN_S);
        let n = apply(stored(5_000, false, false), write(None, None, Some(true)));
        assert!(n.hidden);
        assert_eq!(n.position_ms, 5_000);
        // A move clears it, unless the same write hides it again.
        assert!(!apply(stored(5_000, false, true), write(Some(6_000), None, None)).hidden);
        assert!(
            apply(
                stored(5_000, false, true),
                write(Some(6_000), None, Some(true))
            )
            .hidden
        );
        // No move: it stays.
        assert!(apply(stored(5_000, false, true), write(Some(5_000), None, None)).hidden);
        // Finishing alone doesn't clear it; un-finishing moves the
        // position to 0, which does.
        assert!(apply(stored(5_000, false, true), write(None, Some(true), None)).hidden);
        assert!(!apply(stored(5_000, true, true), write(None, Some(false), None)).hidden);
        // Sent false: shown again without a move.
        assert!(!apply(stored(5_000, false, true), write(None, None, Some(false))).hidden);
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
