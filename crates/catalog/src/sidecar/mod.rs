//! Sidecar metadata stored with the media files (Phase 8, XFER4,
//! D14). Spec: `docs/IMPORT-EXPORT.md` §13.
//!
//! A sidecar is a small JSON document, `parlour-sidecar` v1, next
//! to the media it describes:
//!
//! - `<video stem>.parlour.json` beside each movie or episode file;
//! - `series.parlour.json` in a series' folder.
//!
//! This module is the format and the file handling, with no
//! database: the types, the strict reader (version first, unknown
//! fields refused, [`Sidecar::problems`]), the deterministic writer,
//! where a sidecar lives, and the atomic write (temp file + rename).
//! `sync` is the database half: the scanner's read pass, the
//! matcher's re-apply, and the metadata job's rewrite.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use parlour_core::SegmentKind;
use serde::{Deserialize, Serialize};

pub mod sync;
#[cfg(test)]
mod tests;

pub use sync::{
    mark_overridden, reapply_stored, stored_document, write_item_sidecars, SidecarWriteReport,
};

/// `format` in every sidecar.
pub const FORMAT: &str = "parlour-sidecar";
/// The one version this server reads and writes.
pub const FORMAT_VERSION: u64 = 1;
/// A series folder's sidecar.
pub const SERIES_FILE_NAME: &str = "series.parlour.json";
/// What a movie or episode file's sidecar name ends with.
pub const FILE_SUFFIX: &str = ".parlour.json";
/// Confidence of a sidecar's segments: they are manual rows (SEG4),
/// which the editor stores at 1.0 too.
pub const MANUAL_CONFIDENCE: f32 = 1.0;

/// Which item a sidecar describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidecarKind {
    Movie,
    Series,
    Episode,
}

impl SidecarKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SidecarKind::Movie => "movie",
            SidecarKind::Series => "series",
            SidecarKind::Episode => "episode",
        }
    }

    /// The sidecar kind for a `media_items.kind`; seasons and
    /// audiobooks have none.
    pub fn for_item(kind: &str) -> Option<Self> {
        match kind {
            "movie" => Some(SidecarKind::Movie),
            "series" => Some(SidecarKind::Series),
            "episode" => Some(SidecarKind::Episode),
            _ => None,
        }
    }
}

/// One credit. `name` + `role` identify it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SidecarPerson {
    pub name: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_name: Option<String>,
    /// Billing order; absent means the entry's position in the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
}

/// One manual segment: times in ms into the video, or neither for a
/// D21 "no segment of this kind here" marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SidecarSegment {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<i64>,
}

/// An item's manual segments and the video they were measured on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SidecarSegments {
    /// Size in bytes of the video file the times belong to. The
    /// scanner applies `entries` only to a video of exactly this size.
    pub size_bytes: i64,
    pub entries: Vec<SidecarSegment>,
}

/// A `parlour-sidecar` v1 document. Every metadata field is
/// optional: a field that is present is authoritative, a field that
/// is absent (or null) is not carried and still comes from the
/// provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidecar {
    pub format: String,
    pub format_version: u64,
    pub kind: SidecarKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_number: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_number: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_date: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vote_average: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_minutes: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub people: Option<Vec<SidecarPerson>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<SidecarSegments>,
}

/// Why a sidecar file can't be used.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SidecarError {
    #[error("not valid JSON: {0}")]
    Json(String),
    #[error("not a parlour-sidecar document (\"format\" must be \"parlour-sidecar\")")]
    NotSidecar,
    #[error("the sidecar is format version {0}; this server reads version 1")]
    Version(String),
    #[error("{0}")]
    Shape(String),
    #[error("{}", .0.join("; "))]
    Invalid(Vec<String>),
}

impl Sidecar {
    /// An empty document of a kind: only the identity fields.
    pub fn new(kind: SidecarKind) -> Self {
        Sidecar {
            format: FORMAT.to_string(),
            format_version: FORMAT_VERSION,
            kind,
            title: None,
            year: None,
            season_number: None,
            episode_number: None,
            air_date: None,
            overview: None,
            genres: None,
            content_rating: None,
            vote_average: None,
            runtime_minutes: None,
            tmdb_id: None,
            tvdb_id: None,
            imdb_id: None,
            people: None,
            segments: None,
        }
    }

