//! POD8 (Phase 11; D35): a profile's bookmarks in a book. Spec:
//! `docs/AUDIOBOOKS-PODCASTS.md` → "Playlists, collections and bookmarks
//! (POD8)".
//!
//! - `GET /api/v1/audiobooks/{id}/bookmarks` — this profile's bookmarks
//!   in the book, by time
//! - `POST /api/v1/audiobooks/{id}/bookmarks` `{position_ms, title?}` —
//!   a bookmark at a book-global time (clamped onto the book). As in
//!   Audiobookshelf a book has one bookmark at a given time: adding at a
//!   time that already has one answers that bookmark (200, not 201),
//!   retitled when a title is sent
//! - `PATCH /api/v1/audiobooks/{id}/bookmarks/{bookmark}` `{title}` —
//!   rename it; its time stays (ABS's key)
//! - `DELETE /api/v1/audiobooks/{id}/bookmarks/{bookmark}`
//!
//! Books only: Audiobookshelf has no episode bookmarks (D35). A profile
//! reaches a book's bookmarks only through its grant on the book's
//! library (404 otherwise, as the book); they are kept while the grant
//! is away, and go with the book or the profile.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, patch};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::audiobooks::{active_profile, clamp_position};
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::http::AppState;
use crate::playlists::{parse_id, MAX_NAME_CHARS};

#[derive(Debug, Serialize, FromRow)]
pub struct BookmarkDto {
    pub id: Uuid,
    pub item_id: Uuid,
    /// Book-global, as progress is (ms from the start of track 0).
    pub position_ms: i64,
    /// May be empty: the player then shows the time.
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A title as stored: trimmed, at most [`MAX_NAME_CHARS`]; empty is fine.
pub fn clean_title(raw: Option<&str>) -> Result<String, AppError> {
    let title = raw.unwrap_or("").trim();
    if title.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::BadRequest(format!(
            "a bookmark's title is at most {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(title.to_string())
}

/// The book's length, when `profile_id` can reach the book (its grant
/// on the book's library); 404 otherwise.
async fn reachable_book(
    app: &AppState,
    profile_id: Uuid,
    id: &str,
) -> Result<(Uuid, Option<i64>), AppError> {
    let book_id = parse_id(id, "book")?;
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT COALESCE((SELECT sum(f.duration_ms) FROM media_files f
                          WHERE f.item_id = i.id AND f.available), 0)::bigint
         FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $1
         WHERE i.id = $2 AND i.kind = 'audiobook'",
    )
    .bind(profile_id)
    .bind(book_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    let (length,) = row.ok_or_else(|| AppError::NotFound("no such book".into()))?;
    Ok((book_id, Some(length).filter(|l| *l > 0)))
}

async fn list(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<BookmarkDto>>, AppError> {
    let profile_id = active_profile(&user)?;
    let (book_id, _) = reachable_book(&app, profile_id, &id).await?;
    let rows = sqlx::query_as::<_, BookmarkDto>(
        "SELECT id, item_id, position_ms, title, created_at, updated_at FROM bookmarks
         WHERE profile_id = $1 AND item_id = $2 ORDER BY position_ms, created_at",
    )
    .bind(profile_id)
    .bind(book_id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    position_ms: i64,
    #[serde(default)]
    title: Option<String>,
}

#[derive(FromRow)]
struct Upserted {
    #[sqlx(flatten)]
    bookmark: BookmarkDto,
    inserted: bool,
}

async fn create(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<CreateBody>,
) -> Result<impl IntoResponse, AppError> {
    let profile_id = active_profile(&user)?;
    let (book_id, length) = reachable_book(&app, profile_id, &id).await?;
    if body.position_ms < 0 {
        return Err(AppError::BadRequest("position_ms can't be negative".into()));
    }
    let title = clean_title(body.title.as_deref())?;
    let position = clamp_position(body.position_ms, length);
    // One bookmark per time (ABS): a second add at the same time answers
    // the first, retitled only when a title was sent.
    let row = sqlx::query_as::<_, Upserted>(
        "INSERT INTO bookmarks (profile_id, item_id, position_ms, title)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (profile_id, item_id, position_ms) DO UPDATE
             SET title = CASE WHEN EXCLUDED.title <> '' THEN EXCLUDED.title
                              ELSE bookmarks.title END,
                 updated_at = CASE WHEN EXCLUDED.title <> ''
                                    AND EXCLUDED.title <> bookmarks.title
                                   THEN now() ELSE bookmarks.updated_at END
         RETURNING id, item_id, position_ms, title, created_at, updated_at,
                   (xmax = 0) AS inserted",
    )
    .bind(profile_id)
    .bind(book_id)
    .bind(position)
    .bind(&title)
    .fetch_one(&app.pool)
    .await
    .map_err(AppError::from)?;
    let status = if row.inserted {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(row.bookmark)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBody {
    title: String,
}

async fn rename(
    State(app): State<AppState>,
    user: AuthUser,
    Path((id, bookmark)): Path<(String, String)>,
    Json(body): Json<RenameBody>,
) -> Result<Json<BookmarkDto>, AppError> {
    let profile_id = active_profile(&user)?;
    let (book_id, _) = reachable_book(&app, profile_id, &id).await?;
    let bookmark_id = parse_id(&bookmark, "bookmark")?;
    let title = clean_title(Some(&body.title))?;
    sqlx::query_as::<_, BookmarkDto>(
        "UPDATE bookmarks SET title = $4, updated_at = now()
         WHERE id = $3 AND profile_id = $1 AND item_id = $2
         RETURNING id, item_id, position_ms, title, created_at, updated_at",
    )
    .bind(profile_id)
    .bind(book_id)
    .bind(bookmark_id)
    .bind(&title)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?
    .map(Json)
    .ok_or_else(|| AppError::NotFound("no such bookmark".into()))
}

async fn remove(
    State(app): State<AppState>,
    user: AuthUser,
    Path((id, bookmark)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let profile_id = active_profile(&user)?;
    let (book_id, _) = reachable_book(&app, profile_id, &id).await?;
    let bookmark_id = parse_id(&bookmark, "bookmark")?;
    let gone =
        sqlx::query("DELETE FROM bookmarks WHERE id = $3 AND profile_id = $1 AND item_id = $2")
            .bind(profile_id)
            .bind(book_id)
            .bind(bookmark_id)
            .execute(&app.pool)
            .await
            .map_err(AppError::from)?;
    if gone.rows_affected() == 0 {
        return Err(AppError::NotFound("no such bookmark".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/audiobooks/{id}/bookmarks", get(list).post(create))
        .route(
            "/api/v1/audiobooks/{id}/bookmarks/{bookmark}",
            patch(rename).delete(remove),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_trimmed_and_may_be_empty() {
        assert_eq!(clean_title(None).unwrap(), "");
        assert_eq!(clean_title(Some("  The twist ")).unwrap(), "The twist");
        assert!(clean_title(Some(&"x".repeat(MAX_NAME_CHARS + 1))).is_err());
    }
}
