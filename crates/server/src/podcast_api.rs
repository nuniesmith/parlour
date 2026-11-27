//! POD5 (Phase 11; D31/D32/D41): the listener API for podcasts. Spec:
//! `docs/AUDIOBOOKS-PODCASTS.md` → "Native API (POD5)".
//!
//! - `GET /api/v1/podcasts?library_id=&q=` — the shows the active
//!   profile can reach, with episode counts and this profile's
//!   finished / in-progress counts
//! - `GET /api/v1/podcasts/{id}?client=` — one show with its episodes
//!   (a serial show oldest first, an episodic one newest first), each
//!   with its downloaded state, lengths, tokenised `stream_url` and
//!   this profile's progress
//! - `GET /api/v1/podcasts/opml?library_id=` — the reachable shows as
//!   OPML (admins only: it carries feed addresses, which may hold a
//!   private token — POD4 reading 6)
//! - `GET /api/v1/podcast-episodes/{id}?client=` — one episode, with
//!   its show; starts its AAC copy when the client can't play the file
//! - `GET /api/v1/podcast-episodes/{id}/stream` — a downloaded
//!   episode's file with HTTP Range; an episode **not** downloaded is a
//!   302 to its enclosure (D32 as amended 2026-11-27)
//! - `GET /api/v1/podcast-episodes/{id}/aac.m4a` — AB10's AAC fallback
//!   for a downloaded episode
//! - `GET|PUT|DELETE /api/v1/podcast-episodes/{id}/progress` — this
//!   profile's progress, under AB11's rules (`audiobooks::apply_write`)
//!   and the podcasts library's finish rule
//! - `GET /api/v1/podcasts/{id}/artwork` — the show's artwork from
//!   Parlour's cache (POD6; never the publisher's address)
//! - `GET /api/v1/podcast-episodes/{id}/chapters` — the episode's
//!   chapters, read server-side from its download or the feed's
//!   chapter list (POD6)
//!
//! **Who sees what.** A profile reaches a show only through its
//! `library_access` grant on the show's podcasts library (D31: no
//! subscriptions), and a show whose feed marks it explicit only while
//! the profile's `allow_explicit` is on (D41). Anything else — the show,
//! its episodes, their streams and progress — is the same 404 as a show
//! that doesn't exist. A stream token proves who asked; the grant and
//! the flag are checked again on every request, so taking either away
//! takes effect at once.
//!
//! **The enclosure address** can carry a paid feed's private token. It
//! leaves the server only in the redirect's `Location`; it is never in
//! an answer's body, an error, the log, a job or a notice.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::audiobooks::{
    active_profile, apply_write, clamp_position, FinishRule, FinishedAt, ProgressDto,
    ProgressWrite, StoredProgress,
};
use crate::auth::{AdminUser, AuthUser};
use crate::error::AppError;
use crate::http::AppState;
use crate::playback::{content_type_for, make_token, serve_range, MaybeUser};

fn parse_id(id: &str, what: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound(format!("no such {what}")))
}

fn not_found_episode() -> AppError {
    AppError::NotFound("no such episode".into())
}