    /// Everything that breaks a rule of §13, in field order. Empty
    /// means the document is valid. (The format and version are
    /// checked by [`parse`] before the rest is even read.)
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.format != FORMAT {
            out.push(format!("format must be \"{FORMAT}\""));
        }
        if self.format_version != FORMAT_VERSION {
            out.push(format!("format_version must be {FORMAT_VERSION}"));
        }
        let text = |out: &mut Vec<String>, field: &str, v: &Option<String>| {
            if matches!(v, Some(s) if s.trim().is_empty()) {
                out.push(format!("{field} is empty (leave it out instead)"));
            }
        };
        text(&mut out, "title", &self.title);
        if matches!(self.year, Some(y) if y <= 0) {
            out.push("year must be above 0".to_string());
        }
        if self.kind == SidecarKind::Episode {
            if matches!(self.season_number, Some(n) if n < 0) {
                out.push("season_number must be 0 or more".to_string());
            }
            if matches!(self.episode_number, Some(n) if n < 0) {
                out.push("episode_number must be 0 or more".to_string());
            }
        } else {
            if self.season_number.is_some() {
                out.push("season_number is only for an episode".to_string());
            }
            if self.episode_number.is_some() {
                out.push("episode_number is only for an episode".to_string());
            }
        }
        text(&mut out, "overview", &self.overview);
        if let Some(genres) = &self.genres {
            let mut seen = HashSet::new();
            for g in genres {
                if g.trim().is_empty() {
                    out.push("genres has an empty entry".to_string());
                } else if !seen.insert(g.as_str()) {
                    out.push(format!("genres lists {g:?} twice"));
                }
            }
        }
        text(&mut out, "content_rating", &self.content_rating);
        if let Some(v) = self.vote_average {
            if !(0.0..=10.0).contains(&v) {
                out.push("vote_average must be between 0 and 10".to_string());
            }
        }
        if matches!(self.runtime_minutes, Some(m) if m <= 0) {
            out.push("runtime_minutes must be above 0".to_string());
        }
        if matches!(self.tmdb_id, Some(id) if id <= 0) {
            out.push("tmdb_id must be above 0".to_string());
        }
        if matches!(self.tvdb_id, Some(id) if id <= 0) {
            out.push("tvdb_id must be above 0".to_string());
        }
        text(&mut out, "imdb_id", &self.imdb_id);
        if let Some(people) = &self.people {
            let mut seen = HashSet::new();
            for (i, p) in people.iter().enumerate() {
                if p.name.trim().is_empty() {
                    out.push(format!("people[{i}].name is empty"));
                }
                if p.role.trim().is_empty() {
                    out.push(format!("people[{i}].role is empty"));
                }
                if matches!(&p.character_name, Some(c) if c.trim().is_empty()) {
                    out.push(format!(
                        "people[{i}].character_name is empty (leave it out instead)"
                    ));
                }
                if matches!(p.tmdb_id, Some(id) if id <= 0) {
                    out.push(format!("people[{i}].tmdb_id must be above 0"));
                }
                if !seen.insert((p.name.as_str(), p.role.as_str())) {
                    out.push(format!("people lists {:?} as {:?} twice", p.name, p.role));
                }
            }
        }
        if let Some(segments) = &self.segments {
            if self.kind == SidecarKind::Series {
                out.push("segments are only for a movie or an episode".to_string());
            }
            if segments.size_bytes <= 0 {
                out.push("segments.size_bytes must be above 0".to_string());
            }
            let mut seen = HashSet::new();
            for (i, s) in segments.entries.iter().enumerate() {
                match SegmentKind::parse(&s.kind) {
                    None => out.push(format!(
                        "segments.entries[{i}].kind must be intro, credits or recap"
                    )),
                    Some(k) => {
                        if !seen.insert(k) {
                            out.push(format!("segments lists {} twice", k.as_str()));
                        }
                    }
                }
                match (s.start_ms, s.end_ms) {
                    (None, None) => {}
                    (Some(start), Some(end)) => {
                        if start < 0 || end <= start {
                            out.push(format!(
                                "segments.entries[{i}] needs 0 <= start_ms < end_ms"
                            ));
                        }
                    }
                    _ => out.push(format!(
                        "segments.entries[{i}] has one time without the other"
                    )),
                }
            }
        }
        out
    }

    /// The people list with every `sort_order` filled in.
    pub fn people_in_order(&self) -> Option<Vec<SidecarPerson>> {
        self.people.as_ref().map(|people| {
            people
                .iter()
                .enumerate()
                .map(|(i, p)| SidecarPerson {
                    sort_order: Some(p.sort_order.unwrap_or(i as i32)),
                    ..p.clone()
                })
                .collect()
        })
    }

    /// The provider id a sidecar names for one provider.
    pub fn provider_id(&self, provider: &str) -> Option<i64> {
        match provider {
            "tmdb" => self.tmdb_id,
            "tvdb" => self.tvdb_id,
            _ => None,
        }
    }

    /// The document with its segments taken out: what the sidecars
    /// of one item must agree on, and what `item_sidecars` stores.
    pub fn without_segments(&self) -> Sidecar {
        Sidecar {
            segments: None,
            ..self.clone()
        }
    }
}

