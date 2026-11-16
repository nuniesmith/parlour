//! Segment detection — the cheap pass (SEG2; D15, D17, D18, D19).
//!
//! `DetectSegments` runs after a library scan over every episode
//! whose playback file is new or changed: the scanner clears
//! `media_files.segments_detected_at` whenever it probes a file,
//! and this job sets it again. The playback file is the one
//! playback-info serves (first available by relative path), so the
//! stored times are times into what the player actually plays.
//!
//! Per file:
//!
//! 1. **Chapters** (`ffprobe -show_chapters`): a chapter whose title
//!    holds the whole word "intro" or "opening" is the intro, one
//!    holding "credits" is the credits, ignoring case. Stored as
//!    source `chapters` at confidence 1.0, no threshold (D17).
//! 2. **Heuristic**, only for a kind the chapters did not answer
//!    (SEG2: "source=chapters *or* a low-confidence heuristic row"):
//!    one ffmpeg pass over the first 300 s looks for the intro, one
//!    over the last 600 s for the credits (D18). `blackdetect`
//!    (runs from 0.5 s at its built-in pixel thresholds) and
//!    `silencedetect` (below −50 dB for at least 0.5 s) report
//!    runs; a *break* is a black run and a silence run that align —
//!    they overlap, or are at most 2 s apart (D17) — and covers
//!    both. Spans are built from breaks per D18 (`intro_span`,
//!    `credits_span`) and stored as source `heuristic` at a fixed
//!    0.5: below the playback floor, so a guess never reaches a
//!    player; it exists for the SEG4 editor and for SEG3.
//!
//! A run owns its two sources for intro and credits: what it does
//! not find this time it deletes, so a replaced file never keeps the
//! old file's spans. Manual and fingerprint rows are never touched.
//!
//! Then, per season with a new or changed episode, the fingerprint
//! pass (SEG3, D20; `fingerprint`) re-matches the whole season and
//! replaces its `fingerprint` rows. The job works season by season:
//! a season's files are marked as looked at only together with its
//! fingerprint rows, so a job that stops part-way leaves them queued
//! for the next scan.
//!
//! The rules are pure functions over parsed tool output and are
//! unit-tested below; only `probe_chapters` / `scan_window` run a
//! subprocess, and only `detect_library` / `needs_detection` touch
//! the database.

use std::collections::{HashMap, HashSet};
use std::process::Stdio;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use parlour_core::{SegmentKind, SegmentSource};

use crate::fingerprint;

// ------------------------------------------------------------ the rules

/// Chapter-named rows are real markers in the file (D17).
pub const CHAPTER_CONFIDENCE: f32 = 1.0;
/// Every heuristic row (D17) — below the 0.6 playback floor.
pub const HEURISTIC_CONFIDENCE: f32 = 0.5;
/// Scan extent (D18): the head window is searched for the intro...
pub const HEAD_WINDOW_MS: i64 = 300_000;
/// ...and the tail window for the credits.
pub const TAIL_WINDOW_MS: i64 = 600_000;
/// A black run and a silence run align when they overlap or the gap
/// between them is at most this (D17).
pub const ALIGN_MAX_GAP_MS: i64 = 2_000;
/// The intro's first break starts within this (D18, D17's head bound).
pub const INTRO_FIRST_BREAK_MAX_START_MS: i64 = 120_000;
/// The intro — the gap to the next break — is 15–120 s long (D18).
pub const INTRO_MIN_MS: i64 = 15_000;
pub const INTRO_MAX_MS: i64 = 120_000;
/// Credits end within the last 120 s of the file (D17)...
pub const CREDITS_END_WITHIN_MS: i64 = 120_000;
/// ...and are at least 15 s long (D18).
pub const CREDITS_MIN_MS: i64 = 15_000;
/// D18's detector sensitivity as ffmpeg filter arguments: black runs
/// from 0.5 s at blackdetect's built-in pixel thresholds; silence is
/// anything below −50 dB lasting at least 0.5 s.
const BLACKDETECT: &str = "blackdetect=d=0.5";
const SILENCEDETECT: &str = "silencedetect=n=-50dB:d=0.5";

/// A stretch of a file, `[start_ms, end_ms)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Span {
    pub fn new(start_ms: i64, end_ms: i64) -> Self {
        Self { start_ms, end_ms }
    }

    pub fn len_ms(&self) -> i64 {
        self.end_ms - self.start_ms
    }

    /// What `media_segments` accepts: a non-empty span from 0 on.
    fn is_valid(&self) -> bool {
        self.start_ms >= 0 && self.end_ms > self.start_ms
    }
}

/// One chapter from ffprobe.
#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    pub start_ms: i64,
    pub end_ms: i64,
    pub title: Option<String>,
}

/// Black-frame and silence runs from one ffmpeg pass, in file time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Runs {
    pub black: Vec<Span>,
    pub silence: Vec<Span>,
}

/// One detected segment and the source that found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub span: Span,
    pub source: SegmentSource,
}

/// What one file yielded: at most one span per kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Detection {
    pub intro: Option<Found>,
    pub credits: Option<Found>,
}

fn secs_to_ms(secs: f64) -> i64 {
    (secs * 1000.0).round() as i64
}

// -------------------------------------------------------------- chapters

#[derive(Deserialize)]
struct FfChapterList {
    chapters: Option<Vec<FfChapter>>,
}

#[derive(Deserialize)]
struct FfChapter {
    start_time: Option<String>,
    end_time: Option<String>,
    tags: Option<HashMap<String, serde_json::Value>>,
}