/// The episode, when `profile_id` may reach it: a grant on its show's
/// library, and the show not explicit unless the profile allows
/// explicit content (D41). 404 otherwise. The playback reports use
/// this for `episode_id`.
pub(crate) async fn require_episode_access(
    app: &AppState,
    profile_id: Uuid,
    episode_id: Uuid,
) -> Result<(), AppError> {
    let ok: Option<(bool,)> = sqlx::query_as(
        "SELECT true FROM podcast_episodes e
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
    ok.map(|_| ()).ok_or_else(not_found_episode)
}

// ------------------------------------------------------------------ shows

#[derive(Debug, Serialize, FromRow)]
struct ShowDto {
    id: Uuid,
    library_id: Uuid,
    title: String,
    sort_title: String,
    author: Option<String>,
    /// The feed's description, as the feed gives it (it may be HTML).
    description: Option<String>,
    language: Option<String>,
    explicit: bool,
    /// `episodic` or `serial` (a serial show lists oldest first).
    podcast_type: Option<String>,
    genres: Vec<String>,
    created_at: DateTime<Utc>,
    last_checked_at: Option<DateTime<Utc>>,
    episode_count: i64,
    downloaded_count: i64,
    latest_published_at: Option<DateTime<Utc>>,
    /// This profile's episodes finished, and started but not finished.
    finished_count: i64,
    in_progress_count: i64,
    /// The cached artwork's file (never sent).
    #[serde(skip)]
    image_path: Option<String>,
    /// POD6: where a page loads the show's artwork from — Parlour's own
    /// cache, never the publisher. `null` when none is cached.
    #[sqlx(skip)]
    artwork_url: Option<String>,
}

impl ShowDto {
    fn with_artwork(mut self) -> Self {
        self.artwork_url =
            parlour_catalog::podcast_artwork::artwork_route(self.id, self.image_path.as_deref());
        self
    }
}

/// The one show query: `$1` profile, `$2` show id or NULL, `$3`
/// library id or NULL, `$4` search text or NULL. The grant path is
/// POD1's join through `library_access`, plus D41's explicit flag.
const SHOWS_SQL: &str = "
    SELECT p.id, p.library_id, p.title, p.sort_title, p.author, p.description, p.language,
           p.explicit, p.podcast_type, p.genres, p.created_at, p.last_checked_at,
           (SELECT count(*) FROM podcast_episodes e WHERE e.podcast_id = p.id) AS episode_count,
           (SELECT count(*) FROM podcast_episodes e
            WHERE e.podcast_id = p.id AND e.file_path IS NOT NULL) AS downloaded_count,
           (SELECT max(e.published_at) FROM podcast_episodes e
            WHERE e.podcast_id = p.id) AS latest_published_at,
           (SELECT count(*) FROM podcast_episodes e
            JOIN book_progress bp ON bp.episode_id = e.id AND bp.profile_id = $1
            WHERE e.podcast_id = p.id AND bp.finished) AS finished_count,
           (SELECT count(*) FROM podcast_episodes e
            JOIN book_progress bp ON bp.episode_id = e.id AND bp.profile_id = $1
            WHERE e.podcast_id = p.id AND NOT bp.finished AND bp.position_ms > 0)
               AS in_progress_count,
           p.image_path
    FROM podcasts p
    JOIN library_access a ON a.library_id = p.library_id AND a.profile_id = $1
    JOIN profiles pr ON pr.id = $1
    WHERE (NOT p.explicit OR pr.allow_explicit)
      AND ($2::uuid IS NULL OR p.id = $2)
      AND ($3::uuid IS NULL OR p.library_id = $3)
      AND ($4::text IS NULL OR p.title ILIKE '%' || $4 || '%'
           OR p.author ILIKE '%' || $4 || '%')
    ORDER BY p.sort_title, p.id";

#[derive(Deserialize)]
struct ListQuery {
    library_id: Option<String>,
    q: Option<String>,
}

fn library_param(raw: Option<&str>) -> Result<Option<Uuid>, AppError> {
    match raw.filter(|s| !s.is_empty()) {
        Some(s) => {
            Ok(Some(Uuid::parse_str(s).map_err(|_| {
                AppError::BadRequest("bad library_id".into())
            })?))
        }
        None => Ok(None),
    }
}

async fn list_shows(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<ShowDto>>, AppError> {
    let profile_id = active_profile(&user)?;
    let library_id = library_param(q.library_id.as_deref())?;
    let search =
        q.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    let shows = sqlx::query_as::<_, ShowDto>(SHOWS_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(library_id)
        .bind(search)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(Json(shows.into_iter().map(ShowDto::with_artwork).collect()))
}

async fn load_show(app: &AppState, profile_id: Uuid, show_id: Uuid) -> Result<ShowDto, AppError> {
    sqlx::query_as::<_, ShowDto>(SHOWS_SQL)
        .bind(profile_id)
        .bind(Some(show_id))
        .bind(None::<Uuid>)
        .bind(None::<String>)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?
        .map(ShowDto::with_artwork)
        .ok_or_else(|| AppError::NotFound("no such podcast".into()))
}

// --------------------------------------------------------------- episodes

/// An episode as read for a profile. Deliberately holds neither the
/// enclosure address nor the feed's: neither may reach an answer body.
#[derive(FromRow)]
struct EpisodeRow {
    id: Uuid,
    podcast_id: Uuid,
    library_id: Uuid,
    show_title: String,
    show_image_path: Option<String>,
    title: String,
    subtitle: Option<String>,
    description: Option<String>,
    published_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    season: Option<i32>,
    episode_number: Option<i32>,
    episode_type: Option<String>,
    enclosure_type: Option<String>,
    enclosure_length: Option<i64>,
    file_path: Option<String>,
    file_size_bytes: Option<i64>,
    file_duration_ms: Option<i64>,
    file_container: Option<String>,
    file_codec: Option<String>,
    downloaded_at: Option<DateTime<Utc>>,
    p_position_ms: Option<i64>,
    p_duration_ms: Option<i64>,
    p_finished: Option<bool>,
    p_finished_at: Option<DateTime<Utc>>,
    p_last_listened_at: Option<DateTime<Utc>>,
    p_updated_at: Option<DateTime<Utc>>,
    p_hidden: Option<bool>,
    p_client_updated_at: Option<DateTime<Utc>>,
}

impl EpisodeRow {
    /// The length progress clamps and finishes against: the
    /// downloaded file's, else the feed's (POD1 reading 9).
    fn length_ms(&self) -> Option<i64> {
        self.file_duration_ms
            .or(self.duration_ms)
            .filter(|d| *d > 0)
    }

    fn progress(&self) -> ProgressDto {
        ProgressDto {
            position_ms: self.p_position_ms.unwrap_or(0),
            duration_ms: self.p_duration_ms,
            finished: self.p_finished.unwrap_or(false),
            finished_at: self.p_finished_at,
            last_listened_at: self.p_last_listened_at,
            updated_at: self.p_updated_at,
            hidden: self.p_hidden.unwrap_or(false),
            client_updated_at: self.p_client_updated_at,
        }
    }
}

/// `$1` profile, `$2` episode id or NULL, `$3` show id or NULL, `$4` a
/// list of episode ids or NULL (only those; POD8's playlists). Order:
/// a serial show's episodes oldest first, every other show's newest
/// first; undated ones last, then by guid (POD4 reading 13).
const EPISODES_SQL: &str = "
    SELECT e.id, e.podcast_id, p.library_id, p.title AS show_title,
           p.image_path AS show_image_path, e.title, e.subtitle,
           e.description, e.published_at, e.duration_ms, e.season, e.episode_number,
           e.episode_type, e.enclosure_type, e.enclosure_length, e.file_path,
           e.file_size_bytes, e.file_duration_ms, e.file_container, e.file_codec,
           e.downloaded_at,
           bp.position_ms AS p_position_ms, bp.duration_ms AS p_duration_ms,
           bp.finished AS p_finished, bp.finished_at AS p_finished_at,
           bp.last_listened_at AS p_last_listened_at, bp.updated_at AS p_updated_at,
           bp.hidden AS p_hidden, bp.client_updated_at AS p_client_updated_at
    FROM podcast_episodes e
    JOIN podcasts p ON p.id = e.podcast_id
    JOIN library_access a ON a.library_id = p.library_id AND a.profile_id = $1
    JOIN profiles pr ON pr.id = $1
    LEFT JOIN book_progress bp ON bp.episode_id = e.id AND bp.profile_id = $1
    WHERE (NOT p.explicit OR pr.allow_explicit)
      AND ($2::uuid IS NULL OR e.id = $2)
      AND ($3::uuid IS NULL OR e.podcast_id = $3)
      AND ($4::uuid[] IS NULL OR e.id = ANY($4))
    ORDER BY
      CASE WHEN p.podcast_type = 'serial' THEN e.published_at END ASC NULLS LAST,
      CASE WHEN p.podcast_type IS DISTINCT FROM 'serial' THEN e.published_at END DESC NULLS LAST,
      e.guid";

async fn load_episode(
    app: &AppState,
    profile_id: Uuid,
    episode_id: Uuid,
) -> Result<EpisodeRow, AppError> {
    sqlx::query_as::<_, EpisodeRow>(EPISODES_SQL)
        .bind(profile_id)
        .bind(Some(episode_id))
        .bind(None::<Uuid>)
        .bind(None::<Vec<Uuid>>)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?
        .ok_or_else(not_found_episode)
}

/// POD8: the episodes among `ids` that `profile_id` can reach (its grant
/// and D41's flag), as a listener sees them, keyed by id. An unreachable
/// id is simply absent. `client` decides each one's stream, as the show
/// page does.
pub(crate) async fn episodes_by_ids(
    app: &AppState,
    profile_id: Uuid,
    ids: Vec<Uuid>,
    client: Option<parlour_core::ClientKind>,
) -> Result<std::collections::HashMap<Uuid, EpisodeDto>, AppError> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let rows = sqlx::query_as::<_, EpisodeRow>(EPISODES_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .bind(Some(ids))
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(rows
        .iter()
        .map(|r| (r.id, episode_dto(app, profile_id, r, client, None).0))
        .collect())
}

/// AB10's decision for a downloaded episode, in the book track's words.
#[derive(Debug, Serialize)]
struct EpisodePlanDto {
    /// `DirectPlay` | `Remux` (AAC copied into MP4) | `Transcode` (to AAC)
    action: String,
    /// `Direct` | `ContainerUnsupported` | `CodecUnsupported`
    reason: String,
    /// `direct_play` | `copy` | `transcode`
    mode: &'static str,
    /// `file` (the download) or `aac` (the fallback copy)
    delivery: &'static str,
    client: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct EpisodeDto {
    id: Uuid,
    podcast_id: Uuid,
    library_id: Uuid,
    show_title: String,
    /// POD6: the show's cached artwork (Parlour's route), or `null`.
    artwork_url: Option<String>,
    title: String,
    subtitle: Option<String>,
    /// As the feed gives it (it may be HTML).
    description: Option<String>,
    published_at: Option<DateTime<Utc>>,
    season: Option<i32>,
    episode_number: Option<i32>,
    /// `full`, `trailer` or `bonus`.
    episode_type: Option<String>,
    /// The feed's length (`itunes:duration`).
    duration_ms: Option<i64>,
    /// The length progress uses: the download's, else the feed's.
    length_ms: Option<i64>,
    downloaded: bool,
    downloaded_at: Option<DateTime<Utc>>,
    /// The download's size, else the size the feed announces.
    size_bytes: Option<i64>,
    file_duration_ms: Option<i64>,
    container: Option<String>,
    codec: Option<String>,
    /// Where to play it from, with a 6 h stream token: the download
    /// (Range), its AAC copy, or — not downloaded — the stream route,
    /// which redirects to the publisher.
    stream_url: String,
    /// `file`, `aac` or `enclosure` (a redirect to the publisher).
    delivery: &'static str,
    /// The type of what `stream_url` serves; for an enclosure, the type
    /// the feed announces, when it does.
    content_type: Option<String>,
    /// With `?client=`, the decision for a downloaded episode; `null`
    /// without one, and for an enclosure (no file to decide on).
    plan: Option<EpisodePlanDto>,
    progress: ProgressDto,
}

#[derive(Deserialize, Default)]
struct ClientQuery {
    /// AB10's client profile (`web`, `android`, `androidtv` / `firetv`,
    /// `ios`, `tvos`).
    client: Option<String>,
    network: Option<String>,
}

fn parse_client(q: &ClientQuery) -> Result<Option<parlour_core::ClientKind>, AppError> {
    match q.client.as_deref() {
        None => Ok(None),
        Some(name) => crate::playback::parse_client(Some(name))
            .map(Some)
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "unknown client profile {name:?} (web, android, androidtv, firetv, ios, tvos)"
                ))
            }),
    }
}

