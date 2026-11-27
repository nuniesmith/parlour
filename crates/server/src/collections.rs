//! POD8 (Phase 11; D35): collections. Spec:
//! `docs/AUDIOBOOKS-PODCASTS.md` → "Playlists, collections and bookmarks
//! (POD8)".
//!
//! Listeners:
//! - `GET /api/v1/collections?library_id=&item_id=` — the collections of
//!   the libraries this profile can reach, by name, each with its book
//!   count and a few covers; with a book named, whether each holds it
//! - `GET /api/v1/collections/{id}` — one collection with its books in
//!   order, as the book list shows them (with this profile's progress)
//!
//! Admins (404 to anyone else, as every `/api/v1/admin/` route):
//! - `POST /api/v1/admin/collections` `{library_id, name, description?}`
//! - `PATCH|DELETE /api/v1/admin/collections/{id}`
//! - `POST /api/v1/admin/collections/{id}/items` `{item_id}` — append a
//!   book of the collection's library
//! - `PUT /api/v1/admin/collections/{id}/items` `{items: [book ids]}` —
//!   the new order, naming every book once
//! - `DELETE /api/v1/admin/collections/{id}/items/{item_id}`
//!
//! **Whose.** A collection belongs to its library and is seen by every
//! profile with a grant on that library (D35); there is no per-profile
//! copy. As in Audiobookshelf a collection holds books only, so it lives
//! in an audiobooks library, and changing one takes ABS's "update"
//! permission, which is an admin's (Parlour has one admin role, so
//! deleting one is an admin's too). An empty collection stays.

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::audiobooks::{active_profile, BookDto};
use crate::auth::{AdminUser, AuthUser};
use crate::error::AppError;
use crate::http::AppState;
use crate::playlists::{
    books_by_ids, clean_description, clean_name, cover_urls, parse_id, reorder_slots, CoverRef,
    PatchBody, ReorderBody,
};

#[derive(FromRow)]
struct CollectionRow {
    id: Uuid,
    library_id: Uuid,
    library_name: String,
    name: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    book_count: i64,
    contains: bool,
    covers: sqlx::types::Json<Vec<CoverRef>>,
}

#[derive(Debug, Serialize)]
pub struct CollectionDto {
    pub id: Uuid,
    pub library_id: Uuid,
    pub library_name: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub book_count: i64,
    /// With `?item_id=`: whether the book is in this collection.
    pub contains: bool,
    /// Up to four of its first books' covers.
    pub covers: Vec<String>,
}

impl From<CollectionRow> for CollectionDto {
    fn from(r: CollectionRow) -> Self {
        CollectionDto {
            covers: cover_urls(&r.covers.0),
            id: r.id,
            library_id: r.library_id,
            library_name: r.library_name,
            name: r.name,
            description: r.description,
            created_at: r.created_at,
            updated_at: r.updated_at,
            book_count: r.book_count,
            contains: r.contains,
        }
    }
}

/// `$1` profile, `$2` collection id or NULL, `$3` library id or NULL,
/// `$4` a book id (for `contains`) or NULL. Only collections of libraries
/// the profile has a grant on (every book there is then within reach).
const COLLECTIONS_SQL: &str = "
    SELECT c.id, c.library_id, l.name AS library_name, c.name, c.description,
           c.created_at, c.updated_at,
           (SELECT count(*) FROM collection_items ci
            JOIN media_items i ON i.id = ci.item_id AND i.kind = 'audiobook'
            WHERE ci.collection_id = c.id) AS book_count,
           ($4::uuid IS NOT NULL AND EXISTS (
               SELECT 1 FROM collection_items ci
               WHERE ci.collection_id = c.id AND ci.item_id = $4)) AS contains,
           COALESCE((SELECT json_agg(json_build_object('item_id', f.item_id,
                                                       'has_cover', f.has_cover)
                                     ORDER BY f.position)
                     FROM (SELECT ci.item_id, ci.position,
                                  EXISTS (SELECT 1 FROM images im
                                          WHERE im.item_id = ci.item_id
                                            AND im.kind = 'poster' AND im.path <> ''
                                            AND im.path NOT LIKE 'pending:%') AS has_cover
                           FROM collection_items ci WHERE ci.collection_id = c.id
                           ORDER BY ci.position LIMIT 12) f), '[]'::json) AS covers
    FROM collections c
    JOIN libraries l ON l.id = c.library_id
    JOIN library_access a ON a.library_id = c.library_id AND a.profile_id = $1
    WHERE ($2::uuid IS NULL OR c.id = $2)
      AND ($3::uuid IS NULL OR c.library_id = $3)
    ORDER BY lower(c.name), c.created_at, c.id";

fn not_found() -> AppError {
    AppError::NotFound("no such collection".into())
}

