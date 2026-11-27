//! Listening sessions (Phase 10, AB8; D29/D35;
//! `docs/AUDIOBOOKS-PODCASTS.md`, "Listening sessions and Home shelves
//! (AB8)").
//!
//! A listen is a **session**, not a play count, as Audiobookshelf keeps
//! it: a book's listens never touch `user_data.play_count`. The web
//! player opens one through the existing playback reports
//! (`POST /api/v1/playback/{start,progress,stopped}`, which hand an
//! audiobook to this module) and the session is kept in
//! `listening_sessions`, per profile. The live-session registry
//! (`crate::playback`) carries it too, so an open listening session
//! shows in Now Playing beside the viewing sessions and an admin can
//! stop it there. A closed session goes into the Activity feed
//! (`playback.listened`).
//!
//! - `GET /api/v1/listening/sessions?before=&limit=` — this profile's
//!   own sessions, newest first, open ones included (`live` while the
//!   player is still reporting)
//! - `GET /api/v1/listening/stats?tz=` — this profile's listening
//!   totals: all time, today, per day and per book
//! - `GET /api/v1/admin/listening/sessions?profile_id=&before=&limit=`
//!   — every profile's sessions, for admins
//!
//! POD5 widened sessions to podcast episodes (`Target::Episode`,
//! `listening_sessions.episode_id`): the same open / replace / beat /
//! close rules, reached through the same reports with `episode_id`.
//! An episode's session is listed with its show, and counts in the
//! stats' totals and days (the per-book list stays books).
//!
//! The native-client session API (device tokens, sessions recorded
//! offline and uploaded later, the documented contract) is CL3/CL6.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::AppError;
use crate::http::AppState;

/// Slack on top of the wall-clock time between two reports, for the
/// listening time one report may add (timer jitter, a slow request).
pub const LISTENED_SLACK_MS: i64 = 5_000;

/// A session not heard from for this long is closed at its last
/// report. The same span after which the live registry forgets a
/// session.
pub const STALE_AFTER_SECS: i64 = 3600;

/// A session counts as live (Now Playing, `live`) while it has
/// reported within this many seconds — the viewing sessions' rule.
pub const LIVE_SECS: i64 = 90;

/// How much listening time one report adds. The player says how long
/// it listened since its previous report (`reported`); a client that
/// doesn't say is credited with how far the position moved forward.
/// Either way the claim is never negative and never more than the
/// wall-clock time since the previous report plus
/// [`LISTENED_SLACK_MS`], so a client can't report more listening than
/// there was time for.
pub fn listened_increment(reported: Option<i64>, position_delta_ms: i64, wall_ms: i64) -> i64 {
    let claimed = reported.unwrap_or(position_delta_ms);
    claimed.clamp(0, wall_ms.max(0) + LISTENED_SLACK_MS)
}

/// How a listening time reads in a sentence: "45 s", "18 min",
/// "2 h 5 min".
pub fn listened_label(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    if secs < 60 {
        return format!("{secs} s");
    }
    let mins = secs / 60;
    let (h, m) = (mins / 60, mins % 60);
    match (h, m) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// What a session is a listen to: a book (`listening_sessions.item_id`)
/// or a podcast episode (`episode_id`, POD5). Exactly one is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Book(Uuid),
    Episode(Uuid),
}

impl Target {
    /// The book's or the episode's id.
    pub fn id(self) -> Uuid {
        match self {
            Target::Book(id) | Target::Episode(id) => id,
        }
    }
    /// The `item_id` column's value.
    pub fn item_id(self) -> Option<Uuid> {
        match self {
            Target::Book(id) => Some(id),
            Target::Episode(_) => None,
        }
    }
    /// The `episode_id` column's value.
    pub fn episode_id(self) -> Option<Uuid> {
        match self {
            Target::Episode(id) => Some(id),
            Target::Book(_) => None,
        }
    }
}

/// What a player said about itself when it opened a session.
#[derive(Debug, Clone, Default)]
pub struct ClientLabels {
    pub name: Option<String>,
    pub version: Option<String>,
    pub device: Option<String>,
}