/// Turn a row into what a listener sees, deciding its stream for the
/// client named. Returns whether the AAC fallback is wanted, and the
/// plan to remember for the listening session.
fn episode_dto(
    app: &AppState,
    profile_id: Uuid,
    row: &EpisodeRow,
    client: Option<parlour_core::ClientKind>,
    network: Option<&str>,
) -> (EpisodeDto, Option<crate::playback::StreamPlan>) {
    let token = make_token(&app.playback, row.id, profile_id);
    let downloaded = row.file_path.is_some();
    let decision = match (client, downloaded) {
        (Some(c), true) => Some((
            c,
            parlour_playback::decide_audio(
                row.file_container.as_deref(),
                row.file_codec.as_deref(),
                c,
            ),
        )),
        _ => None,
    };
    let fallback =
        decision.is_some_and(|(_, d)| d.action != parlour_core::PlaybackAction::DirectPlay);
    let (stream_url, delivery, content_type) = if !downloaded {
        (
            format!("/api/v1/podcast-episodes/{}/stream?token={token}", row.id),
            "enclosure",
            row.enclosure_type.clone(),
        )
    } else if fallback {
        (
            format!("/api/v1/podcast-episodes/{}/aac.m4a?token={token}", row.id),
            "aac",
            Some("audio/mp4".to_string()),
        )
    } else {
        (
            format!("/api/v1/podcast-episodes/{}/stream?token={token}", row.id),
            "file",
            Some(
                content_type_for(
                    row.file_container.as_deref(),
                    row.file_path.as_deref().unwrap_or(""),
                )
                .to_string(),
            ),
        )
    };
    let plan = decision.map(|(c, d)| {
        let bitrate_kbps = match (row.file_size_bytes, row.file_duration_ms) {
            (Some(size), Some(ms)) if ms > 0 => Some(size * 8 / ms),
            _ => None,
        };
        crate::playback::StreamPlan::for_book(
            d,
            delivery,
            c,
            crate::playback::parse_network(network),
            row.file_container.clone(),
            bitrate_kbps,
            row.file_codec.clone(),
            None,
        )
    });
    let dto = EpisodeDto {
        id: row.id,
        podcast_id: row.podcast_id,
        library_id: row.library_id,
        show_title: row.show_title.clone(),
        artwork_url: parlour_catalog::podcast_artwork::artwork_route(
            row.podcast_id,
            row.show_image_path.as_deref(),
        ),
        title: row.title.clone(),
        subtitle: row.subtitle.clone(),
        description: row.description.clone(),
        published_at: row.published_at,
        season: row.season,
        episode_number: row.episode_number,
        episode_type: row.episode_type.clone(),
        duration_ms: row.duration_ms,
        length_ms: row.length_ms(),
        downloaded,
        downloaded_at: row.downloaded_at,
        size_bytes: row.file_size_bytes.or(row.enclosure_length),
        file_duration_ms: row.file_duration_ms,
        container: row.file_container.clone(),
        codec: row.file_codec.clone(),
        stream_url,
        delivery,
        content_type,
        plan: decision.map(|(c, d)| EpisodePlanDto {
            action: format!("{:?}", d.action),
            reason: format!("{:?}", d.reason),
            mode: crate::playback::stream_mode(d.action),
            delivery,
            client: crate::playback::client_name(c),
        }),
        progress: row.progress(),
    };
    (dto, plan)
}

