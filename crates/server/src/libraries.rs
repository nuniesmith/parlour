//! Libraries and A6 access control.
//!
//! The rule, enforced in SQL on every query: a library is visible
//! to a profile only through a `library_access` row. There is no
//! code path that reads a library by id without the join, so a
//! library a profile cannot access is a 404 — it does not exist
//! as far as that profile can tell, even if they guess the URL.
//! Library CRUD (name, kind, root paths) is completed in
//! Milestone 1.3 (C2); scanning endpoints live in `catalog`.
//!
//! Sidecar writing (XFER5, D14): `libraries.write_sidecars` is the
//! per-library opt-in that lets Parlour write next to the media
//! (`*.parlour.json`, `poster.jpg`, `backdrop.jpg`). It is off by
//! default and is set **only** by its own endpoint,
//! `PUT /api/v1/admin/libraries/{id}/write-sidecars` — creating or
//! editing a library refuses the field, and an import never sets it.
//! The admin view reports whether each root folder is writable, since
//! nothing is written until the flag is on *and* the mount allows it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
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
/// (C2 — library CRUD carries name, kind, and root paths), and its
/// sidecar opt-in with whether each root can be written (XFER5).
#[derive(Debug, Serialize)]
pub struct LibraryDetailDto {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub paths: Vec<String>,
    /// The sidecar-writing opt-in (D14, XFER5). Off by default.
    pub write_sidecars: bool,
    /// Whether Parlour could write in each root folder right now.
    pub folders: Vec<FolderAccess>,
}

/// Whether Parlour can write in one library root (XFER5).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FolderAccess {
    pub path: String,
    pub writable: bool,
    /// Why not, when it isn't (`Read-only file system`, …).
    pub problem: Option<String>,
}

/// Library kinds whose items get sidecars (and poster / backdrop
/// copies) today: video libraries. Audiobooks come with AB6 (D30).
pub const SIDECAR_KINDS: &[&str] = &["movies", "shows"];

