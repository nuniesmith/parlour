//! The database half of NFO interop (XFER6, D14). Spec:
//! `docs/IMPORT-EXPORT.md` §13.13.
//!
//! - **The scanner's NFO pass**, after the sidecar pass: read every
//!   item's NFO files, and apply the new or changed ones: the title
//!   and provider ids, each only where the item's sidecar doesn't
//!   carry that field. A re-read video gets its NFO's title back. A
//!   bad NFO is reported and skipped.
//! - **The matcher** (`stored_nfo`): when the item's sidecar names no
//!   record for the active provider, the NFO's id picks it.
//! - **The writer** (`write_target_nfos`): where the library writes
//!   sidecars, a `.nfo` beside each `.parlour.json`, never over an NFO
//!   Parlour didn't write.
//!
//! Precedence, highest first: an admin's fix-match, the sidecar, the
//! provider record, the NFO, the file name.

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use super::artwork::{decide, folder_belongs_to, CopyAction};
use super::nfo::{parse_nfo, to_nfo_bytes, NfoContent, NfoFields, MAX_NFO_BYTES};
use super::sync::{fingerprint, load_targets, report_problems, SidecarWriteReport, Target};
use super::{write_atomic, Sidecar, SidecarKind, WriteOutcome};
use crate::artwork::sha256_hex;
use crate::parse::sort_title;
use crate::scan::ScanReport;

/// One NFO file found on disk.
struct FoundNfo {
    path: PathBuf,
    size: u64,
    mtime: DateTime<Utc>,
    bytes: Vec<u8>,
}

/// The NFO files of a target the reader looks at: every per-file and
/// series NFO, and a movie's `movie.nfo` only where its folder belongs
/// to that movie alone.
async fn readable_paths(
    pool: &parlour_db::PgPool,
    target: &Target,
    roots: &[String],
) -> Result<Vec<PathBuf>, sqlx::Error> {
    let mut out = Vec::new();
    for n in &target.nfos {
        if n.folder_level {
            let Some(folder) = n.path.parent() else {
                continue;
            };
            if !folder_belongs_to(pool, folder, target.item_id, roots).await? {
                continue;
            }
        }
        out.push(n.path.clone());
    }
    Ok(out)
}

/// Read the NFO files that exist. Errors other than "not there" are
/// returned per path; an oversized file is refused unread.
fn gather(paths: &[PathBuf]) -> (Vec<FoundNfo>, Vec<(String, String)>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let shown = path.display().to_string();
        let meta = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                errors.push((shown, e.to_string()));
                continue;
            }
        };
        if meta.len() > MAX_NFO_BYTES {
            errors.push((shown, super::nfo::NfoError::TooLarge.to_string()));
            continue;
        }
        match std::fs::read(path) {
            Ok(bytes) => found.push(FoundNfo {
                path: path.clone(),
                size: meta.len(),
                mtime: meta
                    .modified()
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_default(),
                bytes,
            }),
            Err(e) => errors.push((shown, e.to_string())),
        }
    }
    (found, errors)
}

fn nfo_fingerprint(found: &[FoundNfo]) -> String {
    fingerprint(
        &found
            .iter()
            .map(|f| (f.path.clone(), f.size, f.mtime))
            .collect::<Vec<_>>(),
    )
}