/// A session as stored, the parts the reports need.
#[derive(Debug, Clone, FromRow)]
pub struct OpenSession {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub device_name: Option<String>,
}

/// One report against an open session.
#[derive(Debug, Clone, Copy)]
pub struct Beat {
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    /// Listening time since the previous report, as the player
    /// measured it.
    pub listened_ms: Option<i64>,
}

/// Open a session for `profile_id` on a book or an episode. Any
/// session the same profile still has open on the same one is closed
/// first (`replaced`), with its Activity line.
pub async fn open(
    pool: &parlour_db::PgPool,
    session_id: Uuid,
    profile_id: Uuid,
    target: Target,
    position_ms: i64,
    duration_ms: Option<i64>,
    client: &ClientLabels,
) -> Result<(), AppError> {
    let replaced: Vec<(Uuid,)> = sqlx::query_as(
        "UPDATE listening_sessions SET ended_at = now(), end_reason = 'replaced'
         WHERE profile_id = $1 AND item_id IS NOT DISTINCT FROM $2
           AND episode_id IS NOT DISTINCT FROM $3 AND ended_at IS NULL
         RETURNING id",
    )
    .bind(profile_id)
    .bind(target.item_id())
    .bind(target.episode_id())
    .fetch_all(pool)
    .await
    .map_err(AppError::from)?;
    for (id,) in replaced {
        record_closed(pool, id).await;
    }
    sqlx::query(
        "INSERT INTO listening_sessions
             (id, profile_id, item_id, start_position_ms, position_ms, duration_ms,
              client_name, client_version, device_name, episode_id)
         VALUES ($1, $2, $3, $4, $4, $5, $6, $7, $8, $9)",
    )
    .bind(session_id)
    .bind(profile_id)
    .bind(target.item_id())
    .bind(position_ms.max(0))
    .bind(duration_ms.filter(|d| *d > 0))
    .bind(&client.name)
    .bind(&client.version)
    .bind(&client.device)
    .bind(target.episode_id())
    .execute(pool)
    .await
    .map_err(AppError::from)?;
    Ok(())
}

/// Apply one report to the profile's open session on the book or
/// episode: the
/// one named by `session_id`, or without one the most recently heard
/// from. `close` ends it as well (`stopped`). Returns the session, or
/// `None` when no open session matches (it was closed meanwhile, or
/// never opened).
pub async fn report(
    pool: &parlour_db::PgPool,
    profile_id: Uuid,
    target: Target,
    session_id: Option<Uuid>,
    beat: Beat,
    close: bool,
) -> Result<Option<OpenSession>, AppError> {
    let mut tx = pool.begin().await.map_err(AppError::from)?;
    let current: Option<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT id, position_ms,
                GREATEST((EXTRACT(EPOCH FROM (now() - last_seen_at)) * 1000)::bigint, 0)
         FROM listening_sessions
         WHERE profile_id = $1 AND item_id IS NOT DISTINCT FROM $2
           AND episode_id IS NOT DISTINCT FROM $4 AND ended_at IS NULL
           AND ($3::uuid IS NULL OR id = $3)
         ORDER BY last_seen_at DESC, id
         LIMIT 1
         FOR UPDATE",
    )
    .bind(profile_id)
    .bind(target.item_id())
    .bind(session_id)
    .bind(target.episode_id())
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::from)?;
    let Some((id, stored_position, wall_ms)) = current else {
        return Ok(None);
    };
    let position = beat.position_ms.max(0);
    let add = listened_increment(beat.listened_ms, position - stored_position, wall_ms);
    let row = sqlx::query_as::<_, OpenSession>(
        "UPDATE listening_sessions SET
             position_ms = $2,
             duration_ms = COALESCE($3, duration_ms),
             listened_ms = listened_ms + $4,
             last_seen_at = now(),
             ended_at = CASE WHEN $5 THEN now() END,
             end_reason = CASE WHEN $5 THEN 'stopped' END
         WHERE id = $1
         RETURNING id, started_at, position_ms, duration_ms, client_name, client_version,
                   device_name",
    )
    .bind(id)
    .bind(position)
    .bind(beat.duration_ms.filter(|d| *d > 0))
    .bind(add)
    .bind(close)
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
    if close {
        record_closed(pool, id).await;
    }
    Ok(Some(row))
}

