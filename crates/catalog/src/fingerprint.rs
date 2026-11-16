//! Segment detection — the audio-fingerprint pass (SEG3; D15, D20).
//!
//! Runs inside the `DetectSegments` job, after SEG2's cheap pass
//! (`segments`), for every season with a new or changed episode: the
//! whole season is re-matched and only its `fingerprint` rows are
//! replaced — manual, chapter and heuristic rows are never touched.
//! This is the intro-skipper idea (episodes of a season share their
//! intro and credits audio), implemented here from the algorithm's
//! description with D20's numbers.
//!
//! 1. **Fingerprints.** ffmpeg's chromaprint muxer fingerprints each
//!    episode's audio over D20's two search windows: the head — the
//!    first 25 % of the episode, capped at 10 minutes — for the intro,
//!    and the tail — the last 600 s — for the credits. Raw chromaprint
//!    is one 32-bit point every 1365/11025 s (0.124 s); point *i*
//!    stands for the audio from *i* steps into its window. The points
//!    are cached in `audio_fingerprints` by the SHA-256 of the file's
//!    size plus its first and last 4 MiB, so a file is fingerprinted
//!    once, wherever it moves.
//! 2. **Pairs.** For every pair of episodes, every alignment of their
//!    windows is tried. Two points match when at most 6 of their 32
//!    bits differ. A *shared stretch* runs from a matching point to a
//!    matching point, jumping gaps of at most 1 s; at least 80 % of
//!    its points match; and — the silence guard — at least half of
//!    its points are distinct values in each episode (silence, or a
//!    steady tone, fingerprints as one repeated value and would
//!    otherwise match perfectly). The same stretch found at
//!    neighbouring alignments counts once, at the alignment where the
//!    most points match. Every qualifying stretch is a candidate, not
//!    only the longest: an intro candidate is 15–120 s long (D18's
//!    limits); a credits candidate is at least 15 s long and ends
//!    within the last 120 s of the file (D17's limit) — in both
//!    episodes.
//! 3. **Season agreement.** Each episode groups its candidates by the
//!    stretch of its own audio they cover — two candidates agree when
//!    they overlap by at least half the shorter one — and keeps the
//!    group the most other episodes share, with the median of the
//!    group's edges. When two groups are shared equally, the intro is
//!    the earliest and the credits the one nearest the end: D18's
//!    "first match wins" and D19's mirror of it.
//! 4. **Confidence** is quality × agreement (D20). Quality is the
//!    share of the segment's points that match within 6 bits, pooled
//!    over the episodes that share it; agreement is 1 − 0.5ⁿ for n
//!    other episodes. Below 0.6 nothing is stored, so a segment needs
//!    at least 3 episodes sharing it — and a season with fewer than 3
//!    usable episodes is not matched at all.
//!
//! Fingerprint segments are stored only for a kind the episode's
//! chapters left open, as SEG2 does. Measured on synthetic episodes
//! (2026-11-15), edges land within 1.4 s of the true ones and nearly
//! always early — typically 0.5–1.2 s — because a point summarises
//! the next ~2.7 s of audio and points straddling an edge still
//! partly match; an early intro end never skips episode content.
//! Unrelated audio never matched for more than 1.7 s (D20).
//!
//! Steps 2–4 are pure functions over point arrays, unit-tested below;
//! `match_season` runs the tools and keeps the cache, and the job
//! (`segments::detect_library`) writes the rows.

use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use parlour_core::{SegmentKind, SegmentSource};

use crate::segments::{
    run, seconds_arg, stderr_tail, tool, DetectError, DetectReport, Span, ToolError,
    CREDITS_END_WITHIN_MS, CREDITS_MIN_MS, INTRO_MAX_MS, INTRO_MIN_MS,
};

// ------------------------------------------------------------ the rules

/// Raw chromaprint: mono audio at this rate...
pub const SAMPLE_RATE: i64 = 11_025;
/// ...gives one 32-bit point per this many samples (0.124 s).
pub const POINT_SAMPLES: i64 = 1_365;
/// Two points match when at most this many of their 32 bits differ
/// (D20).
pub const MAX_BIT_ERRORS: u32 = 6;
/// A shared stretch may jump gaps of unmatched points up to this long
/// (D20).
pub const MAX_GAP_MS: i64 = 1_000;
/// At least this share of a stretch's points must match (D20)...
pub const MIN_MATCHED_PERCENT: usize = 80;
/// ...and at least this share must be distinct values, in each
/// episode — the silence guard (D20).
pub const MIN_DISTINCT_PERCENT: usize = 50;
/// Results below this confidence are not stored at all (D20). The
/// same number as the playback floor, so every stored fingerprint row
/// can reach a player unless a chapter or manual row wins.
pub const MIN_CONFIDENCE: f32 = 0.6;
/// A season with fewer usable episodes than this is not matched (D20).
pub const MIN_SEASON_EPISODES: usize = 3;
/// The head window is the first 25 % of the episode...
pub const HEAD_PERCENT: i64 = 25;
/// ...capped at 10 minutes (D20).
pub const HEAD_CAP_MS: i64 = 600_000;
/// The tail window is the last 600 s (D20).
pub const TAIL_MS: i64 = 600_000;
/// The cache key hashes the file's size plus this much of each end
/// (D20).
pub const HASH_SAMPLE_BYTES: u64 = 4 * 1024 * 1024;

/// Where point `index` starts, in ms from its window's start.
pub fn point_ms(index: usize) -> i64 {
    (index as i64 * POINT_SAMPLES * 1000 + SAMPLE_RATE / 2) / SAMPLE_RATE
}

/// The most points `ms` covers whole.
fn points_in(ms: i64) -> usize {
    (ms.max(0) * SAMPLE_RATE / (POINT_SAMPLES * 1000)) as usize
}

/// The longest run of unmatched points a stretch may jump: 8 points
/// (0.99 s) fit in D20's 1 s.
pub fn max_gap_points() -> usize {
    points_in(MAX_GAP_MS)
}

/// Stretches shorter than half the shortest segment are dropped before
/// anything else looks at them: chance runs between unrelated audio
/// stay under 2 s (D20), and a segment is never shorter than 15 s.
fn min_stretch_points() -> usize {
    points_in(INTRO_MIN_MS.min(CREDITS_MIN_MS) / 2)
}

