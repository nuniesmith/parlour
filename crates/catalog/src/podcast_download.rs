//! Episode download and retention (POD3, D32), into the podcasts
//! library's writable store.
//!
//! - **The store.** A podcasts library's folder (its `library_paths`
//!   row) is the writable podcast store D32 describes: a folder Parlour
//!   owns, set up as an explicit ops step, never a video or book media
//!   mount. A show's episodes go in its own folder inside it
//!   (`podcasts.folder_path`), named from the show's title the first
//!   time something is downloaded into it. Parlour never creates the
//!   store itself, and refuses a store that overlaps another library's
//!   folder.
//! - **A download** goes to a hidden temporary name in the show's
//!   folder, is probed with ffprobe the way the scanner probes book
//!   tracks (`audiobooks::probe_audio`), and is renamed into place only
//!   when the whole file arrived and holds audio. Only then are the
//!   episode's `file_*` columns and `downloaded_at` filled, together.
//!   A failure leaves the row undownloaded and no file behind.
//! - **New episodes** (the scheduled path) are, as in Audiobookshelf,
//!   the undownloaded episodes published after the newest downloaded
//!   one, or, when none is downloaded, after the show's previous check
//!   (or its creation): newest first, up to the show's
//!   `max_new_episodes_per_check` (0 = no cap).
//! - **Retention.** When a show holds more dated downloaded episodes
//!   than its `max_episodes_to_keep` (0 = keep all), the oldest one's
//!   file is deleted and its row goes back to "not downloaded" (its
//!   progress stays). At most one per check.
//!
//! An enclosure URL can carry a private token (paid feeds put one
//! there), so it is never written to the log or a job's error list;
//! request errors drop it (`without_url`). The rules are written in
//! `docs/AUDIOBOOKS-PODCASTS.md` → "Episode download and retention
//! (POD3)".

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::audiobooks::{probe_audio, AudioProbe};

/// A download larger than this is refused (a sanity limit, not D32's
/// disk cap, which doesn't exist).
pub const MAX_EPISODE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// A download that sends nothing for this long has stalled.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest one download may take, all told.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
/// The longest file or folder name Parlour makes, in bytes (before the
/// extension and any suffix).
const MAX_NAME_BYTES: usize = 150;

/// Extensions taken from an enclosure URL as they are.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "m4b", "aac", "mp4", "ogg", "oga", "opus", "flac", "wav", "mka", "webm",
];

// ------------------------------------------------------------ names

/// A name that is safe as one file or folder name: no path separators,
/// no characters Windows or SMB shares refuse, no control characters,
/// white space folded, no leading or trailing dots or spaces (so never
/// hidden, never `.` or `..`), at most 150 bytes. `fallback` when
/// nothing is left.
pub fn safe_name(raw: &str, fallback: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let folded = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name = folded
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();
    if name.len() > MAX_NAME_BYTES {
        let mut end = MAX_NAME_BYTES;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
        name = name
            .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
            .to_string();
    }
    if name.is_empty() {
        fallback.to_string()
    } else {
        name
    }
}

/// The extension an enclosure URL's path ends in, when it is a known
/// audio one (lowercased). The query string is ignored.
pub fn url_extension(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let last = parsed.path_segments()?.next_back()?;
    let (_, ext) = last.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    AUDIO_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// The extension for a probed container (ffprobe's first format name).
fn container_extension(container: &str) -> Option<&'static str> {
    Some(match container {
        "mp3" => "mp3",
        "mov" | "mp4" | "m4a" => "m4a",
        "aac" => "aac",
        "ogg" => "ogg",
        "flac" => "flac",
        "wav" => "wav",
        "matroska" => "mka",
        _ => return None,
    })
}

/// The extension for a MIME type.
fn mime_extension(mime: &str) -> Option<&'static str> {
    let base = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match base.as_str() {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/x-m4b" => "m4b",
        "audio/aac" | "audio/aacp" => "aac",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        _ => return None,
    })
}

