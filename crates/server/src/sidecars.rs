//! XFER8: per-library sidecar status for Server → Import/Export
//! (D14; spec `docs/IMPORT-EXPORT.md` §16).
//!
//! `GET /api/v1/admin/sidecars` summarises, per library, what the
//! sidecar machinery (XFER4–XFER6) already records — nothing is
//! counted twice and nothing on disk is walked:
//!
//! - the opt-in (`libraries.write_sidecars`, XFER5) and whether each
//!   root folder can be written right now (XFER5's `access(2)` check);
//! - whether the "can't write" notice is up (XFER5);
//! - the ledgers' row counts: `item_sidecars` (a sidecar the scanner
//!   read or the metadata job wrote, and how many an admin's fix-match
//!   overrode), `item_nfos` (NFOs read), `sidecar_nfos` (NFOs Parlour
//!   wrote) and `sidecar_artwork` (poster / backdrop copies Parlour
//!   wrote);
//! - how many available movies, series and episodes the library has,
//!   the items a sidecar can belong to.
//!
//! The switch itself stays on Server → Library, behind XFER5's confirm.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;
use crate::libraries::{folder_access, FolderAccess, SIDECAR_KINDS};

#[derive(Debug, sqlx::FromRow)]
struct Row {
    id: Uuid,
    name: String,
    kind: String,
    write_sidecars: bool,
    items: i64,
    sidecars: i64,
    sidecars_overridden: i64,
    nfos_read: i64,
    nfos_written: i64,
    artwork_written: i64,
    unwritable_notice: bool,
}

/// One library's sidecar status.
#[derive(Debug, Serialize)]
pub struct LibrarySidecars {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    /// Whether this kind of library gets sidecars at all (movies and
    /// shows; audiobooks wait for AB6, D30).
    pub supported: bool,
    /// The opt-in (XFER5). Off by default.
    pub write_sidecars: bool,
    /// Each root folder, and whether Parlour can write in it now.
    pub folders: Vec<FolderAccess>,
    /// Whether the "can't write beside the media" notice is up.
    pub unwritable_notice: bool,
    /// Available movies, series and episodes.
    pub items: i64,
    /// Items with a sidecar on record (read by the scanner or written
    /// by the metadata job): `item_sidecars`.
    pub sidecars: i64,
    /// Of those, overridden by an admin's fix-match until the file
    /// changes.
    pub sidecars_overridden: i64,
    /// Items whose `.nfo` the scanner has read: `item_nfos`.
    pub nfos_read: i64,
    /// `.nfo` files Parlour wrote beside the media: `sidecar_nfos`.
    pub nfos_written: i64,
    /// `poster.jpg` / `backdrop.jpg` copies Parlour wrote:
    /// `sidecar_artwork`.
    pub artwork_written: i64,
}

async fn list(
    State(app): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<LibrarySidecars>>, AppError> {
    let rows = sqlx::query_as::<_, Row>(
        "SELECT l.id, l.name, l.kind, l.write_sidecars,
                (SELECT count(*) FROM media_items i
                  WHERE i.library_id = l.id AND i.available
                    AND i.kind IN ('movie', 'series', 'episode')) AS items,
                (SELECT count(*) FROM item_sidecars s
                   JOIN media_items i ON i.id = s.item_id
                  WHERE i.library_id = l.id) AS sidecars,
                (SELECT count(*) FROM item_sidecars s
                   JOIN media_items i ON i.id = s.item_id
                  WHERE i.library_id = l.id AND s.overridden_at IS NOT NULL)
                    AS sidecars_overridden,
                (SELECT count(*) FROM item_nfos n
                   JOIN media_items i ON i.id = n.item_id
                  WHERE i.library_id = l.id) AS nfos_read,
                (SELECT count(*) FROM sidecar_nfos n
                   JOIN media_items i ON i.id = n.item_id
                  WHERE i.library_id = l.id) AS nfos_written,
                (SELECT count(*) FROM sidecar_artwork a
                   JOIN media_items i ON i.id = a.item_id
                  WHERE i.library_id = l.id) AS artwork_written,
                EXISTS (SELECT 1 FROM notices n
                         WHERE n.key = 'sidecars_unwritable:' || l.id::text
                           AND n.resolved_at IS NULL) AS unwritable_notice
         FROM libraries l
         ORDER BY l.name",
    )
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    let paths: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT library_id, path FROM library_paths ORDER BY path")
            .fetch_all(&app.pool)
            .await
            .map_err(AppError::from)?;
    // The folder checks ask the kernel per folder; keep them off the
    // async threads.
    let folders = tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|(library_id, path)| (library_id, folder_access(&path)))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(
        rows.into_iter()
            .map(|r| LibrarySidecars {
                supported: SIDECAR_KINDS.contains(&r.kind.as_str()),
                folders: folders
                    .iter()
                    .filter(|(library_id, _)| *library_id == r.id)
                    .map(|(_, f)| f.clone())
                    .collect(),
                id: r.id,
                name: r.name,
                kind: r.kind,
                write_sidecars: r.write_sidecars,
                unwritable_notice: r.unwritable_notice,
                items: r.items,
                sidecars: r.sidecars,
                sidecars_overridden: r.sidecars_overridden,
                nfos_read: r.nfos_read,
                nfos_written: r.nfos_written,
                artwork_written: r.artwork_written,
            })
            .collect(),
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/admin/sidecars", get(list))
}