/// Can this process write in `path`? Asks the kernel (`access(2)` with
/// `W_OK`), which answers for both the folder's permissions and a
/// read-only mount (`EROFS`), without writing anything.
#[cfg(unix)]
pub fn write_access(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err("not a folder".to_string()),
        Err(e) => return Err(e.to_string()),
    }
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "the path holds a NUL byte".to_string())?;
    // SAFETY: `c_path` is a valid NUL-terminated string; access(2)
    // only reads it.
    let rc = unsafe { libc::access(c_path.as_ptr(), libc::W_OK) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

#[cfg(not(unix))]
pub fn write_access(path: &std::path::Path) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() && !m.permissions().readonly() => Ok(()),
        Ok(_) => Err("not a writable folder".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn folder_access(path: &str) -> FolderAccess {
    let result = write_access(std::path::Path::new(path));
    FolderAccess {
        path: path.to_string(),
        writable: result.is_ok(),
        problem: result.err(),
    }
}

async fn library_detail(state: &AppState, id: Uuid) -> Result<LibraryDetailDto, AppError> {
    let row: (Uuid, String, String, DateTime<Utc>, bool) = sqlx::query_as(
        "SELECT id, name, kind, created_at, write_sidecars FROM libraries WHERE id = $1",
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
    let paths: Vec<String> = paths.into_iter().map(|(p,)| p).collect();
    Ok(LibraryDetailDto {
        id: row.0,
        name: row.1,
        kind: row.2,
        created_at: row.3,
        folders: paths.iter().map(|p| folder_access(p)).collect(),
        paths,
        write_sidecars: row.4,
    })
}

/// Creating or editing a library never touches the sidecar opt-in:
/// turning it on is its own deliberate step (D14, XFER5).
fn refuse_sidecar_flag(present: bool) -> Result<(), AppError> {
    if present {
        return Err(AppError::BadRequest(
            "write_sidecars is set on its own: PUT /api/v1/admin/libraries/{id}/write-sidecars"
                .into(),
        ));
    }
    Ok(())
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
    /// Refused when present (`refuse_sidecar_flag`).
    write_sidecars: Option<serde::de::IgnoredAny>,
}

async fn create_library(
    State(state): State<AppState>,
    _admin: AdminUser,
    Json(body): Json<CreateLibrary>,
) -> Result<(StatusCode, Json<LibraryDetailDto>), AppError> {
    refuse_sidecar_flag(body.write_sidecars.is_some())?;
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
    /// Refused when present (`refuse_sidecar_flag`).
    write_sidecars: Option<serde::de::IgnoredAny>,
}

async fn update_library(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateLibrary>,
) -> Result<Json<LibraryDetailDto>, AppError> {
    let id = parse_id(&id)?;
    refuse_sidecar_flag(body.write_sidecars.is_some())?;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetWriteSidecars {
    enabled: bool,
}

/// Turn a library's sidecar writing on or off (XFER5, D14). This is
/// the only way the flag changes. Turning it on lets the metadata job
/// write `*.parlour.json` and the artwork step copy `poster.jpg` /
/// `backdrop.jpg` next to the media — but only once the library's
/// folders are mounted writable, which is an ops step outside Parlour
/// (`docs/IMPORT-EXPORT.md` §13.11). It doesn't refuse a read-only
/// mount: the answer says which folders can't be written, writes there
/// fail cleanly onto the job, and a notice stays up until it's fixed.
async fn set_write_sidecars(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<SetWriteSidecars>,
) -> Result<Json<LibraryDetailDto>, AppError> {
    let id = parse_id(&id)?;
    let current = library_detail(&state, id).await?;
    if body.enabled && !SIDECAR_KINDS.contains(&current.kind.as_str()) {
        return Err(AppError::BadRequest(format!(
            "sidecars are written for movie and show libraries only; {} is a {} library",
            current.name, current.kind
        )));
    }
    if current.write_sidecars != body.enabled {
        sqlx::query("UPDATE libraries SET write_sidecars = $2 WHERE id = $1")
            .bind(id)
            .bind(body.enabled)
            .execute(&state.pool)
            .await
            .map_err(AppError::from)?;
        let (kind, summary) = if body.enabled {
            (
                "library.sidecars_on",
                format!(
                    "{} let Parlour write sidecars and artwork next to the media of {}",
                    admin.0.username, current.name
                ),
            )
        } else {
            (
                "library.sidecars_off",
                format!(
                    "{} stopped Parlour writing next to the media of {}",
                    admin.0.username, current.name
                ),
            )
        };
        crate::activity::record(
            &state.pool,
            crate::activity::Entry::by_user(kind, admin.0.user_id, &admin.0.username, summary)
                .detail(serde_json::json!({
                    "library_id": id,
                    "library": current.name,
                    "write_sidecars": body.enabled,
                })),
        )
        .await;
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
        .route(
            "/api/v1/admin/libraries/{id}/write-sidecars",
            put(set_write_sidecars),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_access_reports_folders_honestly() {
        let dir = std::env::temp_dir().join(format!("parlour-access-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let here = folder_access(&dir.to_string_lossy());
        assert!(here.writable, "{here:?}");
        assert_eq!(here.problem, None);

        // A missing folder (an unmounted disk) is not writable.
        let gone = folder_access(&dir.join("gone").to_string_lossy());
        assert!(!gone.writable);
        assert!(gone.problem.is_some());

        // A file is not a folder.
        std::fs::write(dir.join("file"), b"x").unwrap();
        assert!(!folder_access(&dir.join("file").to_string_lossy()).writable);

        // A read-only folder: what a `:ro` mount looks like to the
        // process (EROFS there, EACCES here — both say no). Root
        // ignores permissions, so there is nothing to check as root.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ro = dir.join("ro");
            std::fs::create_dir(&ro).unwrap();
            std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
            if std::fs::File::create(ro.join("probe")).is_err() {
                let access = folder_access(&ro.to_string_lossy());
                assert!(!access.writable, "{access:?}");
                assert!(access.problem.is_some());
            }
            std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_video_libraries_take_the_opt_in() {
        assert_eq!(SIDECAR_KINDS, &["movies", "shows"]);
        assert!(refuse_sidecar_flag(false).is_ok());
        assert!(refuse_sidecar_flag(true).is_err());
    }
}