/// An episode file's extension: the enclosure URL's, when it is a known
/// audio one; else the probed container's; else the MIME type's; else
/// `audio`.
pub fn episode_extension(
    enclosure_url: &str,
    container: Option<&str>,
    mime: Option<&str>,
) -> String {
    url_extension(enclosure_url)
        .or_else(|| container.and_then(container_extension).map(str::to_string))
        .or_else(|| mime.and_then(mime_extension).map(str::to_string))
        .unwrap_or_else(|| "audio".to_string())
}

/// Does the store overlap one of these folders (the same folder, one
/// inside the other)? The first overlapping one's library name.
pub fn overlapping<'a>(store: &Path, others: &'a [(String, String)]) -> Option<&'a str> {
    others.iter().find_map(|(name, path)| {
        let other = Path::new(path);
        (store.starts_with(other) || other.starts_with(store)).then_some(name.as_str())
    })
}

// ------------------------------------------------------------ choices

/// A downloaded (or downloadable) episode as retention and the new-
/// episode rule see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatedEpisode {
    pub id: Uuid,
    pub guid: String,
    pub published_at: Option<DateTime<Utc>>,
}

/// The episode retention removes: when the show keeps a limit (a
/// `keep` above 0) and holds more dated downloaded episodes than that,
/// the oldest by publish date (then guid). Episodes without a date are
/// neither counted nor removed, as in Audiobookshelf.
pub fn retention_pick(downloaded: &[DatedEpisode], keep: i32) -> Option<Uuid> {
    if keep <= 0 {
        return None;
    }
    let dated: Vec<&DatedEpisode> = downloaded
        .iter()
        .filter(|e| e.published_at.is_some())
        .collect();
    if dated.len() <= keep as usize {
        return None;
    }
    dated
        .into_iter()
        .min_by(|a, b| (a.published_at, &a.guid).cmp(&(b.published_at, &b.guid)))
        .map(|e| e.id)
}

/// The episodes a scheduled check downloads: undownloaded ones
/// published after `cutoff`, newest first (then guid), at most `limit`
/// (0 = no cap). Undated episodes are never new.
pub fn pick_new(candidates: &[DatedEpisode], cutoff: DateTime<Utc>, limit: i32) -> Vec<Uuid> {
    let mut new: Vec<&DatedEpisode> = candidates
        .iter()
        .filter(|e| e.published_at.is_some_and(|p| p > cutoff))
        .collect();
    new.sort_by(|a, b| {
        b.published_at
            .cmp(&a.published_at)
            .then_with(|| a.guid.cmp(&b.guid))
    });
    let take = if limit <= 0 {
        new.len()
    } else {
        limit as usize
    };
    new.into_iter().take(take).map(|e| e.id).collect()
}

/// The audio stream whose codec an episode is listed with: the default
/// one, else the first.
fn audio_codec(probe: &AudioProbe) -> Option<String> {
    probe
        .streams
        .iter()
        .find(|s| s.is_default)
        .or_else(|| probe.streams.first())
        .and_then(|s| s.codec.clone())
}

// ------------------------------------------------------------ the store