/// An admin stopped the session from Now Playing: close it where it
/// is (`admin`). Returns whether an open session was closed.
pub async fn close_by_admin(pool: &parlour_db::PgPool, session_id: Uuid) -> bool {
    let closed: Result<Option<(Uuid,)>, _> = sqlx::query_as(
        "UPDATE listening_sessions SET ended_at = now(), end_reason = 'admin'
         WHERE id = $1 AND ended_at IS NULL
         RETURNING id",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await;
    match closed {
        Ok(Some((id,))) => {
            record_closed(pool, id).await;
            true
        }
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(error = %e, %session_id, "could not close a listening session");
            false
        }
    }
}

/// Close every session silent for [`STALE_AFTER_SECS`], at its last
/// report (`timeout`): a player that went away without its stop
/// report. Run by the server's periodic tick. Returns how many closed.
pub async fn close_stale(pool: &parlour_db::PgPool) -> u64 {
    let closed: Result<Vec<(Uuid,)>, _> = sqlx::query_as(
        "UPDATE listening_sessions SET ended_at = last_seen_at, end_reason = 'timeout'
         WHERE ended_at IS NULL AND last_seen_at < now() - make_interval(secs => $1)
         RETURNING id",
    )
    .bind(STALE_AFTER_SECS as f64)
    .fetch_all(pool)
    .await;
    match closed {
        Ok(rows) => {
            for (id,) in &rows {
                record_closed(pool, *id).await;
            }
            rows.len() as u64
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not close stale listening sessions");
            0
        }
    }
}

/// The Activity line for a closed session (D29: closed sessions feed
/// the Activity feed). Never fails the caller.
async fn record_closed(pool: &parlour_db::PgPool, session_id: Uuid) {
    #[derive(FromRow)]
    struct Closed {
        profile_id: Uuid,
        item_id: Option<Uuid>,
        episode_id: Option<Uuid>,
        podcast_id: Option<Uuid>,
        user_id: Uuid,
        username: String,
        start_position_ms: i64,
        position_ms: i64,
        duration_ms: Option<i64>,
        listened_ms: i64,
        started_at: DateTime<Utc>,
        ended_at: Option<DateTime<Utc>>,
        end_reason: Option<String>,
    }
    let row = sqlx::query_as::<_, Closed>(
        "SELECT s.profile_id, s.item_id, s.episode_id,
                (SELECT e.podcast_id FROM podcast_episodes e WHERE e.id = s.episode_id)
                    AS podcast_id,
                u.id AS user_id, u.username, s.start_position_ms,
                s.position_ms, s.duration_ms, s.listened_ms, s.started_at, s.ended_at,
                s.end_reason
         FROM listening_sessions s
         JOIN profiles p ON p.id = s.profile_id
         JOIN users u ON u.id = p.user_id
         WHERE s.id = $1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await;
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(error = %e, %session_id, "could not read a closed listening session");
            return;
        }
    };
    let title = match (row.item_id, row.episode_id) {
        (Some(item_id), _) => crate::activity::item_label(pool, item_id).await,
        (None, Some(episode_id)) => episode_label(pool, episode_id).await,
        (None, None) => "something".to_string(),
    };
    let mut entry = crate::activity::Entry::by_user(
        "playback.listened",
        row.user_id,
        &row.username,
        format!(
            "{} listened to {title} for {}",
            row.username,
            listened_label(row.listened_ms)
        ),
    )
    .profile(row.profile_id);
    // An episode isn't a catalog item, so the line names it in its
    // detail instead (Activity's item link is to `media_items`).
    if let Some(item_id) = row.item_id {
        entry = entry.item(item_id);
    }
    crate::activity::record(
        pool,
        entry.detail(serde_json::json!({
            "session_id": session_id,
            "episode_id": row.episode_id,
            "podcast_id": row.podcast_id,
            "started_at": row.started_at,
            "ended_at": row.ended_at,
            "end_reason": row.end_reason,
            "start_position_ms": row.start_position_ms,
            "position_ms": row.position_ms,
            "duration_ms": row.duration_ms,
            "listened_ms": row.listened_ms,
        })),
    )
    .await;
}

