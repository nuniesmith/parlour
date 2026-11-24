//! Audiobook libraries (Phase 10, AB2; `docs/AUDIOBOOKS-PODCASTS.md`).
//!
//! An audiobooks library is scanned into the shared catalog, not a
//! second one: each book is a top-level `media_items` row of kind
//! `audiobook`, its tracks are its `media_files` in `track_index`
//! order, its chapters are `book_chapters` rows on the book-global
//! timeline, and its authors / narrators are `people` linked through
//! `item_people` with roles `author` / `narrator`.
//!
//! What a book is on disk:
//! - every folder that directly holds audio files is one book;
//! - a disc folder (`CD 1`, `Disc 2`, `Disk3`, `Part 01` — nothing
//!   else in the name) belongs to the book in the folder above it,
//!   its files after the earlier discs' files;
//! - an audio file directly in a library root is a book on its own.
//!
//! Track order: tag disc number, then the disc folder's number, then
//! tag track number, then natural file-name order.
//!
//! Everything that decides — grouping, ordering, tag picks, people
//! splitting, chapters — is a pure function, unit-tested below;
//! only `probe_audio` runs ffprobe and only `scan_audiobooks` touches
//! the database.
//!
//! A book whose files are all unchanged (same paths, sizes and
//! mtimes as stored) is skipped without probing. When any file of a
//! book is new, changed or gone, every track of that book is probed
//! again and the book's metadata, track order and chapters are
//! rebuilt from scratch — book-level facts come from all tracks
//! together, so a partial update could never be right.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::parse::sort_title;
use crate::scan::{mark_missing, ScanReport};

/// Audio file extensions an audiobooks library picks up.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "m4b", "m4a", "mp3", "flac", "ogg", "oga", "opus", "aac", "wav", "wma",
];

/// Folder images used as a book's cover, in order of preference.
pub const COVER_NAMES: &[&str] = &[
    "cover.jpg",
    "cover.jpeg",
    "cover.png",
    "folder.jpg",
    "folder.jpeg",
    "folder.png",
];

// ------------------------------------------------------------------ walk

#[derive(Debug, Clone)]
pub struct DiscoveredAudio {
    pub abs_path: String,
    /// Path relative to the library root, `/`-separated.
    pub rel_path: String,
    pub size_bytes: u64,
    pub mtime: SystemTime,
}

#[derive(Debug, Default)]
pub struct AudioWalk {
    pub files: Vec<DiscoveredAudio>,
    /// Folder (relative, `""` for the root) -> absolute path of its
    /// cover image, the first of `COVER_NAMES` present.
    pub covers: HashMap<String, String>,
    pub errors: Vec<String>,
}

fn is_junk_dir(name: &str) -> bool {
    name.starts_with('.') || name == "@eaDir" || name == "#recycle"
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// Recursively list a library root's audio files and folder covers.
/// Sorted, so scans are deterministic.
pub fn walk_audio(root: &Path) -> AudioWalk {
    let mut out = AudioWalk::default();
    if !root.is_dir() {
        out.errors.push(format!(
            "library path is not a directory: {}",
            root.display()
        ));
        return out;
    }
    let mut covers: HashMap<String, (usize, String)> = HashMap::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                out.errors
                    .push(format!("cannot read {}: {e}", dir.display()));
                continue;
            }
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if path.is_dir() {
                if !is_junk_dir(&name) {
                    stack.push(path);
                }
                continue;
            }
            if name.starts_with("._") {
                continue; // macOS resource forks
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let lower = name.to_lowercase();
            if let Some(rank) = COVER_NAMES.iter().position(|c| *c == lower) {
                let folder = rel
                    .rsplit_once('/')
                    .map(|(d, _)| d.to_string())
                    .unwrap_or_default();
                let abs = path.to_string_lossy().to_string();
                let better = covers.get(&folder).map(|(r, _)| rank < *r).unwrap_or(true);
                if better {
                    covers.insert(folder, (rank, abs));
                }
                continue;
            }
            if !AUDIO_EXTENSIONS.contains(&extension(&path).as_str()) {
                continue;
            }
            match std::fs::metadata(&path) {
                Ok(meta) => out.files.push(DiscoveredAudio {
                    abs_path: path.to_string_lossy().to_string(),
                    rel_path: rel,
                    size_bytes: meta.len(),
                    mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                }),
                Err(e) => out
                    .errors
                    .push(format!("cannot stat {}: {e}", path.display())),
            }
        }
    }
    out.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out.covers = covers.into_iter().map(|(k, (_, v))| (k, v)).collect();
    out
}

// ---------------------------------------------------------- grouping