async fn load_summary(
    app: &AppState,
    profile_id: Uuid,
    collection_id: Uuid,
) -> Result<CollectionDto, AppError> {
    sqlx::query_as::<_, CollectionRow>(COLLECTIONS_SQL)
        .bind(profile_id)
        .bind(Some(collection_id))
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?
        .map(CollectionDto::from)
        .ok_or_else(not_found)
}

#[derive(Deserialize)]
struct ListQuery {
    library_id: Option<String>,
    item_id: Option<String>,
}

fn optional_id(raw: Option<&str>, what: &str) -> Result<Option<Uuid>, AppError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => Uuid::parse_str(s)
            .map(Some)
            .map_err(|_| AppError::BadRequest(format!("bad {what}"))),
        None => Ok(None),
    }
}

async fn list_collections(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<CollectionDto>>, AppError> {
    let profile_id = active_profile(&user)?;
    let rows = sqlx::query_as::<_, CollectionRow>(COLLECTIONS_SQL)
        .bind(profile_id)
        .bind(None::<Uuid>)
        .bind(optional_id(q.library_id.as_deref(), "library_id")?)
        .bind(optional_id(q.item_id.as_deref(), "item_id")?)
        .fetch_all(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(Json(rows.into_iter().map(CollectionDto::from).collect()))
}

#[derive(Serialize)]
struct CollectionDetailDto {
    #[serde(flatten)]
    collection: CollectionDto,
    books: Vec<BookDto>,
}

async fn book_ids(app: &AppState, collection_id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT item_id FROM collection_items WHERE collection_id = $1
         ORDER BY position, added_at, item_id",
    )
    .bind(collection_id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

async fn detail(
    app: &AppState,
    profile_id: Uuid,
    collection_id: Uuid,
) -> Result<CollectionDetailDto, AppError> {
    let collection = load_summary(app, profile_id, collection_id).await?;
    let ids = book_ids(app, collection_id).await?;
    let mut found = books_by_ids(app, profile_id, ids.clone()).await?;
    let books = ids.into_iter().filter_map(|id| found.remove(&id)).collect();
    Ok(CollectionDetailDto { collection, books })
}

async fn get_collection(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<CollectionDetailDto>, AppError> {
    let profile_id = active_profile(&user)?;
    Ok(Json(
        detail(&app, profile_id, parse_id(&id, "collection")?).await?,
    ))
}

// ------------------------------------------------------------------ admin

/// The collection, for an admin: it must exist (any library), and the
/// answer is read through the admin's own profile when it has one.
async fn admin_detail(
    app: &AppState,
    admin: &AuthUser,
    collection_id: Uuid,
) -> Result<serde_json::Value, AppError> {
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM collections WHERE id = $1")
        .bind(collection_id)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?;
    exists.ok_or_else(not_found)?;
    match admin.active_profile_id {
        Some(profile_id) => match detail(app, profile_id, collection_id).await {
            Ok(d) => serde_json::to_value(d).map_err(|e| AppError::Internal(e.to_string())),
            // The admin's own profile has no grant there: say only what changed.
            Err(AppError::NotFound(_)) => Ok(serde_json::json!({ "id": collection_id })),
            Err(e) => Err(e),
        },
        None => Ok(serde_json::json!({ "id": collection_id })),
    }
}

async fn require_collection(app: &AppState, id: &str) -> Result<(Uuid, Uuid), AppError> {
    let collection_id = parse_id(id, "collection")?;
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT library_id FROM collections WHERE id = $1")
        .bind(collection_id)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?;
    let (library_id,) = row.ok_or_else(not_found)?;
    Ok((collection_id, library_id))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    library_id: Uuid,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

async fn create_collection(
    State(app): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(body): Json<CreateBody>,
) -> Result<impl IntoResponse, AppError> {
    let name = clean_name(&body.name)?;
    let description = clean_description(body.description.as_deref())?;
    let kind: Option<(String,)> = sqlx::query_as("SELECT kind FROM libraries WHERE id = $1")
        .bind(body.library_id)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?;
    match kind {
        None => return Err(AppError::NotFound("no such library".into())),
        Some((k,)) if k != "audiobooks" => {
            return Err(AppError::BadRequest(
                "collections are for audiobooks libraries".into(),
            ))
        }
        Some(_) => {}
    }
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO collections (library_id, name, description) VALUES ($1, $2, $3)
         RETURNING id",
    )
    .bind(body.library_id)
    .bind(&name)
    .bind(&description)
    .fetch_one(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(admin_detail(&app, &admin, id).await?),
    ))
}

async fn patch_collection(
    State(app): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (collection_id, _) = require_collection(&app, &id).await?;
    if body.name.is_none() && body.description.is_none() {
        return Err(AppError::BadRequest("send name or description".into()));
    }
    let name = body.name.as_deref().map(clean_name).transpose()?;
    let description = clean_description(body.description.as_deref())?;
    sqlx::query(
        "UPDATE collections SET name = COALESCE($2, name),
                description = CASE WHEN $3 THEN $4 ELSE description END,
                updated_at = now()
         WHERE id = $1",
    )
    .bind(collection_id)
    .bind(name)
    .bind(body.description.is_some())
    .bind(description)
    .execute(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(admin_detail(&app, &admin, collection_id).await?))
}

async fn delete_collection(
    State(app): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let (collection_id, _) = require_collection(&app, &id).await?;
    sqlx::query("DELETE FROM collections WHERE id = $1")
        .bind(collection_id)
        .execute(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddBody {
    item_id: Uuid,
}

async fn add_book(
    State(app): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<String>,
    Json(body): Json<AddBody>,
) -> Result<impl IntoResponse, AppError> {
    let (collection_id, library_id) = require_collection(&app, &id).await?;
    let book: Option<(Uuid,)> =
        sqlx::query_as("SELECT library_id FROM media_items WHERE id = $1 AND kind = 'audiobook'")
            .bind(body.item_id)
            .fetch_optional(&app.pool)
            .await
            .map_err(AppError::from)?;
    match book {
        None => return Err(AppError::NotFound("no such book".into())),
        Some((lib,)) if lib != library_id => {
            return Err(AppError::BadRequest(
                "that book isn't in this collection's library".into(),
            ))
        }
        Some(_) => {}
    }
    let mut tx = app.pool.begin().await.map_err(AppError::from)?;
    sqlx::query("SELECT id FROM collections WHERE id = $1 FOR UPDATE")
        .bind(collection_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    let inserted = sqlx::query(
        "INSERT INTO collection_items (collection_id, item_id, position)
         SELECT $1, $2, COALESCE(max(position) + 1, 0)
         FROM collection_items WHERE collection_id = $1
         ON CONFLICT (collection_id, item_id) DO NOTHING",
    )
    .bind(collection_id)
    .bind(body.item_id)
    .execute(&mut *tx)
    .await
    .map_err(AppError::from)?;
    if inserted.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "that book is already in this collection".into(),
        ));
    }
    sqlx::query("UPDATE collections SET updated_at = now() WHERE id = $1")
        .bind(collection_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(admin_detail(&app, &admin, collection_id).await?),
    ))
}

async fn reorder_books(
    State(app): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<String>,
    Json(body): Json<ReorderBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (collection_id, _) = require_collection(&app, &id).await?;
    let mut tx = app.pool.begin().await.map_err(AppError::from)?;
    sqlx::query("SELECT id FROM collections WHERE id = $1 FOR UPDATE")
        .bind(collection_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    let current: Vec<(Uuid, i32)> = sqlx::query_as(
        "SELECT item_id, position FROM collection_items WHERE collection_id = $1
         ORDER BY position, added_at, item_id",
    )
    .bind(collection_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(AppError::from)?;
    let all: HashSet<Uuid> = current.iter().map(|(id, _)| *id).collect();
    let Some(order) = reorder_slots(&current, &all, &body.items) else {
        return Err(AppError::Conflict(
            "the collection has changed since it was loaded: reload it and try again".into(),
        ));
    };
    let ids: Vec<Uuid> = order.iter().map(|(id, _)| *id).collect();
    let positions: Vec<i32> = order.iter().map(|(_, p)| *p).collect();
    sqlx::query(
        "UPDATE collection_items ci SET position = o.position
         FROM unnest($2::uuid[], $3::int[]) AS o(id, position)
         WHERE ci.item_id = o.id AND ci.collection_id = $1",
    )
    .bind(collection_id)
    .bind(&ids)
    .bind(&positions)
    .execute(&mut *tx)
    .await
    .map_err(AppError::from)?;
    sqlx::query("UPDATE collections SET updated_at = now() WHERE id = $1")
        .bind(collection_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
    Ok(Json(admin_detail(&app, &admin, collection_id).await?))
}

async fn remove_book(
    State(app): State<AppState>,
    _admin: AdminUser,
    Path((id, item)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let (collection_id, _) = require_collection(&app, &id).await?;
    let item_id = parse_id(&item, "book")?;
    let gone =
        sqlx::query("DELETE FROM collection_items WHERE collection_id = $1 AND item_id = $2")
            .bind(collection_id)
            .bind(item_id)
            .execute(&app.pool)
            .await
            .map_err(AppError::from)?;
    if gone.rows_affected() == 0 {
        return Err(AppError::NotFound(
            "that book isn't in this collection".into(),
        ));
    }
    sqlx::query("UPDATE collections SET updated_at = now() WHERE id = $1")
        .bind(collection_id)
        .execute(&app.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/collections", get(list_collections))
        .route("/api/v1/collections/{id}", get(get_collection))
        .route("/api/v1/admin/collections", post(create_collection))
        .route(
            "/api/v1/admin/collections/{id}",
            axum::routing::patch(patch_collection).delete(delete_collection),
        )
        .route(
            "/api/v1/admin/collections/{id}/items",
            post(add_book).put(reorder_books),
        )
        .route(
            "/api/v1/admin/collections/{id}/items/{item}",
            delete(remove_book),
        )
}