#[derive(Serialize)]
struct ShowDetailDto {
    #[serde(flatten)]
    show: ShowDto,
    episodes: Vec<EpisodeDto>,
}

async fn get_show(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<ClientQuery>,
) -> Result<Json<ShowDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let show_id = parse_id(&id, "podcast")?;
    let client = parse_client(&q)?;
    let show = load_show(&app, profile_id, show_id).await?;
    let rows = sqlx::query_as::<_, EpisodeRow>(EPISODES_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(Some(show_id))
        .bind(None::<Vec<Uuid>>)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    let episodes = rows
        .iter()
        .map(|r| episode_dto(&app, profile_id, r, client, q.network.as_deref()).0)
        .collect();
    Ok(Json(ShowDetailDto { show, episodes }))
}

async fn get_episode(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<ClientQuery>,
) -> Result<Json<EpisodeDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let episode_id = parse_id(&id, "episode")?;
    let client = parse_client(&q)?;
    let row = load_episode(&app, profile_id, episode_id).await?;
    let (dto, plan) = episode_dto(&app, profile_id, &row, client, q.network.as_deref());
    if dto.delivery == "aac" {
        // As the book detail does for its resume track: start the copy
        // now, so it is ready (or nearly) when the player asks.
        if let Some(src) = crate::playback::load_episode_audio_source(&app, episode_id).await? {
            crate::playback::prepare_audio_fallback_for(&app, &src);
        }
    }
    if let Some(plan) = plan {
        crate::playback::remember_book_plan(&app.playback, profile_id, episode_id, plan);
    }
    Ok(Json(dto))
}