/// The disc number a folder name stands for: `CD 1`, `cd1`,
/// `Disc 02`, `Disk-3`, `Part_4` → Some(n). Anything else in the
/// name (`CD 1 - Extras`, `Part One`) → None.
pub fn disc_folder_number(name: &str) -> Option<u32> {
    let lower = name.trim().to_lowercase();
    let rest = ["disc", "disk", "part", "cd"]
        .iter()
        .find_map(|p| lower.strip_prefix(p))?;
    let digits = rest.trim_start_matches([' ', '-', '_', '.']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Which book a file belongs to: (book key, disc folder number).
/// The key is the book's folder relative to the library root, or
/// the file's own relative path for a file in the root.
pub fn book_key_for(rel_path: &str) -> (String, Option<u32>) {
    let Some((dir, _file)) = rel_path.rsplit_once('/') else {
        return (rel_path.to_string(), None);
    };
    let (parent, last) = match dir.rsplit_once('/') {
        Some((p, l)) => (p, l),
        None => ("", dir),
    };
    match disc_folder_number(last) {
        // A disc folder with a parent folds into it; a disc folder
        // straight in the root is the book itself.
        Some(n) if !parent.is_empty() => (parent.to_string(), Some(n)),
        _ => (dir.to_string(), None),
    }
}

// ------------------------------------------------------------ ordering

/// Natural order: digit runs compare as numbers ("2" < "10"),
/// everything else case-insensitively.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// Leading number of a `track` / `disc` tag: "3", "03/12" → 3.
pub fn tag_number(value: Option<&str>) -> Option<u32> {
    let v = value?.trim();
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// What ordering needs to know about one track.
#[derive(Debug, Clone)]
pub struct TrackKey<'a> {
    pub rel_path: &'a str,
    pub disc_tag: Option<u32>,
    pub disc_folder: Option<u32>,
    pub track_tag: Option<u32>,
}

/// Track order: tag disc, then disc folder, then tag track, then
/// natural file name. Missing numbers sort after present ones.
pub fn compare_tracks(a: &TrackKey<'_>, b: &TrackKey<'_>) -> Ordering {
    fn opt(a: Option<u32>, b: Option<u32>) -> Ordering {
        match (a, b) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
    let disc_a = a.disc_tag.or(a.disc_folder);
    let disc_b = b.disc_tag.or(b.disc_folder);
    opt(disc_a, disc_b)
        .then_with(|| opt(a.track_tag, b.track_tag))
        .then_with(|| natural_cmp(a.rel_path, b.rel_path))
}

// --------------------------------------------------------------- probe

/// One chapter inside a single file, in that file's own time.
#[derive(Debug, Clone, PartialEq)]
pub struct FileChapter {
    pub start_ms: i64,
    pub end_ms: i64,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStream {
    pub index: i32,
    pub codec: Option<String>,
    pub channels: Option<i32>,
    pub bit_rate: Option<i64>,
    pub language: Option<String>,
    pub is_default: bool,
}

/// What ffprobe says about one audio file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioProbe {
    pub container: Option<String>,
    pub duration_ms: Option<i64>,
    /// Format tags with lowercased keys, then the first audio
    /// stream's tags for keys the format lacked (Ogg/Opus keep their
    /// comments on the stream).
    pub tags: BTreeMap<String, String>,
    pub streams: Vec<AudioStream>,
    pub chapters: Vec<FileChapter>,
}

impl AudioProbe {
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .get(key)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
    }
}

#[derive(Deserialize)]
struct FfOut {
    format: Option<FfFormat>,
    streams: Option<Vec<FfStream>>,
    chapters: Option<Vec<FfChapter>>,
}

#[derive(Deserialize)]
struct FfFormat {
    format_name: Option<String>,
    duration: Option<String>,
    tags: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct FfStream {
    index: i32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    channels: Option<i32>,
    bit_rate: Option<String>,
    tags: Option<HashMap<String, serde_json::Value>>,
    disposition: Option<HashMap<String, i32>>,
}

#[derive(Deserialize)]
struct FfChapter {
    start_time: Option<String>,
    end_time: Option<String>,
    tags: Option<HashMap<String, serde_json::Value>>,
}

fn secs_to_ms(s: Option<&str>) -> Option<i64> {
    s.and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| (v * 1000.0).round() as i64)
}

fn lower_tags(tags: Option<HashMap<String, serde_json::Value>>) -> BTreeMap<String, String> {
    tags.unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => s,
                serde_json::Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some((k.to_lowercase(), s))
        })
        .collect()
}