/// How an episode reads in an Activity line or on Now Playing: "Show ·
/// Episode". Never fails the caller.
pub async fn episode_label(pool: &parlour_db::PgPool, episode_id: Uuid) -> String {
    let row: Result<Option<(String, String)>, _> = sqlx::query_as(
        "SELECT p.title, e.title FROM podcast_episodes e
         JOIN podcasts p ON p.id = e.podcast_id WHERE e.id = $1",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(Some((show, episode))) => format!("{show} · {episode}"),
        _ => "an episode".to_string(),
    }
}

// ------------------------------------------------------------- reading

#[derive(Debug, Serialize, FromRow)]
struct SessionDto {
    id: Uuid,
    profile_id: Uuid,
    username: String,
    profile_name: String,
    /// The book, or null for an episode's session (POD5).
    item_id: Option<Uuid>,
    /// POD5: the episode and its show, for an episode's session.
    episode_id: Option<Uuid>,
    podcast_id: Option<Uuid>,
    show_title: Option<String>,
    title: String,
    has_cover: bool,
    authors: Vec<String>,
    started_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    end_reason: Option<String>,
    start_position_ms: i64,
    position_ms: i64,
    duration_ms: Option<i64>,
    listened_ms: i64,
    client_name: Option<String>,
    client_version: Option<String>,
    device_name: Option<String>,
    /// Open and reporting within the last [`LIVE_SECS`] seconds.
    live: bool,
}

#[derive(Serialize)]
struct SessionPage {
    sessions: Vec<SessionDto>,
    /// Pass as `before` for the next page; null on the last page.
    next_before: Option<DateTime<Utc>>,
}

/// `$1` profile or NULL (every profile), `$2` `before` or NULL, `$3`
/// limit, `$4` whether to keep only books and episodes the profile `$1`
/// can reach (the profile's own view; an admin's view shows
/// everything). An episode is reached through its show's library, and
/// an explicit show only by a profile allowed explicit content (D41).
const SESSIONS_SQL: &str = "
    SELECT s.id, s.profile_id, u.username, p.name AS profile_name, s.item_id,
           s.episode_id, e.podcast_id, pc.title AS show_title,
           COALESCE(i.title, e.title) AS title,
           EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                   AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_cover,
           COALESCE((SELECT array_agg(pp.name ORDER BY ip.sort_order, pp.name)
                     FROM item_people ip JOIN people pp ON pp.id = ip.person_id
                     WHERE ip.item_id = i.id AND ip.role = 'author'), '{}') AS authors,
           s.started_at, s.last_seen_at, s.ended_at, s.end_reason, s.start_position_ms,
           s.position_ms, s.duration_ms, s.listened_ms, s.client_name, s.client_version,
           s.device_name,
           (s.ended_at IS NULL AND s.last_seen_at > now() - make_interval(secs => $5)) AS live
    FROM listening_sessions s
    JOIN profiles p ON p.id = s.profile_id
    JOIN users u ON u.id = p.user_id
    LEFT JOIN media_items i ON i.id = s.item_id
    LEFT JOIN podcast_episodes e ON e.id = s.episode_id
    LEFT JOIN podcasts pc ON pc.id = e.podcast_id
    WHERE ($1::uuid IS NULL OR s.profile_id = $1)
      AND ($2::timestamptz IS NULL OR s.started_at < $2)
      AND (NOT $4 OR (EXISTS (SELECT 1 FROM library_access a
                              WHERE a.library_id = COALESCE(i.library_id, pc.library_id)
                                AND a.profile_id = $1)
                      AND (pc.id IS NULL OR NOT pc.explicit OR p.allow_explicit)))
    ORDER BY s.started_at DESC, s.id
    LIMIT $3";