/// Parse `ffprobe -print_format json -show_chapters` output.
pub fn parse_chapters_json(json: &str) -> Result<Vec<Chapter>, String> {
    let list: FfChapterList = serde_json::from_str(json)
        .map_err(|e| format!("ffprobe chapter output did not parse: {e}"))?;
    let seconds = |v: Option<String>| {
        v.and_then(|s| s.parse::<f64>().ok())
            .filter(|s| s.is_finite())
    };
    Ok(list
        .chapters
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| {
            let title = c.tags.and_then(|tags| {
                tags.into_iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("title"))
                    .and_then(|(_, v)| v.as_str().map(str::to_string))
            });
            Some(Chapter {
                start_ms: secs_to_ms(seconds(c.start_time)?),
                end_ms: secs_to_ms(seconds(c.end_time)?),
                title,
            })
        })
        .collect())
}

/// Which segment a chapter title names: the whole word "intro" or
/// "opening" names the intro, "credits" the credits, ignoring case
/// ("Intro", "OPENING", "End Credits" — but not "Introduction").
///
/// A title holding words for both — "Opening Credits" — names the
/// intro: the opening credits *are* the intro, and calling them the
/// credits would start next-episode autoplay at the top of the
/// episode (D19).
pub fn chapter_kind(title: &str) -> Option<SegmentKind> {
    let (mut intro, mut credits) = (false, false);
    for word in title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        match word.to_lowercase().as_str() {
            "intro" | "opening" => intro = true,
            "credits" => credits = true,
            _ => {}
        }
    }
    if intro {
        Some(SegmentKind::Intro)
    } else if credits {
        Some(SegmentKind::Credits)
    } else {
        None
    }
}

/// The intro and the credits a file's chapters name, as
/// `(intro, credits)`. When several chapters name the same kind, the
/// earliest wins — so "Credits" beats a later "Post-Credits Scene"
/// (D19). Chapters that are no usable span are skipped.
pub fn chapter_spans(chapters: &[Chapter]) -> (Option<Span>, Option<Span>) {
    let mut ordered: Vec<&Chapter> = chapters.iter().collect();
    ordered.sort_by_key(|c| (c.start_ms, c.end_ms));
    let (mut intro, mut credits) = (None, None);
    for chapter in ordered {
        let span = Span::new(chapter.start_ms, chapter.end_ms);
        if !span.is_valid() {
            continue;
        }
        match chapter.title.as_deref().and_then(chapter_kind) {
            Some(SegmentKind::Intro) if intro.is_none() => intro = Some(span),
            Some(SegmentKind::Credits) if credits.is_none() => credits = Some(span),
            _ => {}
        }
    }
    (intro, credits)
}

// ------------------------------------------------------ black + silence

/// The window of the file one ffmpeg pass looks at for the intro:
/// the first 300 s (D18).
pub fn head_window(duration_ms: i64) -> Span {
    Span::new(0, duration_ms.min(HEAD_WINDOW_MS))
}

/// ...and for the credits: the last 600 s (D18).
pub fn tail_window(duration_ms: i64) -> Span {
    Span::new((duration_ms - TAIL_WINDOW_MS).max(0), duration_ms)
}

/// `[blackdetect @ 0x…] black_start:…` → `blackdetect @ 0x…`. Only
/// filter log lines are read, so a file name or tag that happens to
/// contain `black_start:` can never pass for a run.
fn filter_tag(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    Some(&rest[..rest.find(']')?])
}

/// The number after `key` (`black_start:10.5`, `silence_end: 11 |`).
fn value_after(line: &str, key: &str) -> Option<f64> {
    let at = line.find(key)? + key.len();
    let token = line[at..]
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '|')
        .next()?;
    token.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A window-relative run → file time, clipped to the window; an
/// empty run is dropped.
fn push_run(list: &mut Vec<Span>, window: Span, rel_start_ms: i64, rel_end_ms: i64) {
    let start = (window.start_ms + rel_start_ms).max(window.start_ms);
    let end = (window.start_ms + rel_end_ms).min(window.end_ms);
    if end > start {
        list.push(Span::new(start, end));
    }
}

/// Parse what `blackdetect` and `silencedetect` log for one pass over
/// `window` (the same format from ffmpeg 5.1 to 8.x). Times in the
/// log are relative to the window's start (`-ss` resets the clock);
/// the runs come back in file time. A run still open when the pass
/// ended runs to the window's end.
pub fn parse_detect_log(log: &str, window: Span) -> Runs {
    let mut runs = Runs::default();
    let mut silence_from: Option<i64> = None;
    for line in log.lines() {
        let line = line.trim_start();
        let Some(tag) = filter_tag(line) else {
            continue;
        };
        if tag.contains("blackdetect") {
            if let Some(start) = value_after(line, "black_start:") {
                let end_ms = value_after(line, "black_end:")
                    .map(secs_to_ms)
                    .unwrap_or(window.len_ms());
                push_run(&mut runs.black, window, secs_to_ms(start), end_ms);
            }
        } else if tag.contains("silencedetect") {
            if let Some(start) = value_after(line, "silence_start:") {
                silence_from = Some(secs_to_ms(start));
            } else if let Some(end) = value_after(line, "silence_end:") {
                let start_ms = silence_from.take().unwrap_or(0);
                push_run(&mut runs.silence, window, start_ms, secs_to_ms(end));
            }
        }
    }
    if let Some(start_ms) = silence_from {
        push_run(&mut runs.silence, window, start_ms, window.len_ms());
    }
    runs
}

/// How far apart two runs are; zero or less when they touch or overlap.
fn gap_ms(a: &Span, b: &Span) -> i64 {
    a.start_ms.max(b.start_ms) - a.end_ms.min(b.end_ms)
}