// ----------------------------------------------------------------- stream

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// Who is asking for an episode's audio: the profile of a valid stream
/// token for it, else a signed-in session's active profile; then the
/// grant and D41's flag for that profile, checked now. Unauthorised
/// when neither proves anyone (before saying whether it exists).
async fn authorise_episode(
    app: &AppState,
    episode_id: Uuid,
    token: Option<&str>,
    user: Option<&AuthUser>,
) -> Result<Uuid, AppError> {
    let profile_id = match crate::playback::token_profile(&app.playback, token, episode_id)? {
        Some(pid) => pid,
        None => user
            .and_then(|u| u.active_profile_id)
            .ok_or_else(|| AppError::Unauthorized("media access denied".into()))?,
    };
    require_episode_access(app, profile_id, episode_id).await?;
    Ok(profile_id)
}

/// The enclosure as a redirect target: only an absolute http(s)
/// address that fits in a header. `None` otherwise (never echoed).
fn redirect_target(enclosure_url: &str) -> Option<HeaderValue> {
    let url = reqwest::Url::parse(enclosure_url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    HeaderValue::from_str(url.as_str()).ok()
}

/// Where an episode's audio comes from. Read only by the stream route:
/// the enclosure address goes into the redirect and nowhere else.
#[derive(FromRow)]
struct StreamSource {
    file_path: Option<String>,
    file_size_bytes: Option<i64>,
    file_container: Option<String>,
    enclosure_url: String,
}

/// A downloaded episode's file with Range; an episode not downloaded is
/// a 302 to its enclosure (D32 as amended 2026-11-27). The redirect is
/// `no-store` and asks for no Referer, and its address is never logged.
async fn stream_episode(
    State(app): State<AppState>,
    headers: HeaderMap,
    user: MaybeUser,
    Path(id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Result<Response, AppError> {
    let episode_id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) if user.0.is_none() && q.token.is_none() => {
            return Err(AppError::Unauthorized("media access denied".into()))
        }
        Err(_) => return Err(not_found_episode()),
    };
    authorise_episode(&app, episode_id, q.token.as_deref(), user.0.as_ref()).await?;
    let row: Option<StreamSource> = sqlx::query_as(
        "SELECT file_path, file_size_bytes, file_container, enclosure_url
         FROM podcast_episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    let Some(StreamSource {
        file_path,
        file_size_bytes: size,
        file_container: container,
        enclosure_url,
    }) = row
    else {
        return Err(not_found_episode());
    };
    if let Some(path) = file_path {
        return serve_range(
            &path,
            size.unwrap_or(0),
            &headers,
            content_type_for(container.as_deref(), &path),
        )
        .await;
    }
    let Some(location) = redirect_target(&enclosure_url) else {
        return Err(AppError::NotFound(
            "this episode isn't downloaded, and its feed gives no address it can be played from"
                .into(),
        ));
    };
    Ok((
        StatusCode::FOUND,
        [
            (header::LOCATION, location),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ),
        ],
    )
        .into_response())
}

/// AB10's AAC copy of a downloaded episode, with Range; 503 with
/// `Retry-After` while it is made. An episode not downloaded has none.
async fn stream_episode_aac(
    State(app): State<AppState>,
    headers: HeaderMap,
    user: MaybeUser,
    Path(id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Result<Response, AppError> {
    let episode_id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) if user.0.is_none() && q.token.is_none() => {
            return Err(AppError::Unauthorized("media access denied".into()))
        }
        Err(_) => return Err(not_found_episode()),
    };
    authorise_episode(&app, episode_id, q.token.as_deref(), user.0.as_ref()).await?;
    let Some(src) = crate::playback::load_episode_audio_source(&app, episode_id).await? else {
        return Err(AppError::NotFound(
            "this episode isn't downloaded, so it has no AAC copy".into(),
        ));
    };
    crate::playback::serve_audio_fallback(&app, &headers, &src).await
}

// --------------------------------------------------------------- progress

async fn get_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ProgressDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let row = load_episode(&app, profile_id, parse_id(&id, "episode")?).await?;
    Ok(Json(row.progress()))
}

