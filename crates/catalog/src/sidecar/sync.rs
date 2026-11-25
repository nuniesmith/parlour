//! The database half of sidecars (XFER4): the scanner's read pass,
//! applying a document to an item, the matcher's re-apply after a
//! provider lookup, and the metadata job's rewrite.
//!
//! Precedence (spec §13.5): a sidecar's fields win over automatic
//! provider lookups; an admin's fix-match wins over the sidecar (and
//! rewrites it when the library writes sidecars). The scanner applies
//! a sidecar when its files are new or changed — not on every scan —
//! so an edit made in Parlour after that isn't undone by the next
//! scan.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::nfo::{file_nfo_path, MOVIE_NFO, TVSHOW_NFO};
use super::{
    file_sidecar_path, parse, series_folder, to_bytes, write_atomic, Sidecar, SidecarKind,
    SidecarPerson, SidecarSegment, SidecarSegments, WriteOutcome, MANUAL_CONFIDENCE,
    SERIES_FILE_NAME,
};
use crate::parse::sort_title;
use crate::scan::ScanReport;

/// One sidecar file an item reads: where it is, and the sizes of the
/// videos it sits beside (its segments must match one of them).
#[derive(Debug, Clone)]
struct SidecarFile {
    path: PathBuf,
    video_sizes: Vec<i64>,
    /// Size of the item's playback file when this sidecar belongs to
    /// it — the file whose sidecar carries the item's segments.
    playback_size: Option<i64>,
}

/// One NFO file an item reads (XFER6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NfoPath {
    pub(super) path: PathBuf,
    /// A movie's `movie.nfo` in its video's folder: read only when the
    /// folder belongs to that movie alone, and never written.
    pub(super) folder_level: bool,
}

/// An item that can have sidecars, and the files they would be.
#[derive(Debug, Clone)]
pub(super) struct Target {
    pub(super) item_id: Uuid,
    pub(super) kind: SidecarKind,
    pub(super) season_number: Option<i32>,
    pub(super) episode_number: Option<i32>,
    files: Vec<SidecarFile>,
    /// Its NFO files (XFER6): `<video stem>.nfo` beside each video, a
    /// movie's `movie.nfo`, a series' `tvshow.nfo` in each series
    /// folder.
    pub(super) nfos: Vec<NfoPath>,
}

fn push_nfo(nfos: &mut Vec<NfoPath>, path: PathBuf, folder_level: bool) {
    if !nfos.iter().any(|n| n.path == path) {
        nfos.push(NfoPath { path, folder_level });
    }
}

type TargetRow = (
    Uuid,
    String,
    Option<i32>,
    Option<i32>,
    Option<Uuid>,
    String,
    String,
    i64,
);