/// D20's search windows for an episode of `duration_ms`: the head
/// (first 25 %, at most 10 minutes) and the tail (last 600 s).
pub fn search_windows(duration_ms: i64) -> (Span, Span) {
    let duration_ms = duration_ms.max(0);
    (
        Span::new(0, (duration_ms * HEAD_PERCENT / 100).min(HEAD_CAP_MS)),
        Span::new((duration_ms - TAIL_MS).max(0), duration_ms),
    )
}

/// Chromaprint's raw output (`-fp_format raw`): little-endian u32s.
pub fn parse_points(raw: &[u8]) -> Result<Vec<u32>, String> {
    if raw.len() % 4 != 0 {
        return Err(format!(
            "the fingerprint is {} bytes, not whole 4-byte points",
            raw.len()
        ));
    }
    Ok(raw
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn percent_at_least(part: usize, whole: usize, percent: usize) -> bool {
    part * 100 >= whole * percent
}

fn matches(x: u32, y: u32) -> bool {
    (x ^ y).count_ones() <= MAX_BIT_ERRORS
}

fn distinct(points: &[u32]) -> usize {
    let mut values = points.to_vec();
    values.sort_unstable();
    values.dedup();
    values.len()
}

fn overlap(x: &Range<usize>, y: &Range<usize>) -> usize {
    x.end.min(y.end).saturating_sub(x.start.max(y.start))
}

/// Two stretches of one episode's audio are the same stretch when they
/// overlap by at least half the shorter one.
fn same_stretch(x: &Range<usize>, y: &Range<usize>) -> bool {
    overlap(x, y) * 2 >= x.len().min(y.len())
}

/// A stretch two windows share: `len` points from `a_start` in the
/// first and from `b_start` in the second, `matched` of them within
/// 6 bits of each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stretch {
    pub a_start: usize,
    pub b_start: usize,
    pub len: usize,
    pub matched: usize,
}

impl Stretch {
    fn a(&self) -> Range<usize> {
        self.a_start..self.a_start + self.len
    }

    fn b(&self) -> Range<usize> {
        self.b_start..self.b_start + self.len
    }
}

/// Every stretch windows `a` and `b` share (D20) that is at least
/// `min_len` points long, trying every alignment. The same stretch
/// found at several neighbouring alignments — chromaprint points
/// change slowly, so a shift of a point or two still matches — is
/// kept once, where the most points match.
pub fn shared_stretches(a: &[u32], b: &[u32], min_len: usize) -> Vec<Stretch> {
    let min_len = min_len.max(1);
    let gap = max_gap_points();
    let mut found = Vec::new();
    if a.len() < min_len || b.len() < min_len {
        return found;
    }
    let mut hit = vec![false; a.len().min(b.len())];
    // `shift` = index in `a` − index in `b`; every alignment that
    // overlaps by at least `min_len` points.
    for shift in (min_len as isize - b.len() as isize)..=(a.len() - min_len) as isize {
        let a0 = shift.max(0) as usize;
        let b0 = (-shift).max(0) as usize;
        let n = (a.len() - a0).min(b.len() - b0);
        let (aw, bw) = (&a[a0..a0 + n], &b[b0..b0 + n]);
        // Almost every alignment is unrelated audio: count first (a
        // loop the compiler vectorises) and look closer only when
        // there are enough matches for even one stretch.
        let hits = aw.iter().zip(bw).filter(|(x, y)| matches(**x, **y)).count();
        if !percent_at_least(hits, min_len, MIN_MATCHED_PERCENT) {
            continue;
        }
        for (h, (x, y)) in hit.iter_mut().zip(aw.iter().zip(bw)) {
            *h = matches(*x, *y);
        }
        let mut k = 0;
        while k < n {
            if !hit[k] {
                k += 1;
                continue;
            }
            // A run: matching points with gaps of at most `gap`
            // unmatched points between them.
            let (start, mut last, mut matched) = (k, k, 1);
            k += 1;
            while k < n && k - last <= gap + 1 {
                if hit[k] {
                    last = k;
                    matched += 1;
                }
                k += 1;
            }
            let len = last - start + 1;
            if len >= min_len
                && percent_at_least(matched, len, MIN_MATCHED_PERCENT)
                && percent_at_least(
                    distinct(&a[a0 + start..=a0 + last]),
                    len,
                    MIN_DISTINCT_PERCENT,
                )
                && percent_at_least(
                    distinct(&b[b0 + start..=b0 + last]),
                    len,
                    MIN_DISTINCT_PERCENT,
                )
            {
                found.push(Stretch {
                    a_start: a0 + start,
                    b_start: b0 + start,
                    len,
                    matched,
                });
            }
        }
    }
    // Best first, so a stretch's best alignment is the one kept.
    found.sort_by(|x, y| {
        y.matched
            .cmp(&x.matched)
            .then(x.a_start.cmp(&y.a_start))
            .then(x.b_start.cmp(&y.b_start))
    });
    let mut kept: Vec<Stretch> = Vec::new();
    for s in found {
        if !kept
            .iter()
            .any(|k| same_stretch(&k.a(), &s.a()) && same_stretch(&k.b(), &s.b()))
        {
            kept.push(s);
        }
    }
    kept.sort_by_key(|s| (s.a_start, s.b_start));
    kept
}

/// One episode's fingerprint over one search window.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowPrint {
    /// File time of the window's first point.
    pub start_ms: i64,
    pub points: Vec<u32>,
    /// The episode's length: the credits must end near it.
    pub duration_ms: i64,
}

impl WindowPrint {
    /// Points `range` of this window, in file time.
    fn span(&self, range: Range<usize>) -> Span {
        Span::new(
            self.start_ms + point_ms(range.start),
            self.start_ms + point_ms(range.end),
        )
    }
}

/// Whether `span` can be a segment of `kind` in an episode of
/// `duration_ms`: an intro is 15–120 s long (D18's limits, D20); the
/// credits are at least 15 s long and end within the last 120 s
/// (D17's limit, D20).
fn fits(kind: SegmentKind, span: Span, duration_ms: i64) -> bool {
    match kind {
        SegmentKind::Intro => (INTRO_MIN_MS..=INTRO_MAX_MS).contains(&span.len_ms()),
        SegmentKind::Credits => {
            span.len_ms() >= CREDITS_MIN_MS && span.end_ms >= duration_ms - CREDITS_END_WITHIN_MS
        }
        SegmentKind::Recap => false,
    }
}