/// The show's folder in the store, made (and named on the show's row)
/// the first time. `Ok(Err(reason))` when episodes can't be written
/// for the show; only a database error is an `Err`.
pub async fn show_folder(
    pool: &parlour_db::PgPool,
    podcast_id: Uuid,
) -> Result<Result<PathBuf, String>, sqlx::Error> {
    let row: Option<(String, Option<String>, Uuid, String)> = sqlx::query_as(
        "SELECT p.title, p.folder_path, p.library_id, l.kind
         FROM podcasts p JOIN libraries l ON l.id = p.library_id
         WHERE p.id = $1",
    )
    .bind(podcast_id)
    .fetch_optional(pool)
    .await?;
    let Some((title, folder_path, library_id, kind)) = row else {
        return Ok(Err("the show no longer exists".to_string()));
    };
    if kind != "podcasts" {
        return Ok(Err(
            "the show's library is not a podcasts library".to_string()
        ));
    }
    let others: Vec<(String, String)> = sqlx::query_as(
        "SELECT l.name, lp.path FROM library_paths lp
         JOIN libraries l ON l.id = lp.library_id
         WHERE l.kind <> 'podcasts'
         ORDER BY l.name, lp.path",
    )
    .fetch_all(pool)
    .await?;

    if let Some(folder) = folder_path {
        let folder = PathBuf::from(folder);
        if let Some(name) = overlapping(&folder, &others) {
            return Ok(Err(format!(
                "the show's folder {} overlaps the {name} library's folder; \
                 podcast downloads need a folder of their own",
                folder.display()
            )));
        }
        // The folder was named before; make it again if it went away
        // (its parent, the store, must still be there).
        return Ok(match tokio::fs::create_dir(&folder).await {
            Ok(()) => Ok(folder),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(folder),
            Err(e) => Err(format!("could not make {}: {e}", folder.display())),
        });
    }

    let root: Option<(String,)> = sqlx::query_as(
        "SELECT path FROM library_paths WHERE library_id = $1 ORDER BY path LIMIT 1",
    )
    .bind(library_id)
    .fetch_optional(pool)
    .await?;
    let Some((root,)) = root else {
        return Ok(Err(
            "the podcasts library has no folder to download into".to_string()
        ));
    };
    let root = PathBuf::from(root);
    if let Some(name) = overlapping(&root, &others) {
        return Ok(Err(format!(
            "the podcasts library's folder {} overlaps the {name} library's folder; \
             podcast downloads need a folder of their own",
            root.display()
        )));
    }
    match tokio::fs::metadata(&root).await {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Ok(Err(format!("{} is not a folder", root.display()))),
        Err(e) => {
            return Ok(Err(format!(
                "the podcasts library's folder {} can't be used: {e}",
                root.display()
            )))
        }
    }

    let base = safe_name(&title, "Podcast");
    for n in 1..=100u32 {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base} ({n})")
        };
        let candidate = root.join(&name);
        let candidate_str = candidate.to_string_lossy().into_owned();
        let (taken,): (bool,) =
            sqlx::query_as("SELECT EXISTS (SELECT 1 FROM podcasts WHERE folder_path = $1)")
                .bind(&candidate_str)
                .fetch_one(pool)
                .await?;
        if taken {
            continue;
        }
        match tokio::fs::create_dir(&candidate).await {
            Ok(()) => {}
            // A folder already on disk isn't Parlour's to share.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Ok(Err(format!("could not make {}: {e}", candidate.display()))),
        }
        let done = sqlx::query(
            "UPDATE podcasts SET folder_path = $2 WHERE id = $1 AND folder_path IS NULL",
        )
        .bind(podcast_id)
        .bind(&candidate_str)
        .execute(pool)
        .await;
        match done {
            Ok(d) if d.rows_affected() == 1 => return Ok(Ok(candidate)),
            Ok(_) => {
                // Named meanwhile (or the show went): use what is stored.
                let _ = tokio::fs::remove_dir(&candidate).await;
                return Box::pin(show_folder(pool, podcast_id)).await;
            }
            Err(e) => {
                let _ = tokio::fs::remove_dir(&candidate).await;
                return Err(e);
            }
        }
    }
    Ok(Err(format!(
        "no free folder name for the show in {}",
        root.display()
    )))
}

// ------------------------------------------------------------ download

/// (show, title, enclosure URL, enclosure type, file path) of an
/// episode about to be downloaded.
type EpisodeToFetch = (Uuid, String, String, Option<String>, Option<String>);
/// (id, guid, published, title, file path) of a downloaded episode.
type DownloadedRow = (Uuid, String, Option<DateTime<Utc>>, String, String);

/// How downloading one episode ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DownloadOutcome {
    Downloaded {
        path: String,
        size_bytes: i64,
    },
    /// It already had a file; nothing was fetched.
    AlreadyDownloaded,
    Failed {
        error: String,
    },
}