/// Every item of a library (or only `only` and, for a series, its
/// episodes) with an available file, and the sidecar files each one
/// reads: a movie or episode reads one beside each of its videos, a
/// series one in each of its series folders.
pub(super) async fn load_targets(
    conn: &mut PgConnection,
    library_id: Uuid,
    only: Option<Uuid>,
) -> Result<Vec<Target>, sqlx::Error> {
    let rows: Vec<TargetRow> = sqlx::query_as(
        "SELECT i.id, i.kind, i.season_number, i.episode_number, sr.id,
                f.path, f.relative_path, f.size_bytes
         FROM media_items i
         JOIN media_files f ON f.item_id = i.id AND f.available
         LEFT JOIN media_items se ON se.id = i.parent_id AND i.kind = 'episode'
         LEFT JOIN media_items sr ON sr.id = se.parent_id AND sr.kind = 'series'
         WHERE i.library_id = $1 AND i.kind IN ('movie', 'episode')
           AND ($2::uuid IS NULL OR i.id = $2 OR sr.id = $2)
         ORDER BY i.id, f.relative_path, f.path",
    )
    .bind(library_id)
    .bind(only)
    .fetch_all(&mut *conn)
    .await?;

    let mut leaves: BTreeMap<Uuid, Target> = BTreeMap::new();
    let mut series: BTreeMap<Uuid, BTreeSet<PathBuf>> = BTreeMap::new();
    for (item_id, kind, season, episode, series_id, path, rel, size) in rows {
        let Some(kind) = SidecarKind::for_item(&kind) else {
            continue;
        };
        let target = leaves.entry(item_id).or_insert_with(|| Target {
            item_id,
            kind,
            season_number: season,
            episode_number: episode,
            files: Vec::new(),
            nfos: Vec::new(),
        });
        if let Some(nfo) = file_nfo_path(Path::new(&path)) {
            push_nfo(&mut target.nfos, nfo, false);
        }
        if kind == SidecarKind::Movie {
            if let Some(folder) = Path::new(&path).parent() {
                push_nfo(&mut target.nfos, folder.join(MOVIE_NFO), true);
            }
        }
        // Rows come in playback order: the first file of an item is
        // the one the player and the segment editor use.
        let is_playback = target.files.is_empty();
        if let Some(sidecar) = file_sidecar_path(Path::new(&path)) {
            let playback_size = is_playback.then_some(size);
            match target.files.iter_mut().find(|f| f.path == sidecar) {
                Some(f) => f.video_sizes.push(size),
                None => target.files.push(SidecarFile {
                    path: sidecar,
                    video_sizes: vec![size],
                    playback_size,
                }),
            }
        } else if is_playback {
            // A `series.*` video has no sidecar of its own; keep the
            // slot so "first file" still means the playback file.
            target.files.push(SidecarFile {
                path: PathBuf::new(),
                video_sizes: Vec::new(),
                playback_size: None,
            });
        }
        if kind == SidecarKind::Episode {
            if let (Some(series_id), Some(folder)) = (series_id, series_folder(&path, &rel)) {
                series
                    .entry(series_id)
                    .or_default()
                    .insert(folder.join(SERIES_FILE_NAME));
            }
        }
    }
    let mut targets: Vec<Target> = Vec::new();
    for (series_id, paths) in series {
        targets.push(Target {
            item_id: series_id,
            kind: SidecarKind::Series,
            season_number: None,
            episode_number: None,
            nfos: paths
                .iter()
                .map(|p| NfoPath {
                    path: p.with_file_name(TVSHOW_NFO),
                    folder_level: false,
                })
                .collect(),
            files: paths
                .into_iter()
                .map(|path| SidecarFile {
                    path,
                    video_sizes: Vec::new(),
                    playback_size: None,
                })
                .collect(),
        });
    }
    for mut t in leaves.into_values() {
        t.files.retain(|f| !f.path.as_os_str().is_empty());
        targets.push(t);
    }
    Ok(targets)
}

/// One sidecar file found on disk.
struct Found {
    file: SidecarFile,
    size: u64,
    mtime: DateTime<Utc>,
    bytes: Vec<u8>,
}

/// Read a target's sidecar files that exist. Errors other than "not
/// there" are returned per path.
fn gather(target: &Target) -> (Vec<Found>, Vec<(String, String)>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for file in &target.files {
        let meta = match std::fs::metadata(&file.path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                errors.push((file.path.display().to_string(), e.to_string()));
                continue;
            }
        };
        match std::fs::read(&file.path) {
            Ok(bytes) => {
                let mtime: DateTime<Utc> = meta
                    .modified()
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_default();
                found.push(Found {
                    file: file.clone(),
                    size: meta.len(),
                    mtime,
                    bytes,
                });
            }
            Err(e) => errors.push((file.path.display().to_string(), e.to_string())),
        }
    }
    (found, errors)
}

/// The fingerprint of a set of sidecar files: path, size and mtime
/// (microseconds) of each, one per line, sorted.
pub(super) fn fingerprint(found: &[(PathBuf, u64, DateTime<Utc>)]) -> String {
    let mut lines: Vec<String> = found
        .iter()
        .map(|(p, size, mtime)| format!("{}\t{size}\t{}", p.display(), mtime.timestamp_micros()))
        .collect();
    lines.sort();
    lines.join("\n")
}

fn stat_fingerprint(paths: &[PathBuf]) -> String {
    let found: Vec<(PathBuf, u64, DateTime<Utc>)> = paths
        .iter()
        .filter_map(|p| {
            let meta = std::fs::metadata(p).ok()?;
            let mtime = meta.modified().ok().map(DateTime::<Utc>::from)?;
            Some((p.clone(), meta.len(), mtime))
        })
        .collect();
    fingerprint(&found)
}

/// What a target's sidecars say, once they are read and checked.
struct Resolved {
    doc: Sidecar,
    segments: Option<Vec<SidecarSegment>>,
    /// Sidecars carried segments, but none for a video of this size.
    segments_skipped: bool,
}

