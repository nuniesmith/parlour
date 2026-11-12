//! Activity feed (task UI4): the server's audit trail.
//!
//! `record` appends one row; callers are request handlers and
//! background jobs, and a failed write must never fail what they
//! were doing — it is logged and dropped. `GET
//! /api/v1/admin/activity` pages the feed newest-first for the
//! Dashboard and the Activity page. Rows older than
//! [`RETENTION_DAYS`] are removed by the nightly `purge_activity`
//! job (see `catalog::spawn_background`).

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;

/// How long activity is kept.
pub const RETENTION_DAYS: i32 = 90;

/// One event to append. `kind` is a dotted name
/// (`auth.login`, `playback.start`, `scan.completed`, …); the
/// part before the dot is the category the UI groups by.
#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: &'static str,
    pub user_id: Option<Uuid>,
    pub profile_id: Option<Uuid>,
    pub item_id: Option<Uuid>,
    /// Username at the time, when a person did this.
    pub actor: Option<String>,
    /// One plain sentence, already complete.
    pub summary: String,
    pub detail: serde_json::Value,
}

impl Entry {
    /// An event with no person or item attached (jobs).
    pub fn system(kind: &'static str, summary: impl Into<String>) -> Self {
        Self {
            kind,
            user_id: None,
            profile_id: None,
            item_id: None,
            actor: None,
            summary: summary.into(),
            detail: serde_json::json!({}),
        }
    }

    /// An event a signed-in person caused.
    pub fn by_user(
        kind: &'static str,
        user_id: Uuid,
        username: &str,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            user_id: Some(user_id),
            actor: Some(username.to_string()),
            ..Self::system(kind, summary)
        }
    }

    pub fn detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = detail;
        self
    }

    pub fn profile(mut self, profile_id: Uuid) -> Self {
        self.profile_id = Some(profile_id);
        self
    }

    pub fn item(mut self, item_id: Uuid) -> Self {
        self.item_id = Some(item_id);
        self
    }
}

/// Append an event. Never fails the caller.
pub async fn record(pool: &parlour_db::PgPool, entry: Entry) {
    let result = sqlx::query(
        "INSERT INTO activity (kind, user_id, profile_id, item_id, actor, summary, detail)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(entry.kind)
    .bind(entry.user_id)
    .bind(entry.profile_id)
    .bind(entry.item_id)
    .bind(&entry.actor)
    .bind(&entry.summary)
    .bind(&entry.detail)
    .execute(pool)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, kind = entry.kind, "could not record activity");
    }
}

/// How an item reads in a sentence: the title, or for an episode
/// "Show S1:E2 · Episode title".
pub async fn item_label(pool: &parlour_db::PgPool, item_id: Uuid) -> String {
    let row: Result<Option<(String,)>, _> = sqlx::query_as(
        "SELECT CASE WHEN i.kind = 'episode' AND sr.title IS NOT NULL
                     THEN sr.title || ' S' || COALESCE(i.season_number::text, '?')
                          || ':E' || COALESCE(i.episode_number::text, '?')
                          || ' · ' || i.title
                     ELSE i.title END
         FROM media_items i
         LEFT JOIN media_items se ON se.id = i.parent_id AND i.kind = 'episode'
         LEFT JOIN media_items sr ON sr.id = se.parent_id
         WHERE i.id = $1",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(Some((label,))) => label,
        _ => "an item".to_string(),
    }
}

/// Delete activity older than `days`; returns how many rows went.
pub async fn purge_older_than(pool: &parlour_db::PgPool, days: i32) -> Result<u64, sqlx::Error> {
    let done =
        sqlx::query("DELETE FROM activity WHERE created_at < now() - make_interval(days => $1)")
            .bind(days)
            .execute(pool)
            .await?;
    Ok(done.rows_affected())
}

#[derive(Debug, Serialize, FromRow)]
struct ActivityDto {
    id: Uuid,
    created_at: DateTime<Utc>,
    kind: String,
    user_id: Option<Uuid>,
    profile_id: Option<Uuid>,
    item_id: Option<Uuid>,
    actor: Option<String>,
    summary: String,
    detail: serde_json::Value,
}

#[derive(Deserialize)]
struct ActivityQuery {
    limit: Option<i64>,
    /// Page cursor: only entries strictly older than this instant.
    before: Option<DateTime<Utc>>,
    /// Category filter: the part of `kind` before the dot.
    category: Option<String>,
}

#[derive(Serialize)]
struct ActivityPage {
    entries: Vec<ActivityDto>,
    /// Pass as `before` for the next page; null on the last page.
    next_before: Option<DateTime<Utc>>,
}

async fn list_activity(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<ActivityPage>, AppError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let category = q
        .category
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    // One extra row tells us whether another page exists.
    let mut entries = sqlx::query_as::<_, ActivityDto>(
        "SELECT id, created_at, kind, user_id, profile_id, item_id, actor, summary, detail
         FROM activity
         WHERE ($1::timestamptz IS NULL OR created_at < $1)
           AND ($2::text IS NULL OR split_part(kind, '.', 1) = $2)
         ORDER BY created_at DESC, id
         LIMIT $3",
    )
    .bind(q.before)
    .bind(category)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    let has_more = entries.len() as i64 > limit;
    entries.truncate(limit as usize);
    let next_before = if has_more {
        entries.last().map(|e| e.created_at)
    } else {
        None
    };
    Ok(Json(ActivityPage {
        entries,
        next_before,
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/admin/activity", get(list_activity))
}