fn request_error(e: reqwest::Error) -> String {
    // The URL is dropped from the message (it may hold a token).
    let e = e.without_url();
    if e.is_timeout() {
        "could not download the episode: it took too long".to_string()
    } else if e.is_connect() {
        "could not download the episode: could not connect".to_string()
    } else {
        format!("could not download the episode: {e}")
    }
}

/// GET the enclosure into `tmp` (a new file). The size on success; the
/// caller removes `tmp` on failure. Returns the response's MIME type
/// too.
async fn fetch_to(
    http: &reqwest::Client,
    url: &str,
    tmp: &Path,
) -> Result<(u64, Option<String>), String> {
    let mut res = http
        .get(url)
        .header(reqwest::header::ACCEPT, "audio/*, */*;q=0.5")
        .send()
        .await
        .map_err(request_error)?;
    let status = res.status().as_u16();
    if status != 200 {
        return Err(format!("the enclosure answered HTTP {status}"));
    }
    if res.content_length().is_some_and(|n| n > MAX_EPISODE_BYTES) {
        return Err(format!(
            "the episode is larger than {} GB",
            MAX_EPISODE_BYTES / (1024 * 1024 * 1024)
        ));
    }
    let mime = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
        .await
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    let mut size: u64 = 0;
    loop {
        let chunk = match tokio::time::timeout(STALL_TIMEOUT, res.chunk()).await {
            Err(_) => return Err("could not download the episode: it stalled".to_string()),
            Ok(Err(e)) => return Err(request_error(e)),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => chunk,
        };
        size += chunk.len() as u64;
        if size > MAX_EPISODE_BYTES {
            return Err(format!(
                "the episode is larger than {} GB",
                MAX_EPISODE_BYTES / (1024 * 1024 * 1024)
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    }
    let synced = match file.flush().await {
        Ok(()) => file.sync_all().await,
        Err(e) => Err(e),
    };
    synced.map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    if size == 0 {
        return Err("the download was empty".to_string());
    }
    Ok((size, mime))
}

/// Pick the episode's file name in `folder`: the title as a safe name,
/// then with the episode's short id, then its full id, whichever is
/// neither on disk nor another episode's file.
async fn free_episode_path(
    pool: &parlour_db::PgPool,
    folder: &Path,
    episode_id: Uuid,
    title: &str,
    ext: &str,
) -> Result<Option<PathBuf>, sqlx::Error> {
    let base = safe_name(title, "Episode");
    let simple = episode_id.simple().to_string();
    let candidates = [
        format!("{base}.{ext}"),
        format!("{base} [{}].{ext}", &simple[..8]),
        format!("{base} [{simple}].{ext}"),
    ];
    for name in candidates {
        let path = folder.join(name);
        if tokio::fs::symlink_metadata(&path).await.is_ok() {
            continue;
        }
        let (taken,): (bool,) =
            sqlx::query_as("SELECT EXISTS (SELECT 1 FROM podcast_episodes WHERE file_path = $1)")
                .bind(path.to_string_lossy().as_ref())
                .fetch_one(pool)
                .await?;
        if !taken {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Download one episode now, whatever its show's settings say. `None`
/// when there is no such episode. Only a database error is an `Err`.
pub async fn download_episode(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    episode_id: Uuid,
) -> Result<Option<DownloadOutcome>, sqlx::Error> {
    let row: Option<EpisodeToFetch> = sqlx::query_as(
        "SELECT podcast_id, title, enclosure_url, enclosure_type, file_path
         FROM podcast_episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await?;
    let Some((podcast_id, title, enclosure_url, enclosure_type, file_path)) = row else {
        return Ok(None);
    };
    if file_path.is_some() {
        return Ok(Some(DownloadOutcome::AlreadyDownloaded));
    }
    let folder = match show_folder(pool, podcast_id).await? {
        Ok(folder) => folder,
        Err(error) => return Ok(Some(DownloadOutcome::Failed { error })),
    };

    // 1. Fetch to a hidden temporary name in the show's folder.
    let tmp = folder.join(format!(".{}.part", episode_id.simple()));
    let _ = tokio::fs::remove_file(&tmp).await; // a crash's leftover
    let fetched =
        match tokio::time::timeout(DOWNLOAD_TIMEOUT, fetch_to(http, &enclosure_url, &tmp)).await {
            Ok(r) => r,
            Err(_) => Err("could not download the episode: it took too long".to_string()),
        };
    let (size, mime) = match fetched {
        Ok(v) => v,
        Err(error) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Ok(Some(DownloadOutcome::Failed { error }));
        }
    };

    // 2. Probe it the way the scanner probes book tracks.
    let probe_path = tmp.clone();
    let probed = tokio::task::spawn_blocking(move || probe_audio(&probe_path))
        .await
        .unwrap_or_else(|e| Err(format!("the probe stopped: {e}")));
    let probe = match probed {
        Ok(p) if !p.streams.is_empty() => p,
        Ok(_) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Ok(Some(DownloadOutcome::Failed {
                error: "the download holds no audio ffprobe can read".to_string(),
            }));
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Ok(Some(DownloadOutcome::Failed {
                error: format!("the download could not be read: {e}"),
            }));
        }
    };

    // 3. Rename it into place under a name of its own.
    let ext = episode_extension(
        &enclosure_url,
        probe.container.as_deref(),
        mime.as_deref().or(enclosure_type.as_deref()),
    );
    let dest = match free_episode_path(pool, &folder, episode_id, &title, &ext).await {
        Ok(Some(dest)) => dest,
        Ok(None) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Ok(Some(DownloadOutcome::Failed {
                error: format!("no free file name for the episode in {}", folder.display()),
            }));
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    };
    if let Err(e) = tokio::fs::rename(&tmp, &dest).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Ok(Some(DownloadOutcome::Failed {
            error: format!("could not move the download to {}: {e}", dest.display()),
        }));
    }

    // 4. Fill the file columns and downloaded_at together.
    let dest_str = dest.to_string_lossy().into_owned();
    let size_bytes = i64::try_from(size).unwrap_or(i64::MAX);
    let done = sqlx::query(
        "UPDATE podcast_episodes SET
             file_path = $2, file_size_bytes = $3, file_duration_ms = $4,
             file_container = $5, file_codec = $6, downloaded_at = now(), updated_at = now()
         WHERE id = $1 AND file_path IS NULL",
    )
    .bind(episode_id)
    .bind(&dest_str)
    .bind(size_bytes)
    .bind(probe.duration_ms)
    .bind(probe.container.as_deref())
    .bind(audio_codec(&probe))
    .execute(pool)
    .await;
    match done {
        Ok(d) if d.rows_affected() == 1 => Ok(Some(DownloadOutcome::Downloaded {
            path: dest_str,
            size_bytes,
        })),
        Ok(_) => {
            // Downloaded meanwhile, or the episode went: this copy is
            // nobody's.
            let _ = tokio::fs::remove_file(&dest).await;
            Ok(Some(DownloadOutcome::AlreadyDownloaded))
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            Err(e)
        }
    }
}

/// The new episodes a scheduled check of this show downloads (see
/// `pick_new`). The cutoff is the newest downloaded episode's publish
/// date; when nothing is downloaded, `previous_check` (the show's
/// `last_checked_at` before this check), else the show's creation.
pub async fn new_episodes(
    pool: &parlour_db::PgPool,
    podcast_id: Uuid,
    previous_check: Option<DateTime<Utc>>,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let show: Option<(DateTime<Utc>, i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT p.created_at, p.max_new_episodes_per_check,
                (SELECT max(e.published_at) FROM podcast_episodes e
                 WHERE e.podcast_id = p.id AND e.file_path IS NOT NULL)
         FROM podcasts p WHERE p.id = $1",
    )
    .bind(podcast_id)
    .fetch_optional(pool)
    .await?;
    let Some((created_at, limit, newest_downloaded)) = show else {
        return Ok(Vec::new());
    };
    let cutoff = newest_downloaded.or(previous_check).unwrap_or(created_at);
    let rows: Vec<(Uuid, String, Option<DateTime<Utc>>, String)> = sqlx::query_as(
        "SELECT id, guid, published_at, title FROM podcast_episodes
         WHERE podcast_id = $1 AND file_path IS NULL AND published_at > $2",
    )
    .bind(podcast_id)
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    let candidates: Vec<DatedEpisode> = rows
        .iter()
        .map(|(id, guid, published_at, _)| DatedEpisode {
            id: *id,
            guid: guid.clone(),
            published_at: *published_at,
        })
        .collect();
    let picked = pick_new(&candidates, cutoff, limit);
    Ok(picked
        .into_iter()
        .filter_map(|id| rows.iter().find(|r| r.0 == id).map(|r| (id, r.3.clone())))
        .collect())
}

// ------------------------------------------------------------ retention

/// What retention did on one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RetentionOutcome {
    /// The episode's file was deleted and its row is "not downloaded"
    /// again.
    Removed { episode_id: Uuid, title: String },
    /// The file couldn't be deleted; the row was left as it was.
    Failed {
        episode_id: Uuid,
        title: String,
        error: String,
    },
}

/// Apply the show's `max_episodes_to_keep` once: at most one episode
/// removed (`retention_pick`). `None` when nothing is over the limit.
/// The file is deleted first; the row is cleared only once the file is
/// gone (or was already gone).
pub async fn apply_retention(
    pool: &parlour_db::PgPool,
    podcast_id: Uuid,
) -> Result<Option<RetentionOutcome>, sqlx::Error> {
    let keep: Option<(i32,)> =
        sqlx::query_as("SELECT max_episodes_to_keep FROM podcasts WHERE id = $1")
            .bind(podcast_id)
            .fetch_optional(pool)
            .await?;
    let Some((keep,)) = keep else {
        return Ok(None);
    };
    if keep <= 0 {
        return Ok(None);
    }
    let rows: Vec<DownloadedRow> = sqlx::query_as(
        "SELECT id, guid, published_at, title, file_path FROM podcast_episodes
         WHERE podcast_id = $1 AND file_path IS NOT NULL",
    )
    .bind(podcast_id)
    .fetch_all(pool)
    .await?;
    let downloaded: Vec<DatedEpisode> = rows
        .iter()
        .map(|(id, guid, published_at, _, _)| DatedEpisode {
            id: *id,
            guid: guid.clone(),
            published_at: *published_at,
        })
        .collect();
    let Some(victim) = retention_pick(&downloaded, keep) else {
        return Ok(None);
    };
    let Some((_, _, _, title, path)) = rows.into_iter().find(|r| r.0 == victim) else {
        return Ok(None);
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Ok(Some(RetentionOutcome::Failed {
                episode_id: victim,
                title,
                error: format!("could not delete {path}: {e}"),
            }))
        }
    }
    sqlx::query(
        "UPDATE podcast_episodes SET
             file_path = NULL, file_size_bytes = NULL, file_duration_ms = NULL,
             file_container = NULL, file_codec = NULL, downloaded_at = NULL,
             updated_at = now()
         WHERE id = $1 AND file_path = $2",
    )
    .bind(victim)
    .bind(&path)
    .execute(pool)
    .await?;
    Ok(Some(RetentionOutcome::Removed {
        episode_id: victim,
        title,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32) -> Option<DateTime<Utc>> {
        Some(Utc.with_ymd_and_hms(2026, 11, day, 7, 0, 0).unwrap())
    }

    fn ep(n: u128, guid: &str, published_at: Option<DateTime<Utc>>) -> DatedEpisode {
        DatedEpisode {
            id: Uuid::from_u128(n),
            guid: guid.into(),
            published_at,
        }
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("The Lantern Hour", "x"), "The Lantern Hour");
        assert_eq!(safe_name("AC/DC: Live?", "x"), "AC DC Live");
        assert_eq!(safe_name("  ..hidden.. ", "x"), "hidden");
        assert_eq!(safe_name("..", "Podcast"), "Podcast");
        assert_eq!(safe_name("a\tb\nc\u{0}d", "x"), "a b c d");
        assert_eq!(safe_name("<>|*", "Episode"), "Episode");
        assert_eq!(safe_name("Café — Ep. 3", "x"), "Café — Ep. 3");
        let long = "é".repeat(200);
        let cut = safe_name(&long, "x");
        assert!(cut.len() <= MAX_NAME_BYTES, "{}", cut.len());
        assert!(cut.chars().all(|c| c == 'é'));
    }

    #[test]
    fn extensions() {
        assert_eq!(
            url_extension("https://cdn.example/a/ep3.MP3?token=abc&n=3").as_deref(),
            Some("mp3")
        );
        assert_eq!(url_extension("https://cdn.example/a/ep3"), None);
        assert_eq!(url_extension("https://cdn.example/a/ep3.php?f=x.mp3"), None);
        assert_eq!(url_extension("not a url"), None);
        // The URL's own extension wins, then the probe, then the MIME type.
        assert_eq!(
            episode_extension("https://x/e.m4b", Some("mov"), None),
            "m4b"
        );
        assert_eq!(
            episode_extension("https://x/play?id=1", Some("mov"), Some("audio/mpeg")),
            "m4a"
        );
        assert_eq!(
            episode_extension(
                "https://x/play?id=1",
                Some("weird"),
                Some("audio/mpeg; x=1")
            ),
            "mp3"
        );
        assert_eq!(episode_extension("https://x/play", None, None), "audio");
    }

    #[test]
    fn the_store_must_not_overlap_another_library() {
        let others = vec![
            ("Films".to_string(), "/media/films".to_string()),
            ("Books".to_string(), "/media/books/".to_string()),
        ];
        assert_eq!(
            overlapping(Path::new("/media/films"), &others),
            Some("Films")
        );
        assert_eq!(
            overlapping(Path::new("/media/books/podcasts"), &others),
            Some("Books")
        );
        assert_eq!(overlapping(Path::new("/media"), &others), Some("Films"));
        assert_eq!(overlapping(Path::new("/podcasts"), &others), None);
        // Component-wise, not by text.
        assert_eq!(overlapping(Path::new("/media/films-pods"), &others), None);
    }

    #[test]
    fn retention_removes_the_oldest_dated_one_over_the_limit() {
        let eps = [
            ep(1, "b", at(3)),
            ep(2, "a", at(3)),
            ep(3, "c", at(9)),
            ep(4, "undated", None),
        ];
        // Keep all.
        assert_eq!(retention_pick(&eps, 0), None);
        // Three dated, keep 3: nothing over.
        assert_eq!(retention_pick(&eps, 3), None);
        // Keep 2: the oldest, the guid breaking the tie.
        assert_eq!(retention_pick(&eps, 2), Some(Uuid::from_u128(2)));
        assert_eq!(retention_pick(&eps, 1), Some(Uuid::from_u128(2)));
        // Undated ones are never picked or counted.
        assert_eq!(
            retention_pick(&[ep(4, "u", None), ep(5, "v", None)], 1),
            None
        );
    }

    #[test]
    fn new_episodes_are_newest_first_after_the_cutoff() {
        let eps = [
            ep(1, "old", at(1)),
            ep(2, "a", at(5)),
            ep(3, "b", at(7)),
            ep(4, "c", at(7)),
            ep(5, "undated", None),
        ];
        let cutoff = at(2).unwrap();
        let ids = |v: Vec<Uuid>| v.into_iter().map(|u| u.as_u128()).collect::<Vec<_>>();
        assert_eq!(ids(pick_new(&eps, cutoff, 3)), vec![3, 4, 2]);
        assert_eq!(ids(pick_new(&eps, cutoff, 2)), vec![3, 4]);
        // 0 = no cap.
        assert_eq!(ids(pick_new(&eps, cutoff, 0)), vec![3, 4, 2]);
        // Strictly after the cutoff.
        assert_eq!(ids(pick_new(&eps, at(7).unwrap(), 0)), Vec::<u128>::new());
    }
}
