//! POD8 (Phase 11; D35): a profile's playlists. Spec:
//! `docs/AUDIOBOOKS-PODCASTS.md` → "Playlists, collections and bookmarks
//! (POD8)".
//!
//! - `GET /api/v1/playlists?library_id=&item_id=|episode_id=` — this
//!   profile's playlists in the libraries it can reach, by name, each
//!   with its entry count, length and a few covers; with an item or an
//!   episode named, whether each playlist already holds it
//! - `POST /api/v1/playlists` `{library_id, name, description?}` — a new,
//!   empty playlist in an audiobooks or podcasts library the profile can
//!   reach
//! - `GET|PATCH|DELETE /api/v1/playlists/{id}` — one playlist with its
//!   entries in order; rename / describe it; delete it
//! - `POST /api/v1/playlists/{id}/items` `{item_id}` or `{episode_id}` —
//!   append a book or an episode of the playlist's library
//! - `PUT /api/v1/playlists/{id}/items` `{items: [entry ids]}` — the new
//!   order, naming every entry the profile can see
//! - `DELETE /api/v1/playlists/{id}/items/{entry_id}` — remove an entry;
//!   removing the last one deletes the playlist, as Audiobookshelf does
//!
//! **Whose.** A playlist is its profile's alone (D35, as ABS keeps one
//! per user per library): any other profile, an admin's included, gets
//! 404. It is reached only while the profile keeps its grant on the
//! playlist's library; without it the playlist is a 404 and isn't listed,
//! and it returns with the grant.
//!
//! **What it shows.** An entry is shown only while the profile can reach
//! its book or episode: the library grant, and for an episode D41's
//! explicit flag (as POD5). An entry it can't reach is kept, unseen and
//! uncounted, and returns when the profile can reach it again. An entry
//! goes for good with its book or episode (the database's cascade).
//!
//! **One kind per playlist.** A playlist lives in one library, and an
//! audiobooks library holds books while a podcasts library holds
//! episodes, so a playlist never mixes the two.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::audiobooks::{active_profile, BookDto, BookRow, BOOKS_SQL};
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::http::AppState;
use crate::podcast_api::EpisodeDto;

/// The longest name, and description, a playlist or a collection takes.
pub(crate) const MAX_NAME_CHARS: usize = 200;
pub(crate) const MAX_DESCRIPTION_CHARS: usize = 4000;
/// How many covers a playlist or collection summary carries.
pub(crate) const COVERS: usize = 4;

pub(crate) fn parse_id(id: &str, what: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound(format!("no such {what}")))
}