/// Parse and check every NFO of one target and agree them into one
/// set of fields. Any problem refuses all of them for this scan.
fn resolve(target: &Target, found: &[FoundNfo]) -> Result<NfoFields, Vec<(String, String)>> {
    let mut errors = Vec::new();
    let mut read: Vec<(&FoundNfo, NfoFields)> = Vec::new();
    for f in found {
        let path = f.path.display().to_string();
        match parse_nfo(&f.bytes) {
            Err(e) => errors.push((path, e.to_string())),
            Ok(nfo) => {
                if nfo.kind != target.kind {
                    errors.push((
                        path,
                        format!(
                            "the NFO describes a {} but its item is a {}",
                            nfo.kind.as_str(),
                            target.kind.as_str()
                        ),
                    ));
                    continue;
                }
                if target.kind == SidecarKind::Episode {
                    let mismatch = |carried: Option<i32>, actual: Option<i32>| {
                        carried.is_some() && carried != actual
                    };
                    if mismatch(nfo.season_number, target.season_number)
                        || mismatch(nfo.episode_number, target.episode_number)
                    {
                        errors.push((
                            path,
                            "this NFO names another season or episode than its file".to_string(),
                        ));
                        continue;
                    }
                }
                read.push((f, nfo.fields));
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let first = read[0].1.clone();
    if let Some((f, _)) = read.iter().find(|(_, d)| *d != first) {
        return Err(vec![(
            f.path.display().to_string(),
            "the NFOs of one item disagree on its title or ids; make them the same".to_string(),
        )]);
    }
    Ok(first)
}

/// The item's sidecar document, unless an admin's fix overrode it.
async fn live_sidecar(
    conn: &mut PgConnection,
    item_id: Uuid,
) -> Result<Option<Sidecar>, sqlx::Error> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT document FROM item_sidecars WHERE item_id = $1 AND overridden_at IS NULL",
    )
    .bind(item_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.and_then(|(v,)| serde_json::from_value(v).ok()))
}

/// Write an NFO's title and ids onto an item, each only where the
/// item's sidecar doesn't carry that field (the sidecar ranks higher).
/// Nothing is written when nothing differs.
async fn apply_nfo(
    conn: &mut PgConnection,
    item_id: Uuid,
    nfo: &NfoFields,
) -> Result<(), sqlx::Error> {
    let sidecar = live_sidecar(conn, item_id).await?;
    let sc = sidecar.as_ref();
    let title = nfo
        .title
        .clone()
        .filter(|_| sc.and_then(|s| s.title.as_ref()).is_none());
    let tmdb = nfo.tmdb_id.filter(|_| sc.and_then(|s| s.tmdb_id).is_none());
    let tvdb = nfo.tvdb_id.filter(|_| sc.and_then(|s| s.tvdb_id).is_none());
    let imdb = nfo
        .imdb_id
        .clone()
        .filter(|_| sc.and_then(|s| s.imdb_id.as_ref()).is_none());
    let sort = title.as_deref().map(sort_title);
    sqlx::query(
        "UPDATE media_items SET
             title = COALESCE($2, title), sort_title = COALESCE($3, sort_title),
             tmdb_id = COALESCE($4, tmdb_id), tvdb_id = COALESCE($5, tvdb_id),
             imdb_id = COALESCE($6, imdb_id), updated_at = now()
         WHERE id = $1
           AND (title, sort_title, tmdb_id, tvdb_id, imdb_id)
               IS DISTINCT FROM
               (COALESCE($2, title), COALESCE($3, sort_title), COALESCE($4, tmdb_id),
                COALESCE($5, tvdb_id), COALESCE($6, imdb_id))",
    )
    .bind(item_id)
    .bind(&title)
    .bind(&sort)
    .bind(tmdb)
    .bind(tvdb)
    .bind(&imdb)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Record what was read: the files' fingerprint and the fields taken.
/// Clears an override.
async fn store(
    conn: &mut PgConnection,
    item_id: Uuid,
    fingerprint: &str,
    fields: &NfoFields,
) -> Result<(), sqlx::Error> {
    let document = serde_json::to_value(fields).unwrap_or_default();
    sqlx::query(
        "INSERT INTO item_nfos (item_id, fingerprint, document)
         VALUES ($1, $2, $3)
         ON CONFLICT (item_id) DO UPDATE
             SET fingerprint = EXCLUDED.fingerprint, document = EXCLUDED.document,
                 overridden_at = NULL, read_at = now()
         WHERE (item_nfos.fingerprint, item_nfos.document, item_nfos.overridden_at)
               IS DISTINCT FROM (EXCLUDED.fingerprint, EXCLUDED.document, NULL::timestamptz)",
    )
    .bind(item_id)
    .bind(fingerprint)
    .bind(document)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ------------------------------------------------------------ the scanner's pass

/// The scanner's NFO pass, run after the sidecar pass so the sidecar's
/// fields are already in place: read every item's NFO files, and apply
/// the ones that are new or changed since the last read. An item the
/// scan re-created from its file name (`touched`) gets its last NFO
/// re-applied, unless an admin's fix-match overrode it. An NFO that is
/// gone stops counting; the item keeps its values. Bad NFOs are
/// reported and skipped, and the scan carries on.
pub(crate) async fn read_library_nfos(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    touched: &HashSet<Uuid>,
    all_roots_walked: bool,
    job_id: Option<Uuid>,
    report: &mut ScanReport,
) -> Result<(), sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let targets = load_targets(&mut conn, library_id, None).await?;
    let roots: Vec<String> =
        sqlx::query_scalar("SELECT path FROM library_paths WHERE library_id = $1")
            .bind(library_id)
            .fetch_all(&mut *conn)
            .await?;
    for target in targets {
        let paths = readable_paths(pool, &target, &roots).await?;
        let (found, io_errors) = gather(&paths);
        let mut problems = io_errors;
        let stored: Option<(String, serde_json::Value, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT fingerprint, document, overridden_at FROM item_nfos WHERE item_id = $1",
        )
        .bind(target.item_id)
        .fetch_optional(&mut *conn)
        .await?;

        if found.is_empty() {
            // As with sidecars: an unmounted disk is not a deleted NFO.
            if stored.is_some() && all_roots_walked && problems.is_empty() {
                sqlx::query("DELETE FROM item_nfos WHERE item_id = $1")
                    .bind(target.item_id)
                    .execute(&mut *conn)
                    .await?;
            }
            report_problems(pool, job_id, report, &problems).await;
            continue;
        }
        let fp = nfo_fingerprint(&found);
        if let Some((old_fp, document, overridden_at)) = &stored {
            if *old_fp == fp {
                if touched.contains(&target.item_id) && overridden_at.is_none() {
                    if let Ok(fields) = serde_json::from_value::<NfoFields>(document.clone()) {
                        apply_nfo(&mut conn, target.item_id, &fields).await?;
                    }
                }
                report_problems(pool, job_id, report, &problems).await;
                continue;
            }
        }
        match resolve(&target, &found) {
            Err(errors) => problems.extend(errors),
            Ok(fields) => {
                let mut tx = pool.begin().await?;
                apply_nfo(&mut tx, target.item_id, &fields).await?;
                store(&mut tx, target.item_id, &fp, &fields).await?;
                tx.commit().await?;
                report.nfos_applied += 1;
            }
        }
        report_problems(pool, job_id, report, &problems).await;
    }
    Ok(())
}

// ------------------------------------------------------------ the matcher

/// What the scanner last read from an item's NFOs, unless an admin's
/// fix-match has overridden it.
pub async fn stored_nfo(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<Option<NfoFields>, sqlx::Error> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT document FROM item_nfos WHERE item_id = $1 AND overridden_at IS NULL",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(v,)| serde_json::from_value(v).ok()))
}

// ------------------------------------------------------------ the writer

/// The NFO Parlour writes for an item: the identifying fields of the
/// sidecar document it has just written (title, year or season and
/// episode, provider ids), nothing descriptive.
pub(super) fn content_for(doc: &Sidecar) -> NfoContent {
    NfoContent {
        title: doc.title.clone(),
        year: if doc.kind == SidecarKind::Episode {
            None
        } else {
            doc.year
        },
        season_number: doc.season_number,
        episode_number: doc.episode_number,
        tmdb_id: doc.tmdb_id,
        tvdb_id: doc.tvdb_id,
        imdb_id: doc.imdb_id.clone(),
    }
}

/// Record that Parlour's bytes are at `path` (written, or found there
/// already, byte for byte).
async fn record_written(
    conn: &mut PgConnection,
    path: &str,
    item_id: Uuid,
    sha256: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sidecar_nfos (path, item_id, sha256)
         VALUES ($1, $2, $3)
         ON CONFLICT (path) DO UPDATE SET
             item_id = EXCLUDED.item_id, sha256 = EXCLUDED.sha256, written_at = now()
         WHERE (sidecar_nfos.item_id, sidecar_nfos.sha256)
               IS DISTINCT FROM (EXCLUDED.item_id, EXCLUDED.sha256)",
    )
    .bind(path)
    .bind(item_id)
    .bind(sha256)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The writer's NFO step for one target (called by
/// `write_item_sidecars`, so only for matched items in a library that
/// writes sidecars): write `<video stem>.nfo` beside each video, or
/// `tvshow.nfo` in each series folder. A file already holding these
/// bytes is left alone; an NFO Parlour didn't write is never replaced
/// (it is listed instead); each write is atomic. Afterwards, when every
/// NFO the reader would read for this item is Parlour's own, the read
/// record is brought up to date, so the next scan doesn't take
/// Parlour's own NFO for new input.
pub(super) async fn write_target_nfos(
    pool: &parlour_db::PgPool,
    conn: &mut PgConnection,
    target: &Target,
    doc: &Sidecar,
    roots: &[String],
    report: &mut SidecarWriteReport,
) -> Result<(), sqlx::Error> {
    let bytes = to_nfo_bytes(target.kind, &content_for(doc));
    let hash = sha256_hex(&bytes);
    let mut ok = true;
    for n in target.nfos.iter().filter(|n| !n.folder_level) {
        let shown = n.path.display().to_string();
        let existing = match std::fs::read(&n.path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                ok = false;
                report.errors.push(format!("{shown}: {e}"));
                continue;
            }
        };
        let recorded: Option<(String,)> =
            sqlx::query_as("SELECT sha256 FROM sidecar_nfos WHERE path = $1")
                .bind(&shown)
                .fetch_optional(&mut *conn)
                .await?;
        match decide(
            existing.as_deref(),
            recorded.as_ref().map(|(h,)| h.as_str()),
            &bytes,
        ) {
            CopyAction::Unchanged => {
                report.nfos_unchanged += 1;
                // Exactly Parlour's bytes (a copy from another host,
                // say): Parlour may keep it up to date from now on.
                record_written(conn, &shown, target.item_id, &hash).await?;
            }
            CopyAction::Foreign => report.nfos_left_alone.push(format!(
                "{shown}: an NFO Parlour didn't write is already there"
            )),
            CopyAction::Write | CopyAction::Replace => match write_atomic(&n.path, &bytes) {
                Ok(outcome) => {
                    record_written(conn, &shown, target.item_id, &hash).await?;
                    match outcome {
                        WriteOutcome::Written => report.nfos_written += 1,
                        WriteOutcome::Unchanged => report.nfos_unchanged += 1,
                    }
                }
                Err(e) => {
                    ok = false;
                    report.errors.push(format!("{shown}: {e}"));
                }
            },
        }
    }
    if !ok {
        return Ok(());
    }
    // Bring the read record up to date, but only when every NFO the
    // reader would read here is one Parlour wrote: a foreign NFO beside
    // it is left for the scan to read (and report, if they disagree).
    let paths = readable_paths(pool, target, roots).await?;
    let (found, errors) = gather(&paths);
    if found.is_empty() || !errors.is_empty() {
        return Ok(());
    }
    for f in &found {
        let recorded: Option<(String,)> =
            sqlx::query_as("SELECT sha256 FROM sidecar_nfos WHERE path = $1")
                .bind(f.path.display().to_string())
                .fetch_optional(&mut *conn)
                .await?;
        if recorded.map(|(h,)| h) != Some(sha256_hex(&f.bytes)) {
            return Ok(());
        }
    }
    if let Ok(fields) = resolve(target, &found) {
        store(conn, target.item_id, &nfo_fingerprint(&found), &fields).await?;
    }
    Ok(())
}