/// The breaks in a pass: a black-frame run and a silence run that
/// align — they overlap, or are at most 2 s apart (D17) — make one
/// break covering both. A run with no partner of the other kind is
/// not a break (a dark scene, a quiet moment). Overlapping breaks
/// merge. Sorted by start.
pub fn breaks(runs: &Runs) -> Vec<Span> {
    let mut found: Vec<Span> = runs
        .black
        .iter()
        .filter_map(|black| {
            let mut span = *black;
            let mut aligned = false;
            for silence in &runs.silence {
                if gap_ms(black, silence) <= ALIGN_MAX_GAP_MS {
                    aligned = true;
                    span.start_ms = span.start_ms.min(silence.start_ms);
                    span.end_ms = span.end_ms.max(silence.end_ms);
                }
            }
            aligned.then_some(span)
        })
        .collect();
    found.sort_by_key(|s| (s.start_ms, s.end_ms));
    let mut merged: Vec<Span> = Vec::with_capacity(found.len());
    for span in found {
        match merged.last_mut() {
            Some(last) if span.start_ms <= last.end_ms => {
                last.end_ms = last.end_ms.max(span.end_ms)
            }
            _ => merged.push(span),
        }
    }
    merged
}

/// D18: the intro is the gap between two consecutive breaks, the
/// file's start counting as a break: the first of the two starts
/// within 120 s, the gap is 15–120 s long, and the first match wins.
/// `breaks` are the head window's, sorted.
pub fn intro_span(breaks: &[Span]) -> Option<Span> {
    let mut previous = Span::new(0, 0); // the file's start
    for next in breaks {
        if previous.start_ms > INTRO_FIRST_BREAK_MAX_START_MS {
            return None;
        }
        let gap = next.start_ms - previous.end_ms;
        if (INTRO_MIN_MS..=INTRO_MAX_MS).contains(&gap) {
            return Some(Span::new(previous.end_ms, next.start_ms));
        }
        previous = *next;
    }
    None
}

/// D18: the credits run from a break to the next break or the end of
/// the file, end within the last 120 s, and are at least 15 s long.
/// `breaks` are the tail window's, sorted.
///
/// When more than one span qualifies — an act break before the
/// credits break, say — the one nearest the end of the file wins:
/// the tail's mirror of the intro's "first match wins" (D19).
pub fn credits_span(breaks: &[Span], duration_ms: i64) -> Option<Span> {
    let mut best = None;
    for (i, start) in breaks.iter().enumerate() {
        let end = breaks
            .get(i + 1)
            .map_or(duration_ms, |next| next.start_ms)
            .min(duration_ms);
        let span = Span::new(start.end_ms, end);
        if span.end_ms >= duration_ms - CREDITS_END_WITHIN_MS && span.len_ms() >= CREDITS_MIN_MS {
            best = Some(span); // a later qualifying span replaces an earlier one
        }
    }
    best
}

// ------------------------------------------------------------- the tools

/// Why a tool run gave no answer.
pub(crate) enum ToolError {
    /// The tool could not be started at all (missing binary, …). A
    /// server problem, not a file problem: the job stops, and every
    /// file it had not finished stays queued for the next run.
    Unavailable(String),
    /// The tool ran and failed on this file. Recorded against the
    /// file, which is not retried until it changes — the scanner's
    /// rule for a file ffprobe cannot read.
    Failed(String),
}

pub(crate) fn tool(env: &str, default: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| default.to_string())
}

/// The last few non-empty lines of a tool's stderr: the error, not
/// ffmpeg's whole stream listing.
pub(crate) fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(3)..].join(" / ")
}

pub(crate) async fn run(bin: &str, args: &[String]) -> Result<std::process::Output, ToolError> {
    tokio::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| ToolError::Unavailable(format!("could not run {bin}: {e}")))
}

/// `PARLOUR_FFPROBE` overrides the binary, as for the scanner.
async fn probe_chapters(path: &str) -> Result<Vec<Chapter>, ToolError> {
    let bin = tool("PARLOUR_FFPROBE", "ffprobe");
    let args: Vec<String> = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_chapters",
        path,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let output = run(&bin, &args).await?;
    if !output.status.success() {
        return Err(ToolError::Failed(format!(
            "ffprobe could not read the chapters: {}",
            stderr_tail(&output.stderr)
        )));
    }
    parse_chapters_json(&String::from_utf8_lossy(&output.stdout)).map_err(ToolError::Failed)
}