#[derive(Deserialize)]
struct PageQuery {
    limit: Option<i64>,
    before: Option<DateTime<Utc>>,
    profile_id: Option<Uuid>,
}

async fn page(
    app: &AppState,
    profile_id: Option<Uuid>,
    own: bool,
    q: &PageQuery,
) -> Result<SessionPage, AppError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let mut sessions = sqlx::query_as::<_, SessionDto>(SESSIONS_SQL)
        .bind(profile_id)
        .bind(q.before)
        .bind(limit + 1)
        .bind(own)
        .bind(LIVE_SECS as f64)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    let more = sessions.len() as i64 > limit;
    sessions.truncate(limit as usize);
    let next_before = if more {
        sessions.last().map(|s| s.started_at)
    } else {
        None
    };
    Ok(SessionPage {
        sessions,
        next_before,
    })
}

async fn my_sessions(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<PageQuery>,
) -> Result<Json<SessionPage>, AppError> {
    let profile_id = crate::audiobooks::active_profile(&user)?;
    Ok(Json(page(&app, Some(profile_id), true, &q).await?))
}

async fn all_sessions(
    State(app): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<PageQuery>,
) -> Result<Json<SessionPage>, AppError> {
    Ok(Json(page(&app, q.profile_id, false, &q).await?))
}

