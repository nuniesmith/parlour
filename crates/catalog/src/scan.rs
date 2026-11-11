//! The scanner (C6 + C8).
//!
//! One library scan:
//! 1. Walk every root path (walker errors are recorded per root).
//! 2. For each video file, compare the (size, mtime) fingerprint
//!    with the stored row — unchanged files are skipped without
//!    probing. Changed/new files are probed and their streams and
//!    subtitle tracks replaced.
//! 3. Items are upserted by `stable_key` (parsed series/season/
//!    episode structure, or the movie grouping), so re-scanning
//!    the same tree changes nothing: the scan is idempotent.
//! 4. Files that vanished are marked unavailable — but only under
//!    roots that walked cleanly in this run, so an unmounted disk
//!    can never mass-mark a library missing. Rows are never
//!    deleted: watch state (1.6) hangs off items, not files.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::parse::{normalize_key, parse_file_name, sort_title};
use crate::probe::probe_file;
use crate::walk::{walk_library, SidecarSubtitle};

#[derive(Debug, Default, Serialize)]
pub struct ScanReport {
    pub seen_files: i32,
    pub unchanged_files: i32,
    pub probed_files: i32,
    pub created_items: i32,
    pub missing_files: i32,
    pub skipped_samples: i32,
    pub skipped_extras: i32,
    pub errors: Vec<String>,
}

/// mtime at Postgres precision (microseconds) so fingerprint
/// comparisons are exact.
fn mtime_utc(t: std::time::SystemTime) -> DateTime<Utc> {
    let dt: DateTime<Utc> = t.into();
    DateTime::from_timestamp_micros(dt.timestamp_micros()).unwrap_or(dt)
}

struct ItemSpec<'a> {
    parent_id: Option<Uuid>,
    kind: &'a str,
    stable_key: &'a str,
    title: &'a str,
    year: Option<i32>,
    season: Option<i32>,
    episode: Option<i32>,
}

