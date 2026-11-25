//! Poster and backdrop copies beside the media (XFER5, D14). Spec:
//! `docs/IMPORT-EXPORT.md` §13.10.
//!
//! When a library has opted into writing sidecars
//! (`libraries.write_sidecars`), a matched movie's or series' cached
//! poster and backdrop are copied next to its media as `poster.jpg` /
//! `backdrop.jpg`, so they travel with the files like its
//! `.parlour.json` does. The same rules as the sidecar writer apply:
//! only where the library opted in, only matched items, each write
//! atomic (temp file + rename), and a file already holding the same
//! bytes is left alone. Two more keep the copy safe in folders Parlour
//! doesn't own:
//!
//! - **Only a folder that belongs to the item.** A movie's copy goes
//!   in the folder of its video, a series' in its series folder, and
//!   only when every video under that folder belongs to that item and
//!   the folder isn't a library root. Two movies sharing a folder get
//!   no `poster.jpg` there: a single name can't say which one it is.
//! - **Never over someone else's file.** A `poster.jpg` Parlour didn't
//!   write (it has no `sidecar_artwork` row whose hash matches the
//!   file's bytes) is left alone and counted, never replaced.
//!
//! Copies are made by the artwork step (`download_item_artwork`),
//! because that is when the cache holds the item's current artwork.

use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use super::{series_folder, write_atomic, WriteOutcome};
use crate::artwork::sha256_hex;

/// The two kinds copied, and the file name each gets.
pub const ARTWORK_FILES: [(&str, &str); 2] =
    [("poster", "poster.jpg"), ("backdrop", "backdrop.jpg")];

/// What one item's copy pass did.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ArtworkCopyReport {
    /// Files written (new, or Parlour's own copy replaced).
    pub copied: i32,
    /// Files that already held exactly these bytes.
    pub unchanged: i32,
    /// `path: reason` for each file deliberately not written: one
    /// Parlour didn't write is in the way, or the cached artwork isn't
    /// a JPEG.
    pub left_alone: Vec<String>,
    /// `path: reason` for each file that couldn't be written.
    pub errors: Vec<String>,
}

/// Whether these bytes are a JPEG (the `FF D8 FF` start of image).
/// Only JPEGs are copied: the files are named `.jpg`.
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// What to do with one destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyAction {
    /// Nothing there yet: write it.
    Write,
    /// Already holds exactly the new bytes.
    Unchanged,
    /// Holds bytes Parlour wrote earlier (they still hash to the
    /// recorded value): replace them with the current artwork.
    Replace,
    /// Holds something Parlour didn't write: leave it alone.
    Foreign,
}

/// Decide what to do with a destination, given what is there now
/// (`None` = no file), the hash Parlour recorded when it last wrote
/// that path (if it did), and the new bytes.
pub fn decide(existing: Option<&[u8]>, recorded_sha256: Option<&str>, new: &[u8]) -> CopyAction {
    match existing {
        None => CopyAction::Write,
        Some(old) if old == new => CopyAction::Unchanged,
        Some(old) if recorded_sha256 == Some(sha256_hex(old).as_str()) => CopyAction::Replace,
        Some(_) => CopyAction::Foreign,
    }
}

/// Whether `folder` is one of the library's roots (trailing slashes
/// ignored).
fn is_root(folder: &Path, roots: &[String]) -> bool {
    roots
        .iter()
        .any(|r| Path::new(r.trim_end_matches('/')) == folder)
}