/// Parse and check every sidecar of one target, and agree them into
/// one document. Any problem refuses the whole target (it is applied
/// as it was before, or not at all).
fn resolve(target: &Target, found: &[Found]) -> Result<Resolved, Vec<(String, String)>> {
    let mut errors = Vec::new();
    let mut docs: Vec<(&Found, Sidecar)> = Vec::new();
    for f in found {
        let path = f.file.path.display().to_string();
        match parse(&f.bytes) {
            Err(e) => errors.push((path, e.to_string())),
            Ok(doc) => {
                if doc.kind != target.kind {
                    errors.push((
                        path,
                        format!(
                            "the sidecar's kind is \"{}\" but its item's is \"{}\"",
                            doc.kind.as_str(),
                            target.kind.as_str()
                        ),
                    ));
                    continue;
                }
                if target.kind == SidecarKind::Episode {
                    let mismatch = |carried: Option<i32>, actual: Option<i32>| {
                        matches!((carried, actual), (Some(c), Some(a)) if c != a)
                            || (carried.is_some() && actual.is_none())
                    };
                    if mismatch(doc.season_number, target.season_number)
                        || mismatch(doc.episode_number, target.episode_number)
                    {
                        errors.push((
                            path,
                            "this sidecar names another season or episode than its file"
                                .to_string(),
                        ));
                        continue;
                    }
                }
                docs.push((f, doc));
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let first = docs[0].1.without_segments();
    if let Some((f, _)) = docs.iter().find(|(_, d)| d.without_segments() != first) {
        return Err(vec![(
            f.file.path.display().to_string(),
            "the sidecars of one item disagree; make them the same (segments aside)".to_string(),
        )]);
    }
    let carried: Vec<&SidecarSegments> = docs
        .iter()
        .filter_map(|(_, d)| d.segments.as_ref())
        .collect();
    let usable: Vec<&Vec<SidecarSegment>> = docs
        .iter()
        .filter_map(|(f, d)| {
            d.segments
                .as_ref()
                .filter(|s| f.file.video_sizes.contains(&s.size_bytes))
                .map(|s| &s.entries)
        })
        .collect();
    let mut segments = None;
    if let Some(first_entries) = usable.first() {
        if usable.iter().any(|e| e != first_entries) {
            return Err(vec![(
                docs[0].0.file.path.display().to_string(),
                "the sidecars of one item carry different segments for the same video".to_string(),
            )]);
        }
        segments = Some((*first_entries).clone());
    }
    Ok(Resolved {
        doc: first,
        segments_skipped: !carried.is_empty() && usable.is_empty(),
        segments,
    })
}

// ------------------------------------------------------------ applying

/// Write a document's fields onto an item. Only carried fields
/// change; nothing is written when nothing differs.
async fn apply_fields(
    conn: &mut PgConnection,
    item_id: Uuid,
    doc: &Sidecar,
) -> Result<(), sqlx::Error> {
    let sort = doc.title.as_deref().map(sort_title);
    sqlx::query(
        "UPDATE media_items SET
             title = COALESCE($2, title), sort_title = COALESCE($3, sort_title),
             year = COALESCE($4, year), air_date = COALESCE($5, air_date),
             overview = COALESCE($6, overview), genres = COALESCE($7, genres),
             content_rating = COALESCE($8, content_rating),
             vote_average = COALESCE($9, vote_average),
             runtime_minutes = COALESCE($10, runtime_minutes),
             tmdb_id = COALESCE($11, tmdb_id), tvdb_id = COALESCE($12, tvdb_id),
             imdb_id = COALESCE($13, imdb_id), updated_at = now()
         WHERE id = $1
           AND (title, sort_title, year, air_date, overview, genres, content_rating,
                vote_average, runtime_minutes, tmdb_id, tvdb_id, imdb_id)
               IS DISTINCT FROM
               (COALESCE($2, title), COALESCE($3, sort_title), COALESCE($4, year),
                COALESCE($5, air_date), COALESCE($6, overview), COALESCE($7, genres),
                COALESCE($8, content_rating), COALESCE($9, vote_average),
                COALESCE($10, runtime_minutes), COALESCE($11, tmdb_id),
                COALESCE($12, tvdb_id), COALESCE($13, imdb_id))",
    )
    .bind(item_id)
    .bind(&doc.title)
    .bind(&sort)
    .bind(doc.year)
    .bind(doc.air_date)
    .bind(&doc.overview)
    .bind(&doc.genres)
    .bind(&doc.content_rating)
    .bind(doc.vote_average)
    .bind(doc.runtime_minutes)
    .bind(doc.tmdb_id)
    .bind(doc.tvdb_id)
    .bind(&doc.imdb_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

type CreditKey = (String, String, Option<String>, i32);

/// Make an item's credits exactly the sidecar's. A person's `tmdb_id`
/// only fills a gap: people are one row per name server-wide.
async fn apply_people(
    conn: &mut PgConnection,
    item_id: Uuid,
    people: &[SidecarPerson],
) -> Result<(), sqlx::Error> {
    let mut current: Vec<CreditKey> = sqlx::query_as(
        "SELECT p.name, ip.role, ip.character_name, ip.sort_order
         FROM item_people ip JOIN people p ON p.id = ip.person_id
         WHERE ip.item_id = $1",
    )
    .bind(item_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut wanted: Vec<CreditKey> = people
        .iter()
        .map(|p| {
            (
                p.name.clone(),
                p.role.clone(),
                p.character_name.clone(),
                p.sort_order.unwrap_or(0),
            )
        })
        .collect();
    current.sort();
    wanted.sort();
    if current == wanted {
        return Ok(());
    }
    sqlx::query("DELETE FROM item_people WHERE item_id = $1")
        .bind(item_id)
        .execute(&mut *conn)
        .await?;
    for p in people {
        let (person_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO people (name, tmdb_id) VALUES ($1, $2)
             ON CONFLICT (name) DO UPDATE
                 SET tmdb_id = COALESCE(people.tmdb_id, EXCLUDED.tmdb_id)
             RETURNING id",
        )
        .bind(&p.name)
        .bind(p.tmdb_id)
        .fetch_one(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO item_people (item_id, person_id, role, character_name, sort_order)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(item_id)
        .bind(person_id)
        .bind(&p.role)
        .bind(&p.character_name)
        .bind(p.sort_order.unwrap_or(0))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Make an item's manual segments exactly the sidecar's: each listed
/// kind is stored as a manual row (times or a D21 marker), and a kind
/// the sidecar doesn't list loses its manual row. Detected rows are
/// never touched.
async fn apply_segments(
    conn: &mut PgConnection,
    item_id: Uuid,
    entries: &[SidecarSegment],
) -> Result<(), sqlx::Error> {
    for kind in ["recap", "intro", "credits"] {
        match entries.iter().find(|e| e.kind == kind) {
            Some(e) => {
                sqlx::query(
                    "INSERT INTO media_segments
                         (item_id, kind, start_ms, end_ms, source, confidence)
                     VALUES ($1, $2, $3, $4, 'manual', $5)
                     ON CONFLICT (item_id, kind, source) DO UPDATE
                     SET start_ms = EXCLUDED.start_ms, end_ms = EXCLUDED.end_ms,
                         confidence = EXCLUDED.confidence, updated_at = now()
                     WHERE (media_segments.start_ms, media_segments.end_ms,
                            media_segments.confidence)
                           IS DISTINCT FROM
                           (EXCLUDED.start_ms, EXCLUDED.end_ms, EXCLUDED.confidence)",
                )
                .bind(item_id)
                .bind(kind)
                .bind(e.start_ms)
                .bind(e.end_ms)
                .bind(MANUAL_CONFIDENCE)
                .execute(&mut *conn)
                .await?;
            }
            None => {
                sqlx::query(
                    "DELETE FROM media_segments
                     WHERE item_id = $1 AND kind = $2 AND source = 'manual'",
                )
                .bind(item_id)
                .bind(kind)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    Ok(())
}

async fn apply_document(
    conn: &mut PgConnection,
    item_id: Uuid,
    doc: &Sidecar,
) -> Result<(), sqlx::Error> {
    apply_fields(conn, item_id, doc).await?;
    if let Some(people) = doc.people_in_order() {
        apply_people(conn, item_id, &people).await?;
    }
    Ok(())
}

/// Record what was read (or written): the files' fingerprint and the
/// document without segments. Clears an override.
async fn store(
    conn: &mut PgConnection,
    item_id: Uuid,
    fingerprint: &str,
    doc: &Sidecar,
) -> Result<(), sqlx::Error> {
    let document = serde_json::to_value(doc.without_segments()).unwrap_or_default();
    sqlx::query(
        "INSERT INTO item_sidecars (item_id, fingerprint, document)
         VALUES ($1, $2, $3)
         ON CONFLICT (item_id) DO UPDATE
             SET fingerprint = EXCLUDED.fingerprint, document = EXCLUDED.document,
                 overridden_at = NULL, read_at = now()
         WHERE (item_sidecars.fingerprint, item_sidecars.document, item_sidecars.overridden_at)
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

/// The scanner's sidecar pass (after files are catalogued and
/// vanished ones marked): read every item's sidecars, and apply the
/// ones that are new or changed since the last read. An item the scan
/// re-created from its file name (`touched` — its video was new or
/// changed) gets its last sidecar re-applied too, unless an admin's
/// fix-match overrode it. A sidecar that is gone stops counting; the
/// item keeps its values. Bad sidecars are reported and skipped.
pub(crate) async fn read_library_sidecars(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    touched: &HashSet<Uuid>,
    all_roots_walked: bool,
    job_id: Option<Uuid>,
    report: &mut ScanReport,
) -> Result<(), sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let targets = load_targets(&mut conn, library_id, None).await?;
    for target in targets {
        let (found, io_errors) = gather(&target);
        let mut problems = io_errors;
        let stored: Option<(String, serde_json::Value, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT fingerprint, document, overridden_at FROM item_sidecars WHERE item_id = $1",
        )
        .bind(target.item_id)
        .fetch_optional(&mut *conn)
        .await?;

        if found.is_empty() {
            // Only drop the record when every root walked and every
            // sidecar path could be looked at: an unmounted disk is
            // not a deleted sidecar.
            if stored.is_some() && all_roots_walked && problems.is_empty() {
                sqlx::query("DELETE FROM item_sidecars WHERE item_id = $1")
                    .bind(target.item_id)
                    .execute(&mut *conn)
                    .await?;
            }
            report_problems(pool, job_id, report, &problems).await;
            continue;
        }
        let fp = fingerprint(
            &found
                .iter()
                .map(|f| (f.file.path.clone(), f.size, f.mtime))
                .collect::<Vec<_>>(),
        );
        if let Some((old_fp, document, overridden_at)) = &stored {
            if *old_fp == fp {
                if touched.contains(&target.item_id) && overridden_at.is_none() {
                    if let Ok(doc) = serde_json::from_value::<Sidecar>(document.clone()) {
                        apply_document(&mut conn, target.item_id, &doc).await?;
                    }
                }
                report_problems(pool, job_id, report, &problems).await;
                continue;
            }
        }
        match resolve(&target, &found) {
            Err(errors) => problems.extend(errors),
            Ok(resolved) => {
                let mut tx = pool.begin().await?;
                apply_document(&mut tx, target.item_id, &resolved.doc).await?;
                if let Some(entries) = &resolved.segments {
                    apply_segments(&mut tx, target.item_id, entries).await?;
                }
                store(&mut tx, target.item_id, &fp, &resolved.doc).await?;
                tx.commit().await?;
                report.sidecars_applied += 1;
                if resolved.segments_skipped {
                    report.sidecar_segments_skipped += 1;
                }
            }
        }
        report_problems(pool, job_id, report, &problems).await;
    }
    Ok(())
}

pub(super) async fn report_problems(
    pool: &parlour_db::PgPool,
    job_id: Option<Uuid>,
    report: &mut ScanReport,
    problems: &[(String, String)],
) {
    for (path, message) in problems {
        report.errors.push(format!("{path}: {message}"));
        if let Some(job) = job_id {
            let _ = parlour_jobs::add_error(pool, job, Some(path), message).await;
        }
    }
}

// ------------------------------------------------------------ the matcher

/// The sidecar document the scanner last applied to an item, unless
/// an admin's fix-match has overridden it.
pub async fn stored_document(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<Option<Sidecar>, sqlx::Error> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT document FROM item_sidecars WHERE item_id = $1 AND overridden_at IS NULL",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(v,)| serde_json::from_value(v).ok()))
}

/// After an automatic provider lookup: put the sidecars' fields back
/// over what the provider wrote — on the item and, for a series, on
/// its episodes (the lookup enriches them too).
pub async fn reapply_stored(pool: &parlour_db::PgPool, item_id: Uuid) -> Result<(), sqlx::Error> {
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT s.item_id, s.document FROM item_sidecars s
         JOIN media_items i ON i.id = s.item_id
         WHERE s.overridden_at IS NULL
           AND (i.id = $1 OR i.parent_id IN
                (SELECT id FROM media_items WHERE parent_id = $1 AND kind = 'season'))
         ORDER BY s.item_id",
    )
    .bind(item_id)
    .fetch_all(pool)
    .await?;
    let mut tx = pool.begin().await?;
    for (id, document) in rows {
        if let Ok(doc) = serde_json::from_value::<Sidecar>(document) {
            apply_document(&mut tx, id, &doc).await?;
        }
    }
    tx.commit().await
}

/// After an admin's fix-match: the provider record they chose wins
/// over the item's sidecars and NFOs (and its episodes') until a
/// sidecar or NFO file changes or Parlour rewrites it.
pub async fn mark_overridden(pool: &parlour_db::PgPool, item_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE item_sidecars SET overridden_at = now()
         WHERE overridden_at IS NULL AND item_id IN (
             SELECT i.id FROM media_items i
             WHERE i.id = $1 OR i.parent_id IN
                   (SELECT id FROM media_items WHERE parent_id = $1 AND kind = 'season'))",
    )
    .bind(item_id)
    .execute(pool)
    .await?;
    // XFER6: the item's NFO records too, or the matcher would fetch
    // the NFO's record again at the next library match.
    sqlx::query(
        "UPDATE item_nfos SET overridden_at = now()
         WHERE overridden_at IS NULL AND item_id IN (
             SELECT i.id FROM media_items i
             WHERE i.id = $1 OR i.parent_id IN
                   (SELECT id FROM media_items WHERE parent_id = $1 AND kind = 'season'))",
    )
    .bind(item_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ------------------------------------------------------------ the writer

/// What one rewrite did.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SidecarWriteReport {
    /// Files written (new or changed).
    pub written: i32,
    /// Files that already held exactly these bytes.
    pub unchanged: i32,
    /// `path: reason` for each file that couldn't be written (a
    /// sidecar or an NFO).
    pub errors: Vec<String>,
    /// NFO files written (new, or Parlour's own replaced) (XFER6).
    pub nfos_written: i32,
    /// NFO files that already held exactly these bytes.
    pub nfos_unchanged: i32,
    /// `path: reason` for each NFO deliberately not written: one
    /// Parlour didn't write is already there.
    pub nfos_left_alone: Vec<String>,
}

type ItemFields = (
    String,
    Option<i32>,
    Option<NaiveDate>,
    Option<String>,
    Vec<String>,
    Option<String>,
    Option<f64>,
    Option<i32>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

/// A credit as stored: name, person's tmdb id, role, character, order.
type CreditRow = (String, Option<i64>, String, Option<String>, i32);

fn some_text(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

/// An item's metadata as a sidecar (without segments), from the
/// database. Values a sidecar can't hold (an empty overview, a zero
/// runtime) are left out rather than written.
async fn document_from_db(
    conn: &mut PgConnection,
    target: &Target,
) -> Result<Sidecar, sqlx::Error> {
    let (title, year, air_date, overview, genres, content_rating, vote, runtime, tmdb, tvdb, imdb): ItemFields =
        sqlx::query_as(
            "SELECT title, year, air_date, overview, genres, content_rating, vote_average,
                    runtime_minutes, tmdb_id, tvdb_id, imdb_id
             FROM media_items WHERE id = $1",
        )
        .bind(target.item_id)
        .fetch_one(&mut *conn)
        .await?;
    let people: Vec<CreditRow> = sqlx::query_as(
        "SELECT p.name, p.tmdb_id, ip.role, ip.character_name, ip.sort_order
         FROM item_people ip JOIN people p ON p.id = ip.person_id
         WHERE ip.item_id = $1
         ORDER BY ip.sort_order, ip.role, p.name",
    )
    .bind(target.item_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut seen = HashSet::new();
    let genres: Vec<String> = genres
        .into_iter()
        .filter(|g| !g.trim().is_empty() && seen.insert(g.clone()))
        .collect();
    let mut doc = Sidecar::new(target.kind);
    doc.title = some_text(Some(title));
    doc.year = year.filter(|y| *y > 0);
    if target.kind == SidecarKind::Episode {
        doc.season_number = target.season_number.filter(|n| *n >= 0);
        doc.episode_number = target.episode_number.filter(|n| *n >= 0);
    }
    doc.air_date = air_date;
    doc.overview = some_text(overview);
    doc.genres = (!genres.is_empty()).then_some(genres);
    doc.content_rating = some_text(content_rating);
    doc.vote_average = vote.filter(|v| (0.0..=10.0).contains(v));
    doc.runtime_minutes = runtime.filter(|m| *m > 0);
    doc.tmdb_id = tmdb.filter(|id| *id > 0);
    doc.tvdb_id = tvdb.filter(|id| *id > 0);
    doc.imdb_id = some_text(imdb);
    let people: Vec<SidecarPerson> = people
        .into_iter()
        .filter(|(name, _, role, _, _)| !name.trim().is_empty() && !role.trim().is_empty())
        .map(
            |(name, tmdb_id, role, character_name, sort_order)| SidecarPerson {
                name,
                role,
                character_name: some_text(character_name),
                sort_order: Some(sort_order),
                tmdb_id: tmdb_id.filter(|id| *id > 0),
            },
        )
        .collect();
    doc.people = (!people.is_empty()).then_some(people);
    Ok(doc)
}

async fn manual_segments(
    conn: &mut PgConnection,
    item_id: Uuid,
) -> Result<Vec<SidecarSegment>, sqlx::Error> {
    let rows: Vec<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT kind, start_ms, end_ms FROM media_segments
         WHERE item_id = $1 AND source = 'manual'
         ORDER BY CASE kind WHEN 'recap' THEN 0 WHEN 'intro' THEN 1 ELSE 2 END",
    )
    .bind(item_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, start_ms, end_ms)| SidecarSegment {
            kind,
            start_ms,
            end_ms,
        })
        .collect())
}

/// The metadata job's rewrite (XFER4): write the sidecars of a
/// matched movie, or of a matched series and its episodes, from what
/// the database now holds — **only** when the item's library has
/// opted in (`libraries.write_sidecars`, D14; off by default, and
/// nothing on disk is even looked at otherwise). A file that already
/// holds the same bytes is left alone, so re-running the job writes
/// nothing. Each write is atomic. A file that can't be written is
/// reported and the rest carry on.
pub async fn write_item_sidecars(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<SidecarWriteReport, sqlx::Error> {
    let mut report = SidecarWriteReport::default();
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
    let mut conn = pool.acquire().await?;
    let targets = load_targets(&mut conn, library_id, Some(item_id)).await?;
    let roots: Vec<String> =
        sqlx::query_scalar("SELECT path FROM library_paths WHERE library_id = $1")
            .bind(library_id)
            .fetch_all(&mut *conn)
            .await?;
    for target in targets {
        let base = document_from_db(&mut conn, &target).await?;
        let problems = base.problems();
        if !problems.is_empty() {
            // The database holds something no sidecar may carry;
            // write nothing rather than a file the reader refuses.
            for f in &target.files {
                report
                    .errors
                    .push(format!("{}: {}", f.path.display(), problems.join("; ")));
            }
            continue;
        }
        let segments = if target.kind == SidecarKind::Series {
            Vec::new()
        } else {
            manual_segments(&mut conn, target.item_id).await?
        };
        let mut ok = true;
        for f in &target.files {
            let mut doc = base.clone();
            if let (Some(size), false) = (f.playback_size, segments.is_empty()) {
                doc.segments = Some(SidecarSegments {
                    size_bytes: size,
                    entries: segments.clone(),
                });
            }
            match write_atomic(&f.path, &to_bytes(&doc)) {
                Ok(WriteOutcome::Written) => report.written += 1,
                Ok(WriteOutcome::Unchanged) => report.unchanged += 1,
                Err(e) => {
                    ok = false;
                    report.errors.push(format!("{}: {e}", f.path.display()));
                }
            }
        }
        if ok && !target.files.is_empty() {
            let paths: Vec<PathBuf> = target.files.iter().map(|f| f.path.clone()).collect();
            store(&mut conn, target.item_id, &stat_fingerprint(&paths), &base).await?;
        }
        // XFER6: the `.nfo` beside the `.parlour.json`, same opt-in.
        super::nfo_sync::write_target_nfos(pool, &mut conn, &target, &base, &roots, &mut report)
            .await?;
    }
    Ok(report)
}