pub(crate) fn seconds_arg(ms: i64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// One ffmpeg pass over `window`: the first video stream that is not
/// cover art through `blackdetect`, the first audio stream through
/// `silencedetect`, output discarded. The head pass reads at most
/// 300 s; the tail pass seeks to the window and reads to the end.
/// `PARLOUR_FFMPEG` overrides the binary, as for remuxing.
async fn scan_window(path: &str, window: Span, read_to_end: bool) -> Result<Runs, ToolError> {
    let bin = tool("PARLOUR_FFMPEG", "ffmpeg");
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostats".into(), "-nostdin".into()];
    if window.start_ms > 0 {
        args.extend(["-ss".into(), seconds_arg(window.start_ms)]);
    }
    if !read_to_end {
        args.extend(["-t".into(), seconds_arg(HEAD_WINDOW_MS)]);
    }
    args.extend(
        [
            "-i",
            path,
            "-map",
            "0:V:0?",
            "-map",
            "0:a:0?",
            "-vf",
            BLACKDETECT,
            "-af",
            SILENCEDETECT,
            "-f",
            "null",
            "-",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let output = run(&bin, &args).await?;
    if !output.status.success() {
        return Err(ToolError::Failed(format!(
            "ffmpeg could not scan {}–{} s: {}",
            window.start_ms / 1000,
            window.end_ms / 1000,
            stderr_tail(&output.stderr)
        )));
    }
    Ok(parse_detect_log(
        &String::from_utf8_lossy(&output.stderr),
        window,
    ))
}

// ------------------------------------------------------------- the job

/// A finished job's summary — flat counts for Server → Jobs.
#[derive(Debug, Default, Serialize)]
pub struct DetectReport {
    /// Episodes whose playback file was new or changed.
    pub episodes: i32,
    /// What the cheap pass stored for those episodes (SEG2).
    pub chapter_intros: i32,
    pub chapter_credits: i32,
    pub heuristic_intros: i32,
    pub heuristic_credits: i32,
    /// New or changed episodes that ended the job with no segment
    /// row from either pass.
    pub nothing_found: i32,
    /// Episodes where a tool failed on the file in the cheap pass
    /// (errors list why).
    pub failed: i32,
    /// Episodes whose file changed while it was being examined:
    /// nothing was stored; the scan that saw the change queues it.
    pub changed_during_detection: i32,
    /// Seasons re-matched by audio fingerprint because an episode in
    /// them was new or changed (SEG3)...
    pub seasons_matched: i32,
    /// ...and those left unmatched: fewer than 3 usable episodes
    /// (D20), so their fingerprint rows were cleared.
    pub seasons_skipped: i32,
    /// Episodes whose audio ffmpeg fingerprinted this run...
    pub fingerprinted: i32,
    /// ...and those whose cached fingerprint was used instead.
    pub fingerprints_reused: i32,
    /// Episodes the fingerprinting failed on (errors list why).
    pub fingerprint_failed: i32,
    /// Fingerprint segments stored, over every re-matched season.
    pub fingerprint_intros: i32,
    pub fingerprint_credits: i32,
    /// Cached fingerprints no file references any more, deleted.
    pub fingerprints_removed: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum DetectError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    ToolUnavailable(String),
    #[error("fingerprint matching failed: {0}")]
    Matching(String),
}

#[derive(Debug, sqlx::FromRow)]
struct PendingFile {
    id: Uuid,
    item_id: Uuid,
    /// The episode's season item.
    season_id: Option<Uuid>,
    path: String,
    relative_path: String,
    duration_ms: Option<i64>,
    size_bytes: i64,
    mtime: DateTime<Utc>,
    has_video: bool,
    has_audio: bool,
}

/// Does any episode in the library have a playback file segment
/// detection has not looked at yet? Decides whether a scan queues
/// the job.
pub async fn needs_detection(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS (
             SELECT 1 FROM (
                 SELECT DISTINCT ON (f.item_id) f.segments_detected_at
                 FROM media_files f
                 JOIN media_items i ON i.id = f.item_id
                 WHERE f.library_id = $1 AND i.kind = 'episode' AND f.available
                 ORDER BY f.item_id, f.relative_path, f.path
             ) p
             WHERE p.segments_detected_at IS NULL
         )",
    )
    .bind(library_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// The chapters, then the heuristic for whatever kind they left open.
/// Per-tool failures on this file come back in the error list; the
/// rest of the result still stands.
async fn detect_file(file: &PendingFile) -> Result<(Detection, Vec<String>), String> {
    let mut found = Detection::default();
    let mut errors = Vec::new();
    let mut note = |result: Result<(), ToolError>| match result {
        Ok(()) => Ok(()),
        Err(ToolError::Unavailable(msg)) => Err(msg),
        Err(ToolError::Failed(msg)) => {
            errors.push(msg);
            Ok(())
        }
    };

    note(probe_chapters(&file.path).await.map(|chapters| {
        let (intro, credits) = chapter_spans(&chapters);
        let chapter = |span| Found {
            span,
            source: SegmentSource::Chapters,
        };
        found.intro = intro.map(chapter);
        found.credits = credits.map(chapter);
    }))?;

    // The heuristic needs a picture, a sound track and the length.
    let duration_ms = match file.duration_ms {
        Some(d) if d > 0 && file.has_video && file.has_audio => d,
        _ => return Ok((found, errors)),
    };
    let heuristic = |span| Found {
        span,
        source: SegmentSource::Heuristic,
    };
    if found.intro.is_none() {
        note(
            scan_window(&file.path, head_window(duration_ms), false)
                .await
                .map(|runs| found.intro = intro_span(&breaks(&runs)).map(heuristic)),
        )?;
    }
    if found.credits.is_none() {
        note(
            scan_window(&file.path, tail_window(duration_ms), true)
                .await
                .map(|runs| {
                    found.credits = credits_span(&breaks(&runs), duration_ms).map(heuristic)
                }),
        )?;
    }
    Ok((found, errors))
}

fn confidence_of(source: SegmentSource) -> f32 {
    match source {
        SegmentSource::Heuristic => HEURISTIC_CONFIDENCE,
        _ => CHAPTER_CONFIDENCE,
    }
}

/// Write one file's cheap-pass result: upsert what was found and
/// delete this pass's rows for what was not — all or nothing, and
/// only if the file is still the one that was examined (same size and
/// mtime). Returns false when it had changed. The file is marked as
/// looked at later, together with its season (`finish_season`).
async fn store(
    pool: &parlour_db::PgPool,
    file: &PendingFile,
    found: &Detection,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !unchanged(&mut tx, file.id, file.size_bytes, file.mtime).await? {
        tx.rollback().await?;
        return Ok(false);
    }
    for (kind, slot) in [
        (SegmentKind::Intro, found.intro),
        (SegmentKind::Credits, found.credits),
    ] {
        for source in [SegmentSource::Chapters, SegmentSource::Heuristic] {
            match slot {
                Some(f) if f.source == source => {
                    sqlx::query(
                        "INSERT INTO media_segments
                             (item_id, kind, start_ms, end_ms, source, confidence)
                         VALUES ($1, $2, $3, $4, $5, $6)
                         ON CONFLICT (item_id, kind, source) DO UPDATE
                         SET start_ms = EXCLUDED.start_ms, end_ms = EXCLUDED.end_ms,
                             confidence = EXCLUDED.confidence, updated_at = now()",
                    )
                    .bind(file.item_id)
                    .bind(kind.as_str())
                    .bind(f.span.start_ms)
                    .bind(f.span.end_ms)
                    .bind(source.as_str())
                    .bind(confidence_of(source))
                    .execute(&mut *tx)
                    .await?;
                }
                _ => {
                    sqlx::query(
                        "DELETE FROM media_segments
                         WHERE item_id = $1 AND kind = $2 AND source = $3",
                    )
                    .bind(file.item_id)
                    .bind(kind.as_str())
                    .bind(source.as_str())
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
    }
    tx.commit().await?;
    Ok(true)
}

/// Is the file row still the one that was read (same size and mtime)?
async fn unchanged(
    conn: &mut sqlx::PgConnection,
    file_id: Uuid,
    size_bytes: i64,
    mtime: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let (same,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM media_files
                        WHERE id = $1 AND size_bytes = $2 AND mtime = $3)",
    )
    .bind(file_id)
    .bind(size_bytes)
    .bind(mtime)
    .fetch_one(conn)
    .await?;
    Ok(same)
}

/// One new or changed file's cheap-pass result, waiting for its
/// season to finish.
struct Examined<'a> {
    file: &'a PendingFile,
    found: Detection,
    /// No tool failed on the file.
    clean: bool,
    /// `store` wrote the result: the file had not changed.
    stored: bool,
}

/// Close a season: write the fingerprint match's rows (SEG3) and mark
/// the season's new or changed files as looked at, in one
/// transaction; then count what those files ended up with. An episode
/// whose file changed since the season was read keeps its rows, and a
/// changed file is not marked: the scan that saw the change queues
/// the season again.
async fn finish_season(
    pool: &parlour_db::PgPool,
    examined: &[Examined<'_>],
    outcome: &[fingerprint::EpisodeOutcome],
    report: &mut DetectReport,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let mut printed: HashSet<Uuid> = HashSet::new();
    for episode in outcome {
        if !unchanged(&mut tx, episode.file_id, episode.size_bytes, episode.mtime).await? {
            continue;
        }
        fingerprint::write_rows(&mut tx, episode).await?;
        report.fingerprint_intros += episode.intro.is_some() as i32;
        report.fingerprint_credits += episode.credits.is_some() as i32;
        if episode.intro.is_some() || episode.credits.is_some() {
            printed.insert(episode.item_id);
        }
    }
    let mut marked = Vec::with_capacity(examined.len());
    for e in examined {
        let done = e.stored
            && sqlx::query(
                "UPDATE media_files SET segments_detected_at = now()
                 WHERE id = $1 AND size_bytes = $2 AND mtime = $3",
            )
            .bind(e.file.id)
            .bind(e.file.size_bytes)
            .bind(e.file.mtime)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
        marked.push(done);
    }
    tx.commit().await?;

    for (e, marked) in examined.iter().zip(marked) {
        if !marked {
            report.changed_during_detection += 1;
            continue;
        }
        for (slot, from_chapters, from_heuristic) in [
            (
                e.found.intro,
                &mut report.chapter_intros,
                &mut report.heuristic_intros,
            ),
            (
                e.found.credits,
                &mut report.chapter_credits,
                &mut report.heuristic_credits,
            ),
        ] {
            match slot.map(|f| f.source) {
                Some(SegmentSource::Chapters) => *from_chapters += 1,
                Some(_) => *from_heuristic += 1,
                None => {}
            }
        }
        if e.found == Detection::default() && e.clean && !printed.contains(&e.file.item_id) {
            report.nothing_found += 1;
        }
    }
    Ok(())
}

/// Run detection over every episode in the library whose playback
/// file has not been looked at since it last changed: the cheap pass
/// per file, then the fingerprint pass per season (SEG3), one season
/// at a time. `job_id`, when set, receives progress and per-file
/// errors as it goes. Ends by deleting cached fingerprints no file
/// references any more (D20).
pub async fn detect_library(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    job_id: Option<Uuid>,
) -> Result<DetectReport, DetectError> {
    let files: Vec<PendingFile> = sqlx::query_as(
        "SELECT p.id, p.item_id, p.season_id, p.path, p.relative_path, p.duration_ms,
                p.size_bytes, p.mtime,
                EXISTS (SELECT 1 FROM media_streams s
                        WHERE s.file_id = p.id AND s.kind = 'video') AS has_video,
                EXISTS (SELECT 1 FROM media_streams s
                        WHERE s.file_id = p.id AND s.kind = 'audio') AS has_audio
         FROM (
             SELECT DISTINCT ON (f.item_id) f.id, f.item_id, i.parent_id AS season_id,
                    f.path, f.relative_path, f.duration_ms, f.size_bytes, f.mtime,
                    f.segments_detected_at
             FROM media_files f
             JOIN media_items i ON i.id = f.item_id
             WHERE f.library_id = $1 AND i.kind = 'episode' AND f.available
             ORDER BY f.item_id, f.relative_path, f.path
         ) p
         WHERE p.segments_detected_at IS NULL
         ORDER BY p.relative_path, p.path",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    let total = files.len() as i32;
    if let Some(job) = job_id {
        let _ = parlour_jobs::set_progress(pool, job, 0, total).await;
    }
    let mut report = DetectReport {
        episodes: total,
        ..DetectReport::default()
    };
    // Group by season, in the order each season's first file sorts.
    let mut seasons: Vec<(Option<Uuid>, Vec<&PendingFile>)> = Vec::new();
    for file in &files {
        match seasons.iter_mut().find(|(id, _)| *id == file.season_id) {
            Some((_, list)) => list.push(file),
            None => seasons.push((file.season_id, vec![file])),
        }
    }
    let mut done = 0;
    for (season_id, season_files) in seasons {
        let mut examined = Vec::with_capacity(season_files.len());
        for file in season_files {
            let (found, errors) = detect_file(file)
                .await
                .map_err(DetectError::ToolUnavailable)?;
            if !errors.is_empty() {
                report.failed += 1;
                if let Some(job) = job_id {
                    for message in &errors {
                        let _ =
                            parlour_jobs::add_error(pool, job, Some(&file.relative_path), message)
                                .await;
                    }
                }
            }
            let stored = store(pool, file, &found).await?;
            examined.push(Examined {
                file,
                found,
                clean: errors.is_empty(),
                stored,
            });
            done += 1;
            if let Some(job) = job_id {
                let _ = parlour_jobs::set_progress(pool, job, done, total).await;
            }
        }
        // The whole season, re-matched by fingerprint.
        let outcome = match season_id {
            Some(season) => fingerprint::match_season(pool, season, job_id, &mut report).await?,
            None => Vec::new(),
        };
        finish_season(pool, &examined, &outcome, &mut report).await?;
    }
    report.fingerprints_removed = fingerprint::remove_unreferenced(pool).await? as i32;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start_s: f64, end_s: f64) -> Span {
        Span::new(secs_to_ms(start_s), secs_to_ms(end_s))
    }

    #[test]
    fn filter_arguments_are_d18s_sensitivity() {
        assert_eq!(BLACKDETECT, "blackdetect=d=0.5");
        assert_eq!(SILENCEDETECT, "silencedetect=n=-50dB:d=0.5");
        assert_eq!(seconds_arg(300_000), "300.000");
        assert_eq!(seconds_arg(1_234_567), "1234.567");
    }

    #[test]
    fn windows_are_the_first_300_and_last_600_seconds() {
        // A 45-minute episode.
        assert_eq!(head_window(2_700_000), Span::new(0, 300_000));
        assert_eq!(tail_window(2_700_000), Span::new(2_100_000, 2_700_000));
        // Shorter than a window: the window is the whole file.
        assert_eq!(head_window(70_000), Span::new(0, 70_000));
        assert_eq!(tail_window(70_000), Span::new(0, 70_000));
    }

    // Captured from ffmpeg 8.0.1 (host) and 5.1.9 (the runtime image)
    // over the same fixture: breaks at 10, 31 and 50 s.
    const LOG_FFMPEG_8: &str = "\
Input #0, matroska,webm, from 'black_start:99 silence_start: 1.mkv':
[silencedetect @ 0x7e50fc004440] silence_start: 10.0322
[silencedetect @ 0x7e50fc004440] silence_end: 11.029161 | silence_duration: 0.996961
[blackdetect @ 0x7e510c004400] black_start:10.023 black_end:11.123 black_duration:1.1
[silencedetect @ 0x7e50fc004440] silence_start: 31.044989
[silencedetect @ 0x7e50fc004440] silence_end: 32.043288 | silence_duration: 0.998299
[blackdetect @ 0x7e510c004400] black_start:31.023 black_end:32.123 black_duration:1.1
[silencedetect @ 0x7e50fc004440] silence_start: 50.038866
[silencedetect @ 0x7e50fc004440] silence_end: 51.037143 | silence_duration: 0.998277
[blackdetect @ 0x7e510c004400] black_start:50.023 black_end:51.123 black_duration:1.1
[out#0/null @ 0x645e0ebbf500] video:20KiB audio:0KiB subtitle:0KiB
";
    const LOG_FFMPEG_5: &str = "\
    title           : black_start:5 black_end:6
[silencedetect @ 0x6265263d05c0] silence_start: 10.0323
[silencedetect @ 0x6265263d05c0] silence_end: 11.0295 | silence_duration: 0.997279
[blackdetect @ 0x6265272aabc0] black_start:10.023 black_end:11.123 black_duration:1.1
[Parsed_silencedetect_1 @ 0x6265263d05c0] silence_start: 31.045
[Parsed_silencedetect_1 @ 0x6265263d05c0] silence_end: 32.0436 | silence_duration: 0.998594
[Parsed_blackdetect_0 @ 0x6265272aabc0] black_start:31.023 black_end:32.123 black_duration:1.1
[silencedetect @ 0x6265263d05c0] silence_start: 50.039
[silencedetect @ 0x6265263d05c0] silence_end: 51.0376 | silence_duration: 0.998594
[blackdetect @ 0x6265272aabc0] black_start:50.023 black_end:51.123 black_duration:1.1
";

    #[test]
    fn parses_detect_logs_from_ffmpeg_5_and_8() {
        for log in [LOG_FFMPEG_8, LOG_FFMPEG_5] {
            let runs = parse_detect_log(log, Span::new(0, 70_000));
            assert_eq!(
                runs.black,
                vec![
                    span(10.023, 11.123),
                    span(31.023, 32.123),
                    span(50.023, 51.123)
                ],
                "{log}"
            );
            assert_eq!(runs.silence.len(), 3, "{log}");
            assert_eq!(runs.silence[0].start_ms, 10_032);
            assert_eq!(
                breaks(&runs),
                vec![
                    span(10.023, 11.123),
                    span(31.023, 32.123),
                    span(50.023, 51.123)
                ]
            );
        }
    }

    #[test]
    fn tail_pass_times_are_shifted_into_file_time_and_open_runs_close_at_the_end() {
        // A pass seeked to 2100 s: ffmpeg's clock starts at 0 there.
        let log = "\
[blackdetect @ 0x1] black_start:540.5 black_end:541.5 black_duration:1
[silencedetect @ 0x2] silence_start: 540.6
[silencedetect @ 0x2] silence_end: 541.4 | silence_duration: 0.8
[silencedetect @ 0x2] silence_start: 598.9
";
        let runs = parse_detect_log(log, Span::new(2_100_000, 2_700_000));
        assert_eq!(runs.black, vec![span(2640.5, 2641.5)]);
        assert_eq!(
            runs.silence,
            vec![span(2640.6, 2641.4), span(2698.9, 2700.0)]
        );
        // Runs are clipped to the window; a negative start (encoder
        // priming) clips to the window's start.
        let log = "\
[silencedetect @ 0x2] silence_start: -0.023
[silencedetect @ 0x2] silence_end: 1.5 | silence_duration: 1.523
[blackdetect @ 0x1] black_start:299 black_end:302 black_duration:3
";
        let runs = parse_detect_log(log, Span::new(0, 300_000));
        assert_eq!(runs.silence, vec![span(0.0, 1.5)]);
        assert_eq!(runs.black, vec![span(299.0, 300.0)]);
        assert_eq!(parse_detect_log("", Span::new(0, 1_000)), Runs::default());
    }

    #[test]
    fn chapters_parse_from_ffprobe_json() {
        let json = r#"{"chapters": [
            {"id": 1, "time_base": "1/1000000000", "start": 0, "start_time": "0.000000",
             "end": 5000000000, "end_time": "5.000000", "tags": {"title": "Prologue"}},
            {"id": 2, "start_time": "5.000000", "end_time": "20.000000",
             "tags": {"TITLE": "Intro"}},
            {"id": 3, "start_time": "20.000000", "end_time": "50.000000"},
            {"id": 4, "start_time": "bad", "end_time": "60.000000", "tags": {"title": "x"}}
        ]}"#;
        let chapters = parse_chapters_json(json).unwrap();
        assert_eq!(
            chapters,
            vec![
                Chapter {
                    start_ms: 0,
                    end_ms: 5_000,
                    title: Some("Prologue".into())
                },
                Chapter {
                    start_ms: 5_000,
                    end_ms: 20_000,
                    title: Some("Intro".into())
                },
                Chapter {
                    start_ms: 20_000,
                    end_ms: 50_000,
                    title: None
                },
            ]
        );
        assert_eq!(parse_chapters_json("{}").unwrap(), vec![]);
        assert!(parse_chapters_json("not json").is_err());
    }

    #[test]
    fn chapter_titles_match_whole_words_ignoring_case() {
        use SegmentKind::*;
        for (title, kind) in [
            ("Intro", Some(Intro)),
            ("OPENING", Some(Intro)),
            ("Opening Theme", Some(Intro)),
            ("intro.", Some(Intro)),
            ("Credits", Some(Credits)),
            ("End Credits", Some(Credits)),
            ("closing credits", Some(Credits)),
            // Whole words only.
            ("Introduction", None),
            ("Reopening", None),
            ("Creditsroll", None),
            ("Chapter 1", None),
            ("", None),
            // Words for both kinds: the opening credits are the intro
            // (D19).
            ("Opening Credits", Some(Intro)),
            ("Credits (Intro)", Some(Intro)),
            // A hyphen separates words, so this names the credits;
            // `chapter_spans` keeps the earlier "Credits" chapter
            // when both are present.
            ("Post-Credits Scene", Some(Credits)),
        ] {
            assert_eq!(chapter_kind(title), kind, "{title:?}");
        }
    }

    #[test]
    fn the_earliest_chapter_wins_per_kind_and_bad_spans_are_skipped() {
        let ch = |start: i64, end: i64, title: &str| Chapter {
            start_ms: start,
            end_ms: end,
            title: Some(title.to_string()),
        };
        let chapters = vec![
            ch(65_000, 70_000, "Post-Credits Scene"),
            ch(0, 5_000, "Prologue"),
            ch(20_000, 20_000, "Intro"), // empty: skipped
            ch(5_000, 20_000, "Opening"),
            ch(20_000, 50_000, "Part A"),
            ch(50_000, 65_000, "Credits"),
            ch(30_000, 35_000, "Intro"), // later than "Opening"
        ];
        assert_eq!(
            chapter_spans(&chapters),
            (
                Some(Span::new(5_000, 20_000)),
                Some(Span::new(50_000, 65_000))
            )
        );
        assert_eq!(chapter_spans(&[]), (None, None));
        let untitled = vec![Chapter {
            start_ms: 0,
            end_ms: 10_000,
            title: None,
        }];
        assert_eq!(chapter_spans(&untitled), (None, None));
    }

    fn runs(black: &[(f64, f64)], silence: &[(f64, f64)]) -> Runs {
        Runs {
            black: black.iter().map(|&(a, b)| span(a, b)).collect(),
            silence: silence.iter().map(|&(a, b)| span(a, b)).collect(),
        }
    }

    #[test]
    fn a_break_needs_black_and_silence_within_two_seconds() {
        // Alone, neither kind of run is a break.
        assert!(breaks(&runs(&[(10.0, 11.0)], &[])).is_empty());
        assert!(breaks(&runs(&[], &[(10.0, 11.0)])).is_empty());
        // Overlapping runs make one break covering both.
        assert_eq!(
            breaks(&runs(&[(10.0, 11.0)], &[(10.5, 12.0)])),
            vec![span(10.0, 12.0)]
        );
        // Two seconds apart still align; a hair more does not.
        assert_eq!(
            breaks(&runs(&[(10.0, 11.0)], &[(13.0, 14.0)])),
            vec![span(10.0, 14.0)]
        );
        assert!(breaks(&runs(&[(10.0, 11.0)], &[(13.001, 14.0)])).is_empty());
        assert_eq!(
            breaks(&runs(&[(13.0, 14.0)], &[(10.0, 11.0)])),
            vec![span(10.0, 14.0)]
        );
        // Breaks that end up overlapping merge; unaligned runs drop out.
        assert_eq!(
            breaks(&runs(
                &[(10.0, 11.0), (12.0, 13.0), (40.0, 41.0), (90.0, 91.0)],
                &[(10.5, 12.5), (95.0, 96.0)],
            )),
            vec![span(10.0, 13.0)]
        );
    }

    #[test]
    fn the_intro_is_the_first_15_to_120_second_gap_starting_in_the_first_two_minutes() {
        let b = |list: &[(f64, f64)]| list.iter().map(|&(a, z)| span(a, z)).collect::<Vec<_>>();
        // The fixture's breaks: 0→10 s is too short, 11→31 s is it.
        assert_eq!(
            intro_span(&b(&[(10.0, 11.0), (31.0, 32.0), (50.0, 51.0)])),
            Some(span(11.0, 31.0))
        );
        // The file's start counts as a break.
        assert_eq!(intro_span(&b(&[(30.0, 31.0)])), Some(span(0.0, 30.0)));
        // A break at the very start is just a break.
        assert_eq!(
            intro_span(&b(&[(0.0, 2.0), (40.0, 41.0)])),
            Some(span(2.0, 40.0))
        );
        // Gap bounds are inclusive: 15 s and 120 s qualify...
        assert_eq!(
            intro_span(&b(&[(5.0, 6.0), (21.0, 22.0)])),
            Some(span(6.0, 21.0))
        );
        assert_eq!(
            intro_span(&b(&[(5.0, 6.0), (126.0, 127.0)])),
            Some(span(6.0, 126.0))
        );
        // ...a hair outside them does not.
        assert_eq!(intro_span(&b(&[(5.0, 6.0), (20.999, 22.0)])), None);
        assert_eq!(intro_span(&b(&[(5.0, 6.0), (126.001, 127.0)])), None);
        // The first break of the pair starts within 120 s (inclusive):
        // breaks every 10 s up to 110 s leave only 9 s gaps, so the
        // pair that can qualify starts at the 120 s mark.
        let every_ten: Vec<(f64, f64)> = (1..=11)
            .map(|k| (10.0 * k as f64, 10.0 * k as f64 + 1.0))
            .collect();
        let with = |first: f64| {
            let mut list = every_ten.clone();
            list.extend([(first, 121.0), (150.0, 151.0)]);
            b(&list)
        };
        assert_eq!(intro_span(&with(120.0)), Some(span(121.0, 150.0)));
        assert_eq!(intro_span(&with(120.001)), None);
        // First match wins.
        assert_eq!(
            intro_span(&b(&[(20.0, 21.0), (50.0, 51.0), (90.0, 91.0)])),
            Some(span(0.0, 20.0))
        );
        assert_eq!(intro_span(&[]), None);
    }

    #[test]
    fn credits_end_in_the_last_two_minutes_and_last_at_least_15_seconds() {
        let b = |list: &[(f64, f64)]| list.iter().map(|&(a, z)| span(a, z)).collect::<Vec<_>>();
        let t = 2_700_000; // a 45-minute episode
                           // A break, then credits to the end of the file.
        assert_eq!(
            credits_span(&b(&[(2640.0, 2641.0)]), t),
            Some(span(2641.0, 2700.0))
        );
        // Credits may also stop at the next break, if that break
        // starts in the last 120 s (inclusive; the span after it,
        // 10 s here, is too short to count)...
        assert_eq!(
            credits_span(&b(&[(2500.0, 2501.0), (2580.0, 2690.0)]), t),
            Some(span(2501.0, 2580.0))
        );
        // ...not when that break starts any earlier, and never
        // shorter than 15 s.
        assert_eq!(
            credits_span(&b(&[(2500.0, 2501.0), (2579.999, 2700.0)]), t),
            None
        );
        assert_eq!(credits_span(&b(&[(2685.001, 2686.0)]), t), None);
        assert_eq!(
            credits_span(&b(&[(2684.0, 2685.0)]), t),
            Some(span(2685.0, 2700.0))
        );
        // Black + silence over the last seconds: the span before it.
        assert_eq!(
            credits_span(&b(&[(2620.0, 2621.0), (2698.0, 2700.0)]), t),
            Some(span(2621.0, 2698.0))
        );
        // An act break before the credits break: both spans qualify,
        // and the one nearest the end wins (D19).
        assert_eq!(
            credits_span(&b(&[(2220.0, 2221.0), (2640.0, 2641.0)]), t),
            Some(span(2641.0, 2700.0))
        );
        // A break is needed: the file's start never opens credits.
        assert_eq!(credits_span(&[], t), None);
        assert_eq!(credits_span(&[], 70_000), None);
    }

    #[test]
    fn rows_are_stored_at_d17s_confidences() {
        assert_eq!(confidence_of(SegmentSource::Chapters), 1.0);
        // Below parlour-playback's 0.6 floor, by design.
        assert_eq!(confidence_of(SegmentSource::Heuristic), 0.5);
    }
}