/// Parse `ffprobe -print_format json -show_format -show_streams
/// -show_chapters` output for an audio file. Pure.
pub fn parse_audio_probe_json(json: &str) -> Result<AudioProbe, String> {
    let out: FfOut =
        serde_json::from_str(json).map_err(|e| format!("ffprobe output did not parse: {e}"))?;
    let mut probe = AudioProbe::default();
    if let Some(fmt) = out.format {
        probe.container = fmt
            .format_name
            .as_ref()
            .map(|n| n.split(',').next().unwrap_or(n).to_string());
        probe.duration_ms = secs_to_ms(fmt.duration.as_deref());
        probe.tags = lower_tags(fmt.tags);
    }
    let mut stream_tags_merged = false;
    for s in out.streams.unwrap_or_default() {
        if s.codec_type.as_deref() != Some("audio") {
            continue; // cover art arrives as a one-frame "video" stream
        }
        let tags = lower_tags(s.tags);
        if !stream_tags_merged {
            for (k, v) in &tags {
                probe.tags.entry(k.clone()).or_insert_with(|| v.clone());
            }
            stream_tags_merged = true;
        }
        let flag = |name: &str| {
            s.disposition
                .as_ref()
                .and_then(|d| d.get(name))
                .copied()
                .unwrap_or(0)
                == 1
        };
        probe.streams.push(AudioStream {
            index: s.index,
            codec: s.codec_name.clone(),
            channels: s.channels,
            bit_rate: s.bit_rate.as_deref().and_then(|b| b.parse().ok()),
            language: tags.get("language").cloned(),
            is_default: flag("default"),
        });
    }
    for c in out.chapters.unwrap_or_default() {
        let (Some(start), Some(end)) = (
            secs_to_ms(c.start_time.as_deref()),
            secs_to_ms(c.end_time.as_deref()),
        ) else {
            continue;
        };
        let title = lower_tags(c.tags)
            .remove("title")
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        probe.chapters.push(FileChapter {
            start_ms: start,
            end_ms: end.max(start),
            title,
        });
    }
    Ok(probe)
}

/// Run ffprobe on one audio file.
pub fn probe_audio(path: &Path) -> Result<AudioProbe, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_chapters",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("could not run ffprobe: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffprobe failed: {}", err.trim()));
    }
    parse_audio_probe_json(&String::from_utf8_lossy(&output.stdout))
}

// ------------------------------------------------------------ metadata

/// Split a people tag: always on `;`, `&` and ` / `; on `,` only
/// when every part looks like a full name (has a space), so
/// "Tolkien, J.R.R." stays one person while "Jane Doe, John Roe"
/// becomes two.
pub fn split_people(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in value.split([';', '&']).flat_map(|p| p.split(" / ")) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let commas: Vec<&str> = part
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if commas.len() > 1 && commas.iter().all(|p| p.contains(' ')) {
            out.extend(commas.into_iter().map(str::to_string));
        } else {
            out.push(part.to_string());
        }
    }
    let mut seen = HashSet::new();
    out.retain(|p| seen.insert(p.to_lowercase()));
    out
}

/// Genres from a `genre` tag, split on `;`, `,` and `/`.
pub fn split_genres(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    value
        .split([';', ',', '/'])
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_string)
        .filter(|g| seen.insert(g.to_lowercase()))
        .collect()
}

fn year_of(value: &str) -> Option<i32> {
    let v = value.trim();
    let y: i32 = v.get(0..4)?.parse().ok()?;
    (1000..=2999).contains(&y).then_some(y)
}

fn file_stem(rel_path: &str) -> String {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    name.rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(name)
        .to_string()
}

/// Book-level facts, from the tracks' tags and the folder layout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BookMeta {
    pub title: String,
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub genres: Vec<String>,
}

/// Pick a book's metadata. `tracks` are in play order; for each
/// field the first track carrying the tag wins.
///
/// - title: `album`; for a single-file book also `title`; else the
///   book folder's name (or the file's name, for a root file).
/// - authors: `album_artist`, else `artist`; else the top-level
///   folder when the book sits in one (`Author/Title`,
///   `Author/Series/Title`).
/// - narrators: `composer`.
/// - year: the first four digits of `date`, else `year`.
/// - overview: `description`, else `comment`.
/// - genres: `genre`.
pub fn book_meta(book_key: &str, root_file: bool, tracks: &[&AudioProbe]) -> BookMeta {
    let first = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| tracks.iter().find_map(|t| t.tag(k)).map(|s| s.to_string()))
    };
    let fallback_title = if root_file {
        file_stem(book_key)
    } else {
        book_key.rsplit('/').next().unwrap_or(book_key).to_string()
    };
    let title = if root_file {
        first(&["album", "title"])
    } else {
        first(&["album"])
    }
    .unwrap_or(fallback_title);
    let mut authors = first(&["album_artist"])
        .or_else(|| first(&["artist"]))
        .map(|a| split_people(&a))
        .unwrap_or_default();
    if authors.is_empty() && !root_file {
        if let Some((top, _)) = book_key.split_once('/') {
            authors.push(top.to_string());
        }
    }
    BookMeta {
        title,
        authors,
        narrators: first(&["composer"])
            .map(|n| split_people(&n))
            .unwrap_or_default(),
        year: first(&["date"])
            .as_deref()
            .and_then(year_of)
            .or_else(|| first(&["year"]).as_deref().and_then(year_of)),
        overview: first(&["description"]).or_else(|| first(&["comment"])),
        genres: first(&["genre"])
            .map(|g| split_genres(&g))
            .unwrap_or_default(),
    }
}

// ------------------------------------------------------------ chapters

/// A chapter on the book-global timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct BookChapter {
    pub start_ms: i64,
    pub end_ms: i64,
    pub title: String,
}

/// One track as chapter-building sees it, in play order.
#[derive(Debug, Clone)]
pub struct ChapterTrack<'a> {
    pub rel_path: &'a str,
    pub duration_ms: Option<i64>,
    pub title_tag: Option<&'a str>,
    pub chapters: &'a [FileChapter],
}