#[derive(Deserialize)]
struct StatsQuery {
    /// IANA time zone the days are counted in (the listener's); UTC
    /// when absent.
    tz: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
struct DayDto {
    /// `YYYY-MM-DD` in the requested time zone.
    date: String,
    listened_ms: i64,
}

#[derive(Debug, Serialize, FromRow)]
struct BookTotalDto {
    item_id: Uuid,
    title: String,
    has_cover: bool,
    listened_ms: i64,
    sessions: i64,
    last_listened_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct StatsDto {
    time_zone: String,
    total_ms: i64,
    sessions: i64,
    days_listened: i64,
    today_ms: i64,
    /// Days with listening, newest first.
    days: Vec<DayDto>,
    /// Books by listening time, most first. Episodes count in the
    /// totals and days above, not here (POD5).
    books: Vec<BookTotalDto>,
}

/// This profile's listening stats, from its sessions. A session counts
/// on the day it started, in the listener's time zone.
async fn my_stats(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<StatsQuery>,
) -> Result<Json<StatsDto>, AppError> {
    let profile_id = crate::audiobooks::active_profile(&user)?;
    let tz =
        q.tz.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("UTC")
            .to_string();
    let known: Option<(bool,)> =
        sqlx::query_as("SELECT true FROM pg_timezone_names WHERE name = $1 LIMIT 1")
            .bind(&tz)
            .fetch_optional(&app.pool)
            .await
            .map_err(AppError::from)?;
    if known.is_none() {
        return Err(AppError::BadRequest(format!("unknown time zone '{tz}'")));
    }
    let (total_ms, sessions, days_listened, today_ms): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(s.listened_ms), 0)::bigint,
                COUNT(*),
                COUNT(DISTINCT (s.started_at AT TIME ZONE $2)::date)
                    FILTER (WHERE s.listened_ms > 0),
                COALESCE(SUM(s.listened_ms) FILTER (
                    WHERE (s.started_at AT TIME ZONE $2)::date = (now() AT TIME ZONE $2)::date
                ), 0)::bigint
         FROM listening_sessions s
         JOIN profiles pr ON pr.id = s.profile_id
         LEFT JOIN media_items i ON i.id = s.item_id
         LEFT JOIN podcast_episodes e ON e.id = s.episode_id
         LEFT JOIN podcasts pc ON pc.id = e.podcast_id
         JOIN library_access a ON a.library_id = COALESCE(i.library_id, pc.library_id)
                              AND a.profile_id = $1
         WHERE s.profile_id = $1 AND (pc.id IS NULL OR NOT pc.explicit OR pr.allow_explicit)",
    )
    .bind(profile_id)
    .bind(&tz)
    .fetch_one(&app.pool)
    .await
    .map_err(AppError::from)?;
    let days = sqlx::query_as::<_, DayDto>(
        "SELECT to_char((s.started_at AT TIME ZONE $2)::date, 'YYYY-MM-DD') AS date,
                SUM(s.listened_ms)::bigint AS listened_ms
         FROM listening_sessions s
         JOIN profiles pr ON pr.id = s.profile_id
         LEFT JOIN media_items i ON i.id = s.item_id
         LEFT JOIN podcast_episodes e ON e.id = s.episode_id
         LEFT JOIN podcasts pc ON pc.id = e.podcast_id
         JOIN library_access a ON a.library_id = COALESCE(i.library_id, pc.library_id)
                              AND a.profile_id = $1
         WHERE s.profile_id = $1 AND s.listened_ms > 0
           AND (pc.id IS NULL OR NOT pc.explicit OR pr.allow_explicit)
         GROUP BY 1
         ORDER BY 1 DESC",
    )
    .bind(profile_id)
    .bind(&tz)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    let books = sqlx::query_as::<_, BookTotalDto>(
        "SELECT i.id AS item_id, i.title,
                EXISTS (SELECT 1 FROM images im WHERE im.item_id = i.id AND im.kind = 'poster'
                        AND im.path <> '' AND im.path NOT LIKE 'pending:%') AS has_cover,
                SUM(s.listened_ms)::bigint AS listened_ms,
                COUNT(*) AS sessions,
                MAX(s.last_seen_at) AS last_listened_at
         FROM listening_sessions s
         JOIN media_items i ON i.id = s.item_id
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE s.profile_id = $1
         GROUP BY i.id
         ORDER BY 4 DESC, i.sort_title, i.id",
    )
    .bind(profile_id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(StatsDto {
        time_zone: tz,
        total_ms,
        sessions,
        days_listened,
        today_ms,
        days,
        books,
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/listening/sessions", get(my_sessions))
        .route("/api/v1/listening/stats", get(my_stats))
        .route("/api/v1/admin/listening/sessions", get(all_sessions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_time_is_the_players_word_capped_by_the_clock() {
        // The player's own measure wins when it sends one...
        assert_eq!(listened_increment(Some(15_000), 40_000, 15_200), 15_000);
        // ...but never more than the time since the last report (+5 s).
        assert_eq!(listened_increment(Some(600_000), 0, 15_000), 20_000);
        // A paused player that reports 0 adds nothing, whatever moved.
        assert_eq!(listened_increment(Some(0), 30_000, 30_000), 0);
        // Never negative.
        assert_eq!(listened_increment(Some(-5), 0, 10_000), 0);
        assert_eq!(listened_increment(None, -60_000, 10_000), 0);
        // Without a measure: how far the position moved forward.
        assert_eq!(listened_increment(None, 12_000, 15_000), 12_000);
        // A seek forward is not listening beyond the clock.
        assert_eq!(listened_increment(None, 3_600_000, 15_000), 20_000);
        // A clock that reads backwards counts as no time.
        assert_eq!(listened_increment(Some(9_000), 0, -1_000), 5_000);
    }

    #[test]
    fn listening_time_reads_as_a_phrase() {
        assert_eq!(listened_label(0), "0 s");
        assert_eq!(listened_label(45_900), "45 s");
        assert_eq!(listened_label(60_000), "1 min");
        assert_eq!(listened_label(18 * 60_000 + 59_000), "18 min");
        assert_eq!(listened_label(2 * 3_600_000), "2 h");
        assert_eq!(listened_label(2 * 3_600_000 + 5 * 60_000), "2 h 5 min");
        assert_eq!(listened_label(-10), "0 s");
    }
}