/// A name as stored: trimmed, not empty, at most [`MAX_NAME_CHARS`].
pub(crate) fn clean_name(raw: &str) -> Result<String, AppError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("give it a name".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::BadRequest(format!(
            "a name is at most {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name.to_string())
}

/// A description as stored: trimmed, NULL when empty.
pub(crate) fn clean_description(raw: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if text.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(AppError::BadRequest(format!(
            "a description is at most {MAX_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(Some(text.to_string()))
}

fn optional_id(raw: Option<&str>, what: &str) -> Result<Option<Uuid>, AppError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => Uuid::parse_str(s)
            .map(Some)
            .map_err(|_| AppError::BadRequest(format!("bad {what}"))),
        None => Ok(None),
    }
}

/// The new order of a list whose entries the caller can see: `wanted`
/// must name each of `visible` exactly once (else `None`, a stale or
/// wrong list). The visible entries take the places they held between
/// them, in the new order; entries the caller can't see keep theirs.
/// `current` is every entry with its position, in position order.
pub fn reorder_slots(
    current: &[(Uuid, i32)],
    visible: &HashSet<Uuid>,
    wanted: &[Uuid],
) -> Option<Vec<(Uuid, i32)>> {
    let distinct: HashSet<Uuid> = wanted.iter().copied().collect();
    if wanted.len() != visible.len() || distinct.len() != wanted.len() || distinct != *visible {
        return None;
    }
    let slots: Vec<i32> = current
        .iter()
        .filter(|(id, _)| visible.contains(id))
        .map(|(_, pos)| *pos)
        .collect();
    Some(wanted.iter().copied().zip(slots).collect())
}

/// Books among `ids` that `profile_id` can reach (AB3's book query),
/// keyed by id.
pub(crate) async fn books_by_ids(
    app: &AppState,
    profile_id: Uuid,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, BookDto>, AppError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, BookRow>(BOOKS_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .bind(None::<String>)
        .bind("title")
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .bind(Some(ids))
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(rows
        .into_iter()
        .map(BookDto::from)
        .map(|b| (b.id, b))
        .collect())
}

/// One cover as the summaries carry it: a book's poster, or a show's
/// cached artwork (never a publisher's address).
#[derive(Debug, Deserialize)]
pub(crate) struct CoverRef {
    item_id: Option<Uuid>,
    #[serde(default)]
    has_cover: bool,
    podcast_id: Option<Uuid>,
    image_path: Option<String>,
}

/// The cover addresses of `refs`, at most [`COVERS`], without repeats.
pub(crate) fn cover_urls(refs: &[CoverRef]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in refs {
        let url = match (r.item_id, r.podcast_id) {
            (Some(id), _) if r.has_cover => {
                Some(format!("/api/v1/items/{id}/artwork/poster?v=w300"))
            }
            (_, Some(show)) => {
                parlour_catalog::podcast_artwork::artwork_route(show, r.image_path.as_deref())
            }
            _ => None,
        };
        if let Some(url) = url {
            if !out.contains(&url) {
                out.push(url);
            }
        }
        if out.len() >= COVERS {
            break;
        }
    }
    out
}

// --------------------------------------------------------------- summary

#[derive(FromRow)]
struct PlaylistRow {
    id: Uuid,
    library_id: Uuid,
    library_name: String,
    library_kind: String,
    name: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    item_count: i64,
    duration_ms: i64,
    contains: bool,
    covers: sqlx::types::Json<Vec<CoverRef>>,
}

#[derive(Debug, Serialize)]
pub struct PlaylistDto {
    pub id: Uuid,
    pub library_id: Uuid,
    pub library_name: String,
    /// `audiobooks` (a playlist of books) or `podcasts` (of episodes).
    pub library_kind: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Entries this profile can reach (D41's hidden ones aren't counted).
    pub item_count: i64,
    /// Their total length: a book's available files, an episode's
    /// download, else the feed's length.
    pub duration_ms: i64,
    /// With `?item_id=` / `?episode_id=`: whether it is in this playlist.
    pub contains: bool,
    /// Up to four cover addresses, from the first entries.
    pub covers: Vec<String>,
}

impl From<PlaylistRow> for PlaylistDto {
    fn from(r: PlaylistRow) -> Self {
        PlaylistDto {
            covers: cover_urls(&r.covers.0),
            id: r.id,
            library_id: r.library_id,
            library_name: r.library_name,
            library_kind: r.library_kind,
            name: r.name,
            description: r.description,
            created_at: r.created_at,
            updated_at: r.updated_at,
            item_count: r.item_count,
            duration_ms: r.duration_ms,
            contains: r.contains,
        }
    }
}

/// `$1` profile, `$2` playlist id or NULL, `$3` library id or NULL, `$4`
/// a book or episode id (for `contains`) or NULL. Only this profile's
/// playlists, in libraries it has a grant on. "Visible" entries are
/// those whose book it can reach, or whose episode it can reach under
/// D41's flag.
const PLAYLISTS_SQL: &str = "
    WITH visible AS (
        SELECT pi.playlist_id, pi.position, pi.item_id, pi.episode_id,
               COALESCE((SELECT sum(f.duration_ms) FROM media_files f
                         WHERE f.item_id = pi.item_id AND f.available),
                        e.file_duration_ms, e.duration_ms, 0)::bigint AS length_ms,
               EXISTS (SELECT 1 FROM images im WHERE im.item_id = pi.item_id
                       AND im.kind = 'poster' AND im.path <> ''
                       AND im.path NOT LIKE 'pending:%') AS has_cover,
               p.id AS podcast_id, p.image_path
        FROM playlist_items pi
        JOIN playlists pl ON pl.id = pi.playlist_id AND pl.profile_id = $1
        LEFT JOIN media_items i ON i.id = pi.item_id
        LEFT JOIN podcast_episodes e ON e.id = pi.episode_id
        LEFT JOIN podcasts p ON p.id = e.podcast_id
        JOIN profiles pr ON pr.id = $1
        WHERE ($2::uuid IS NULL OR pi.playlist_id = $2)
          AND ((i.id IS NOT NULL AND i.kind = 'audiobook'
                AND EXISTS (SELECT 1 FROM library_access a
                            WHERE a.library_id = i.library_id AND a.profile_id = $1))
               OR (e.id IS NOT NULL AND (NOT p.explicit OR pr.allow_explicit)
                   AND EXISTS (SELECT 1 FROM library_access a
                               WHERE a.library_id = p.library_id AND a.profile_id = $1)))
    )
    SELECT pl.id, pl.library_id, l.name AS library_name, l.kind AS library_kind,
           pl.name, pl.description, pl.created_at, pl.updated_at,
           (SELECT count(*) FROM visible v WHERE v.playlist_id = pl.id) AS item_count,
           (SELECT COALESCE(sum(v.length_ms), 0)::bigint FROM visible v
            WHERE v.playlist_id = pl.id) AS duration_ms,
           ($4::uuid IS NOT NULL AND EXISTS (
               SELECT 1 FROM visible v WHERE v.playlist_id = pl.id
               AND (v.item_id = $4 OR v.episode_id = $4))) AS contains,
           COALESCE((SELECT json_agg(json_build_object(
                         'item_id', c.item_id, 'has_cover', c.has_cover,
                         'podcast_id', c.podcast_id, 'image_path', c.image_path)
                         ORDER BY c.position)
                     FROM (SELECT * FROM visible v WHERE v.playlist_id = pl.id
                           ORDER BY v.position LIMIT 12) c), '[]'::json) AS covers
    FROM playlists pl
    JOIN libraries l ON l.id = pl.library_id
    JOIN library_access a ON a.library_id = pl.library_id AND a.profile_id = $1
    WHERE pl.profile_id = $1
      AND ($2::uuid IS NULL OR pl.id = $2)
      AND ($3::uuid IS NULL OR pl.library_id = $3)
    ORDER BY lower(pl.name), pl.created_at, pl.id";

async fn load_summary(
    app: &AppState,
    profile_id: Uuid,
    playlist_id: Uuid,
) -> Result<PlaylistDto, AppError> {
    sqlx::query_as::<_, PlaylistRow>(PLAYLISTS_SQL)
        .bind(profile_id)
        .bind(Some(playlist_id))
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?
        .map(PlaylistDto::from)
        .ok_or_else(not_found)
}

fn not_found() -> AppError {
    AppError::NotFound("no such playlist".into())
}

#[derive(Deserialize)]
struct ListQuery {
    library_id: Option<String>,
    item_id: Option<String>,
    episode_id: Option<String>,
}

async fn list_playlists(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<PlaylistDto>>, AppError> {
    let profile_id = active_profile(&user)?;
    let library_id = optional_id(q.library_id.as_deref(), "library_id")?;
    let item = optional_id(q.item_id.as_deref(), "item_id")?;
    let episode = optional_id(q.episode_id.as_deref(), "episode_id")?;
    let rows = sqlx::query_as::<_, PlaylistRow>(PLAYLISTS_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(library_id)
        .bind(item.or(episode))
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(Json(rows.into_iter().map(PlaylistDto::from).collect()))
}

// ---------------------------------------------------------------- detail

#[derive(FromRow)]
struct EntryRow {
    id: Uuid,
    item_id: Option<Uuid>,
    episode_id: Option<Uuid>,
    position: i32,
    added_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct EntryDto {
    /// The entry's own id: what reorder and remove name.
    id: Uuid,
    /// `book` or `episode`.
    kind: &'static str,
    added_at: DateTime<Utc>,
    /// The book, as `GET /api/v1/audiobooks/{id}` lists it (with this
    /// profile's progress).
    #[serde(skip_serializing_if = "Option::is_none")]
    book: Option<BookDto>,
    /// The episode, as POD5 serves it (with this profile's progress and
    /// a tokenised `stream_url`).
    #[serde(skip_serializing_if = "Option::is_none")]
    episode: Option<EpisodeDto>,
}

#[derive(Serialize)]
struct PlaylistDetailDto {
    #[serde(flatten)]
    playlist: PlaylistDto,
    items: Vec<EntryDto>,
}

async fn entries(app: &AppState, playlist_id: Uuid) -> Result<Vec<EntryRow>, AppError> {
    sqlx::query_as::<_, EntryRow>(
        "SELECT id, item_id, episode_id, position, added_at FROM playlist_items
         WHERE playlist_id = $1 ORDER BY position, added_at, id",
    )
    .bind(playlist_id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)
}

/// Every entry with what it points at, when this profile can reach it.
async fn resolved(
    app: &AppState,
    profile_id: Uuid,
    playlist_id: Uuid,
    client: Option<parlour_core::ClientKind>,
) -> Result<Vec<(EntryRow, Option<BookDto>, Option<EpisodeDto>)>, AppError> {
    let rows = entries(app, playlist_id).await?;
    let book_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.item_id).collect();
    let episode_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.episode_id).collect();
    let mut books = books_by_ids(app, profile_id, book_ids).await?;
    let mut episodes =
        crate::podcast_api::episodes_by_ids(app, profile_id, episode_ids, client).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let book = r.item_id.and_then(|id| books.remove(&id));
            let episode = r.episode_id.and_then(|id| episodes.remove(&id));
            (r, book, episode)
        })
        .collect())
}

#[derive(Deserialize, Default)]
struct ClientQuery {
    client: Option<String>,
}

fn parse_client(q: &ClientQuery) -> Result<Option<parlour_core::ClientKind>, AppError> {
    match q.client.as_deref() {
        None => Ok(None),
        Some(name) => crate::playback::parse_client(Some(name))
            .map(Some)
            .ok_or_else(|| AppError::BadRequest(format!("unknown client profile {name:?}"))),
    }
}

async fn detail(
    app: &AppState,
    profile_id: Uuid,
    playlist_id: Uuid,
    client: Option<parlour_core::ClientKind>,
) -> Result<PlaylistDetailDto, AppError> {
    let playlist = load_summary(app, profile_id, playlist_id).await?;
    let items = resolved(app, profile_id, playlist_id, client)
        .await?
        .into_iter()
        .filter_map(|(r, book, episode)| match (book, episode) {
            (Some(b), _) => Some(EntryDto {
                id: r.id,
                kind: "book",
                added_at: r.added_at,
                book: Some(b),
                episode: None,
            }),
            (None, Some(e)) => Some(EntryDto {
                id: r.id,
                kind: "episode",
                added_at: r.added_at,
                book: None,
                episode: Some(e),
            }),
            // Out of this profile's reach (its grant, D41): kept, unseen.
            (None, None) => None,
        })
        .collect();
    Ok(PlaylistDetailDto { playlist, items })
}

async fn get_playlist(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<ClientQuery>,
) -> Result<Json<PlaylistDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let client = parse_client(&q)?;
    Ok(Json(
        detail(&app, profile_id, parse_id(&id, "playlist")?, client).await?,
    ))
}

// ------------------------------------------------------------ create etc

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    library_id: Uuid,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// The kind of an audio library `profile_id` has a grant on; 404 for a
/// library it can't reach, 400 for one that holds no audio.
pub(crate) async fn reachable_audio_library(
    app: &AppState,
    profile_id: Uuid,
    library_id: Uuid,
) -> Result<String, AppError> {
    let kind: Option<(String,)> = sqlx::query_as(
        "SELECT l.kind FROM libraries l
         JOIN library_access a ON a.library_id = l.id AND a.profile_id = $1
         WHERE l.id = $2",
    )
    .bind(profile_id)
    .bind(library_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    match kind {
        None => Err(AppError::NotFound("no such library".into())),
        Some((k,)) if k == "audiobooks" || k == "podcasts" => Ok(k),
        Some(_) => Err(AppError::BadRequest(
            "playlists are for audiobooks and podcasts libraries".into(),
        )),
    }
}

async fn create_playlist(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateBody>,
) -> Result<impl IntoResponse, AppError> {
    let profile_id = active_profile(&user)?;
    let name = clean_name(&body.name)?;
    let description = clean_description(body.description.as_deref())?;
    reachable_audio_library(&app, profile_id, body.library_id).await?;
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO playlists (profile_id, library_id, name, description)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(profile_id)
    .bind(body.library_id)
    .bind(&name)
    .bind(&description)
    .fetch_one(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(detail(&app, profile_id, id, None).await?),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PatchBody {
    pub name: Option<String>,
    /// An empty text clears it.
    pub description: Option<String>,
}

async fn patch_playlist(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<PlaylistDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let playlist_id = parse_id(&id, "playlist")?;
    load_summary(&app, profile_id, playlist_id).await?;
    if body.name.is_none() && body.description.is_none() {
        return Err(AppError::BadRequest("send name or description".into()));
    }
    let name = body.name.as_deref().map(clean_name).transpose()?;
    let description = clean_description(body.description.as_deref())?;
    sqlx::query(
        "UPDATE playlists SET name = COALESCE($2, name),
                description = CASE WHEN $3 THEN $4 ELSE description END,
                updated_at = now()
         WHERE id = $1",
    )
    .bind(playlist_id)
    .bind(name)
    .bind(body.description.is_some())
    .bind(description)
    .execute(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(detail(&app, profile_id, playlist_id, None).await?))
}

async fn delete_playlist(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let profile_id = active_profile(&user)?;
    let playlist_id = parse_id(&id, "playlist")?;
    load_summary(&app, profile_id, playlist_id).await?;
    sqlx::query("DELETE FROM playlists WHERE id = $1 AND profile_id = $2")
        .bind(playlist_id)
        .bind(profile_id)
        .execute(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ----------------------------------------------------------------- items

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddBody {
    item_id: Option<Uuid>,
    episode_id: Option<Uuid>,
}

/// The library of a book `profile_id` can reach; 404 otherwise.
pub(crate) async fn reachable_book_library(
    app: &AppState,
    profile_id: Uuid,
    item_id: Uuid,
) -> Result<Uuid, AppError> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT i.library_id FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE i.id = $2 AND i.kind = 'audiobook'",
    )
    .bind(profile_id)
    .bind(item_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    row.map(|r| r.0)
        .ok_or_else(|| AppError::NotFound("no such book".into()))
}

/// The library of an episode `profile_id` can reach (grant and D41's
/// flag, as POD5); 404 otherwise.
async fn reachable_episode_library(
    app: &AppState,
    profile_id: Uuid,
    episode_id: Uuid,
) -> Result<Uuid, AppError> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT p.library_id FROM podcast_episodes e
         JOIN podcasts p ON p.id = e.podcast_id
         JOIN library_access a ON a.library_id = p.library_id AND a.profile_id = $1
         JOIN profiles pr ON pr.id = $1
         WHERE e.id = $2 AND (NOT p.explicit OR pr.allow_explicit)",
    )
    .bind(profile_id)
    .bind(episode_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    row.map(|r| r.0)
        .ok_or_else(|| AppError::NotFound("no such episode".into()))
}

async fn add_item(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<AddBody>,
) -> Result<impl IntoResponse, AppError> {
    let profile_id = active_profile(&user)?;
    let playlist_id = parse_id(&id, "playlist")?;
    let playlist = load_summary(&app, profile_id, playlist_id).await?;
    let (target_library, what) = match (body.item_id, body.episode_id) {
        (Some(item), None) => (
            reachable_book_library(&app, profile_id, item).await?,
            "book",
        ),
        (None, Some(episode)) => (
            reachable_episode_library(&app, profile_id, episode).await?,
            "episode",
        ),
        _ => {
            return Err(AppError::BadRequest(
                "send exactly one of item_id and episode_id".into(),
            ))
        }
    };
    if target_library != playlist.library_id {
        return Err(AppError::BadRequest(format!(
            "that {what} isn't in this playlist's library"
        )));
    }
    let mut tx = app.pool.begin().await.map_err(AppError::from)?;
    // One writer at a time per playlist, so appends don't collide.
    sqlx::query("SELECT id FROM playlists WHERE id = $1 FOR UPDATE")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    // Under the lock, so the check and the insert can't be split. (No
    // ON CONFLICT: the position key is deferrable, which can't arbitrate.)
    let (already,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM playlist_items WHERE playlist_id = $1
                        AND (item_id = $2 OR episode_id = $3))",
    )
    .bind(playlist_id)
    .bind(body.item_id)
    .bind(body.episode_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::from)?;
    if already {
        return Err(AppError::Conflict(format!(
            "that {what} is already in this playlist"
        )));
    }
    sqlx::query(
        "INSERT INTO playlist_items (playlist_id, item_id, episode_id, position)
         SELECT $1, $2, $3, COALESCE(max(position) + 1, 0)
         FROM playlist_items WHERE playlist_id = $1",
    )
    .bind(playlist_id)
    .bind(body.item_id)
    .bind(body.episode_id)
    .execute(&mut *tx)
    .await
    .map_err(AppError::from)?;
    sqlx::query("UPDATE playlists SET updated_at = now() WHERE id = $1")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(detail(&app, profile_id, playlist_id, None).await?),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReorderBody {
    pub items: Vec<Uuid>,
}

async fn reorder_items(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ReorderBody>,
) -> Result<Json<PlaylistDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let playlist_id = parse_id(&id, "playlist")?;
    load_summary(&app, profile_id, playlist_id).await?;
    let mut tx = app.pool.begin().await.map_err(AppError::from)?;
    sqlx::query("SELECT id FROM playlists WHERE id = $1 FOR UPDATE")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    let all = resolved(&app, profile_id, playlist_id, None).await?;
    let current: Vec<(Uuid, i32)> = all.iter().map(|(r, _, _)| (r.id, r.position)).collect();
    let visible: HashSet<Uuid> = all
        .iter()
        .filter(|(_, b, e)| b.is_some() || e.is_some())
        .map(|(r, _, _)| r.id)
        .collect();
    let Some(order) = reorder_slots(&current, &visible, &body.items) else {
        return Err(AppError::Conflict(
            "the playlist has changed since it was loaded: reload it and try again".into(),
        ));
    };
    let ids: Vec<Uuid> = order.iter().map(|(id, _)| *id).collect();
    let positions: Vec<i32> = order.iter().map(|(_, p)| *p).collect();
    sqlx::query(
        "UPDATE playlist_items pi SET position = o.position
         FROM unnest($2::uuid[], $3::int[]) AS o(id, position)
         WHERE pi.id = o.id AND pi.playlist_id = $1",
    )
    .bind(playlist_id)
    .bind(&ids)
    .bind(&positions)
    .execute(&mut *tx)
    .await
    .map_err(AppError::from)?;
    sqlx::query("UPDATE playlists SET updated_at = now() WHERE id = $1")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
    Ok(Json(detail(&app, profile_id, playlist_id, None).await?))
}

#[derive(Serialize)]
struct RemovedDto {
    /// The entry removed was the playlist's last, so the playlist went
    /// too (as in Audiobookshelf).
    playlist_deleted: bool,
}

async fn remove_item(
    State(app): State<AppState>,
    user: AuthUser,
    Path((id, entry)): Path<(String, String)>,
) -> Result<Json<RemovedDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let playlist_id = parse_id(&id, "playlist")?;
    let entry_id = parse_id(&entry, "playlist entry")?;
    load_summary(&app, profile_id, playlist_id).await?;
    // Only an entry this profile can see can be removed.
    let seen = resolved(&app, profile_id, playlist_id, None)
        .await?
        .into_iter()
        .any(|(r, b, e)| r.id == entry_id && (b.is_some() || e.is_some()));
    if !seen {
        return Err(AppError::NotFound("no such playlist entry".into()));
    }
    let mut tx = app.pool.begin().await.map_err(AppError::from)?;
    sqlx::query("SELECT id FROM playlists WHERE id = $1 FOR UPDATE")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    sqlx::query("DELETE FROM playlist_items WHERE id = $1 AND playlist_id = $2")
        .bind(entry_id)
        .bind(playlist_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    let (left,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM playlist_items WHERE playlist_id = $1")
            .bind(playlist_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(AppError::from)?;
    if left == 0 {
        sqlx::query("DELETE FROM playlists WHERE id = $1")
            .bind(playlist_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::from)?;
    } else {
        sqlx::query("UPDATE playlists SET updated_at = now() WHERE id = $1")
            .bind(playlist_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::from)?;
    }
    tx.commit().await.map_err(AppError::from)?;
    Ok(Json(RemovedDto {
        playlist_deleted: left == 0,
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/playlists",
            get(list_playlists).post(create_playlist),
        )
        .route(
            "/api/v1/playlists/{id}",
            get(get_playlist)
                .patch(patch_playlist)
                .delete(delete_playlist),
        )
        .route(
            "/api/v1/playlists/{id}/items",
            axum::routing::post(add_item).put(reorder_items),
        )
        .route("/api/v1/playlists/{id}/items/{entry}", delete(remove_item))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(clean_name("  Road trip ").unwrap(), "Road trip");
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(MAX_NAME_CHARS)).is_ok());
        assert!(clean_name(&"x".repeat(MAX_NAME_CHARS + 1)).is_err());
        assert_eq!(clean_description(Some("  ")).unwrap(), None);
        assert_eq!(clean_description(None).unwrap(), None);
        assert_eq!(
            clean_description(Some(" notes ")).unwrap().as_deref(),
            Some("notes")
        );
        assert!(clean_description(Some(&"x".repeat(MAX_DESCRIPTION_CHARS + 1))).is_err());
    }

    #[test]
    fn a_reorder_names_every_visible_entry_once() {
        let e = ids(3);
        let current: Vec<(Uuid, i32)> = e.iter().copied().zip([0, 1, 2]).collect();
        let visible: HashSet<Uuid> = e.iter().copied().collect();
        let order = reorder_slots(&current, &visible, &[e[2], e[0], e[1]]).unwrap();
        assert_eq!(order, vec![(e[2], 0), (e[0], 1), (e[1], 2)]);
        // Short, repeated, or naming a stranger: refused.
        assert!(reorder_slots(&current, &visible, &[e[2], e[0]]).is_none());
        assert!(reorder_slots(&current, &visible, &[e[2], e[0], e[0]]).is_none());
        assert!(reorder_slots(&current, &visible, &[e[2], e[0], Uuid::new_v4()]).is_none());
    }

    #[test]
    fn hidden_entries_keep_their_places_in_a_reorder() {
        // e1 is out of reach (D41): it stays at position 5; the visible
        // three share positions 0, 9 and 12 in their new order.
        let e = ids(4);
        let current = vec![(e[0], 0), (e[1], 5), (e[2], 9), (e[3], 12)];
        let visible: HashSet<Uuid> = [e[0], e[2], e[3]].into_iter().collect();
        let order = reorder_slots(&current, &visible, &[e[3], e[0], e[2]]).unwrap();
        assert_eq!(order, vec![(e[3], 0), (e[0], 9), (e[2], 12)]);
        // Naming the hidden one is a mismatch.
        assert!(reorder_slots(&current, &visible, &[e[3], e[0], e[1]]).is_none());
    }

    #[test]
    fn covers_are_parlours_own_addresses_without_repeats() {
        let book = Uuid::new_v4();
        let show = Uuid::new_v4();
        let refs = vec![
            CoverRef {
                item_id: Some(book),
                has_cover: true,
                podcast_id: None,
                image_path: None,
            },
            CoverRef {
                item_id: Some(Uuid::new_v4()),
                has_cover: false,
                podcast_id: None,
                image_path: None,
            },
            CoverRef {
                item_id: None,
                has_cover: false,
                podcast_id: Some(show),
                image_path: Some("/artwork/podcasts/x/cover-0123456789abcdef.jpg".into()),
            },
            CoverRef {
                item_id: None,
                has_cover: false,
                podcast_id: Some(show),
                image_path: Some("/artwork/podcasts/x/cover-0123456789abcdef.jpg".into()),
            },
        ];
        let urls = cover_urls(&refs);
        assert_eq!(urls.len(), 2, "{urls:?}");
        assert_eq!(
            urls[0],
            format!("/api/v1/items/{book}/artwork/poster?v=w300")
        );
        assert!(urls[1].starts_with(&format!("/api/v1/podcasts/{show}/artwork")));
    }
}
