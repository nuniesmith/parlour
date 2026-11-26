//! Kodi/Emby-style `.nfo` files beside the media (XFER6, D14). Spec:
//! `docs/IMPORT-EXPORT.md` §13.13.
//!
//! An NFO is the plain XML file other media software keeps next to a
//! video: `<movie>`, `<tvshow>` or `<episodedetails>` with a title and
//! provider ids. Parlour reads it as an **input source** (its ids and
//! title, nothing else) and, where a library writes sidecars, writes a
//! small one of its own beside the `.parlour.json`.
//!
//! This module is the format and the file names, with no database.
//! `sync` does the scanner's NFO pass and the writer's NFO step.
//!
//! The reader is lenient about what it doesn't use and strict about
//! what it does:
//!
//! - Only the root element's **direct children** are looked at:
//!   `title`, `tmdbid`, `tvdbid`, `imdbid`, and `uniqueid` with a
//!   `type` of `tmdb`, `tvdb` or `imdb`; and, in an episode's NFO,
//!   `season` and `episode` as checks. Every other element (plot,
//!   year, cast, ratings, artwork, …) is skipped, at any depth.
//! - Reading stops when the root element closes, so a link after it
//!   or a second `<episodedetails>` (a multi-episode file) is never
//!   read.
//! - A value Parlour reads must be usable: an id is a whole number
//!   above 0 (`tt` and digits for IMDb), and the same field given
//!   twice must agree. Otherwise the file is refused, and the scanner
//!   reports it and carries on.

use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde::{Deserialize, Serialize};

use super::SidecarKind;

/// A movie's folder-level NFO, read only in a folder that belongs to
/// that movie alone. Parlour never writes it.
pub const MOVIE_NFO: &str = "movie.nfo";
/// A series' NFO, in its series folder (beside `series.parlour.json`).
pub const TVSHOW_NFO: &str = "tvshow.nfo";
/// The NFO extension, without the dot.
pub const NFO_EXTENSION: &str = "nfo";
/// A larger file is refused unread. Real NFOs with a full cast list
/// are a few hundred KiB at most.
pub const MAX_NFO_BYTES: u64 = 4 * 1024 * 1024;

/// What Parlour takes from an NFO: the title and the provider ids.
/// Absent means the NFO doesn't say. This is also what `item_nfos`
/// stores.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NfoFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
}

impl NfoFields {
    /// The provider id the NFO names for one provider.
    pub fn provider_id(&self, provider: &str) -> Option<i64> {
        match provider {
            "tmdb" => self.tmdb_id,
            "tvdb" => self.tvdb_id,
            _ => None,
        }
    }
}

/// One NFO file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nfo {
    /// From the root element: `<movie>`, `<tvshow>`, `<episodedetails>`.
    pub kind: SidecarKind,
    /// `<season>` / `<episode>` of an episode's NFO: checks against
    /// the file name, never applied.
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub fields: NfoFields,
}

/// Why an NFO can't be used.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NfoError {
    #[error("the NFO is larger than {} MiB", MAX_NFO_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("the NFO isn't UTF-8 text")]
    NotUtf8,
    #[error("the NFO isn't well-formed XML: {0}")]
    Xml(String),
    #[error(
        "the NFO has no XML root element (a link-only NFO isn't read; \
         Parlour reads <movie>, <tvshow> or <episodedetails>)"
    )]
    NoRoot,
    #[error("the NFO's root is <{0}>; Parlour reads <movie>, <tvshow> or <episodedetails>")]
    Root(String),
    #[error("{0}")]
    Value(String),
}

/// Text of one direct child of the root, as it is collected.
struct Child {
    name: String,
    id_type: Option<String>,
    text: String,
}

fn child_of(start: &BytesStart<'_>) -> Result<Child, NfoError> {
    let name = String::from_utf8_lossy(start.local_name().as_ref()).to_ascii_lowercase();
    let mut id_type = None;
    if name == "uniqueid" {
        for attr in start.attributes() {
            let attr = attr.map_err(|e| NfoError::Xml(e.to_string()))?;
            if attr.key.local_name().as_ref().eq_ignore_ascii_case(b"type") {
                let v = attr
                    .unescape_value()
                    .map_err(|e| NfoError::Xml(e.to_string()))?;
                id_type = Some(v.trim().to_ascii_lowercase());
            }
        }
    }
    Ok(Child {
        name,
        id_type,
        text: String::new(),
    })
}

fn root_kind(start: &BytesStart<'_>) -> Result<SidecarKind, NfoError> {
    let name = String::from_utf8_lossy(start.local_name().as_ref()).to_ascii_lowercase();
    match name.as_str() {
        "movie" => Ok(SidecarKind::Movie),
        "tvshow" => Ok(SidecarKind::Series),
        "episodedetails" => Ok(SidecarKind::Episode),
        _ => Err(NfoError::Root(name)),
    }
}