#[derive(Deserialize)]
struct ProgressBody {
    position_ms: Option<i64>,
    finished: Option<bool>,
    hidden: Option<bool>,
    client_updated_at: Option<DateTime<Utc>>,
}

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

/// Write this profile's progress on an episode: AB11's rules as they
/// are for a book (`apply_write`), against the episode's length (the
/// download's, else the feed's) and its podcasts library's finish rule.
/// An episode that isn't downloaded takes writes too — it plays through
/// the redirect (D32 as amended) — with the feed's length; with no
/// length known, positions aren't clamped and never finish it.
async fn put_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ProgressBody>,
) -> Result<Json<ProgressDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let episode = load_episode(&app, profile_id, parse_id(&id, "episode")?).await?;
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
    let length = episode.length_ms();
    let write = ProgressWrite {
        position_ms: body.position_ms.map(|p| clamp_position(p, length)),
        finished: body.finished,
        hidden: body.hidden,
    };

    let mut tx = app.pool.begin().await.map_err(AppError::from)?;
    let (remaining_s, percent): (i32, Option<i32>) = sqlx::query_as(
        "SELECT finish_remaining_seconds, finish_percent FROM libraries WHERE id = $1",
    )
    .bind(episode.library_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::from)?;
    let rule = FinishRule {
        remaining_ms: i64::from(remaining_s) * 1000,
        percent,
    };
    let stored: Option<(i64, bool, bool)> = sqlx::query_as(
        "SELECT position_ms, finished, hidden FROM book_progress
         WHERE profile_id = $1 AND episode_id = $2 FOR UPDATE",
    )
    .bind(profile_id)
    .bind(episode.id)
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
    let next = apply_write(stored, write, length, rule);
    let finished_at = match next.finished_at {
        FinishedAt::Keep => "keep",
        FinishedAt::Now => "now",
        FinishedAt::Clear => "clear",
    };
    let row = sqlx::query_as::<_, ProgressRow>(
        "INSERT INTO book_progress
             (profile_id, episode_id, position_ms, duration_ms, finished, finished_at,
              last_listened_at, updated_at, hidden, client_updated_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 = 'now' THEN now() END,
                 CASE WHEN $7 THEN now() END, now(), $8, COALESCE($9, now()))
         ON CONFLICT (profile_id, episode_id) DO UPDATE SET
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
    .bind(episode.id)
    .bind(next.position_ms)
    .bind(length)
    .bind(next.finished)
    .bind(finished_at)
    .bind(next.listened)
    .bind(next.hidden)
    .bind(body.client_updated_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
    Ok(Json(ProgressDto {
        position_ms: row.position_ms,
        duration_ms: row.duration_ms,
        finished: row.finished,
        finished_at: row.finished_at,
        last_listened_at: row.last_listened_at,
        updated_at: Some(row.updated_at),
        hidden: row.hidden,
        client_updated_at: row.client_updated_at,
    }))
}

async fn delete_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let profile_id = active_profile(&user)?;
    let episode = load_episode(&app, profile_id, parse_id(&id, "episode")?).await?;
    sqlx::query("DELETE FROM book_progress WHERE profile_id = $1 AND episode_id = $2")
        .bind(profile_id)
        .bind(episode.id)
        .execute(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- artwork

/// POD6: a show's artwork from Parlour's cache, to a profile that can
/// reach the show (its grant, and D41's flag). The publisher's address
/// is never sent or followed here: the copy was fetched by a feed check
/// (`parlour_catalog::podcast_artwork`). 404 when nothing is cached, so
/// a page falls back to its initial tile.
async fn show_artwork(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let profile_id = active_profile(&user)?;
    let show_id = parse_id(&id, "podcast")?;
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT p.image_path FROM podcasts p
         JOIN library_access a ON a.library_id = p.library_id AND a.profile_id = $1
         JOIN profiles pr ON pr.id = $1
         WHERE p.id = $2 AND (NOT p.explicit OR pr.allow_explicit)",
    )
    .bind(profile_id)
    .bind(show_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    let Some((image_path,)) = row else {
        return Err(AppError::NotFound("no such podcast".into()));
    };
    let no_artwork = || AppError::NotFound("no artwork cached for that podcast".into());
    let path = image_path.ok_or_else(no_artwork)?;
    let content_type =
        parlour_catalog::podcast_artwork::content_type_of(&path).ok_or_else(no_artwork)?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| no_artwork())?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            // The address carries the copy's key (`?v=`), so a browser
            // may keep it; `private` because it sits behind a sign-in.
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=604800"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        bytes,
    )
        .into_response())
}