/// The book's chapters: a track's own chapters shifted by its start
/// offset (clamped to the track), or one chapter per track titled
/// with its `title` tag or file name. A track of unknown length
/// counts as 0 ms long.
pub fn build_chapters(tracks: &[ChapterTrack<'_>]) -> Vec<BookChapter> {
    let mut out = Vec::new();
    let mut offset = 0i64;
    for track in tracks {
        let len = track.duration_ms.unwrap_or(0).max(0);
        let track_end = offset + len;
        if track.chapters.is_empty() {
            let title = track
                .title_tag
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| file_stem(track.rel_path));
            out.push(BookChapter {
                start_ms: offset,
                end_ms: track_end,
                title,
            });
        } else {
            for c in track.chapters {
                let start = (offset + c.start_ms).min(track_end.max(offset));
                let mut end = offset + c.end_ms;
                if len > 0 {
                    end = end.min(track_end);
                }
                let end = end.max(start);
                let n = out.len() + 1;
                out.push(BookChapter {
                    start_ms: start,
                    end_ms: end,
                    title: c.title.clone().unwrap_or_else(|| format!("Chapter {n}")),
                });
            }
        }
        offset = track_end;
    }
    out
}

// ---------------------------------------------------------------- scan

fn mtime_utc(t: SystemTime) -> DateTime<Utc> {
    let dt: DateTime<Utc> = t.into();
    DateTime::from_timestamp_micros(dt.timestamp_micros()).unwrap_or(dt)
}

struct StoredFile {
    item_id: Uuid,
    size: i64,
    mtime: DateTime<Utc>,
    available: bool,
}

/// Scan one audiobooks library (AB2). Same contract as the video
/// scanner: idempotent, progress + per-file errors on the job, and
/// vanished files marked unavailable only under roots that walked
/// cleanly (C8) — never deleted, so progress survives a missing disk.
pub async fn scan_audiobooks(
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

    // Walk every root. Book keys are relative to their root, like
    // the video scanner's keys, so adding or re-mounting a root
    // doesn't turn every book into a new one (and lose its progress).
    let mut walked_roots = Vec::new();
    let mut books: BTreeMap<String, Vec<(DiscoveredAudio, Option<u32>)>> = BTreeMap::new();
    let mut book_cover: HashMap<String, String> = HashMap::new();
    let mut book_root_file: HashMap<String, bool> = HashMap::new();
    for (root,) in &paths {
        let walk = walk_audio(Path::new(root));
        if walk.errors.is_empty() {
            walked_roots.push(root.clone());
        }
        for e in &walk.errors {
            report.errors.push(format!("{root}: {e}"));
            if let Some(job) = job_id {
                let _ = parlour_jobs::add_error(pool, job, Some(root), e).await;
            }
        }
        for file in walk.files {
            let (key, disc) = book_key_for(&file.rel_path);
            let root_file = key == file.rel_path;
            book_root_file.insert(key.clone(), root_file);
            if !root_file {
                if let Some(cover) = walk.covers.get(&key) {
                    book_cover.insert(key.clone(), cover.clone());
                }
            }
            books.entry(key).or_default().push((file, disc));
        }
    }

    let total: i32 = books.values().map(|f| f.len() as i32).sum();
    if let Some(job) = job_id {
        let _ = parlour_jobs::set_progress(pool, job, 0, total).await;
    }

    let stored: Vec<(String, Uuid, i64, DateTime<Utc>, bool)> = sqlx::query_as(
        "SELECT path, item_id, size_bytes, mtime, available FROM media_files WHERE library_id = $1",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;
    let mut available_per_item: HashMap<Uuid, usize> = HashMap::new();
    let stored: HashMap<String, StoredFile> = stored
        .into_iter()
        .map(|(path, item_id, size, mtime, available)| {
            if available {
                *available_per_item.entry(item_id).or_default() += 1;
            }
            (
                path,
                StoredFile {
                    item_id,
                    size,
                    mtime,
                    available,
                },
            )
        })
        .collect();
    let items: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT stable_key, id FROM media_items WHERE library_id = $1 AND kind = 'audiobook'",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;
    let items: HashMap<String, Uuid> = items.into_iter().collect();

    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut done = 0i32;
    for (key, files) in &books {
        let stable_key = format!("audiobook:{key}");
        for (f, _) in files {
            seen_paths.insert(f.abs_path.clone());
        }
        report.seen_files += files.len() as i32;
        done += files.len() as i32;

        let existing_item = items.get(&stable_key).copied();
        let unchanged = existing_item.is_some_and(|item_id| {
            available_per_item.get(&item_id).copied().unwrap_or(0) == files.len()
                && files.iter().all(|(f, _)| {
                    stored.get(&f.abs_path).is_some_and(|s| {
                        s.item_id == item_id
                            && s.available
                            && s.size == f.size_bytes as i64
                            && s.mtime == mtime_utc(f.mtime)
                    })
                })
        });

        let item_id = if let (true, Some(item_id)) = (unchanged, existing_item) {
            report.unchanged_files += files.len() as i32;
            sqlx::query(
                "UPDATE media_files SET last_seen_at = now() WHERE item_id = $1 AND available",
            )
            .bind(item_id)
            .execute(pool)
            .await?;
            item_id
        } else {
            let root_file = book_root_file.get(key).copied().unwrap_or(false);
            rebuild_book(
                pool,
                library_id,
                job_id,
                &stable_key,
                key,
                root_file,
                files,
                &mut report,
            )
            .await?
        };

        sync_cover(pool, item_id, book_cover.get(key).map(String::as_str)).await?;

        if let Some(job) = job_id {
            let _ = parlour_jobs::set_progress(pool, job, done, total).await;
        }
    }

    if !walked_roots.is_empty() {
        report.missing_files += mark_missing(pool, library_id, &seen_paths, &walked_roots).await?;
        sqlx::query(
            "UPDATE media_items i SET available = EXISTS (
                 SELECT 1 FROM media_files f WHERE f.item_id = i.id AND f.available
             ) WHERE i.library_id = $1 AND i.kind = 'audiobook'",
        )
        .bind(library_id)
        .execute(pool)
        .await?;
    }
    Ok(report)
}