fn empty_nfo(kind: SidecarKind) -> Nfo {
    Nfo {
        kind,
        season_number: None,
        episode_number: None,
        fields: NfoFields::default(),
    }
}

/// Set a field once; a second, different value refuses the file.
fn set_once<T: PartialEq + std::fmt::Display + Clone>(
    slot: &mut Option<T>,
    value: T,
    what: &str,
) -> Result<(), NfoError> {
    match slot {
        Some(old) if *old != value => Err(NfoError::Value(format!(
            "the NFO gives two different {what}s ({old} and {value})"
        ))),
        _ => {
            *slot = Some(value);
            Ok(())
        }
    }
}

fn positive_id(raw: &str, what: &str) -> Result<i64, NfoError> {
    match raw.parse::<i64>() {
        Ok(v) if v > 0 => Ok(v),
        _ => Err(NfoError::Value(format!(
            "the NFO's {what} must be a whole number above 0 (it is {raw:?})"
        ))),
    }
}

fn imdb_id(raw: &str) -> Result<String, NfoError> {
    let ok = raw
        .strip_prefix("tt")
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    if ok {
        Ok(raw.to_string())
    } else {
        Err(NfoError::Value(format!(
            "the NFO's IMDb id must be \"tt\" and digits (it is {raw:?})"
        )))
    }
}

fn number(raw: &str, what: &str) -> Result<i32, NfoError> {
    match raw.parse::<i32>() {
        Ok(v) if v >= 0 => Ok(v),
        _ => Err(NfoError::Value(format!(
            "the NFO's {what} must be a whole number, 0 or more (it is {raw:?})"
        ))),
    }
}

/// Take one finished child of the root into the result.
fn take(nfo: &mut Nfo, child: Child) -> Result<(), NfoError> {
    let value = child.text.trim();
    if value.is_empty() {
        return Ok(());
    }
    let f = &mut nfo.fields;
    let id_field = match (child.name.as_str(), child.id_type.as_deref()) {
        ("title", _) => return set_once(&mut f.title, value.to_string(), "title"),
        ("season", _) if nfo.kind == SidecarKind::Episode => {
            return set_once(&mut nfo.season_number, number(value, "season")?, "season")
        }
        ("episode", _) if nfo.kind == SidecarKind::Episode => {
            return set_once(
                &mut nfo.episode_number,
                number(value, "episode")?,
                "episode",
            )
        }
        ("tmdbid", _) | ("uniqueid", Some("tmdb")) => "tmdb",
        ("tvdbid", _) | ("uniqueid", Some("tvdb")) => "tvdb",
        ("imdbid", _) | ("uniqueid", Some("imdb")) => "imdb",
        _ => return Ok(()),
    };
    match id_field {
        "tmdb" => set_once(&mut f.tmdb_id, positive_id(value, "TMDB id")?, "TMDB id"),
        "tvdb" => set_once(&mut f.tvdb_id, positive_id(value, "TVDB id")?, "TVDB id"),
        _ => set_once(&mut f.imdb_id, imdb_id(value)?, "IMDb id"),
    }
}

/// Read an NFO: the root element's kind, its title and provider ids,
/// and (for an episode) its season and episode numbers as checks.
/// Everything else in the file is skipped; reading stops at the end of
/// the root element.
pub fn parse_nfo(bytes: &[u8]) -> Result<Nfo, NfoError> {
    if bytes.len() as u64 > MAX_NFO_BYTES {
        return Err(NfoError::TooLarge);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| NfoError::NotUtf8)?;
    let mut reader = Reader::from_str(text);
    let xml = |e: quick_xml::Error| NfoError::Xml(e.to_string());

    let mut nfo: Option<Nfo> = None;
    // Depth below the root: 0 = before the root, 1 = inside the root,
    // 2 = inside one of its children, deeper = nested further.
    let mut depth = 0usize;
    let mut child: Option<Child> = None;
    loop {
        let event = reader.read_event().map_err(xml)?;
        match event {
            Event::Eof => {
                return match nfo {
                    None => Err(NfoError::NoRoot),
                    // quick-xml reports an unclosed root itself; this is
                    // a fallback.
                    Some(_) => Err(NfoError::Xml(
                        "the file ends inside its root element".into(),
                    )),
                };
            }
            Event::Start(start) if depth == 0 => {
                nfo = Some(empty_nfo(root_kind(&start)?));
                depth = 1;
            }
            Event::Empty(start) if depth == 0 => {
                nfo = Some(empty_nfo(root_kind(&start)?));
                break;
            }
            Event::Text(t) if depth == 0 => {
                let t = t.decode().map_err(|e| NfoError::Xml(e.to_string()))?;
                if !t.trim().is_empty() {
                    return Err(NfoError::NoRoot);
                }
            }
            Event::Start(start) => {
                if depth == 1 {
                    child = Some(child_of(&start)?);
                }
                depth += 1;
            }
            Event::Empty(_) => {}
            Event::Text(t) if depth == 2 => {
                let t = t.decode().map_err(|e| NfoError::Xml(e.to_string()))?;
                if let Some(c) = child.as_mut() {
                    c.text.push_str(&t);
                }
            }
            Event::CData(t) if depth == 2 => {
                let t = t.decode().map_err(|e| NfoError::Xml(e.to_string()))?;
                if let Some(c) = child.as_mut() {
                    c.text.push_str(&t);
                }
            }
            Event::GeneralRef(r) if depth == 2 => {
                let resolved = match r.resolve_char_ref().map_err(xml)? {
                    Some(ch) => ch.to_string(),
                    None => {
                        let name = r.decode().map_err(|e| NfoError::Xml(e.to_string()))?;
                        quick_xml::escape::resolve_predefined_entity(&name)
                            .ok_or_else(|| NfoError::Xml(format!("unknown entity &{name};")))?
                            .to_string()
                    }
                };
                if let Some(c) = child.as_mut() {
                    c.text.push_str(&resolved);
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 1 {
                    if let (Some(n), Some(c)) = (nfo.as_mut(), child.take()) {
                        take(n, c)?;
                    }
                } else if depth == 0 {
                    break;
                }
            }
            // Declarations, comments, processing instructions, a
            // doctype, and text outside the children read: skipped.
            _ => {}
        }
    }
    nfo.ok_or(NfoError::NoRoot)
}

/// What Parlour writes in an NFO: the identifying fields only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NfoContent {
    pub title: Option<String>,
    /// Movies and series only.
    pub year: Option<i32>,
    /// Episodes only.
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
}