// --------------------------------------------------------------- chapters

#[derive(Serialize)]
struct ChaptersDto {
    /// `file` (the download's own chapter marks), `feed` (the feed's
    /// Podcasting 2.0 chapter list), or `null` (neither has any).
    source: Option<&'static str>,
    chapters: Vec<parlour_catalog::podcast_chapters::EpisodeChapter>,
    /// Why the feed's chapter list couldn't be read (never its address).
    error: Option<String>,
}

/// POD6: an episode's chapters for the player, read server-side (POD5
/// reading 12): the download's own chapter marks when it has titled
/// ones (no publisher is asked), else the feed's chapter list, fetched
/// now. Only start times and titles are sent; a chapter's image and
/// link are publisher addresses and are dropped. A chapter list that
/// can't be fetched is an empty list with the reason, not an error, so
/// the player plays on without chapters.
async fn episode_chapters(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ChaptersDto>, AppError> {
    use parlour_catalog::podcast_chapters::{fetch_json_chapters, from_file};
    let profile_id = active_profile(&user)?;
    let episode = load_episode(&app, profile_id, parse_id(&id, "episode")?).await?;
    if let Some(path) = episode.file_path.clone() {
        let probed = tokio::task::spawn_blocking(move || {
            parlour_catalog::audiobooks::probe_audio(std::path::Path::new(&path))
        })
        .await
        .ok()
        .and_then(Result::ok);
        let chapters = probed.map(|p| from_file(&p.chapters)).unwrap_or_default();
        if !chapters.is_empty() {
            return Ok(Json(ChaptersDto {
                source: Some("file"),
                chapters,
                error: None,
            }));
        }
    }
    let (chapters_url,): (Option<String>,) =
        sqlx::query_as("SELECT chapters_url FROM podcast_episodes WHERE id = $1")
            .bind(episode.id)
            .fetch_one(&app.pool)
            .await
            .map_err(AppError::from)?;
    let Some(url) = chapters_url.filter(|u| !u.trim().is_empty()) else {
        return Ok(Json(ChaptersDto {
            source: None,
            chapters: Vec::new(),
            error: None,
        }));
    };
    Ok(Json(
        match fetch_json_chapters(&app.podcast_http, &url).await {
            Ok(chapters) => ChaptersDto {
                source: Some("feed"),
                chapters,
                error: None,
            },
            Err(why) => {
                tracing::info!(episode_id = %episode.id, reason = %why, "podcast chapters not read");
                ChaptersDto {
                    source: Some("feed"),
                    chapters: Vec::new(),
                    error: Some(format!(
                        "The publisher's chapter list couldn't be read: {why}."
                    )),
                }
            }
        },
    ))
}

// ------------------------------------------------------------------- OPML

#[derive(Deserialize)]
struct OpmlQuery {
    library_id: Option<String>,
}

/// The shows the admin's active profile can reach (grants and D41's
/// flag), as OPML 2.0 — POD4's writer and file name. Admins only: an
/// OPML file is nothing without each feed's address, and an address
/// may hold a paid feed's private token, which only admins see (POD4
/// reading 6). A non-admin gets the admin surface's 404.
async fn export_opml(
    State(app): State<AppState>,
    admin: AdminUser,
    Query(q): Query<OpmlQuery>,
) -> Result<Response, AppError> {
    let profile_id = active_profile(&admin.0)?;
    let library_id = library_param(q.library_id.as_deref())?;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT p.title, p.feed_url
         FROM podcasts p
         JOIN library_access a ON a.library_id = p.library_id AND a.profile_id = $1
         JOIN profiles pr ON pr.id = $1
         WHERE (NOT p.explicit OR pr.allow_explicit)
           AND ($2::uuid IS NULL OR p.library_id = $2)
         ORDER BY p.sort_title, p.id",
    )
    .bind(profile_id)
    .bind(library_id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    let name = match library_id {
        Some(id) => sqlx::query_as::<_, (String,)>("SELECT name FROM libraries WHERE id = $1")
            .bind(id)
            .fetch_optional(&app.pool)
            .await
            .map_err(AppError::from)?
            .map(|(n,)| n)
            .unwrap_or_else(|| "Podcasts".to_string()),
        None => "Podcasts".to_string(),
    };
    let shows: Vec<parlour_catalog::opml::OpmlShow> = rows
        .into_iter()
        .map(|(title, feed_url)| parlour_catalog::opml::OpmlShow { title, feed_url })
        .collect();
    let doc = parlour_catalog::opml::write_opml(&format!("Parlour — {name}"), Utc::now(), &shows);
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "text/x-opml; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"{}\"",
                    crate::podcast_admin::opml_file_name(&name)
                ),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        doc,
    )
        .into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/podcasts", get(list_shows))
        .route("/api/v1/podcasts/opml", get(export_opml))
        .route("/api/v1/podcasts/{id}", get(get_show))
        .route("/api/v1/podcasts/{id}/artwork", get(show_artwork))
        .route("/api/v1/podcast-episodes/{id}", get(get_episode))
        .route("/api/v1/podcast-episodes/{id}/stream", get(stream_episode))
        .route(
            "/api/v1/podcast-episodes/{id}/aac.m4a",
            get(stream_episode_aac),
        )
        .route(
            "/api/v1/podcast-episodes/{id}/chapters",
            get(episode_chapters),
        )
        .route(
            "/api/v1/podcast-episodes/{id}/progress",
            get(get_progress).put(put_progress).delete(delete_progress),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absolute_web_addresses_are_redirect_targets() {
        let ok = redirect_target("https://cdn.example/ep1.mp3?token=fake-feed-token").unwrap();
        assert_eq!(
            ok.to_str().unwrap(),
            "https://cdn.example/ep1.mp3?token=fake-feed-token"
        );
        assert!(redirect_target("  http://cdn.example/a b.mp3 ").is_some());
        assert!(redirect_target("javascript:alert(1)").is_none());
        assert!(redirect_target("ftp://cdn.example/ep1.mp3").is_none());
        assert!(redirect_target("/relative/ep1.mp3").is_none());
        assert!(redirect_target("file:///etc/passwd").is_none());
        assert!(redirect_target("").is_none());
    }
}