async fn upsert_item(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    spec: ItemSpec<'_>,
) -> Result<(Uuid, bool), sqlx::Error> {
    let parent_id = spec.parent_id;
    let kind = spec.kind;
    let stable_key = spec.stable_key;
    let title = spec.title;
    let year = spec.year;
    let season = spec.season;
    let episode = spec.episode;
    let row: (Uuid, bool) = sqlx::query_as(
        "INSERT INTO media_items
             (library_id, parent_id, kind, stable_key, title, sort_title, year,
              season_number, episode_number)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (library_id, stable_key) DO UPDATE
             SET title = EXCLUDED.title, sort_title = EXCLUDED.sort_title,
                 year = EXCLUDED.year, parent_id = EXCLUDED.parent_id,
                 season_number = EXCLUDED.season_number,
                 episode_number = EXCLUDED.episode_number, updated_at = now()
         RETURNING id, (xmax = 0) AS inserted",
    )
    .bind(library_id)
    .bind(parent_id)
    .bind(kind)
    .bind(stable_key)
    .bind(title)
    .bind(sort_title(title))
    .bind(year)
    .bind(season)
    .bind(episode)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Resolve (creating as needed) the leaf item a file belongs to:
/// series -> season -> episode for episode files, one movie item
/// otherwise. Returns (leaf item id, number of items created).
async fn resolve_item(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    video: &crate::walk::DiscoveredVideo,
) -> Result<(Uuid, i32), sqlx::Error> {
    let parsed = parse_file_name(&video.rel_path);
    let mut created = 0;
    if let (Some(season), Some(episode)) = (parsed.season, parsed.episode) {
        let series_title = parsed
            .series_title
            .clone()
            .unwrap_or_else(|| parsed.title.clone());
        let series_key = format!(
            "series:{}:{}",
            normalize_key(&series_title),
            parsed.year.unwrap_or(0)
        );
        let (series_id, ins) = upsert_item(
            pool,
            library_id,
            ItemSpec {
                parent_id: None,
                kind: "series",
                stable_key: &series_key,
                title: &series_title,
                year: parsed.year,
                season: None,
                episode: None,
            },
        )
        .await?;
        created += ins as i32;
        let season_key = format!("{series_key}:s{season:02}");
        let season_title = format!("Season {season}");
        let (season_id, ins) = upsert_item(
            pool,
            library_id,
            ItemSpec {
                parent_id: Some(series_id),
                kind: "season",
                stable_key: &season_key,
                title: &season_title,
                year: parsed.year,
                season: Some(season),
                episode: None,
            },
        )
        .await?;
        created += ins as i32;
        let episode_key = format!("{season_key}:e{episode:03}");
        let (episode_id, ins) = upsert_item(
            pool,
            library_id,
            ItemSpec {
                parent_id: Some(season_id),
                kind: "episode",
                stable_key: &episode_key,
                title: &parsed.title,
                year: parsed.year,
                season: Some(season),
                episode: Some(episode),
            },
        )
        .await?;
        created += ins as i32;
        return Ok((episode_id, created));
    }

    // Movie: group by folder + parsed title + year (multi-part
    // files in one folder are one item; root files group by title).
    let dir = video
        .rel_path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();
    let key = format!(
        "movie:{}:{}:{}",
        normalize_key(&dir),
        normalize_key(&parsed.title),
        parsed.year.unwrap_or(0)
    );
    let (id, ins) = upsert_item(
        pool,
        library_id,
        ItemSpec {
            parent_id: None,
            kind: "movie",
            stable_key: &key,
            title: &parsed.title,
            year: parsed.year,
            season: None,
            episode: None,
        },
    )
    .await?;
    created += ins as i32;
    Ok((id, created))
}

/// Scan one library. `job_id`, when set, receives progress + errors
/// as the scan runs (C6: progress visible in admin).
pub async fn scan_library(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    job_id: Option<Uuid>,
) -> Result<ScanReport, sqlx::Error> {
    let mut report = ScanReport::default();

    let paths: Vec<(String,)> =
        sqlx::query_as("SELECT path FROM library_paths WHERE library_id = $1 ORDER BY path")
            .bind(library_id)
            .fetch_all(pool)
            .await?;
    if paths.is_empty() {
        report
            .errors
            .push("library has no paths configured".to_string());
        return Ok(report);
    }

    // Walk all roots first: totals are known before probing starts,
    // and missing-marking can be scoped to roots that walked clean.
    let mut videos = Vec::new();
    let mut sidecars: HashMap<String, Vec<SidecarSubtitle>> = HashMap::new();
    let mut walked_roots: Vec<String> = Vec::new();
    for (root,) in &paths {
        let result = walk_library(Path::new(root));
        if result.errors.is_empty() {
            walked_roots.push(root.clone());
        }
        for e in &result.errors {
            report.errors.push(format!("{root}: {e}"));
            if let Some(job) = job_id {
                let _ = parlour_jobs::add_error(pool, job, Some(root), e).await;
            }
        }
        report.skipped_samples += result.skipped_samples as i32;
        report.skipped_extras += result.skipped_extras as i32;
        for sc in result.sidecars {
            sidecars
                .entry(format!("{root}/{}", sc.video_stem_rel))
                .or_default()
                .push(sc);
        }
        videos.extend(result.videos);
    }

    let total = videos.len() as i32;
    if let Some(job) = job_id {
        let _ = parlour_jobs::set_progress(pool, job, 0, total).await;
    }

    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut done = 0i32;
    for video in &videos {
        done += 1;
        seen_paths.insert(video.abs_path.clone());
        report.seen_files += 1;
        let mtime = mtime_utc(video.mtime);
        let size = video.size_bytes as i64;

        // Fingerprint check (C3): unchanged -> no probe, just seen.
        let existing: Option<(Uuid, i64, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, size_bytes, mtime FROM media_files WHERE library_id = $1 AND path = $2",
        )
        .bind(library_id)
        .bind(&video.abs_path)
        .fetch_optional(pool)
        .await?;
        if let Some((file_id, old_size, old_mtime)) = existing {
            if old_size == size && old_mtime == mtime {
                report.unchanged_files += 1;
                sqlx::query(
                    "UPDATE media_files SET available = true, last_seen_at = now() WHERE id = $1",
                )
                .bind(file_id)
                .execute(pool)
                .await?;
                if let Some(job) = job_id {
                    let _ = parlour_jobs::set_progress(pool, job, done, total).await;
                }
                continue;
            }
        }

        let (item_id, created) = resolve_item(pool, library_id, video).await?;
        report.created_items += created;

        // Probe (C4). A probe failure still catalogues the file —
        // the item exists, probe_error says why streams are empty.
        let (probe, probe_error) = match probe_file(Path::new(&video.abs_path)) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };
        if let Some(e) = &probe_error {
            report.errors.push(format!("{}: {e}", video.rel_path));
            if let Some(job) = job_id {
                let _ = parlour_jobs::add_error(pool, job, Some(&video.rel_path), e).await;
            }
        }
        report.probed_files += 1;

        let (container, duration_ms) = match &probe {
            Some(p) => (p.container.clone(), p.duration_ms),
            None => (None, None),
        };
        let (file_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO media_files
                 (item_id, library_id, path, relative_path, size_bytes, mtime,
                  container, duration_ms, probe_error, available, last_seen_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, true, now())
             ON CONFLICT (library_id, path) DO UPDATE
                 SET item_id = EXCLUDED.item_id, relative_path = EXCLUDED.relative_path,
                     size_bytes = EXCLUDED.size_bytes, mtime = EXCLUDED.mtime,
                     container = EXCLUDED.container, duration_ms = EXCLUDED.duration_ms,
                     probe_error = EXCLUDED.probe_error, available = true,
                     last_seen_at = now(), updated_at = now()
             RETURNING id",
        )
        .bind(item_id)
        .bind(library_id)
        .bind(&video.abs_path)
        .bind(&video.rel_path)
        .bind(size)
        .bind(mtime)
        .bind(&container)
        .bind(duration_ms)
        .bind(&probe_error)
        .fetch_one(pool)
        .await?;

        // Streams/subtitles are replaced wholesale for a re-probed file.
        sqlx::query("DELETE FROM media_streams WHERE file_id = $1")
            .bind(file_id)
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM subtitle_tracks WHERE file_id = $1")
            .bind(file_id)
            .execute(pool)
            .await?;
        if let Some(p) = &probe {
            for s in &p.streams {
                sqlx::query(
                    "INSERT INTO media_streams
                         (file_id, stream_index, kind, codec, width, height, hdr_format,
                          language, title, channels, bit_rate, is_default)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
                )
                .bind(file_id)
                .bind(s.index)
                .bind(&s.kind)
                .bind(&s.codec)
                .bind(s.width)
                .bind(s.height)
                .bind(&s.hdr_format)
                .bind(&s.language)
                .bind(&s.title)
                .bind(s.channels)
                .bind(s.bit_rate)
                .bind(s.is_default)
                .execute(pool)
                .await?;
            }
            for sub in &p.subtitles {
                sqlx::query(
                    "INSERT INTO subtitle_tracks
                         (file_id, stream_index, source, format, language, title,
                          is_forced, is_default, is_sdh)
                     VALUES ($1, $2, 'embedded', $3, $4, $5, $6, $7, $8)",
                )
                .bind(file_id)
                .bind(sub.index)
                .bind(&sub.format)
                .bind(&sub.language)
                .bind(&sub.title)
                .bind(sub.is_forced)
                .bind(sub.is_default)
                .bind(sub.is_sdh)
                .execute(pool)
                .await?;
            }
        }
        // Sidecar subtitles belonging to this file (same stem).
        let stem_abs = video
            .abs_path
            .rsplit_once('.')
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| video.abs_path.clone());
        if let Some(list) = sidecars.get(&stem_abs) {
            for sc in list {
                sqlx::query(
                    "INSERT INTO subtitle_tracks
                         (file_id, source, path, format, language, is_forced, is_sdh)
                     VALUES ($1, 'sidecar', $2, $3, $4, $5, $6)",
                )
                .bind(file_id)
                .bind(&sc.abs_path)
                .bind(&sc.format)
                .bind(&sc.language)
                .bind(sc.is_forced)
                .bind(sc.is_sdh)
                .execute(pool)
                .await?;
            }
        }

        if let Some(job) = job_id {
            let _ = parlour_jobs::set_progress(pool, job, done, total).await;
        }
    }

    // C8: mark vanished files unavailable — only under roots that
    // walked cleanly this run. Never delete the row.
    if !walked_roots.is_empty() {
        let stored: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, path FROM media_files WHERE library_id = $1 AND available = true",
        )
        .bind(library_id)
        .fetch_all(pool)
        .await?;
        for (file_id, path) in stored {
            if seen_paths.contains(&path) {
                continue;
            }
            let under_walked_root = walked_roots
                .iter()
                .any(|r| path.starts_with(&format!("{r}/")));
            if under_walked_root {
                sqlx::query("UPDATE media_files SET available = false WHERE id = $1")
                    .bind(file_id)
                    .execute(pool)
                    .await?;
                report.missing_files += 1;
            }
        }
        // Item availability follows its files (leaves), then rolls up.
        sqlx::query(
            "UPDATE media_items i SET available = EXISTS (
                 SELECT 1 FROM media_files f WHERE f.item_id = i.id AND f.available
             ) WHERE i.library_id = $1 AND i.kind IN ('movie', 'episode')",
        )
        .bind(library_id)
        .execute(pool)
        .await?;
        sqlx::query(
            "UPDATE media_items i SET available = EXISTS (
                 SELECT 1 FROM media_items c WHERE c.parent_id = i.id AND c.available
             ) WHERE i.library_id = $1 AND i.kind = 'season'",
        )
        .bind(library_id)
        .execute(pool)
        .await?;
        sqlx::query(
            "UPDATE media_items i SET available = EXISTS (
                 SELECT 1 FROM media_items c WHERE c.parent_id = i.id AND c.available
             ) WHERE i.library_id = $1 AND i.kind = 'series'",
        )
        .bind(library_id)
        .execute(pool)
        .await?;
    }

    Ok(report)
}
