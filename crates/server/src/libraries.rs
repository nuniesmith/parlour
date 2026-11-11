//! Libraries and A6 access control.
//!
//! The rule, enforced in SQL on every query: a library is visible
//! to a profile only through a `library_access` row. There is no
//! code path that reads a library by id without the join, so a
//! library a profile cannot access is a 404 — it does not exist
//! as far as that profile can tell, even if they guess the URL.
//! Library CRUD (name, kind, root paths) is completed in
//! Milestone 1.3 (C2); scanning endpoints live in `catalog`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::AppError;
use crate::http::AppState;

#[derive(Debug, Serialize, FromRow)]
pub struct LibraryDto {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
}

fn parse_id(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound("no such library".into()))
}

/// Libraries the session's ACTIVE profile can access. Selecting a
/// profile is required — visibility is per profile by design.
async fn list_libraries(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<LibraryDto>>, AppError> {
    let Some(profile_id) = user.active_profile_id else {
        return Err(AppError::BadRequest("select a profile first".into()));
    };
    let rows = sqlx::query_as::<_, LibraryDto>(
        "SELECT l.id, l.name, l.kind, l.created_at FROM libraries l
         JOIN library_access a ON a.library_id = l.id AND a.profile_id = $1
         ORDER BY l.name",
    )
    .bind(profile_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(rows))
}

async fn get_library(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<LibraryDto>, AppError> {
    let Some(profile_id) = user.active_profile_id else {
        return Err(AppError::BadRequest("select a profile first".into()));
    };
    let id = parse_id(&id)?;
    let row = sqlx::query_as::<_, LibraryDto>(
        "SELECT l.id, l.name, l.kind, l.created_at FROM libraries l
         JOIN library_access a ON a.library_id = l.id AND a.profile_id = $2
         WHERE l.id = $1",
    )
    .bind(id)
    .bind(profile_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| AppError::NotFound("no such library".into()))?;
    Ok(Json(row))
}

/// Admin view of a library: the library row plus its root paths
/// (C2 — library CRUD carries name, kind, and root paths).
#[derive(Debug, Serialize)]
pub struct LibraryDetailDto {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub paths: Vec<String>,
}

async fn library_detail(state: &AppState, id: Uuid) -> Result<LibraryDetailDto, AppError> {
    let row = sqlx::query_as::<_, LibraryDto>(
        "SELECT id, name, kind, created_at FROM libraries WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| AppError::NotFound("no such library".into()))?;
    let paths: Vec<(String,)> =
        sqlx::query_as("SELECT path FROM library_paths WHERE library_id = $1 ORDER BY path")
            .bind(id)
            .fetch_all(&state.pool)
            .await
            .map_err(AppError::from)?;
    Ok(LibraryDetailDto {
        id: row.id,
        name: row.name,
        kind: row.kind,
        created_at: row.created_at,
        paths: paths.into_iter().map(|(p,)| p).collect(),
    })
}

fn validate_library(name: &str, kind: &str) -> Result<(), AppError> {
    if name.trim().is_empty() || name.len() > 80 {
        return Err(AppError::BadRequest(
            "library name must be 1-80 characters".into(),
        ));
    }
    if !matches!(
        kind,
        "movies" | "shows" | "audiobooks" | "ebooks" | "music" | "podcasts"
    ) {
        return Err(AppError::BadRequest(
            "kind must be movies, shows, audiobooks, ebooks, music or podcasts".into(),
        ));
    }
    Ok(())
}

fn validate_paths(paths: &[String]) -> Result<(), AppError> {
    for p in paths {
        if p.trim().is_empty() || !p.starts_with('/') {
            return Err(AppError::BadRequest(format!(
                "library paths must be absolute: {p:?}"
            )));
        }
    }
    Ok(())
}

async fn replace_paths(state: &AppState, id: Uuid, paths: &[String]) -> Result<(), AppError> {
    sqlx::query("DELETE FROM library_paths WHERE library_id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    for p in paths {
        sqlx::query("INSERT INTO library_paths (library_id, path) VALUES ($1, $2)")
            .bind(id)
            .bind(p.trim())
            .execute(&state.pool)
            .await
            .map_err(AppError::from)?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct CreateLibrary {
    name: String,
    kind: String,
    #[serde(default)]
    paths: Vec<String>,
}

async fn create_library(
    State(state): State<AppState>,
    _admin: AdminUser,
    Json(body): Json<CreateLibrary>,
) -> Result<(StatusCode, Json<LibraryDetailDto>), AppError> {
    validate_library(&body.name, &body.kind)?;
    validate_paths(&body.paths)?;
    let row = sqlx::query_as::<_, LibraryDto>(
        "INSERT INTO libraries (name, kind) VALUES ($1, $2) RETURNING id, name, kind, created_at",
    )
    .bind(body.name.trim())
    .bind(&body.kind)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("a library with that name already exists".into())
        }
        _ => AppError::from(e),
    })?;
    replace_paths(&state, row.id, &body.paths).await?;
    Ok((
        StatusCode::CREATED,
        Json(library_detail(&state, row.id).await?),
    ))
}

async fn get_library_admin(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Json<LibraryDetailDto>, AppError> {
    let id = parse_id(&id)?;
    Ok(Json(library_detail(&state, id).await?))
}

#[derive(Deserialize)]
struct UpdateLibrary {
    name: Option<String>,
    kind: Option<String>,
    paths: Option<Vec<String>>,
}

async fn update_library(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateLibrary>,
) -> Result<Json<LibraryDetailDto>, AppError> {
    let id = parse_id(&id)?;
    let current = library_detail(&state, id).await?;
    let name = body.name.as_deref().unwrap_or(&current.name);
    let kind = body.kind.as_deref().unwrap_or(&current.kind);
    validate_library(name, kind)?;
    if let Some(paths) = &body.paths {
        validate_paths(paths)?;
    }
    sqlx::query("UPDATE libraries SET name = $2, kind = $3 WHERE id = $1")
        .bind(id)
        .bind(name.trim())
        .bind(kind)
        .execute(&state.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                AppError::Conflict("a library with that name already exists".into())
            }
            _ => AppError::from(e),
        })?;
    if let Some(paths) = &body.paths {
        replace_paths(&state, id, paths).await?;
    }
    Ok(Json(library_detail(&state, id).await?))
}

async fn delete_library(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id = parse_id(&id)?;
    let done = sqlx::query("DELETE FROM libraries WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound("no such library".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_all_libraries(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<LibraryDto>>, AppError> {
    let rows = sqlx::query_as::<_, LibraryDto>(
        "SELECT id, name, kind, created_at FROM libraries ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct GrantAccess {
    profile_id: Uuid,
    max_rating: Option<String>,
}

/// Grant (or update) a profile's access to a library, with an
/// optional rating cap that item queries will enforce in 1.3+.
async fn grant_access(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<GrantAccess>,
) -> Result<StatusCode, AppError> {
    let id = parse_id(&id)?;
    let done = sqlx::query(
        "INSERT INTO library_access (profile_id, library_id, max_rating)
         SELECT p.id, l.id, $3 FROM profiles p, libraries l
         WHERE p.id = $1 AND l.id = $2
         ON CONFLICT (profile_id, library_id)
         DO UPDATE SET max_rating = EXCLUDED.max_rating",
    )
    .bind(body.profile_id)
    .bind(id)
    .bind(&body.max_rating)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound("no such profile or library".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/libraries", get(list_libraries))
        .route("/api/v1/libraries/{id}", get(get_library))
        .route(
            "/api/v1/admin/libraries",
            post(create_library).get(list_all_libraries),
        )
        .route(
            "/api/v1/admin/libraries/{id}",
            get(get_library_admin)
                .patch(update_library)
                .delete(delete_library),
        )
        .route("/api/v1/admin/libraries/{id}/access", post(grant_access))
}