/// Read a sidecar strictly: the format and version first, so a newer
/// sidecar is refused for its version rather than for some field;
/// then the document itself, with unknown or repeated fields refused;
/// then the rules ([`Sidecar::problems`]).
pub fn parse(bytes: &[u8]) -> Result<Sidecar, SidecarError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| SidecarError::Json(e.to_string()))?;
    let obj = value.as_object().ok_or(SidecarError::NotSidecar)?;
    if obj.get("format").and_then(|v| v.as_str()) != Some(FORMAT) {
        return Err(SidecarError::NotSidecar);
    }
    match obj.get("format_version") {
        Some(v) if v.as_u64() == Some(FORMAT_VERSION) => {}
        Some(v) => return Err(SidecarError::Version(v.to_string())),
        None => return Err(SidecarError::Version("(missing)".to_string())),
    }
    let doc: Sidecar =
        serde_json::from_slice(bytes).map_err(|e| SidecarError::Shape(e.to_string()))?;
    let problems = doc.problems();
    if problems.is_empty() {
        Ok(doc)
    } else {
        Err(SidecarError::Invalid(problems))
    }
}

/// The bytes Parlour writes: two-space indents and a final newline,
/// fields in a fixed order and absent ones left out — so the same
/// metadata always gives the same bytes, and an unchanged sidecar is
/// never rewritten.
pub fn to_bytes(doc: &Sidecar) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(doc).expect("a sidecar always serialises");
    bytes.push(b'\n');
    bytes
}

/// The sidecar of one movie or episode file: the video's name with
/// its extension replaced by `.parlour.json`. A video whose stem is
/// `series` has none (that name is the series folder's).
pub fn file_sidecar_path(video: &Path) -> Option<PathBuf> {
    let stem = video.file_stem()?.to_str()?;
    if stem == "series" || stem.is_empty() {
        return None;
    }
    Some(video.with_file_name(format!("{stem}{FILE_SUFFIX}")))
}

/// The folder whose `series.parlour.json` describes an episode's
/// series: the nearest folder above the episode that is not a season
/// folder (`Season 01`, `S01`, or `Specials`). `None` when the
/// episode sits directly in the library root. `abs` is the episode's absolute path and
/// `rel` the same path relative to its library root.
pub fn series_folder(abs: &str, rel: &str) -> Option<PathBuf> {
    let root = abs.strip_suffix(rel)?.trim_end_matches('/');
    let dirs: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    let idx = dirs.iter().rposition(|d| {
        crate::parse::season_from_folder(d).is_none() && !d.eq_ignore_ascii_case("specials")
    })?;
    let mut folder = PathBuf::from(if root.is_empty() { "/" } else { root });
    for d in &dirs[..=idx] {
        folder.push(d);
    }
    Some(folder)
}

/// Whether a write replaced the file or found it already identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    Unchanged,
}

/// Write a sidecar atomically: the bytes go to a hidden temp file in
/// the same folder (`.<name>.<random>.tmp`), are synced, and the temp
/// file is renamed over the target — so a reader sees the old sidecar
/// or the new one, never part of one. A file that already holds
/// exactly these bytes is left alone. On any failure the temp file is
/// removed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<WriteOutcome> {
    if let Ok(existing) = std::fs::read(path) {
        if existing == bytes {
            return Ok(WriteOutcome::Unchanged);
        }
    }
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "a sidecar needs a folder")
    })?;
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "a sidecar needs a name")
    })?;
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        // Make the rename itself durable; not every filesystem lets
        // a folder be synced, and the write has happened either way.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| WriteOutcome::Written)
}