/// D20's agreement for a segment `n` other episodes share: 1 − 0.5ⁿ.
pub fn agreement(n: usize) -> f64 {
    1.0 - 0.5f64.powi(n.min(64) as i32)
}

/// What the season agreed on for one episode and one kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matched {
    pub span: Span,
    /// Quality × agreement (D20); never below 0.6.
    pub confidence: f32,
    /// How many other episodes share the segment.
    pub sharing: usize,
}

/// One stretch of an episode's window that another episode shares.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    partner: usize,
    start: usize,
    end: usize,
    /// The partner's point index is this episode's minus `offset`.
    offset: isize,
}

impl Candidate {
    fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// How many of `own`'s points in `range` match `other`'s at `offset`.
fn matching_points(own: &[u32], other: &[u32], range: Range<usize>, offset: isize) -> usize {
    range
        .filter(|&p| {
            let q = p as isize - offset;
            q >= 0 && (q as usize) < other.len() && matches(own[p], other[q as usize])
        })
        .count()
}

fn median(mut values: Vec<usize>) -> usize {
    values.sort_unstable();
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2
    }
}

/// Match one search window across a season's episodes (D20): for each
/// episode, in order, the `kind` segment the season agrees on, or
/// `None`. `windows` are every usable episode's prints of the same
/// window (all heads for the intro, all tails for the credits).
pub fn match_windows(windows: &[WindowPrint], kind: SegmentKind) -> Vec<Option<Matched>> {
    let mut candidates: Vec<Vec<Candidate>> = vec![Vec::new(); windows.len()];
    for i in 0..windows.len() {
        for j in i + 1..windows.len() {
            let (a, b) = (&windows[i], &windows[j]);
            for s in shared_stretches(&a.points, &b.points, min_stretch_points()) {
                if !fits(kind, a.span(s.a()), a.duration_ms)
                    || !fits(kind, b.span(s.b()), b.duration_ms)
                {
                    continue;
                }
                let offset = s.a_start as isize - s.b_start as isize;
                candidates[i].push(Candidate {
                    partner: j,
                    start: s.a_start,
                    end: s.a_start + s.len,
                    offset,
                });
                candidates[j].push(Candidate {
                    partner: i,
                    start: s.b_start,
                    end: s.b_start + s.len,
                    offset: -offset,
                });
            }
        }
    }
    (0..windows.len())
        .map(|i| agree(i, windows, &candidates[i], kind))
        .collect()
}

/// The segment of episode `own` most of the season agrees on, if it
/// clears D20's floor.
fn agree(
    own: usize,
    windows: &[WindowPrint],
    candidates: &[Candidate],
    kind: SegmentKind,
) -> Option<Matched> {
    let window = &windows[own];
    let mut partners: Vec<usize> = candidates.iter().map(|c| c.partner).collect();
    partners.sort_unstable();
    partners.dedup();

    let mut best: Option<(Vec<Candidate>, Range<usize>, Span)> = None;
    for anchor in candidates {
        // From each other episode, the candidate that agrees with the
        // anchor and covers the most of it.
        let group: Vec<Candidate> = partners
            .iter()
            .filter_map(|&p| {
                candidates
                    .iter()
                    .filter(|c| c.partner == p && same_stretch(&anchor.range(), &c.range()))
                    .max_by_key(|c| {
                        (
                            overlap(&anchor.range(), &c.range()),
                            std::cmp::Reverse(c.start),
                        )
                    })
                    .copied()
            })
            .collect();
        let points = median(group.iter().map(|c| c.start).collect())
            ..median(group.iter().map(|c| c.end).collect());
        if points.is_empty() {
            continue;
        }
        let span = window.span(points.clone());
        if !fits(kind, span, window.duration_ms) {
            continue;
        }
        let replaces = match &best {
            None => true,
            Some((current, _, current_span)) => {
                group.len() > current.len()
                    || (group.len() == current.len()
                        && match kind {
                            // Nearest the end wins (D19).
                            SegmentKind::Credits => {
                                (span.end_ms, span.start_ms)
                                    > (current_span.end_ms, current_span.start_ms)
                            }
                            // First match wins (D18).
                            _ => {
                                (span.start_ms, span.end_ms)
                                    < (current_span.start_ms, current_span.end_ms)
                            }
                        })
            }
        };
        if replaces {
            best = Some((group, points, span));
        }
    }

    let (group, points, span) = best?;
    let matched: usize = group
        .iter()
        .map(|c| {
            matching_points(
                &window.points,
                &windows[c.partner].points,
                points.clone(),
                c.offset,
            )
        })
        .sum();
    let quality = matched as f64 / (group.len() * points.len()) as f64;
    let confidence = (quality * agreement(group.len())) as f32;
    (confidence >= MIN_CONFIDENCE).then_some(Matched {
        span,
        confidence,
        sharing: group.len(),
    })
}

// ------------------------------------------------------------ the cache

/// D20's cache key: the SHA-256 of the file's size (8 bytes,
/// little-endian), then its first 4 MiB, then its last 4 MiB — each
/// the whole file when it is shorter than that.
pub fn file_hash_of(size: u64, first: &[u8], last: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    hasher.update(first);
    hasher.update(last);
    hasher.finalize().to_vec()
}

/// Read a file's ends and hash them. `None` when the file is not
/// `expected_size` bytes any more: it changed after the scan, which
/// will look at it again.
pub fn hash_file(path: &str, expected_size: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    if size != expected_size {
        return Ok(None);
    }
    let sample = HASH_SAMPLE_BYTES.min(size) as usize;
    let mut first = vec![0u8; sample];
    file.read_exact(&mut first)?;
    let mut last = vec![0u8; sample];
    file.seek(SeekFrom::Start(size - sample as u64))?;
    file.read_exact(&mut last)?;
    Ok(Some(file_hash_of(size, &first, &last)))
}