/// Copy a matched movie's or series' cached poster and backdrop next
/// to its media, where its library opted in. A no-op (and no disk
/// access) for any other item, or when the library hasn't opted in.
pub async fn copy_item_artwork(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<ArtworkCopyReport, sqlx::Error> {
    let mut report = ArtworkCopyReport::default();
    let row: Option<(Uuid, String, String, bool, String)> = sqlx::query_as(
        "SELECT i.library_id, i.kind, i.metadata_status, l.write_sidecars, l.kind
         FROM media_items i JOIN libraries l ON l.id = i.library_id
         WHERE i.id = $1",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some((library_id, kind, status, write_sidecars, library_kind)) = row else {
        return Ok(report);
    };
    if !write_sidecars
        || library_kind == "audiobooks"
        || !matches!(kind.as_str(), "movie" | "series")
        || status != "matched"
    {
        return Ok(report);
    }

    // The folders the item's media sits in.
    let files: Vec<(String, String)> = sqlx::query_as(
        "SELECT f.path, f.relative_path
         FROM media_files f JOIN media_items i ON i.id = f.item_id
         LEFT JOIN media_items se ON se.id = i.parent_id AND i.kind = 'episode'
         WHERE f.available AND i.library_id = $1
           AND (i.id = $2 OR se.parent_id = $2)
         ORDER BY f.relative_path, f.path",
    )
    .bind(library_id)
    .bind(item_id)
    .fetch_all(pool)
    .await?;
    let mut folders: Vec<PathBuf> = Vec::new();
    for (path, rel) in &files {
        let folder = if kind == "series" {
            series_folder(path, rel)
        } else {
            Path::new(path).parent().map(Path::to_path_buf)
        };
        if let Some(f) = folder {
            if !folders.contains(&f) {
                folders.push(f);
            }
        }
    }
    let roots: Vec<String> =
        sqlx::query_scalar("SELECT path FROM library_paths WHERE library_id = $1")
            .bind(library_id)
            .fetch_all(pool)
            .await?;

    // Only folders that belong to this item alone.
    let mut own: Vec<PathBuf> = Vec::new();
    for folder in folders {
        if is_root(&folder, &roots) {
            continue;
        }
        let prefix = format!("{}/", folder.to_string_lossy().trim_end_matches('/'));
        let (shared,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM media_files f
                 JOIN media_items i ON i.id = f.item_id
                 LEFT JOIN media_items se ON se.id = i.parent_id AND i.kind = 'episode'
                 WHERE f.available AND starts_with(f.path, $1)
                   AND COALESCE(se.parent_id, i.id) <> $2)",
        )
        .bind(&prefix)
        .bind(item_id)
        .fetch_one(pool)
        .await?;
        if !shared {
            own.push(folder);
        }
    }
    if own.is_empty() {
        return Ok(report);
    }

    for (art_kind, file_name) in ARTWORK_FILES {
        // The cached file: the original first, never the generated tile.
        let cached: Option<(String,)> = sqlx::query_as(
            "SELECT path FROM images
             WHERE item_id = $1 AND kind = $2 AND variant <> 'generated'
               AND path <> '' AND path NOT LIKE 'pending:%'
             ORDER BY CASE variant WHEN 'original' THEN 0 ELSE 1 END, path
             LIMIT 1",
        )
        .bind(item_id)
        .bind(art_kind)
        .fetch_optional(pool)
        .await?;
        // No artwork of this kind cached: nothing to copy, and an
        // earlier copy stays where it is (Parlour never deletes
        // beside the media).
        let Some((cached,)) = cached else { continue };
        let Ok(bytes) = std::fs::read(&cached) else {
            continue;
        };
        if !is_jpeg(&bytes) {
            for folder in &own {
                report.left_alone.push(format!(
                    "{}: the cached {art_kind} isn't a JPEG",
                    folder.join(file_name).display()
                ));
            }
            continue;
        }
        let new_hash = sha256_hex(&bytes);
        for folder in &own {
            let dest = folder.join(file_name);
            let shown = dest.display().to_string();
            let existing = match std::fs::read(&dest) {
                Ok(b) => Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    report.errors.push(format!("{shown}: {e}"));
                    continue;
                }
            };
            let recorded: Option<(String,)> =
                sqlx::query_as("SELECT sha256 FROM sidecar_artwork WHERE path = $1")
                    .bind(&shown)
                    .fetch_optional(pool)
                    .await?;
            match decide(
                existing.as_deref(),
                recorded.as_ref().map(|(h,)| h.as_str()),
                &bytes,
            ) {
                CopyAction::Unchanged => report.unchanged += 1,
                CopyAction::Foreign => report.left_alone.push(format!(
                    "{shown}: a file Parlour didn't write is already there"
                )),
                CopyAction::Write | CopyAction::Replace => match write_atomic(&dest, &bytes) {
                    Ok(WriteOutcome::Unchanged) => report.unchanged += 1,
                    Ok(WriteOutcome::Written) => {
                        sqlx::query(
                            "INSERT INTO sidecar_artwork (path, item_id, kind, sha256)
                             VALUES ($1, $2, $3, $4)
                             ON CONFLICT (path) DO UPDATE SET
                                item_id = EXCLUDED.item_id, kind = EXCLUDED.kind,
                                sha256 = EXCLUDED.sha256, written_at = now()",
                        )
                        .bind(&shown)
                        .bind(item_id)
                        .bind(art_kind)
                        .bind(&new_hash)
                        .execute(pool)
                        .await?;
                        report.copied += 1;
                    }
                    Err(e) => report.errors.push(format!("{shown}: {e}")),
                },
            }
        }
    }
    Ok(report)
}