fn root_name(kind: SidecarKind) -> &'static str {
    match kind {
        SidecarKind::Movie => "movie",
        SidecarKind::Series => "tvshow",
        SidecarKind::Episode => "episodedetails",
        // Parlour writes no NFO beside a book (AB6): Audiobookshelf's
        // book NFO is a different, `key: value` format, read only.
        SidecarKind::Audiobook => "audiobook",
    }
}

/// Text for an element: XML's three special characters escaped, and
/// characters XML 1.0 can't hold dropped.
fn text(s: &str) -> String {
    let clean: String = s
        .trim()
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n'))
        .collect();
    quick_xml::escape::partial_escape(clean.as_str()).into_owned()
}

/// The bytes Parlour writes: UTF-8, two-space indents, a final
/// newline, elements in a fixed order and absent ones left out, so the
/// same item always gives the same bytes. Each id is written twice, as
/// `<uniqueid type="…">` and as `<tmdbid>` / `<tvdbid>` / `<imdbid>`,
/// because readers differ in which form they look for; the reader
/// accepts both and requires them to agree.
pub fn to_nfo_bytes(kind: SidecarKind, c: &NfoContent) -> Vec<u8> {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let root = root_name(kind);
    out.push_str(&format!("<{root}>\n"));
    if let Some(t) = c.title.as_deref().filter(|t| !t.trim().is_empty()) {
        out.push_str(&format!("  <title>{}</title>\n", text(t)));
    }
    if kind == SidecarKind::Episode {
        if let Some(n) = c.season_number.filter(|n| *n >= 0) {
            out.push_str(&format!("  <season>{n}</season>\n"));
        }
        if let Some(n) = c.episode_number.filter(|n| *n >= 0) {
            out.push_str(&format!("  <episode>{n}</episode>\n"));
        }
    } else if let Some(y) = c.year.filter(|y| *y > 0) {
        out.push_str(&format!("  <year>{y}</year>\n"));
    }
    let tmdb = c.tmdb_id.filter(|v| *v > 0);
    let tvdb = c.tvdb_id.filter(|v| *v > 0);
    let imdb = c.imdb_id.as_deref().and_then(|v| imdb_id(v.trim()).ok());
    let ids: Vec<(&str, String)> = [
        ("tmdb", tmdb.map(|v| v.to_string())),
        ("tvdb", tvdb.map(|v| v.to_string())),
        ("imdb", imdb),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|v| (k, v)))
    .collect();
    for (i, (k, v)) in ids.iter().enumerate() {
        let default = if i == 0 { " default=\"true\"" } else { "" };
        out.push_str(&format!(
            "  <uniqueid type=\"{k}\"{default}>{v}</uniqueid>\n"
        ));
    }
    for (k, v) in &ids {
        out.push_str(&format!("  <{k}id>{v}</{k}id>\n"));
    }
    out.push_str(&format!("</{root}>\n"));
    out.into_bytes()
}

/// The NFO of one movie or episode file: the video's name with its
/// extension replaced by `.nfo`. A video whose stem is `tvshow` or
/// `season` has none, because those names belong to a folder's NFO.
pub fn file_nfo_path(video: &Path) -> Option<PathBuf> {
    let stem = video.file_stem()?.to_str()?;
    if stem.is_empty() || stem.eq_ignore_ascii_case("tvshow") || stem.eq_ignore_ascii_case("season")
    {
        return None;
    }
    Some(video.with_file_name(format!("{stem}.{NFO_EXTENSION}")))
}