/// One ffmpeg run fingerprinting the first audio stream over
/// `window`: the head reads `window`'s length from the start, the
/// tail seeks to it and reads to the end. Returns the raw points (an
/// empty fingerprint is normal for audio under ~3 s). `PARLOUR_FFMPEG`
/// overrides the binary, as for SEG2's pass.
async fn fingerprint_window(path: &str, window: Span, is_tail: bool) -> Result<Vec<u8>, ToolError> {
    if window.len_ms() <= 0 {
        return Ok(Vec::new());
    }
    let bin = tool("PARLOUR_FFMPEG", "ffmpeg");
    let mut args: Vec<String> = ["-hide_banner", "-nostats", "-nostdin", "-v", "error"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if window.start_ms > 0 {
        args.extend(["-ss".into(), seconds_arg(window.start_ms)]);
    }
    if !is_tail {
        args.extend(["-t".into(), seconds_arg(window.len_ms())]);
    }
    let rate = SAMPLE_RATE.to_string();
    args.extend(
        [
            "-i",
            path,
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            &rate,
            "-f",
            "chromaprint",
            "-fp_format",
            "raw",
            "-",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let output = run(&bin, &args).await?;
    if !output.status.success() {
        return Err(ToolError::Failed(format!(
            "ffmpeg could not fingerprint the audio at {}–{} s: {}",
            window.start_ms / 1000,
            window.end_ms / 1000,
            stderr_tail(&output.stderr)
        )));
    }
    parse_points(&output.stdout).map_err(ToolError::Failed)?;
    Ok(output.stdout)
}

/// Why an episode has no fingerprint this run.
enum NoPrint {
    /// The file is not the one the scan saw; the next scan looks again.
    Changed,
    Tool(ToolError),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for NoPrint {
    fn from(e: sqlx::Error) -> Self {
        NoPrint::Db(e)
    }
}

impl From<ToolError> for NoPrint {
    fn from(e: ToolError) -> Self {
        NoPrint::Tool(e)
    }
}

/// An episode's playback file, as season matching needs it.
#[derive(Debug, sqlx::FromRow)]
struct SeasonFile {
    id: Uuid,
    item_id: Uuid,
    path: String,
    relative_path: String,
    duration_ms: Option<i64>,
    size_bytes: i64,
    mtime: DateTime<Utc>,
    file_hash: Option<Vec<u8>>,
    has_audio: bool,
    chapter_intro: bool,
    chapter_credits: bool,
}

/// Point this file's row at its fingerprint — only if the row still
/// describes the bytes that were hashed.
async fn link_hash(
    conn: &mut sqlx::PgConnection,
    file: &SeasonFile,
    hash: &[u8],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE media_files SET file_hash = $2
         WHERE id = $1 AND size_bytes = $3 AND mtime = $4",
    )
    .bind(file.id)
    .bind(hash)
    .bind(file.size_bytes)
    .bind(file.mtime)
    .execute(conn)
    .await?;
    Ok(())
}

/// An episode's head and tail prints: from the cache when it has
/// them for these windows, else from ffmpeg (then cached). The flag is
/// true for a cache hit.
async fn episode_prints(
    pool: &parlour_db::PgPool,
    file: &SeasonFile,
    duration_ms: i64,
) -> Result<(WindowPrint, WindowPrint, bool), NoPrint> {
    let (head, tail) = search_windows(duration_ms);
    let hash_now = || async {
        let (path, size) = (file.path.clone(), file.size_bytes as u64);
        match tokio::task::spawn_blocking(move || hash_file(&path, size)).await {
            Ok(Ok(Some(hash))) => Ok(hash),
            Ok(Ok(None)) => Err(NoPrint::Changed),
            Ok(Err(e)) => Err(NoPrint::Tool(ToolError::Failed(format!(
                "could not read the file to fingerprint it: {e}"
            )))),
            Err(e) => Err(NoPrint::Tool(ToolError::Failed(format!(
                "could not hash the file: {e}"
            )))),
        }
    };
    // A stored hash is good for a cache lookup: the scanner clears it
    // when the file changes.
    let mut hash = match &file.file_hash {
        Some(hash) => hash.clone(),
        None => hash_now().await?,
    };
    let cached: Option<(i64, Vec<u8>, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT head_end_ms, head_points, tail_start_ms, tail_points
         FROM audio_fingerprints WHERE file_hash = $1",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    let (head_raw, tail_raw, reused) = match cached {
        Some((head_end_ms, head_raw, tail_start_ms, tail_raw))
            if head_end_ms == head.end_ms && tail_start_ms == tail.start_ms =>
        {
            if file.file_hash.is_none() {
                link_hash(&mut *pool.acquire().await?, file, &hash).await?;
            }
            (head_raw, tail_raw, true)
        }
        _ => {
            // About to read the file itself: hash what is there now.
            if file.file_hash.is_some() {
                hash = hash_now().await?;
            }
            let head_raw = fingerprint_window(&file.path, head, false).await?;
            let tail_raw = fingerprint_window(&file.path, tail, true).await?;
            let mut tx = pool.begin().await?;
            sqlx::query(
                "INSERT INTO audio_fingerprints
                     (file_hash, head_end_ms, head_points, tail_start_ms, tail_points)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (file_hash) DO UPDATE
                 SET head_end_ms = EXCLUDED.head_end_ms, head_points = EXCLUDED.head_points,
                     tail_start_ms = EXCLUDED.tail_start_ms, tail_points = EXCLUDED.tail_points",
            )
            .bind(&hash)
            .bind(head.end_ms)
            .bind(&head_raw)
            .bind(tail.start_ms)
            .bind(&tail_raw)
            .execute(&mut *tx)
            .await?;
            link_hash(&mut tx, file, &hash).await?;
            tx.commit().await?;
            (head_raw, tail_raw, false)
        }
    };
    let print = |window: Span, raw: &[u8]| {
        parse_points(raw)
            .map(|points| WindowPrint {
                start_ms: window.start_ms,
                points,
                duration_ms,
            })
            .map_err(|e| NoPrint::Tool(ToolError::Failed(e)))
    };
    Ok((print(head, &head_raw)?, print(tail, &tail_raw)?, reused))
}

// --------------------------------------------------------- the season

/// One episode's result from a season match: the segments to store as
/// its `fingerprint` rows (`None` = no row), and the file they were
/// computed from.
#[derive(Debug)]
pub(crate) struct EpisodeOutcome {
    pub file_id: Uuid,
    pub item_id: Uuid,
    pub size_bytes: i64,
    pub mtime: DateTime<Utc>,
    pub intro: Option<Matched>,
    pub credits: Option<Matched>,
}

/// Re-match a whole season (D20): fingerprint every usable episode
/// (one with an audio stream and a known length), then match heads
/// for the intro and tails for the credits. Returns an outcome for
/// every available episode — those that got nothing included, so the
/// caller can clear their old rows. A season with fewer than 3 usable
/// episodes is not matched: every outcome is empty.
pub(crate) async fn match_season(
    pool: &parlour_db::PgPool,
    season_id: Uuid,
    job_id: Option<Uuid>,
    report: &mut DetectReport,
) -> Result<Vec<EpisodeOutcome>, DetectError> {
    let files: Vec<SeasonFile> = sqlx::query_as(
        "SELECT p.id, p.item_id, p.path, p.relative_path, p.duration_ms, p.size_bytes,
                p.mtime, p.file_hash,
                EXISTS (SELECT 1 FROM media_streams s
                        WHERE s.file_id = p.id AND s.kind = 'audio') AS has_audio,
                EXISTS (SELECT 1 FROM media_segments m
                        WHERE m.item_id = p.item_id AND m.kind = 'intro'
                          AND m.source = 'chapters') AS chapter_intro,
                EXISTS (SELECT 1 FROM media_segments m
                        WHERE m.item_id = p.item_id AND m.kind = 'credits'
                          AND m.source = 'chapters') AS chapter_credits
         FROM (
             SELECT DISTINCT ON (f.item_id) f.id, f.item_id, f.path, f.relative_path,
                    f.duration_ms, f.size_bytes, f.mtime, f.file_hash
             FROM media_files f
             JOIN media_items i ON i.id = f.item_id
             WHERE i.parent_id = $1 AND i.kind = 'episode' AND f.available
             ORDER BY f.item_id, f.relative_path, f.path
         ) p
         ORDER BY p.relative_path, p.path",
    )
    .bind(season_id)
    .fetch_all(pool)
    .await?;

    let mut outcome: Vec<EpisodeOutcome> = files
        .iter()
        .map(|f| EpisodeOutcome {
            file_id: f.id,
            item_id: f.item_id,
            size_bytes: f.size_bytes,
            mtime: f.mtime,
            intro: None,
            credits: None,
        })
        .collect();
    let usable: Vec<(usize, i64)> = files
        .iter()
        .enumerate()
        .filter_map(|(i, f)| match f.duration_ms {
            Some(d) if d > 0 && f.has_audio => Some((i, d)),
            _ => None,
        })
        .collect();
    if usable.len() < MIN_SEASON_EPISODES {
        report.seasons_skipped += 1;
        return Ok(outcome);
    }

    let (mut printed, mut heads, mut tails) = (Vec::new(), Vec::new(), Vec::new());
    for (i, duration_ms) in usable {
        let file = &files[i];
        match episode_prints(pool, file, duration_ms).await {
            Ok((head, tail, reused)) => {
                if reused {
                    report.fingerprints_reused += 1;
                } else {
                    report.fingerprinted += 1;
                }
                printed.push(i);
                heads.push(head);
                tails.push(tail);
            }
            Err(NoPrint::Changed) => {}
            Err(NoPrint::Db(e)) => return Err(e.into()),
            Err(NoPrint::Tool(ToolError::Unavailable(msg))) => {
                return Err(DetectError::ToolUnavailable(msg))
            }
            Err(NoPrint::Tool(ToolError::Failed(msg))) => {
                report.fingerprint_failed += 1;
                if let Some(job) = job_id {
                    let _ =
                        parlour_jobs::add_error(pool, job, Some(&file.relative_path), &msg).await;
                }
            }
        }
    }
    if printed.len() < MIN_SEASON_EPISODES {
        report.seasons_skipped += 1;
        return Ok(outcome);
    }
    report.seasons_matched += 1;

    // Pure CPU work, off the async runtime.
    let (intros, credits) = tokio::task::spawn_blocking(move || {
        (
            match_windows(&heads, SegmentKind::Intro),
            match_windows(&tails, SegmentKind::Credits),
        )
    })
    .await
    .map_err(|e| DetectError::Matching(e.to_string()))?;
    for (k, &i) in printed.iter().enumerate() {
        // Only where the chapters left the kind open (D20, as SEG2).
        if !files[i].chapter_intro {
            outcome[i].intro = intros[k];
        }
        if !files[i].chapter_credits {
            outcome[i].credits = credits[k];
        }
    }
    Ok(outcome)
}

/// Replace one episode's `fingerprint` rows with a season match's
/// outcome: upsert what was found, delete the kinds that were not.
pub(crate) async fn write_rows(
    conn: &mut sqlx::PgConnection,
    outcome: &EpisodeOutcome,
) -> Result<(), sqlx::Error> {
    let source = SegmentSource::Fingerprint.as_str();
    for (kind, slot) in [
        (SegmentKind::Intro, outcome.intro),
        (SegmentKind::Credits, outcome.credits),
    ] {
        match slot {
            Some(found) => {
                sqlx::query(
                    "INSERT INTO media_segments
                         (item_id, kind, start_ms, end_ms, source, confidence)
                     VALUES ($1, $2, $3, $4, $5, $6)
                     ON CONFLICT (item_id, kind, source) DO UPDATE
                     SET start_ms = EXCLUDED.start_ms, end_ms = EXCLUDED.end_ms,
                         confidence = EXCLUDED.confidence, updated_at = now()",
                )
                .bind(outcome.item_id)
                .bind(kind.as_str())
                .bind(found.span.start_ms)
                .bind(found.span.end_ms)
                .bind(source)
                .bind(found.confidence)
                .execute(&mut *conn)
                .await?;
            }
            None => {
                sqlx::query(
                    "DELETE FROM media_segments
                     WHERE item_id = $1 AND kind = $2 AND source = $3",
                )
                .bind(outcome.item_id)
                .bind(kind.as_str())
                .bind(source)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    Ok(())
}

/// Delete cached fingerprints no file row references any more (D20).
pub(crate) async fn remove_unreferenced(pool: &parlour_db::PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "DELETE FROM audio_fingerprints a
         WHERE NOT EXISTS (SELECT 1 FROM media_files f WHERE f.file_hash = a.file_hash)",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64*: deterministic stand-in points for unrelated audio.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
        }

        fn points(seed: u64, len: usize) -> Vec<u32> {
            let mut noise = Noise(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            (0..len).map(|_| noise.next()).collect()
        }
    }

    /// `points` as another encode of the same audio would print them:
    /// up to `bits` (≤ 6) bits flipped per point.
    fn reencoded(points: &[u32], seed: u64, bits: u32) -> Vec<u32> {
        let mut noise = Noise(seed | 1);
        points
            .iter()
            .map(|&p| {
                let mut out = p;
                for _ in 0..(noise.next() % (bits + 1)) {
                    out ^= 1 << (noise.next() % 32);
                }
                out
            })
            .collect()
    }

    /// `body` with `shared` pasted in at `at`.
    fn with(mut body: Vec<u32>, at: usize, shared: &[u32]) -> Vec<u32> {
        body[at..at + shared.len()].copy_from_slice(shared);
        body
    }

    /// Points that last `secs` seconds.
    fn pts(secs: f64) -> usize {
        (secs * SAMPLE_RATE as f64 / POINT_SAMPLES as f64).round() as usize
    }

    fn head(points: Vec<u32>) -> WindowPrint {
        WindowPrint {
            start_ms: 0,
            duration_ms: 4 * point_ms(points.len()),
            points,
        }
    }

    #[test]
    fn d20s_numbers() {
        assert_eq!(MAX_BIT_ERRORS, 6);
        assert_eq!(MAX_GAP_MS, 1_000);
        assert_eq!(max_gap_points(), 8, "8 points = 0.99 s; 9 would be 1.11 s");
        assert_eq!(MIN_MATCHED_PERCENT, 80);
        assert_eq!(MIN_DISTINCT_PERCENT, 50);
        assert_eq!(MIN_CONFIDENCE, 0.6);
        assert_eq!(MIN_SEASON_EPISODES, 3);
        assert_eq!(HASH_SAMPLE_BYTES, 4 * 1024 * 1024);
        // Agreement 1 − 0.5ⁿ: one other episode can never clear the
        // floor; two can, at quality 0.8 or better.
        assert_eq!(agreement(0), 0.0);
        assert_eq!(agreement(1), 0.5);
        assert_eq!(agreement(2), 0.75);
        assert_eq!(agreement(3), 0.875);
        assert!(((0.8 * agreement(2)) as f32) >= MIN_CONFIDENCE);
        assert!(((0.79 * agreement(2)) as f32) < MIN_CONFIDENCE);
    }

    #[test]
    fn points_are_chromaprint_steps() {
        assert_eq!(point_ms(0), 0);
        assert_eq!(point_ms(1), 124); // 123.8 ms
        assert_eq!(point_ms(11_025), 1_365_000);
        assert_eq!(points_in(1_000), 8);
        assert_eq!(points_in(15_000), 121);
        assert_eq!(min_stretch_points(), 60); // 7.5 s
    }

    #[test]
    fn search_windows_are_the_first_quarter_capped_and_the_last_600_seconds() {
        // 45 minutes: a quarter is 11.25 min, capped at 10.
        assert_eq!(
            search_windows(2_700_000),
            (Span::new(0, 600_000), Span::new(2_100_000, 2_700_000))
        );
        // 22 minutes: the first 5.5 min, the last 10.
        assert_eq!(
            search_windows(1_320_000),
            (Span::new(0, 330_000), Span::new(720_000, 1_320_000))
        );
        // Under 10 minutes the tail is the whole file.
        assert_eq!(
            search_windows(480_000),
            (Span::new(0, 120_000), Span::new(0, 480_000))
        );
        assert_eq!(search_windows(0), (Span::new(0, 0), Span::new(0, 0)));
    }

    #[test]
    fn raw_points_are_little_endian_u32s() {
        assert_eq!(
            parse_points(&[0x77, 0x46, 0x6E, 0x25, 1, 0, 0, 0]).unwrap(),
            vec![627_983_991, 1]
        );
        assert_eq!(parse_points(&[]).unwrap(), Vec::<u32>::new());
        assert!(parse_points(&[1, 2, 3]).is_err());
    }

    #[test]
    fn the_cache_key_is_size_then_first_then_last_4_mib() {
        let dir = std::env::temp_dir().join(format!("parlour-fphash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Small: both "ends" are the whole file.
        let small = dir.join("small.bin");
        std::fs::write(&small, b"episode").unwrap();
        let mut expected = Sha256::new();
        expected.update(7u64.to_le_bytes());
        expected.update(b"episode");
        expected.update(b"episode");
        assert_eq!(
            hash_file(small.to_str().unwrap(), 7).unwrap(),
            Some(expected.finalize().to_vec())
        );
        // Big: the middle is never read, so changing it keeps the key.
        let mib = 1024 * 1024;
        let mut bytes: Vec<u8> = (0..9 * mib).map(|i| (i % 251) as u8).collect();
        let big = dir.join("big.bin");
        std::fs::write(&big, &bytes).unwrap();
        let key = hash_file(big.to_str().unwrap(), bytes.len() as u64)
            .unwrap()
            .unwrap();
        assert_eq!(
            key,
            file_hash_of(9 * mib as u64, &bytes[..4 * mib], &bytes[5 * mib..])
        );
        bytes[4 * mib + 10] ^= 0xFF;
        std::fs::write(&big, &bytes).unwrap();
        assert_eq!(
            hash_file(big.to_str().unwrap(), bytes.len() as u64).unwrap(),
            Some(key.clone())
        );
        bytes[0] ^= 0xFF;
        std::fs::write(&big, &bytes).unwrap();
        assert_ne!(
            hash_file(big.to_str().unwrap(), bytes.len() as u64).unwrap(),
            Some(key)
        );
        // A file that is not the size the scan saw is not hashed.
        assert_eq!(hash_file(small.to_str().unwrap(), 8).unwrap(), None);
        assert!(hash_file(dir.join("missing").to_str().unwrap(), 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unrelated_audio_shares_nothing() {
        let a = Noise::points(1, 2_000);
        let b = Noise::points(2, 2_000);
        assert!(shared_stretches(&a, &b, min_stretch_points()).is_empty());
    }

    #[test]
    fn a_shared_stretch_is_found_once_wherever_it_sits() {
        let theme = Noise::points(7, 300);
        let a = with(Noise::points(1, 1_000), 120, &theme);
        let b = with(Noise::points(2, 900), 560, &reencoded(&theme, 3, 6));
        assert_eq!(
            shared_stretches(&a, &b, min_stretch_points()),
            vec![Stretch {
                a_start: 120,
                b_start: 560,
                len: 300,
                matched: 300
            }]
        );
        // Shorter than asked for: not a stretch.
        assert!(shared_stretches(&a, &b, 301).is_empty());
    }

    #[test]
    fn gaps_up_to_one_second_are_jumped() {
        let theme = Noise::points(7, 300);
        // Points 100.. differ completely for `gap` points.
        let broken = |gap: usize| {
            let mut copy = theme.clone();
            for p in &mut copy[100..100 + gap] {
                *p = !*p;
            }
            with(Noise::points(2, 500), 50, &copy)
        };
        let a = with(Noise::points(1, 500), 100, &theme);
        // 8 unmatched points (0.99 s): still one stretch, 292 of 300
        // matching.
        assert_eq!(
            shared_stretches(&a, &broken(8), 60),
            vec![Stretch {
                a_start: 100,
                b_start: 50,
                len: 300,
                matched: 292
            }]
        );
        // 9 (1.11 s): two stretches either side of the gap.
        assert_eq!(
            shared_stretches(&a, &broken(9), 60),
            vec![
                Stretch {
                    a_start: 100,
                    b_start: 50,
                    len: 100,
                    matched: 100
                },
                Stretch {
                    a_start: 209,
                    b_start: 159,
                    len: 191,
                    matched: 191
                },
            ]
        );
    }

    #[test]
    fn at_least_80_percent_of_a_stretch_must_match() {
        let theme = Noise::points(7, 300);
        // Every `every`-th point differs completely.
        let thinned = |every: usize| {
            let copy: Vec<u32> = theme
                .iter()
                .enumerate()
                .map(|(i, &p)| if i % every == every - 1 { !p } else { p })
                .collect();
            with(Noise::points(2, 500), 50, &copy)
        };
        let a = with(Noise::points(1, 500), 100, &theme);
        // One in five differs: exactly 80 % match — enough.
        let found = shared_stretches(&a, &thinned(5), 60);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].a_start, 100);
        assert_eq!(found[0].matched * 100 / found[0].len, 80);
        // One in four: 75 % — not a stretch, though no gap exceeds 1 s.
        assert!(shared_stretches(&a, &thinned(4), 60).is_empty());
    }

    #[test]
    fn silence_never_matches_silence() {
        // Digital silence and steady tones print one repeated value
        // (627964279 is chromaprint's silence): a perfect bitwise
        // match the silence guard must refuse.
        let silence = vec![627_964_279u32; 400];
        let a = with(Noise::points(1, 1_000), 100, &silence);
        let b = with(Noise::points(2, 1_000), 500, &silence);
        assert!(shared_stretches(&a, &b, 60).is_empty());
        // Half distinct values is enough (199 + silence = 200 of 400);
        // fewer is not (199 of 400).
        let mut half: Vec<u32> = Noise::points(9, 199);
        half.extend(vec![627_964_279u32; 201]);
        let a = with(Noise::points(1, 1_000), 100, &half);
        let b = with(Noise::points(2, 1_000), 500, &half);
        assert_eq!(shared_stretches(&a, &b, 60).len(), 1);
        let mut under: Vec<u32> = Noise::points(9, 198);
        under.extend(vec![627_964_279u32; 202]);
        let a = with(Noise::points(1, 1_000), 100, &under);
        let b = with(Noise::points(2, 1_000), 500, &under);
        assert!(shared_stretches(&a, &b, 60).is_empty());
    }

    /// A season of `episodes` heads, each `len` points of its own
    /// audio with `theme` pasted in at `at[e]`, re-encoded per episode
    /// (up to 3 bits each, so any two copies stay within 6).
    fn season(episodes: usize, len: usize, theme: &[u32], at: &[usize]) -> Vec<WindowPrint> {
        (0..episodes)
            .map(|e| {
                let copy = reencoded(theme, 100 + e as u64, 3);
                head(with(Noise::points(10 + e as u64, len), at[e], &copy))
            })
            .collect()
    }

    #[test]
    fn a_season_finds_its_shared_intro_wherever_each_episode_puts_it() {
        let theme = Noise::points(7, pts(30.0));
        let at = [0, 40, 177, 300, 12];
        let found = match_windows(&season(5, 800, &theme, &at), SegmentKind::Intro);
        for (e, result) in found.iter().enumerate() {
            let m = result.unwrap_or_else(|| panic!("episode {e}: nothing"));
            assert_eq!(
                m.span,
                Span::new(point_ms(at[e]), point_ms(at[e] + theme.len())),
                "episode {e}"
            );
            assert_eq!(m.sharing, 4);
            // Every point within 6 bits: quality 1, agreement 15/16.
            assert_eq!(m.confidence, 0.9375);
        }
    }

    #[test]
    fn two_episodes_never_agree_enough_and_three_just_do() {
        let theme = Noise::points(7, pts(30.0));
        let two = match_windows(&season(2, 800, &theme, &[0, 50]), SegmentKind::Intro);
        assert_eq!(two, vec![None, None], "n = 1: at most 0.5");
        let three = match_windows(&season(3, 800, &theme, &[0, 50, 9]), SegmentKind::Intro);
        for m in &three {
            let m = m.unwrap();
            assert_eq!((m.sharing, m.confidence), (2, 0.75));
        }
    }

    #[test]
    fn quality_is_the_share_of_the_segments_points_that_match() {
        // Episode 0's copy has every 10th point ruined: 90 % of its
        // points match each partner; the others match each other fully.
        let theme = Noise::points(7, pts(30.0));
        let mut windows = season(3, 800, &theme, &[0, 50, 9]);
        for (i, p) in windows[0].points[..theme.len()].iter_mut().enumerate() {
            if i % 10 == 9 {
                *p = !*p;
            }
        }
        let found = match_windows(&windows, SegmentKind::Intro);
        let q = |m: Option<Matched>| m.unwrap().confidence;
        let ruined = (theme.len() / 10) as f64; // points ruined in each pairing with ep 0
        let len = theme.len() as f64;
        assert_eq!(q(found[0]), ((len - ruined) / len * 0.75) as f32);
        // Episodes 1 and 2 each pair once with episode 0 and once with
        // the other: the quality pools both.
        assert_eq!(
            q(found[1]),
            ((2.0 * len - ruined) / (2.0 * len) * 0.75) as f32
        );

        // Every 5th point ruined instead: 194 of 242 (80.2 %) still
        // make a stretch, and 0.802 × 0.75 just clears the 0.6 floor.
        let mut windows = season(3, 800, &theme, &[0, 50, 9]);
        for (i, p) in windows[0].points[..theme.len()].iter_mut().enumerate() {
            if i % 5 == 4 {
                *p = !*p;
            }
        }
        let found = match_windows(&windows, SegmentKind::Intro);
        assert_eq!(q(found[0]), (194.0 / 242.0 * 0.75) as f32);
        assert!(q(found[0]) >= MIN_CONFIDENCE);
        // One more ruined point in every ten: under 80 %, so episode 0
        // shares nothing — and the other two, left with one partner
        // each (agreement 0.5), get nothing either.
        for (i, p) in windows[0].points[..theme.len()].iter_mut().enumerate() {
            if i % 10 == 2 {
                *p = !*p;
            }
        }
        assert_eq!(
            match_windows(&windows, SegmentKind::Intro),
            vec![None, None, None]
        );
    }

    #[test]
    fn each_episode_keeps_what_most_of_the_season_shares() {
        // All five share the theme; episodes 0 and 1 also share a
        // longer melody — a single pair would pick the melody.
        let theme = Noise::points(7, pts(30.0));
        let melody = Noise::points(8, pts(50.0));
        let at = [0, 40, 177, 300, 12];
        let mut windows = season(5, 900, &theme, &at);
        for (e, from) in [(0, 400), (1, 450)] {
            windows[e].points[from..from + melody.len()].copy_from_slice(&melody);
        }
        let found = match_windows(&windows, SegmentKind::Intro);
        for (e, m) in found.iter().enumerate() {
            let m = m.unwrap();
            assert_eq!(m.span.start_ms, point_ms(at[e]), "episode {e}");
            assert_eq!(m.sharing, 4);
        }
    }

    #[test]
    fn an_intro_is_15_to_120_seconds() {
        for (secs, found) in [(14.0, false), (16.0, true), (119.0, true), (122.0, false)] {
            let theme = Noise::points(7, pts(secs));
            let windows = season(4, 1_500, &theme, &[0, 100, 300, 20]);
            let result = match_windows(&windows, SegmentKind::Intro);
            assert_eq!(result.iter().all(|m| m.is_some()), found, "{secs} s");
            assert_eq!(result.iter().all(|m| m.is_none()), !found, "{secs} s");
        }
    }

    #[test]
    fn credits_end_within_the_last_two_minutes() {
        // Tails of four 30-minute episodes: the last 600 s. The shared
        // credits are 40 s, then each episode's own preview.
        let duration_ms = 1_800_000;
        let len = pts(600.0);
        let credits = Noise::points(7, pts(40.0));
        let at = |preview_secs: f64| len - pts(preview_secs) - credits.len();
        let tails = |preview_secs: f64| -> Vec<WindowPrint> {
            (0..4)
                .map(|e| WindowPrint {
                    start_ms: 1_200_000,
                    points: with(
                        Noise::points(20 + e, len),
                        at(preview_secs),
                        &reencoded(&credits, 30 + e, 3),
                    ),
                    duration_ms,
                })
                .collect()
        };
        // A 30 s preview after them: they end 30 s before the end.
        let found = match_windows(&tails(30.0), SegmentKind::Credits);
        for m in &found {
            let m = m.unwrap();
            assert_eq!(
                m.span,
                Span::new(
                    1_200_000 + point_ms(at(30.0)),
                    1_200_000 + point_ms(at(30.0) + credits.len())
                )
            );
            assert!(m.span.end_ms >= duration_ms - 30_000);
            assert_eq!(m.sharing, 3);
        }
        // A 125 s one: they end too early to be the credits.
        assert!(match_windows(&tails(125.0), SegmentKind::Credits)
            .iter()
            .all(|m| m.is_none()));
    }

    #[test]
    fn equally_shared_intros_go_to_the_earliest_and_credits_to_the_latest() {
        let first = Noise::points(7, pts(20.0));
        let second = Noise::points(8, pts(40.0));
        let at = [(0, 300), (50, 400), (120, 600), (10, 250)];
        let windows: Vec<WindowPrint> = at
            .iter()
            .enumerate()
            .map(|(e, &(x, y))| {
                let body = with(Noise::points(40 + e as u64, 1_200), x, &first);
                head(with(body, y, &second))
            })
            .collect();
        for (e, m) in match_windows(&windows, SegmentKind::Intro)
            .iter()
            .enumerate()
        {
            assert_eq!(m.unwrap().span.start_ms, point_ms(at[e].0), "episode {e}");
        }
        // The same audio as short whole-file tails, so both shared
        // stretches end within the last 120 s: the later one is the
        // credits.
        let tails: Vec<WindowPrint> = windows
            .iter()
            .zip(at)
            .map(|(w, (_, y))| {
                let end = y + second.len() + 5;
                WindowPrint {
                    start_ms: 0,
                    points: w.points[..end].to_vec(),
                    duration_ms: point_ms(end),
                }
            })
            .collect();
        for (e, m) in match_windows(&tails, SegmentKind::Credits)
            .iter()
            .enumerate()
        {
            assert_eq!(m.unwrap().span.start_ms, point_ms(at[e].1), "episode {e}");
        }
    }
}