/// Probe every track of one book and rewrite its item, files,
/// streams, chapters and people. Returns the book's item id.
#[allow(clippy::too_many_arguments)]
async fn rebuild_book(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    job_id: Option<Uuid>,
    stable_key: &str,
    key: &str,
    root_file: bool,
    files: &[(DiscoveredAudio, Option<u32>)],
    report: &mut ScanReport,
) -> Result<Uuid, sqlx::Error> {
    // Probe all tracks (a failed probe still catalogues the file).
    let mut probed: Vec<(&DiscoveredAudio, Option<u32>, AudioProbe, Option<String>)> = Vec::new();
    for (file, disc) in files {
        let (probe, err) = match probe_audio(Path::new(&file.abs_path)) {
            Ok(p) => (p, None),
            Err(e) => (AudioProbe::default(), Some(e)),
        };
        if let Some(e) = &err {
            report.errors.push(format!("{}: {e}", file.rel_path));
            if let Some(job) = job_id {
                let _ = parlour_jobs::add_error(pool, job, Some(&file.rel_path), e).await;
            }
        }
        report.probed_files += 1;
        probed.push((file, *disc, probe, err));
    }
    probed.sort_by(|a, b| {
        compare_tracks(
            &TrackKey {
                rel_path: &a.0.rel_path,
                disc_tag: tag_number(a.2.tag("disc")),
                disc_folder: a.1,
                track_tag: tag_number(a.2.tag("track")),
            },
            &TrackKey {
                rel_path: &b.0.rel_path,
                disc_tag: tag_number(b.2.tag("disc")),
                disc_folder: b.1,
                track_tag: tag_number(b.2.tag("track")),
            },
        )
    });

    let probes: Vec<&AudioProbe> = probed.iter().map(|p| &p.2).collect();
    let meta = book_meta(key, root_file, &probes);
    let (item_id, inserted): (Uuid, bool) = sqlx::query_as(
        "INSERT INTO media_items
             (library_id, parent_id, kind, stable_key, title, sort_title, year,
              overview, genres)
         VALUES ($1, NULL, 'audiobook', $2, $3, $4, $5, $6, $7)
         ON CONFLICT (library_id, stable_key) DO UPDATE
             SET title = EXCLUDED.title, sort_title = EXCLUDED.sort_title,
                 year = EXCLUDED.year, overview = EXCLUDED.overview,
                 genres = EXCLUDED.genres, updated_at = now()
         RETURNING id, (xmax = 0) AS inserted",
    )
    .bind(library_id)
    .bind(stable_key)
    .bind(&meta.title)
    .bind(sort_title(&meta.title))
    .bind(meta.year)
    .bind(&meta.overview)
    .bind(&meta.genres)
    .fetch_one(pool)
    .await?;
    report.created_items += inserted as i32;

    // Tracks: every file of the book, in play order. Rows of files
    // the book no longer has keep their old index until missing-
    // marking turns them unavailable; reads only use available rows.
    for (index, (file, _, probe, err)) in probed.iter().enumerate() {
        let (file_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO media_files
                 (item_id, library_id, path, relative_path, size_bytes, mtime,
                  container, duration_ms, probe_error, track_index, available, last_seen_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, true, now())
             ON CONFLICT (library_id, path) DO UPDATE
                 SET item_id = EXCLUDED.item_id, relative_path = EXCLUDED.relative_path,
                     size_bytes = EXCLUDED.size_bytes, mtime = EXCLUDED.mtime,
                     container = EXCLUDED.container, duration_ms = EXCLUDED.duration_ms,
                     probe_error = EXCLUDED.probe_error, track_index = EXCLUDED.track_index,
                     available = true, last_seen_at = now(), updated_at = now()
             RETURNING id",
        )
        .bind(item_id)
        .bind(library_id)
        .bind(&file.abs_path)
        .bind(&file.rel_path)
        .bind(file.size_bytes as i64)
        .bind(mtime_utc(file.mtime))
        .bind(&probe.container)
        .bind(probe.duration_ms)
        .bind(err)
        .bind(index as i32)
        .fetch_one(pool)
        .await?;
        sqlx::query("DELETE FROM media_streams WHERE file_id = $1")
            .bind(file_id)
            .execute(pool)
            .await?;
        for s in &probe.streams {
            sqlx::query(
                "INSERT INTO media_streams
                     (file_id, stream_index, kind, codec, language, channels, bit_rate, is_default)
                 VALUES ($1, $2, 'audio', $3, $4, $5, $6, $7)",
            )
            .bind(file_id)
            .bind(s.index)
            .bind(&s.codec)
            .bind(&s.language)
            .bind(s.channels)
            .bind(s.bit_rate)
            .bind(s.is_default)
            .execute(pool)
            .await?;
        }
    }
    // Earlier rows of this book that are not among its files now
    // (renamed or deleted tracks) stop counting at once, so the
    // chapters below and the track list always agree.
    let current: Vec<String> = probed.iter().map(|p| p.0.abs_path.clone()).collect();
    sqlx::query(
        "UPDATE media_files SET available = false, track_index = NULL
         WHERE item_id = $1 AND NOT (path = ANY($2))",
    )
    .bind(item_id)
    .bind(&current)
    .execute(pool)
    .await?;

    // Chapters on the book-global timeline.
    let chapter_tracks: Vec<ChapterTrack<'_>> = probed
        .iter()
        .map(|(file, _, probe, _)| ChapterTrack {
            rel_path: &file.rel_path,
            duration_ms: probe.duration_ms,
            title_tag: probe.tag("title"),
            chapters: &probe.chapters,
        })
        .collect();
    let chapters = build_chapters(&chapter_tracks);
    sqlx::query("DELETE FROM book_chapters WHERE item_id = $1")
        .bind(item_id)
        .execute(pool)
        .await?;
    for (i, c) in chapters.iter().enumerate() {
        sqlx::query(
            "INSERT INTO book_chapters (item_id, chapter_index, start_ms, end_ms, title)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(item_id)
        .bind(i as i32)
        .bind(c.start_ms)
        .bind(c.end_ms)
        .bind(&c.title)
        .execute(pool)
        .await?;
    }

    // Authors and narrators.
    sqlx::query("DELETE FROM item_people WHERE item_id = $1 AND role IN ('author', 'narrator')")
        .bind(item_id)
        .execute(pool)
        .await?;
    for (role, names) in [("author", &meta.authors), ("narrator", &meta.narrators)] {
        for (order, name) in names.iter().enumerate() {
            let (person_id,): (Uuid,) = sqlx::query_as(
                "INSERT INTO people (name) VALUES ($1)
                 ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
                 RETURNING id",
            )
            .bind(name)
            .fetch_one(pool)
            .await?;
            sqlx::query(
                "INSERT INTO item_people (item_id, person_id, role, sort_order)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (item_id, person_id, role) DO NOTHING",
            )
            .bind(item_id)
            .bind(person_id)
            .bind(role)
            .bind(order as i32)
            .execute(pool)
            .await?;
        }
    }
    Ok(item_id)
}

/// Keep a book's folder cover in `images` (kind poster, variant
/// `file`), served by the existing artwork endpoint straight from
/// the media path. Writes only when it changed.
async fn sync_cover(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
    cover: Option<&str>,
) -> Result<(), sqlx::Error> {
    let current: Option<(String,)> = sqlx::query_as(
        "SELECT path FROM images WHERE item_id = $1 AND kind = 'poster' AND variant = 'file'",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    if current.as_ref().map(|(p,)| p.as_str()) == cover {
        return Ok(());
    }
    sqlx::query("DELETE FROM images WHERE item_id = $1 AND kind = 'poster' AND variant = 'file'")
        .bind(item_id)
        .execute(pool)
        .await?;
    if let Some(path) = cover {
        sqlx::query(
            "INSERT INTO images (item_id, kind, variant, path)
             VALUES ($1, 'poster', 'file', $2)
             ON CONFLICT (item_id, kind, path) DO NOTHING",
        )
        .bind(item_id)
        .bind(path)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disc_folders_are_recognised_and_nothing_else() {
        assert_eq!(disc_folder_number("CD 1"), Some(1));
        assert_eq!(disc_folder_number("cd1"), Some(1));
        assert_eq!(disc_folder_number("Disc 02"), Some(2));
        assert_eq!(disc_folder_number("Disk-3"), Some(3));
        assert_eq!(disc_folder_number("Part_4"), Some(4));
        assert_eq!(disc_folder_number("CD 1 - Extras"), None);
        assert_eq!(disc_folder_number("Part One"), None);
        assert_eq!(disc_folder_number("Discworld"), None);
        assert_eq!(disc_folder_number("CD"), None);
    }

    #[test]
    fn files_group_into_books() {
        assert_eq!(
            book_key_for("Jane Author/Book One/01.mp3"),
            ("Jane Author/Book One".into(), None)
        );
        assert_eq!(
            book_key_for("Jane Author/Big Book/CD 2/01.mp3"),
            ("Jane Author/Big Book".into(), Some(2))
        );
        // A loose root file is its own book.
        assert_eq!(book_key_for("Single.m4b"), ("Single.m4b".into(), None));
        // A disc folder straight in the root is the book itself.
        assert_eq!(book_key_for("CD 1/01.mp3"), ("CD 1".into(), None));
    }

    #[test]
    fn natural_order_compares_numbers_as_numbers() {
        let mut names = vec![
            "Chapter 10.mp3",
            "chapter 2.mp3",
            "Chapter 1.mp3",
            "Chapter 01b.mp3",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec![
                "Chapter 1.mp3",
                "Chapter 01b.mp3",
                "chapter 2.mp3",
                "Chapter 10.mp3"
            ]
        );
    }

    #[test]
    fn tracks_order_by_disc_then_track_then_name() {
        let key = |rel, disc_tag, disc_folder, track_tag| TrackKey {
            rel_path: rel,
            disc_tag,
            disc_folder,
            track_tag,
        };
        let mut tracks = [
            key("b/CD 2/01.mp3", None, Some(2), Some(1)),
            key("b/CD 1/10.mp3", None, Some(1), Some(10)),
            key("b/CD 1/02.mp3", None, Some(1), Some(2)),
            key("b/CD 1/x.mp3", None, Some(1), None),
        ];
        tracks.sort_by(compare_tracks);
        let order: Vec<&str> = tracks.iter().map(|t| t.rel_path).collect();
        assert_eq!(
            order,
            [
                "b/CD 1/02.mp3",
                "b/CD 1/10.mp3",
                "b/CD 1/x.mp3",
                "b/CD 2/01.mp3"
            ]
        );
        // A disc tag beats the folder; with no numbers, names decide.
        let mut tracks = [
            key("a/2.mp3", Some(1), None, None),
            key("a/10.mp3", Some(1), None, None),
        ];
        tracks.sort_by(compare_tracks);
        assert_eq!(tracks[0].rel_path, "a/2.mp3");
        assert_eq!(tag_number(Some("03/12")), Some(3));
        assert_eq!(tag_number(Some("x")), None);
    }

    #[test]
    fn people_split_on_separators_but_keep_last_first_names() {
        assert_eq!(
            split_people("Jane Doe, John Roe"),
            vec!["Jane Doe", "John Roe"]
        );
        assert_eq!(split_people("Tolkien, J.R.R."), vec!["Tolkien, J.R.R."]);
        assert_eq!(
            split_people("A One & B Two; C Three"),
            vec!["A One", "B Two", "C Three"]
        );
        assert_eq!(split_people("Ann / Bob"), vec!["Ann", "Bob"]);
        assert_eq!(split_people("Same Name; same name"), vec!["Same Name"]);
        assert!(split_people("  ").is_empty());
        assert_eq!(
            split_genres("Fantasy; Sci-Fi/fantasy"),
            vec!["Fantasy", "Sci-Fi"]
        );
    }

    fn probe_with(tags: &[(&str, &str)], duration_ms: i64) -> AudioProbe {
        AudioProbe {
            duration_ms: Some(duration_ms),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn metadata_prefers_tags_then_folders() {
        let a = probe_with(
            &[
                ("album", "Book One"),
                ("artist", "Track Artist"),
                ("album_artist", "Jane Author"),
                ("composer", "Nick Narrator & Nora Reader"),
                ("date", "2019-05-01"),
                ("genre", "Fantasy"),
                ("comment", "A comment"),
                ("title", "Opening"),
            ],
            1000,
        );
        let b = probe_with(&[("description", "Later description")], 1000);
        let meta = book_meta("Jane Author/Book One", false, &[&a, &b]);
        assert_eq!(meta.title, "Book One");
        assert_eq!(meta.authors, vec!["Jane Author"]);
        assert_eq!(meta.narrators, vec!["Nick Narrator", "Nora Reader"]);
        assert_eq!(meta.year, Some(2019));
        // description wins over comment even on a later track.
        assert_eq!(meta.overview.as_deref(), Some("Later description"));
        assert_eq!(meta.genres, vec!["Fantasy"]);

        // No tags: folder name for the title, top-level folder for
        // the author — also through a series folder.
        let bare = AudioProbe::default();
        let meta = book_meta("Jane Author/A Series/Book Two", false, &[&bare]);
        assert_eq!(meta.title, "Book Two");
        assert_eq!(meta.authors, vec!["Jane Author"]);
        assert!(meta.narrators.is_empty());

        // A single root file: title tag counts, file stem as fallback,
        // no folder author.
        let single = probe_with(&[("title", "The Single")], 1000);
        let meta = book_meta("Single.m4b", true, &[&single]);
        assert_eq!(meta.title, "The Single");
        assert!(meta.authors.is_empty());
        let meta = book_meta("Loose Book.mp3", true, &[&bare]);
        assert_eq!(meta.title, "Loose Book");
        // A folder book's track title is not the book's title.
        let meta = book_meta("Folder Book", false, &[&single]);
        assert_eq!(meta.title, "Folder Book");
        // `year` tag as fallback; nonsense years are ignored.
        let y = probe_with(&[("year", "1999")], 1);
        assert_eq!(book_meta("x", false, &[&y]).year, Some(1999));
        let y = probe_with(&[("date", "unknown")], 1);
        assert_eq!(book_meta("x", false, &[&y]).year, None);
    }

    #[test]
    fn chapters_come_from_files_or_one_per_track() {
        let embedded = vec![
            FileChapter {
                start_ms: 0,
                end_ms: 1500,
                title: Some("Prologue".into()),
            },
            FileChapter {
                start_ms: 1500,
                end_ms: 9000, // runs past the track: clamped
                title: None,
            },
        ];
        let tracks = [
            ChapterTrack {
                rel_path: "b/01 - Opening.mp3",
                duration_ms: Some(3000),
                title_tag: Some("Opening"),
                chapters: &[],
            },
            ChapterTrack {
                rel_path: "b/02 - Middle.mp3",
                duration_ms: Some(2000),
                title_tag: None,
                chapters: &[],
            },
            ChapterTrack {
                rel_path: "b/03.m4b",
                duration_ms: Some(4000),
                title_tag: Some("ignored, file has chapters"),
                chapters: &embedded,
            },
        ];
        let chapters = build_chapters(&tracks);
        assert_eq!(
            chapters,
            vec![
                BookChapter {
                    start_ms: 0,
                    end_ms: 3000,
                    title: "Opening".into()
                },
                BookChapter {
                    start_ms: 3000,
                    end_ms: 5000,
                    title: "02 - Middle".into()
                },
                BookChapter {
                    start_ms: 5000,
                    end_ms: 6500,
                    title: "Prologue".into()
                },
                BookChapter {
                    start_ms: 6500,
                    end_ms: 9000,
                    title: "Chapter 4".into()
                },
            ]
        );
    }

    #[test]
    fn probe_json_reads_tags_streams_and_chapters() {
        let json = r#"{
          "streams": [
            {"index": 0, "codec_type": "audio", "codec_name": "opus", "channels": 2,
             "bit_rate": "64000", "disposition": {"default": 1},
             "tags": {"ARTIST": "Stream Artist", "TITLE": "Stream Title", "language": "eng"}},
            {"index": 1, "codec_type": "video", "codec_name": "mjpeg"}
          ],
          "chapters": [
            {"start_time": "0.000000", "end_time": "2.500000", "tags": {"title": "One"}},
            {"start_time": "2.500000", "end_time": "4.000000", "tags": {}},
            {"start_time": "bad", "end_time": "5.0"}
          ],
          "format": {"format_name": "ogg", "duration": "4.000000",
                     "tags": {"ALBUM": "Format Album", "title": "Format Title", "track": 3}}
        }"#;
        let p = parse_audio_probe_json(json).unwrap();
        assert_eq!(p.container.as_deref(), Some("ogg"));
        assert_eq!(p.duration_ms, Some(4000));
        // Format tags win; stream tags fill gaps; keys lowercased.
        assert_eq!(p.tag("album"), Some("Format Album"));
        assert_eq!(p.tag("title"), Some("Format Title"));
        assert_eq!(p.tag("artist"), Some("Stream Artist"));
        assert_eq!(p.tag("track"), Some("3"));
        // Only the audio stream; the cover's mjpeg is not a track.
        assert_eq!(p.streams.len(), 1);
        assert_eq!(p.streams[0].codec.as_deref(), Some("opus"));
        assert_eq!(p.streams[0].bit_rate, Some(64000));
        assert_eq!(p.streams[0].language.as_deref(), Some("eng"));
        assert!(p.streams[0].is_default);
        assert_eq!(
            p.chapters,
            vec![
                FileChapter {
                    start_ms: 0,
                    end_ms: 2500,
                    title: Some("One".into())
                },
                FileChapter {
                    start_ms: 2500,
                    end_ms: 4000,
                    title: None
                },
            ]
        );
        assert!(parse_audio_probe_json("not json").is_err());
    }

    #[test]
    fn walk_finds_audio_and_covers() {
        let dir = std::env::temp_dir().join(format!("parlour-ab-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let book = dir.join("Author").join("Book");
        std::fs::create_dir_all(book.join(".hidden")).unwrap();
        for f in [
            "01.mp3",
            "02.MP3",
            "notes.txt",
            "folder.jpg",
            "Cover.JPG",
            "._01.mp3",
        ] {
            std::fs::write(book.join(f), b"x").unwrap();
        }
        std::fs::write(book.join(".hidden").join("x.mp3"), b"x").unwrap();
        std::fs::write(dir.join("Single.m4b"), b"x").unwrap();
        let walk = walk_audio(&dir);
        let rels: Vec<&str> = walk.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(
            rels,
            ["Author/Book/01.mp3", "Author/Book/02.MP3", "Single.m4b"]
        );
        // cover.* beats folder.*, case-insensitively.
        assert!(walk.covers["Author/Book"].ends_with("Cover.JPG"));
        assert!(walk.errors.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!walk_audio(&dir).errors.is_empty());
    }
}
