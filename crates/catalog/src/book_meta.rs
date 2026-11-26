//! A book's metadata from its local sources, merged in the library's
//! precedence order (Phase 10, AB6, D30). Spec:
//! `docs/AUDIOBOOKS-PODCASTS.md`, "Metadata sources, matching and
//! covers (AB6)".
//!
//! D30 mirrors Audiobookshelf: each audiobooks library has an ordered
//! list of metadata sources, and a later source overwrites what an
//! earlier one gave, field by field. The default order is ABS's:
//!
//! `folder` → `tags` → `nfo` → `txt` → `opf` → `abs_json` → `sidecar`
//!
//! - `folder` — the title folder's name, parsed (AB11), the author
//!   folder and the series folder;
//! - `tags` — the audio tags of the **first track** in disc/track
//!   order;
//! - `nfo` — an Audiobookshelf-style `.nfo` (`Key: value` lines);
//! - `txt` — `desc.txt` (description) and `reader.txt` (narrators);
//! - `opf` — an `.opf` package document (Calibre and others);
//! - `abs_json` — Audiobookshelf's own `metadata.json`, read as an
//!   input;
//! - `sidecar` — Parlour's `book.parlour.json` (D14), or the server's
//!   own copy of it that a match left: the last word.
//!
//! Everything here is pure (no database, no network); the readers take
//! file contents and the merge takes what they returned. The scanner
//! (`audiobooks`) does the reading, and `book_match` the applying.

use std::collections::HashSet;

use serde_json::Value;

use crate::audiobooks::{split_genres, split_people, AudioProbe};
use crate::sidecar::{Sidecar, SidecarKind, SidecarPerson};

// ---------------------------------------------------------- precedence

/// One metadata source a book can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataSource {
    Folder,
    Tags,
    Nfo,
    Txt,
    Opf,
    AbsJson,
    Sidecar,
}

impl MetadataSource {
    pub const ALL: [MetadataSource; 7] = [
        MetadataSource::Folder,
        MetadataSource::Tags,
        MetadataSource::Nfo,
        MetadataSource::Txt,
        MetadataSource::Opf,
        MetadataSource::AbsJson,
        MetadataSource::Sidecar,
    ];

    pub fn id(self) -> &'static str {
        match self {
            MetadataSource::Folder => "folder",
            MetadataSource::Tags => "tags",
            MetadataSource::Nfo => "nfo",
            MetadataSource::Txt => "txt",
            MetadataSource::Opf => "opf",
            MetadataSource::AbsJson => "abs_json",
            MetadataSource::Sidecar => "sidecar",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// D30's default: Audiobookshelf's order, with Parlour's sidecar last.
pub const DEFAULT_PRECEDENCE: [MetadataSource; 7] = MetadataSource::ALL;

/// The ids of the default order, as stored.
pub fn default_precedence_ids() -> Vec<String> {
    DEFAULT_PRECEDENCE
        .iter()
        .map(|s| s.id().to_string())
        .collect()
}

/// Check a precedence list an admin sent: at least one source, each
/// a known id, none twice. Order is the admin's.
pub fn parse_precedence(ids: &[String]) -> Result<Vec<MetadataSource>, String> {
    if ids.is_empty() {
        return Err("list at least one metadata source".to_string());
    }
    let mut out = Vec::new();
    for id in ids {
        let s = MetadataSource::parse(id).ok_or_else(|| {
            format!(
                "unknown metadata source {id:?}; the sources are {}",
                MetadataSource::ALL.map(|s| s.id()).join(", ")
            )
        })?;
        if out.contains(&s) {
            return Err(format!("{id:?} is listed twice"));
        }
        out.push(s);
    }
    Ok(out)
}

/// A stored list, read leniently: unknown or repeated ids are dropped
/// (a newer server's id read by an older one), and an empty result
/// falls back to the default.
pub fn stored_precedence(ids: &[String]) -> Vec<MetadataSource> {
    let mut out: Vec<MetadataSource> = Vec::new();
    for s in ids.iter().filter_map(|i| MetadataSource::parse(i)) {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    if out.is_empty() {
        DEFAULT_PRECEDENCE.to_vec()
    } else {
        out
    }
}

// --------------------------------------------------------------- fields

/// What one source says about a book. `None` means "this source says
/// nothing"; a value means "this source says this".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookFields {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub authors: Option<Vec<String>>,
    pub narrators: Option<Vec<String>>,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub overview: Option<String>,
    pub genres: Option<Vec<String>>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub language: Option<String>,
    pub google_books_id: Option<String>,
    pub open_library_id: Option<String>,
    /// The series the book sits in, each with its sequence (AB7,
    /// D28). A list like the others: a later source that names any
    /// series replaces it whole.
    pub series: Option<Vec<SeriesEntry>>,
}

/// One series a book sits in, and its place there (AB7, D28). The
/// sequence is text as the source gave it (`1`, `2.5`, `1a`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesEntry {
    pub name: String,
    pub sequence: Option<String>,
}

impl SeriesEntry {
    pub fn new(name: &str, sequence: Option<&str>) -> SeriesEntry {
        SeriesEntry {
            name: name.to_string(),
            sequence: sequence.map(str::to_string),
        }
    }
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn clean_list(v: Option<Vec<String>>) -> Option<Vec<String>> {
    v.map(|list| {
        let mut seen = HashSet::new();
        list.into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && seen.insert(s.to_lowercase()))
            .collect()
    })
}

/// Names and sequences trimmed, blank names dropped, a repeated name
/// (without regard to case) dropped after its first appearance.
fn clean_series(v: Option<Vec<SeriesEntry>>) -> Option<Vec<SeriesEntry>> {
    v.map(|list| {
        let mut seen = HashSet::new();
        list.into_iter()
            .map(|e| SeriesEntry {
                name: e.name.trim().to_string(),
                sequence: clean(e.sequence),
            })
            .filter(|e| !e.name.is_empty() && seen.insert(e.name.to_lowercase()))
            .collect()
    })
}

impl BookFields {
    /// Trimmed text, blank text and blank list entries dropped,
    /// repeats in a list dropped. Empty lists stay (see [`merge`]).
    pub fn cleaned(self) -> BookFields {
        BookFields {
            title: clean(self.title),
            subtitle: clean(self.subtitle),
            authors: clean_list(self.authors),
            narrators: clean_list(self.narrators),
            year: self.year.filter(|y| (1000..=2999).contains(y)),
            publisher: clean(self.publisher),
            overview: clean(self.overview),
            genres: clean_list(self.genres),
            isbn: clean(self.isbn),
            asin: clean(self.asin),
            language: clean(self.language),
            google_books_id: clean(self.google_books_id),
            open_library_id: clean(self.open_library_id),
            series: clean_series(self.series),
        }
    }
}

/// Lay `src` over `base`, Audiobookshelf's way: a field `src` has
/// replaces `base`'s; a list is replaced whole, never merged. An empty
/// list replaces only when `src` is `authoritative` (a sidecar, where
/// a present `[]` means "none", D14 §13.2); from any other source an
/// empty list says nothing.
pub fn merge(base: &mut BookFields, src: &BookFields, authoritative: bool) {
    fn text(b: &mut Option<String>, s: &Option<String>) {
        if let Some(v) = s.as_ref().filter(|v| !v.trim().is_empty()) {
            *b = Some(v.clone());
        }
    }
    fn list(b: &mut Option<Vec<String>>, s: &Option<Vec<String>>, authoritative: bool) {
        if let Some(v) = s {
            if !v.is_empty() || authoritative {
                *b = Some(v.clone());
            }
        }
    }
    text(&mut base.title, &src.title);
    text(&mut base.subtitle, &src.subtitle);
    list(&mut base.authors, &src.authors, authoritative);
    list(&mut base.narrators, &src.narrators, authoritative);
    if src.year.is_some() {
        base.year = src.year;
    }
    text(&mut base.publisher, &src.publisher);
    text(&mut base.overview, &src.overview);
    list(&mut base.genres, &src.genres, authoritative);
    text(&mut base.isbn, &src.isbn);
    text(&mut base.asin, &src.asin);
    text(&mut base.language, &src.language);
    text(&mut base.google_books_id, &src.google_books_id);
    text(&mut base.open_library_id, &src.open_library_id);
    if let Some(v) = &src.series {
        if !v.is_empty() || authoritative {
            base.series = Some(v.clone());
        }
    }
}

/// What each source said, read from one book's files.
#[derive(Debug, Clone, Default)]
pub struct BookSources {
    pub folder: BookFields,
    pub tags: BookFields,
    pub nfo: Option<BookFields>,
    pub txt: Option<BookFields>,
    pub opf: Option<BookFields>,
    pub abs_json: Option<BookFields>,
    pub sidecar: Option<BookFields>,
}

/// Merge a book's sources in `precedence` order, later winning. A
/// source not in the list isn't read. The title always ends up set:
/// when no source gave one, the folder's (or file's) name is used.
pub fn merge_sources(precedence: &[MetadataSource], sources: &BookSources) -> BookFields {
    let mut out = BookFields::default();
    for s in precedence {
        let (fields, authoritative) = match s {
            MetadataSource::Folder => (Some(&sources.folder), false),
            MetadataSource::Tags => (Some(&sources.tags), false),
            MetadataSource::Nfo => (sources.nfo.as_ref(), false),
            MetadataSource::Txt => (sources.txt.as_ref(), false),
            MetadataSource::Opf => (sources.opf.as_ref(), false),
            MetadataSource::AbsJson => (sources.abs_json.as_ref(), false),
            MetadataSource::Sidecar => (sources.sidecar.as_ref(), true),
        };
        if let Some(f) = fields {
            merge(&mut out, f, authoritative);
        }
    }
    if out.title.is_none() {
        out.title = sources.folder.title.clone();
    }
    out
}

// --------------------------------------------------------------- folder

/// Split an author folder's name Audiobookshelf's way (spec "Follow-up
/// from D28–D35" item 5): on `;`, `&` and the word `and`, then on
/// commas. Within one group, comma parts that all lack a space are
/// read as `Last, First` pairs (`Sanderson, Brandon` → `Brandon
/// Sanderson`); parts with spaces are whole names (`Jane Doe, John
/// Roe` → two people). A lone single-word part stays as it is.
pub fn split_folder_authors(name: &str) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    for part in name.split([';', '&']) {
        // The word "and", any case, between spaces. ASCII lowercase
        // keeps byte offsets, so they index `part` too.
        let lower = part.to_ascii_lowercase();
        let mut last = 0;
        let mut i = 0;
        while let Some(found) = lower[i..].find(" and ") {
            let at = i + found;
            groups.push(part[last..at].to_string());
            last = at + 5;
            i = last;
        }
        groups.push(part[last..].to_string());
    }
    let mut out: Vec<String> = Vec::new();
    for g in groups {
        let parts: Vec<&str> = g
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() >= 2 && parts.len() % 2 == 0 && parts.iter().all(|p| !p.contains(' ')) {
            for pair in parts.chunks(2) {
                out.push(format!("{} {}", pair[1], pair[0]));
            }
        } else if parts.len() == 2 && !parts[0].contains(' ') && parts[1].contains(' ') {
            // `Tolkien, J. R. R.`: a surname, then given names.
            out.push(format!("{} {}", parts[1], parts[0]));
        } else {
            out.extend(parts.into_iter().map(str::to_string));
        }
    }
    let mut seen = HashSet::new();
    out.retain(|p| seen.insert(p.to_lowercase()));
    out
}

fn file_stem(rel_path: &str) -> String {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    name.rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(name)
        .to_string()
}

/// The folders above a book's title folder that count, nearest first
/// (D28, Audiobookshelf's rule): only the three folders nearest the
/// book. With two or more above the title, the one right above is the
/// **series** and the next the **author**; with one, it is the
/// author. Anything higher (`Genre/…`) is ignored. A book that is one
/// file in a library root has neither.
fn author_and_series_folders(book_key: &str, root_file: bool) -> (Option<&str>, Option<&str>) {
    if root_file {
        return (None, None);
    }
    let parts: Vec<&str> = book_key.split('/').filter(|p| !p.is_empty()).collect();
    let n = parts.len();
    match n {
        0 | 1 => (None, None),
        2 => (Some(parts[0]), None),
        _ => (Some(parts[n - 3]), Some(parts[n - 2])),
    }
}

/// The series folder of a book folder (AB7, D28): the folder right
/// above the title folder, only when two or more folders sit above
/// the title (`Author/Series/Title`); `Author/Title` has none, and
/// neither has a book that is one file in a library root. Only the
/// three folders nearest the book count, so in
/// `Genre/Author/Series/Title` the series is `Series`. Its sequence
/// comes from the title folder's name ([`parse_title_folder`], AB11).
pub fn folder_series(book_key: &str, root_file: bool) -> Option<String> {
    author_and_series_folders(book_key, root_file)
        .1
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// What a title folder's name says (AB11, D28: Audiobookshelf's
/// title-folder parsing). `None`/empty means the name didn't say it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleFolder {
    pub title: String,
    pub subtitle: Option<String>,
    pub sequence: Option<String>,
    pub year: Option<i32>,
    pub narrators: Vec<String>,
    pub asin: Option<String>,
}

/// An ASIN in square brackets: exactly ten capital letters or digits,
/// standing at the start, at the end, or between spaces. The first
/// one is taken out of the name.
fn take_asin(name: &str) -> (String, Option<String>) {
    let b = name.as_bytes();
    let mut i = 0;
    while let Some(off) = name[i..].find('[') {
        let at = i + off;
        let end = at + 11;
        if end < b.len()
            && b[end] == b']'
            && b[at + 1..end]
                .iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && (at == 0 || b[at - 1] == b' ')
            && (end + 1 == b.len() || b[end + 1] == b' ')
        {
            let asin = name[at + 1..end].to_string();
            let rest = format!("{} {}", name[..at].trim_end(), name[end + 1..].trim_start());
            return (rest.trim().to_string(), Some(asin));
        }
        i = at + 1;
    }
    (name.to_string(), None)
}

/// A narrator in braces at the very end, after a space:
/// `Title {Jim Dale}`. The last ` {` opens it.
fn take_narrator(name: &str) -> (String, Option<String>) {
    if let Some(body) = name.strip_suffix('}') {
        if let Some(at) = body.rfind(" {") {
            let narrator = body[at + 2..].trim();
            if !narrator.is_empty() {
                return (body[..at].trim().to_string(), Some(narrator.to_string()));
            }
        }
    }
    (name.to_string(), None)
}

/// One ` - `-separated part read as a series sequence: an optional
/// label (`Vol `, `Vol. `, `Volume `, `Book `, any case), up to three
/// digits with an optional decimal of one or two digits, an optional
/// trailing dot, then the end or a space and more text. More text is
/// accepted only after a label or a trailing dot, so `101 Dalmatians`
/// is a title and `101. Dalmatians` is book 101. Returns the sequence
/// and what is left of the part.
fn sequence_part(part: &str) -> Option<(String, String)> {
    let lower = part.to_ascii_lowercase();
    let label = ["vol. ", "vol ", "volume ", "book "]
        .iter()
        .find(|l| lower.starts_with(*l))
        .map(|l| l.len())
        .unwrap_or(0);
    let s = &part[label..];
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && i < 3 && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i;
    // A decimal: a dot followed by one or two digits.
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        while j < b.len() && j - (i + 1) < 2 && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > i + 1 {
            i = j;
        }
    }
    let sequence = &s[..i];
    if int_digits == 0 && !sequence.starts_with('.') || sequence.is_empty() {
        return None;
    }
    let mut rest = &s[i..];
    let dot = rest.starts_with('.');
    if dot {
        rest = &rest[1..];
    }
    let leftover = if rest.is_empty() {
        ""
    } else {
        rest.strip_prefix(' ')?
    };
    if !leftover.trim().is_empty() && label == 0 && !dot {
        return None;
    }
    Some((sequence.to_string(), leftover.trim().to_string()))
}

/// A publish year at the very start: four digits, optionally in
/// parentheses, then a hyphen (spaces around it optional) and the
/// rest of the name: `1994 - Title`, `(1994) - Title`, `1994-Title`.
fn take_year(name: &str) -> (String, Option<i32>) {
    let t = name.trim_start();
    let inner = t.strip_prefix('(').unwrap_or(t);
    let open = inner.len() != t.len();
    let digits = inner
        .get(..4)
        .filter(|d| d.bytes().all(|c| c.is_ascii_digit()));
    if let Some(d) = digits {
        let mut rest = &inner[4..];
        if open {
            match rest.strip_prefix(')') {
                Some(r) => rest = r,
                None => return (name.to_string(), None),
            }
        } else if let Some(r) = rest.strip_prefix(')') {
            rest = r;
        }
        if let Some(r) = rest.trim_start().strip_prefix('-') {
            let title = r.trim();
            if !title.is_empty() {
                return (title.to_string(), d.parse().ok());
            }
        }
    }
    (name.to_string(), None)
}

/// Parse a title folder's name, Audiobookshelf's way (D28, AB11), in
/// its order: an ASIN `[…]`, a narrator `{…}` at the end, a series
/// sequence (only when the book has a series folder: the first
/// ` - `-separated part that reads as one, see [`sequence_part`]), a
/// publish year at the start, and — only with the library's
/// `parse_subtitles` on — a subtitle after the first ` - `. Narrators
/// are split as folder authors are. When nothing is left for a title,
/// the name is the title, untouched.
pub fn parse_title_folder(name: &str, has_series: bool, parse_subtitles: bool) -> TitleFolder {
    let raw = name.trim();
    let (rest, asin) = take_asin(raw);
    let (rest, narrator) = take_narrator(&rest);
    let mut rest = rest;
    let mut sequence = None;
    if has_series {
        let parts: Vec<&str> = rest.split(" - ").collect();
        if let Some((i, (seq, left))) = parts
            .iter()
            .enumerate()
            .find_map(|(i, p)| sequence_part(p).map(|r| (i, r)))
        {
            sequence = Some(seq);
            let mut kept: Vec<String> = parts.iter().map(|p| p.to_string()).collect();
            if left.is_empty() {
                kept.remove(i);
            } else {
                kept[i] = left;
            }
            rest = kept.join(" - ");
        }
    }
    let (rest, year) = take_year(&rest);
    let (title, subtitle) = match rest.split_once(" - ") {
        Some((t, sub)) if parse_subtitles && !sub.trim().is_empty() => {
            (t.trim().to_string(), Some(sub.trim().to_string()))
        }
        _ => (rest.trim().to_string(), None),
    };
    TitleFolder {
        title: if title.is_empty() {
            raw.to_string()
        } else {
            title
        },
        subtitle,
        sequence,
        year,
        narrators: narrator
            .map(|n| split_folder_authors(&n))
            .unwrap_or_default(),
        asin,
    }
}

/// The `folder` source (AB2; AB6, AB7, AB11 aligned it with
/// Audiobookshelf, D28):
///
/// - the **title folder's name**, parsed ([`parse_title_folder`]):
///   title, and what the name carries — ASIN, narrators, publish year,
///   the sequence in the series folder, the subtitle when the library
///   parses subtitles. A book that is one file in a library root keeps
///   its file name, without the extension, as the title, unparsed;
/// - the **author folder**, the nearest of the three folders that
///   count (`Author/Title`, `Author/Series/Title`; `A/B/Author/Series/
///   Title` too), split as ABS splits names;
/// - the **series folder** ([`folder_series`]), with the sequence the
///   title folder gave.
pub fn folder_fields(book_key: &str, root_file: bool, parse_subtitles: bool) -> BookFields {
    if root_file {
        return BookFields {
            title: Some(file_stem(book_key)),
            ..Default::default()
        }
        .cleaned();
    }
    let (author, series) = author_and_series_folders(book_key, root_file);
    let series = series.map(str::trim).filter(|s| !s.is_empty());
    let name = book_key.rsplit('/').next().unwrap_or(book_key);
    let parsed = parse_title_folder(name, series.is_some(), parse_subtitles);
    BookFields {
        title: Some(parsed.title),
        subtitle: parsed.subtitle,
        authors: author.map(split_folder_authors).filter(|a| !a.is_empty()),
        narrators: Some(parsed.narrators).filter(|n| !n.is_empty()),
        year: parsed.year,
        asin: parsed.asin,
        series: series.map(|name| vec![SeriesEntry::new(name, parsed.sequence.as_deref())]),
        ..Default::default()
    }
    .cleaned()
}

// ----------------------------------------------------------------- tags

fn year_of(value: &str) -> Option<i32> {
    parlour_metadata::books::year_of(value)
}

/// The `tags` source, from the **first track** in disc/track order
/// (D30, as Audiobookshelf reads them; AB2 took each field from the
/// first track that had it):
///
/// - title: `album`; for a single-file book also `title`;
/// - subtitle: `subtitle`;
/// - authors: `album_artist`, else `artist` (split as AB2 splits);
/// - narrators: `composer`;
/// - year: the first four digits of `date`, else `year`;
/// - publisher, isbn, asin, language: the tags of those names;
/// - overview: `description`, else `comment`;
/// - genres: `genre`.
pub fn tag_fields(root_file: bool, first: Option<&AudioProbe>) -> BookFields {
    let Some(t) = first else {
        return BookFields::default();
    };
    let tag = |k: &str| t.tag(k).map(str::to_string);
    let title = if root_file {
        tag("album").or_else(|| tag("title"))
    } else {
        tag("album")
    };
    BookFields {
        title,
        subtitle: tag("subtitle"),
        authors: tag("album_artist")
            .or_else(|| tag("artist"))
            .map(|a| split_people(&a)),
        narrators: tag("composer").map(|n| split_people(&n)),
        year: tag("date")
            .as_deref()
            .and_then(year_of)
            .or_else(|| tag("year").as_deref().and_then(year_of)),
        publisher: tag("publisher"),
        overview: tag("description").or_else(|| tag("comment")),
        genres: tag("genre").map(|g| split_genres(&g)),
        isbn: tag("isbn"),
        asin: tag("asin"),
        language: tag("language").filter(|l| !l.eq_ignore_ascii_case("und")),
        google_books_id: None,
        open_library_id: None,
        series: tag_series(t),
    }
    .cleaned()
}

/// One `Name #sequence` piece, Audiobookshelf's way: the sequence is
/// what follows the last ` #`, when that is one word with no `#` in
/// it; otherwise the whole piece is the name and there is no
/// sequence.
pub fn parse_series_string(piece: &str) -> SeriesEntry {
    let piece = piece.trim();
    if let Some(at) = piece.rfind(" #") {
        let seq = &piece[at + 2..];
        if !seq.is_empty() && !seq.contains('#') && !seq.chars().any(char::is_whitespace) {
            return SeriesEntry::new(piece[..at].trim(), Some(seq));
        }
    }
    SeriesEntry::new(piece, None)
}

/// The series a track's tags name (AB7, D28, as Audiobookshelf reads
/// them):
///
/// - the name from `series`, else `show`, else `mvnm` (MP4's movement
///   name); the sequence from `series-part`, else `episode_id`, else
///   `mvin`, else `part`. When both name and sequence hold the same
///   number (more than one) of `;`-separated parts, they pair up by
///   position into several series; otherwise it is one series, both
///   values taken whole;
/// - only when there is no name tag, `grouping` (else `grp1`): split
///   on `;`, each part `Name #sequence` or just `Name`.
pub fn tag_series(t: &AudioProbe) -> Option<Vec<SeriesEntry>> {
    let first = |keys: &[&str]| keys.iter().find_map(|k| t.tag(k));
    if let Some(name) = first(&["series", "show", "mvnm"]) {
        let part = first(&["series-part", "episode_id", "mvin", "part"]);
        if let Some(part) = part.filter(|p| name.contains(';') && p.contains(';')) {
            let names: Vec<&str> = name.split(';').collect();
            let parts: Vec<&str> = part.split(';').collect();
            if names.len() == parts.len() {
                return Some(
                    names
                        .iter()
                        .zip(parts)
                        .map(|(n, p)| SeriesEntry::new(n, Some(p)))
                        .collect(),
                );
            }
        }
        return Some(vec![SeriesEntry::new(name, part)]);
    }
    let grouping = first(&["grouping", "grp1"])?;
    Some(grouping.split(';').map(parse_series_string).collect())
}

// ------------------------------------------------------------------ nfo

/// Largest local metadata file read (as XFER6 caps NFOs).
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

fn text_of(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err("the file is larger than 4 MiB".to_string());
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    Ok(String::from_utf8_lossy(bytes).replace("\r\n", "\n"))
}

/// The last four-digit year in a value: `(P)2014 Example ©2013` →
/// 2013.
fn last_year(value: &str) -> Option<i32> {
    let chars: Vec<char> = value.chars().collect();
    let mut found = None;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == 4 {
                let y: String = chars[start..i].iter().collect();
                if let Ok(y) = y.parse::<i32>() {
                    if (1000..=2999).contains(&y) {
                        found = Some(y);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    found
}

/// The `nfo` source: an Audiobookshelf-style book `.nfo`, plain text
/// with one `Key: value` per line (keys matched without regard to
/// case). Read: `Title` (a `Title: Subtitle` value splits at the first
/// `: `), `Author`, `Narrator` / `Read by`, `Genre`, `Publisher`,
/// `ASIN`, `ISBN` / `ISBN-10` / `ISBN-13`, `Language` / `Lang`, the
/// date and copyright lines (`Release Date`, `Date`, `Copyright`, …:
/// the last four-digit year), and `Book Description`, which runs to
/// the end of the file. Later lines win over earlier ones. An XML
/// (Kodi-style) `.nfo` is not this format and says nothing.
pub fn parse_book_nfo(bytes: &[u8]) -> Result<BookFields, String> {
    let text = text_of(bytes)?;
    if text.trim_start().starts_with('<') {
        return Ok(BookFields::default());
    }
    let mut f = BookFields::default();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        // `Book Description` (with or without a colon) starts the
        // description, which runs to the end of the file; a ruler line
        // (`=====`) right under the heading is skipped.
        let head = line.trim_start();
        if head.len() >= 16 && head[..16].eq_ignore_ascii_case("book description") {
            let first = head[16..].trim_start().trim_start_matches(':').trim();
            let mut desc: Vec<&str> = Vec::new();
            if !first.is_empty() {
                desc.push(first);
            }
            let mut rest = lines.by_ref().peekable();
            while rest.peek().is_some_and(|l| {
                !l.trim().is_empty() && l.trim().chars().all(|c| c == '=' || c == '-')
            }) {
                rest.next();
            }
            desc.extend(rest);
            f.overview = Some(desc.join("\n"));
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = value.trim();
        match key.as_str() {
            _ if value.is_empty() => {}
            "title" => match value.split_once(": ") {
                Some((t, sub)) => {
                    f.title = Some(t.to_string());
                    f.subtitle = Some(sub.to_string());
                }
                None => f.title = Some(value.to_string()),
            },
            "author" => f.authors = Some(split_people(value)),
            "narrator" | "read by" => f.narrators = Some(split_people(value)),
            "genre" => f.genres = Some(split_genres(value)),
            "publisher" => f.publisher = Some(value.to_string()),
            "asin" => f.asin = Some(value.to_string()),
            "isbn" | "isbn-10" | "isbn-13" => f.isbn = Some(value.to_string()),
            "language" | "lang" => f.language = Some(value.to_string()),
            "copyright"
            | "audible.com release"
            | "audiobook copyright"
            | "book copyright"
            | "recording copyright"
            | "release date"
            | "date" => {
                if let Some(y) = last_year(value) {
                    f.year = Some(y);
                }
            }
            _ => {}
        }
    }
    Ok(f.cleaned())
}

// ------------------------------------------------------------------ txt

/// The `txt` source: `desc.txt` (the whole file, trimmed) is the
/// description, and the first line of `reader.txt` the narrators.
pub fn txt_fields(desc: Option<&[u8]>, reader: Option<&[u8]>) -> Result<BookFields, String> {
    let mut f = BookFields::default();
    if let Some(d) = desc {
        f.overview = Some(text_of(d)?);
    }
    if let Some(r) = reader {
        let t = text_of(r)?;
        if let Some(first) = t.lines().map(str::trim).find(|l| !l.is_empty()) {
            f.narrators = Some(split_people(first));
        }
    }
    Ok(f.cleaned())
}

// ------------------------------------------------------------------ opf

/// The `opf` source: an OPF package document's `<metadata>`
/// (namespace prefixes ignored): `dc:title`, `dc:subtitle`,
/// `dc:creator` (role `aut`, or no role, → authors; role `nrt` →
/// narrators), `dc:date` (its year), `dc:publisher`, `dc:identifier`
/// with scheme `ISBN` or `ASIN`, `dc:description` (HTML stripped),
/// `dc:subject` (genres) and `dc:language`. Series (`calibre:series`)
/// waits for AB7.
pub fn parse_opf(bytes: &[u8]) -> Result<BookFields, String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let text = text_of(bytes)?;
    let mut reader = Reader::from_str(&text);
    let xml = |e: quick_xml::Error| format!("not a readable OPF: {e}");
    let local = |name: &[u8]| -> String {
        let n = String::from_utf8_lossy(name).to_lowercase();
        n.rsplit(':').next().unwrap_or(&n).to_string()
    };

    let mut f = BookFields::default();
    let mut authors: Vec<String> = Vec::new();
    let mut narrators: Vec<String> = Vec::new();
    let mut genres: Vec<String> = Vec::new();
    let mut in_metadata = false;
    let mut saw_package = false;
    // The open element inside <metadata>: (name, role, scheme, text).
    let mut open: Option<(String, Option<String>, Option<String>, String)> = None;
    loop {
        match reader.read_event().map_err(xml)? {
            Event::Eof => break,
            Event::Start(e) => {
                let name = local(e.name().as_ref());
                if name == "package" {
                    saw_package = true;
                } else if name == "metadata" || name == "dc-metadata" {
                    in_metadata = true;
                } else if in_metadata && open.is_none() {
                    let mut role = None;
                    let mut scheme = None;
                    for a in e.attributes().flatten() {
                        let key = local(a.key.as_ref());
                        let value = a
                            .unescape_value()
                            .map(|v| v.trim().to_lowercase())
                            .unwrap_or_default();
                        match key.as_str() {
                            "role" => role = Some(value),
                            "scheme" => scheme = Some(value),
                            _ => {}
                        }
                    }
                    open = Some((name, role, scheme, String::new()));
                }
            }
            Event::Text(t) => {
                if let Some(o) = open.as_mut() {
                    o.3.push_str(&t.decode().map_err(|e| e.to_string())?);
                }
            }
            Event::CData(t) => {
                if let Some(o) = open.as_mut() {
                    o.3.push_str(&t.decode().map_err(|e| e.to_string())?);
                }
            }
            Event::GeneralRef(r) => {
                if let Some(o) = open.as_mut() {
                    let resolved = match r.resolve_char_ref().map_err(xml)? {
                        Some(ch) => ch.to_string(),
                        None => {
                            let name = r.decode().map_err(|e| e.to_string())?;
                            quick_xml::escape::resolve_predefined_entity(&name)
                                .unwrap_or("")
                                .to_string()
                        }
                    };
                    o.3.push_str(&resolved);
                }
            }
            Event::End(e) => {
                let name = local(e.name().as_ref());
                if name == "metadata" {
                    in_metadata = false;
                    open = None;
                } else if let Some((el, role, scheme, value)) = open.take() {
                    if el != name {
                        // A nested element ended; keep the outer open.
                        open = Some((el, role, scheme, value));
                        continue;
                    }
                    let value = value.trim().to_string();
                    if value.is_empty() {
                        continue;
                    }
                    match el.as_str() {
                        "title" if f.title.is_none() => f.title = Some(value),
                        "subtitle" => f.subtitle = Some(value),
                        "creator" => match role.as_deref() {
                            None | Some("aut") => authors.push(value),
                            Some("nrt") => narrators.push(value),
                            _ => {}
                        },
                        "date" => f.year = year_of(&value).or(f.year),
                        "publisher" => f.publisher = Some(value),
                        "identifier" => {
                            let lower = value.to_lowercase();
                            match scheme.as_deref() {
                                Some("isbn") => f.isbn = Some(value),
                                Some("asin") => f.asin = Some(value),
                                _ if lower.starts_with("urn:isbn:") => {
                                    f.isbn = Some(value[9..].trim().to_string())
                                }
                                _ => {}
                            }
                        }
                        "description" => {
                            f.overview = Some(parlour_metadata::books::strip_html(&value))
                        }
                        "subject" => genres.push(value),
                        "language" => f.language = Some(value),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if !saw_package {
        return Err("not an OPF package document (no <package> element)".to_string());
    }
    if !authors.is_empty() {
        f.authors = Some(authors);
    }
    if !narrators.is_empty() {
        f.narrators = Some(narrators);
    }
    if !genres.is_empty() {
        f.genres = Some(genres);
    }
    Ok(f.cleaned())
}

// ------------------------------------------------------------- abs json

fn json_strings(v: Option<&Value>) -> Option<Vec<String>> {
    let arr = v?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o.get("name").and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
            .collect(),
    )
}

/// The `abs_json` source: Audiobookshelf's `metadata.json`, read
/// leniently (it isn't Parlour's format): `title`, `subtitle`,
/// `authors`, `narrators`, `genres`, `publishedYear` (else the year of
/// `publishedDate`), `publisher`, `description`, `isbn`, `asin` and
/// `language`. A field of an unexpected type is skipped; anything
/// else in the file (series, tags, chapters, …) is not read.
pub fn parse_abs_json(bytes: &[u8]) -> Result<BookFields, String> {
    let text = text_of(bytes)?;
    let v: Value =
        serde_json::from_str(&text).map_err(|e| format!("metadata.json is not JSON: {e}"))?;
    let obj = v
        .as_object()
        .ok_or_else(|| "metadata.json is not a JSON object".to_string())?;
    let s = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_string);
    let year = match obj.get("publishedYear") {
        Some(Value::String(y)) => year_of(y),
        Some(Value::Number(n)) => n.as_i64().and_then(|y| i32::try_from(y).ok()),
        _ => None,
    }
    .or_else(|| s("publishedDate").as_deref().and_then(year_of));
    Ok(BookFields {
        title: s("title"),
        subtitle: s("subtitle"),
        authors: json_strings(obj.get("authors")),
        narrators: json_strings(obj.get("narrators")),
        year,
        publisher: s("publisher"),
        overview: s("description").map(|d| parlour_metadata::books::strip_html(&d)),
        genres: json_strings(obj.get("genres")),
        isbn: s("isbn"),
        asin: s("asin"),
        language: s("language"),
        google_books_id: None,
        open_library_id: None,
        // Series from the folder and tags only, so far (AB7 reading
        // 3); metadata.json's `series` is left for later.
        series: None,
    }
    .cleaned())
}

// -------------------------------------------------------------- sidecar

/// The `sidecar` source from a checked `audiobook` sidecar. Every
/// field it carries is authoritative; `people`, when present, carries
/// both the authors and the narrators (a role it doesn't list is
/// "none").
pub fn sidecar_fields(doc: &Sidecar) -> BookFields {
    let people = doc.people_in_order();
    let role = |r: &str| -> Option<Vec<String>> {
        people.as_ref().map(|list| {
            let mut v: Vec<&SidecarPerson> = list.iter().filter(|p| p.role == r).collect();
            v.sort_by_key(|p| p.sort_order.unwrap_or(0));
            v.into_iter().map(|p| p.name.clone()).collect()
        })
    };
    BookFields {
        title: doc.title.clone(),
        subtitle: doc.subtitle.clone(),
        authors: role("author"),
        narrators: role("narrator"),
        year: doc.year,
        publisher: doc.publisher.clone(),
        overview: doc.overview.clone(),
        genres: doc.genres.clone(),
        isbn: doc.isbn.clone(),
        asin: doc.asin.clone(),
        language: doc.language.clone(),
        google_books_id: doc.google_books_id.clone(),
        open_library_id: doc.open_library_id.clone(),
        // The sidecar format (§13.15) carries no series: it says
        // nothing about them, so the folder's and the tags' stand.
        series: None,
    }
}

/// The sidecar Parlour writes for a book: everything the book holds,
/// absent fields left out (so the same book always gives the same
/// bytes). `people` lists the authors, then the narrators, each in
/// order.
pub fn book_sidecar_document(f: &BookFields) -> Sidecar {
    let mut doc = Sidecar::new(SidecarKind::Audiobook);
    let f = f.clone().cleaned();
    doc.title = f.title;
    doc.subtitle = f.subtitle;
    doc.year = f.year;
    doc.overview = f.overview;
    doc.genres = f.genres.filter(|g| !g.is_empty());
    doc.publisher = f.publisher;
    doc.isbn = f.isbn;
    doc.asin = f.asin;
    doc.language = f.language;
    doc.google_books_id = f
        .google_books_id
        .filter(|id| parlour_metadata::books::valid_google_id(id));
    doc.open_library_id = f
        .open_library_id
        .filter(|id| parlour_metadata::books::valid_open_library_id(id));
    let mut people = Vec::new();
    for (role, names) in [("author", &f.authors), ("narrator", &f.narrators)] {
        for (i, name) in names.iter().flatten().enumerate() {
            people.push(SidecarPerson {
                name: name.clone(),
                role: role.to_string(),
                character_name: None,
                sort_order: Some(i as i32),
                tmdb_id: None,
            });
        }
    }
    if !people.is_empty() {
        doc.people = Some(people);
    }
    doc
}

// --------------------------------------------------------------- match

/// How a match lays a provider's values over a book (D30: the admin
/// chooses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// Every value the provider has replaces the book's.
    Overwrite,
    /// The provider's values fill only what the book lacks. A
    /// library-wide quick match always fills gaps, as
    /// Audiobookshelf's does by default.
    FillGaps,
}

impl MatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchMode::Overwrite => "overwrite",
            MatchMode::FillGaps => "fill_gaps",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "overwrite" => Some(MatchMode::Overwrite),
            "fill_gaps" => Some(MatchMode::FillGaps),
            _ => None,
        }
    }
}

/// A provider's record as book fields. The provider's own id goes in
/// its id field.
pub fn candidate_fields(c: &parlour_metadata::books::BookCandidate) -> BookFields {
    BookFields {
        title: Some(c.title.clone()),
        subtitle: c.subtitle.clone(),
        authors: Some(c.authors.clone()),
        narrators: Some(c.narrators.clone()),
        year: c.year,
        publisher: c.publisher.clone(),
        overview: c.description.clone(),
        genres: Some(c.genres.clone()),
        isbn: c.isbn.clone(),
        asin: c.asin.clone(),
        language: c.language.clone(),
        google_books_id: (c.provider == "google").then(|| c.provider_id.clone()),
        open_library_id: (c.provider == "openlibrary").then(|| c.provider_id.clone()),
        // Neither provider gives series; a match leaves them alone.
        series: None,
    }
    .cleaned()
}

/// The book after a match. `Overwrite`: whatever the provider has
/// replaces the book's value (an empty provider list replaces
/// nothing). `FillGaps`: the provider fills only fields the book has
/// no value for. Either way the provider's id is recorded, since it
/// says what the book was matched to.
pub fn apply_match(current: &BookFields, matched: &BookFields, mode: MatchMode) -> BookFields {
    let mut out = current.clone();
    match mode {
        MatchMode::Overwrite => merge(&mut out, matched, false),
        MatchMode::FillGaps => {
            fn fill<T: Clone>(b: &mut Option<T>, s: &Option<T>, empty: impl Fn(&T) -> bool) {
                if b.as_ref().is_none_or(&empty) {
                    if let Some(v) = s.as_ref().filter(|v| !empty(v)) {
                        *b = Some(v.clone());
                    }
                }
            }
            let blank = |s: &String| s.trim().is_empty();
            let none = |v: &Vec<String>| v.is_empty();
            fill(&mut out.title, &matched.title, blank);
            fill(&mut out.subtitle, &matched.subtitle, blank);
            fill(&mut out.authors, &matched.authors, none);
            fill(&mut out.narrators, &matched.narrators, none);
            fill(&mut out.year, &matched.year, |_| false);
            fill(&mut out.publisher, &matched.publisher, blank);
            fill(&mut out.overview, &matched.overview, blank);
            fill(&mut out.genres, &matched.genres, none);
            fill(&mut out.isbn, &matched.isbn, blank);
            fill(&mut out.asin, &matched.asin, blank);
            fill(&mut out.language, &matched.language, blank);
        }
    }
    if matched.google_books_id.is_some() {
        out.google_books_id = matched.google_books_id.clone();
    }
    if matched.open_library_id.is_some() {
        out.open_library_id = matched.open_library_id.clone();
    }
    out
}

// --------------------------------------------------------- source files

/// The local metadata files one book reads, by absolute path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BookSourceFiles {
    pub nfo: Option<String>,
    pub opf: Option<String>,
    pub desc: Option<String>,
    pub reader: Option<String>,
    pub abs_json: Option<String>,
    /// `book.parlour.json` (or, for a book that is one file in the
    /// library root, `<stem>.parlour.json` beside it).
    pub sidecar: Option<String>,
}

impl BookSourceFiles {
    pub fn paths(&self) -> Vec<&str> {
        [
            &self.nfo,
            &self.opf,
            &self.desc,
            &self.reader,
            &self.abs_json,
            &self.sidecar,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect()
    }
}

/// Which of a book folder's files are its metadata sources, given the
/// folder's files as (name, absolute path), sorted by name. Names
/// match without regard to case. The first `.nfo` and the first
/// `.opf` by name are read (a folder rarely has more than one).
pub fn pick_source_files(files: &[(String, String)]) -> BookSourceFiles {
    let mut out = BookSourceFiles::default();
    for (name, abs) in files {
        let lower = name.to_lowercase();
        let slot = if lower == crate::sidecar::BOOK_FILE_NAME {
            &mut out.sidecar
        } else if lower == "desc.txt" {
            &mut out.desc
        } else if lower == "reader.txt" {
            &mut out.reader
        } else if lower == "metadata.json" {
            &mut out.abs_json
        } else if lower.ends_with(".nfo") {
            &mut out.nfo
        } else if lower.ends_with(".opf") {
            &mut out.opf
        } else {
            continue;
        };
        if slot.is_none() {
            *slot = Some(abs.clone());
        }
    }
    out
}

/// Whether a file name could be a book's metadata source (the walk
/// keeps only these).
pub fn is_source_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".nfo")
        || lower.ends_with(".opf")
        || lower.ends_with(crate::sidecar::FILE_SUFFIX)
        || matches!(lower.as_str(), "desc.txt" | "reader.txt" | "metadata.json")
}

/// Bumped when the merge rules change, so every book is merged again
/// at the next scan. `ab11.1`: the title folder's name is parsed and
/// the author folder is the nearest one (AB11).
pub const MERGE_RULES_VERSION: &str = "ab11.1";

/// The fingerprint a book's merge depends on: the rules' version, the
/// precedence, the library's parse-subtitles setting (AB11), and each
/// source file's path, size and mtime (µs).
pub fn sources_fingerprint(
    precedence: &[MetadataSource],
    parse_subtitles: bool,
    files: &[(String, u64, i64)],
) -> String {
    let mut lines = vec![
        MERGE_RULES_VERSION.to_string(),
        precedence
            .iter()
            .map(|s| s.id())
            .collect::<Vec<_>>()
            .join(","),
    ];
    if parse_subtitles {
        lines.push("subtitles".to_string());
    }
    let mut f: Vec<String> = files
        .iter()
        .map(|(p, size, mtime)| format!("{p}\t{size}\t{mtime}"))
        .collect();
    f.sort();
    lines.extend(f);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    fn list(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|x| x.to_string()).collect())
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/books/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn precedence_lists_are_checked_and_read_leniently() {
        assert_eq!(
            default_precedence_ids(),
            ["folder", "tags", "nfo", "txt", "opf", "abs_json", "sidecar"]
        );
        let ok = parse_precedence(&["sidecar".into(), "tags".into()]).unwrap();
        assert_eq!(ok, vec![MetadataSource::Sidecar, MetadataSource::Tags]);
        assert!(parse_precedence(&[]).is_err());
        assert!(parse_precedence(&["tags".into(), "tags".into()]).is_err());
        assert!(parse_precedence(&["audnexus".into()]).is_err());
        assert_eq!(
            stored_precedence(&["tags".into(), "future".into(), "tags".into()]),
            vec![MetadataSource::Tags]
        );
        assert_eq!(stored_precedence(&[]), DEFAULT_PRECEDENCE.to_vec());
    }

    #[test]
    fn later_sources_win_field_by_field_and_lists_whole() {
        let sources = BookSources {
            folder: folder_fields("Folder Author/Folder Title", false, false),
            tags: BookFields {
                title: s("Tag Title"),
                authors: list(&["Tag Author"]),
                genres: list(&["Tag Genre", "Other"]),
                year: Some(2001),
                ..Default::default()
            },
            nfo: Some(BookFields {
                genres: list(&["Nfo Genre"]),
                narrators: Some(vec![]), // an empty list says nothing
                ..Default::default()
            }),
            txt: Some(BookFields {
                overview: s("From desc.txt"),
                narrators: list(&["Reader One"]),
                ..Default::default()
            }),
            opf: None,
            abs_json: Some(BookFields {
                title: s("Json Title"),
                ..Default::default()
            }),
            sidecar: Some(BookFields {
                genres: Some(vec![]), // authoritative: "no genres"
                publisher: s("Sidecar Press"),
                ..Default::default()
            }),
        };
        let merged = merge_sources(&DEFAULT_PRECEDENCE, &sources);
        assert_eq!(merged.title, s("Json Title"));
        assert_eq!(merged.authors, list(&["Tag Author"]));
        assert_eq!(merged.narrators, list(&["Reader One"]));
        assert_eq!(merged.overview, s("From desc.txt"));
        assert_eq!(merged.genres, Some(vec![]));
        assert_eq!(merged.publisher, s("Sidecar Press"));
        assert_eq!(merged.year, Some(2001));

        // The admin's order: tags last, so tags beat the others.
        let order = parse_precedence(&[
            "sidecar".into(),
            "abs_json".into(),
            "txt".into(),
            "nfo".into(),
            "folder".into(),
            "tags".into(),
        ])
        .unwrap();
        let merged = merge_sources(&order, &sources);
        assert_eq!(merged.title, s("Tag Title"));
        assert_eq!(merged.genres, list(&["Tag Genre", "Other"]));

        // A source left out isn't read; the title falls back to the
        // folder's name even when the folder source is out too.
        let merged = merge_sources(&[MetadataSource::Txt], &sources);
        assert_eq!(merged.title, s("Folder Title"));
        assert_eq!(merged.authors, None);
        assert_eq!(merged.overview, s("From desc.txt"));
    }

    #[test]
    fn folder_authors_split_like_audiobookshelf() {
        assert_eq!(
            split_folder_authors("Jane Doe, John Roe"),
            ["Jane Doe", "John Roe"]
        );
        assert_eq!(
            split_folder_authors("Sanderson, Brandon"),
            ["Brandon Sanderson"]
        );
        assert_eq!(
            split_folder_authors("Doe, Jane, Roe, John"),
            ["Jane Doe", "John Roe"]
        );
        assert_eq!(
            split_folder_authors("Tolkien, J. R. R."),
            ["J. R. R. Tolkien"]
        );
        assert_eq!(
            split_folder_authors("Ann Writer and Bob Penman & Cy Scribe; Di Author"),
            ["Ann Writer", "Bob Penman", "Cy Scribe", "Di Author"]
        );
        assert_eq!(split_folder_authors("Anderson"), ["Anderson"]);
        assert_eq!(split_folder_authors("Sandy AND Andy"), ["Sandy", "Andy"]);
        assert_eq!(split_folder_authors("Rand"), ["Rand"]);
        assert!(split_folder_authors(" , ").is_empty());

        let f = folder_fields("Doe, Jane/A Series/Book Two", false, false);
        assert_eq!(f.title, s("Book Two"));
        assert_eq!(f.authors, list(&["Jane Doe"]));
        let f = folder_fields("Loose Book.mp3", true, false);
        assert_eq!(f.title, s("Loose Book"));
        assert_eq!(f.authors, None);
        assert_eq!(folder_fields("Only Folder", false, false).authors, None);
    }

    fn probe(tags: &[(&str, &str)]) -> AudioProbe {
        AudioProbe {
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn tags_come_from_the_first_track_only() {
        let first = probe(&[
            ("album", "Book One"),
            ("album_artist", "Jane Author"),
            ("artist", "Track Artist"),
            ("composer", "Nick Narrator & Nora Reader"),
            ("date", "2019-05-01"),
            ("genre", "Fantasy; Adventure"),
            ("comment", "A comment"),
            ("subtitle", "A Subtitle"),
            ("publisher", "Tag Press"),
            ("isbn", "9780000000002"),
            ("asin", "B000000000"),
            ("language", "eng"),
            ("title", "Opening"),
        ]);
        let f = tag_fields(false, Some(&first));
        assert_eq!(f.title, s("Book One"));
        assert_eq!(f.subtitle, s("A Subtitle"));
        assert_eq!(f.authors, list(&["Jane Author"]));
        assert_eq!(f.narrators, list(&["Nick Narrator", "Nora Reader"]));
        assert_eq!(f.year, Some(2019));
        assert_eq!(f.overview, s("A comment"));
        assert_eq!(f.genres, list(&["Fantasy", "Adventure"]));
        assert_eq!(f.publisher, s("Tag Press"));
        assert_eq!(f.isbn, s("9780000000002"));
        assert_eq!(f.asin, s("B000000000"));
        assert_eq!(f.language, s("eng"));
        // `artist` when there's no album artist; `title` only for a
        // single-file book; `und` is no language.
        let f = tag_fields(
            true,
            Some(&probe(&[
                ("artist", "Solo Writer"),
                ("title", "The Single"),
                ("language", "und"),
                ("year", "1999"),
            ])),
        );
        assert_eq!(f.authors, list(&["Solo Writer"]));
        assert_eq!(f.title, s("The Single"));
        assert_eq!(f.language, None);
        assert_eq!(f.year, Some(1999));
        assert_eq!(
            tag_fields(false, Some(&probe(&[("title", "Opening")]))).title,
            None
        );
        assert_eq!(tag_fields(false, None), BookFields::default());
    }

    fn series(v: &[(&str, Option<&str>)]) -> Option<Vec<SeriesEntry>> {
        Some(v.iter().map(|(n, q)| SeriesEntry::new(n, *q)).collect())
    }

    #[test]
    fn the_series_folder_is_the_one_above_the_title() {
        assert_eq!(
            folder_series("Jane Doe/The Long Road/Book Two", false),
            s("The Long Road")
        );
        // Only the nearest three folders count.
        assert_eq!(
            folder_series("Fantasy/Jane Doe/The Long Road/Book Two", false),
            s("The Long Road")
        );
        assert_eq!(folder_series("Jane Doe/Book Two", false), None);
        assert_eq!(folder_series("Book Two", false), None);
        assert_eq!(folder_series("A/B/Loose.m4b", true), None);
        let f = folder_fields("Jane Doe/The Long Road/Book Two", false, false);
        assert_eq!(f.series, series(&[("The Long Road", None)]));
        assert_eq!(
            folder_fields("Jane Doe/Book Two", false, false).series,
            None
        );
    }

    #[test]
    fn series_tags_are_read_like_audiobookshelf() {
        let f = |tags: &[(&str, &str)]| tag_fields(false, Some(&probe(tags))).series;
        assert_eq!(
            f(&[("series", "The Long Road"), ("series-part", "2")]),
            series(&[("The Long Road", Some("2"))])
        );
        // The fallbacks, in order.
        assert_eq!(
            f(&[
                ("show", "Shown"),
                ("mvnm", "Movement"),
                ("mvin", "3"),
                ("part", "9")
            ]),
            series(&[("Shown", Some("3"))])
        );
        assert_eq!(f(&[("mvnm", "Movement")]), series(&[("Movement", None)]));
        // Paired lists of the same length make several series...
        assert_eq!(
            f(&[("series", "One; Two"), ("series-part", "1; 4.5")]),
            series(&[("One", Some("1")), ("Two", Some("4.5"))])
        );
        // ...a mismatch keeps both values whole.
        assert_eq!(
            f(&[("series", "One; Two"), ("series-part", "1")]),
            series(&[("One; Two", Some("1"))])
        );
        // `Name #1` in the series tag is a name, not parsed.
        assert_eq!(f(&[("series", "Saga #1")]), series(&[("Saga #1", None)]));
        // Grouping only without a series tag: `;`-split, `Name #seq`.
        assert_eq!(
            f(&[("grouping", "Saga #1; Side Tales #0.5; Plain")]),
            series(&[
                ("Saga", Some("1")),
                ("Side Tales", Some("0.5")),
                ("Plain", None)
            ])
        );
        assert_eq!(f(&[("grp1", "Saga #2")]), series(&[("Saga", Some("2"))]));
        assert_eq!(
            f(&[("series", "Real"), ("grouping", "Ignored #1")]),
            series(&[("Real", None)])
        );
        // Blank and repeated names go; a repeat differing only in
        // case is a repeat.
        assert_eq!(
            f(&[("grouping", "Saga #1; ; saga #2")]),
            series(&[("Saga", Some("1"))])
        );
        assert_eq!(f(&[("album", "No Series")]), None);
    }

    #[test]
    fn series_strings_split_on_the_last_hash() {
        assert_eq!(
            parse_series_string("Saga #1a"),
            SeriesEntry::new("Saga", Some("1a"))
        );
        assert_eq!(
            parse_series_string(" Number #9 Dream #3 "),
            SeriesEntry::new("Number #9 Dream", Some("3"))
        );
        assert_eq!(
            parse_series_string("Saga #"),
            SeriesEntry::new("Saga #", None)
        );
        assert_eq!(
            parse_series_string("Saga#1"),
            SeriesEntry::new("Saga#1", None)
        );
        assert_eq!(
            parse_series_string("Saga #1 2"),
            SeriesEntry::new("Saga #1 2", None)
        );
    }

    #[test]
    fn a_later_source_replaces_the_series_whole() {
        let folder = folder_fields("Jane Doe/The Long Road/Book Two", false, false);
        let tags = BookFields {
            series: series(&[("Other", Some("1")), ("Third", None)]),
            ..Default::default()
        };
        let sources = BookSources {
            folder: folder.clone(),
            tags: tags.clone(),
            ..Default::default()
        };
        let out = merge_sources(&DEFAULT_PRECEDENCE, &sources);
        assert_eq!(out.series, tags.series);
        // Tags without series leave the folder's.
        let sources = BookSources {
            folder: folder.clone(),
            tags: BookFields {
                title: s("Tagged"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            merge_sources(&DEFAULT_PRECEDENCE, &sources).series,
            folder.series
        );
        // A source left out of the list isn't read: no folder, no
        // folder series.
        assert_eq!(
            merge_sources(&[MetadataSource::Tags], &sources).series,
            None
        );
        // Precedence reversed: the folder wins.
        let sources = BookSources {
            folder: folder.clone(),
            tags,
            ..Default::default()
        };
        assert_eq!(
            merge_sources(&[MetadataSource::Tags, MetadataSource::Folder], &sources).series,
            folder.series
        );
    }

    #[test]
    fn audiobookshelf_style_nfo_is_read() {
        let f = parse_book_nfo(&fixture("book.nfo")).unwrap();
        assert_eq!(f.title, s("The Lantern Keeper"));
        assert_eq!(f.subtitle, s("A Harbour Tale"));
        assert_eq!(f.authors, list(&["Ada Fictional"]));
        assert_eq!(f.narrators, list(&["Nell Reader", "Ned Voice"]));
        assert_eq!(f.genres, list(&["Fiction", "Sea Stories"]));
        assert_eq!(f.publisher, s("Example Audio"));
        assert_eq!(f.asin, s("B000000000"));
        assert_eq!(f.isbn, s("9780000000002"));
        assert_eq!(f.language, s("English"));
        assert_eq!(f.year, Some(2012), "the last year line wins");
        assert_eq!(
            f.overview,
            s("A keeper of lights waits.\n\nShe watches: the harbour, the sea.")
        );
        // An XML NFO (Kodi-style) says nothing here.
        assert_eq!(
            parse_book_nfo(b"<?xml version=\"1.0\"?><album><title>X</title></album>").unwrap(),
            BookFields::default()
        );
        assert_eq!(last_year("(P)2014 Example (C)2013"), Some(2013));
        assert_eq!(last_year("no year 12345"), None);
        assert!(parse_book_nfo(&vec![b'a'; MAX_SOURCE_BYTES + 1]).is_err());
        // The heading form, with a ruler under it.
        let f =
            parse_book_nfo(b"Title: T\nBook Description\n================\nLine one.\nLine two.\n")
                .unwrap();
        assert_eq!(f.overview, s("Line one.\nLine two."));
    }

    #[test]
    fn txt_files_give_description_and_narrators() {
        let f = txt_fields(
            Some(b"\xEF\xBB\xBF  A plain description.\r\nSecond line.  \n"),
            Some(b"\nNell Reader & Ned Voice\nignored second line\n"),
        )
        .unwrap();
        assert_eq!(f.overview, s("A plain description.\nSecond line."));
        assert_eq!(f.narrators, list(&["Nell Reader", "Ned Voice"]));
        assert_eq!(
            txt_fields(Some(b"   "), None).unwrap(),
            BookFields::default()
        );
    }

    #[test]
    fn opf_metadata_is_read() {
        let f = parse_opf(&fixture("book.opf")).unwrap();
        assert_eq!(f.title, s("The Lantern Keeper"));
        assert_eq!(f.subtitle, s("A Harbour Tale"));
        assert_eq!(f.authors, list(&["Ada Fictional", "Bo Imaginary"]));
        assert_eq!(f.narrators, list(&["Nell Reader"]));
        assert_eq!(f.year, Some(2011));
        assert_eq!(f.publisher, s("Example Press"));
        assert_eq!(f.isbn, s("9780000000002"));
        assert_eq!(f.asin, s("B000000000"));
        assert_eq!(f.overview, s("A keeper of lights.\nShe waits & watches."));
        assert_eq!(f.genres, list(&["Fiction", "Sea Stories"]));
        assert_eq!(f.language, s("en"));
        assert!(parse_opf(b"<html><body/></html>").is_err());
        assert!(parse_opf(b"<package><metadata><title>x</metadata>").is_err());
    }

    #[test]
    fn audiobookshelf_metadata_json_is_read_leniently() {
        let f = parse_abs_json(&fixture("metadata.json")).unwrap();
        assert_eq!(f.title, s("The Lantern Keeper"));
        assert_eq!(f.subtitle, s("A Harbour Tale"));
        assert_eq!(f.authors, list(&["Ada Fictional"]));
        assert_eq!(f.narrators, list(&["Nell Reader"]));
        assert_eq!(f.genres, list(&["Fiction"]));
        assert_eq!(f.year, Some(2011));
        assert_eq!(f.publisher, s("Example Press"));
        assert_eq!(f.overview, s("A keeper of lights."));
        assert_eq!(f.isbn, None, "a number where text belongs is skipped");
        assert_eq!(f.asin, s("B000000000"));
        assert_eq!(f.language, s("English"));
        let f = parse_abs_json(
            br#"{"publishedDate": "1999-01-02", "authors": [{"name": "Obj Author"}]}"#,
        )
        .unwrap();
        assert_eq!(f.year, Some(1999));
        assert_eq!(f.authors, list(&["Obj Author"]));
        assert!(parse_abs_json(b"[1]").is_err());
        assert!(parse_abs_json(b"{nope").is_err());
    }

    #[test]
    fn book_sidecars_round_trip() {
        let fields = BookFields {
            title: s("The Lantern Keeper"),
            subtitle: s("A Harbour Tale"),
            authors: list(&["Ada Fictional", "Bo Imaginary"]),
            narrators: list(&["Nell Reader"]),
            year: Some(2011),
            publisher: s("Example Press"),
            overview: s("A keeper of lights."),
            genres: list(&["Fiction"]),
            isbn: s("9780000000002"),
            asin: None,
            language: s("en"),
            google_books_id: s("gBk0001-_x"),
            open_library_id: None,
            series: None,
        };
        let doc = book_sidecar_document(&fields);
        assert!(doc.problems().is_empty(), "{:?}", doc.problems());
        let bytes = crate::sidecar::to_bytes(&doc);
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            String::from_utf8(fixture("book.parlour.json")).unwrap(),
            "byte for byte what the writer produces"
        );
        let back = crate::sidecar::parse(&bytes).unwrap();
        assert_eq!(sidecar_fields(&back), fields);

        // An empty book writes only its identity and title.
        let bare = book_sidecar_document(&BookFields {
            title: s("Bare"),
            genres: Some(vec![]),
            ..Default::default()
        });
        assert!(bare.genres.is_none() && bare.people.is_none());
        // `people` present without narrators means "no narrators".
        let mut doc = Sidecar::new(SidecarKind::Audiobook);
        doc.people = Some(vec![SidecarPerson {
            name: "A".into(),
            role: "author".into(),
            character_name: None,
            sort_order: None,
            tmdb_id: None,
        }]);
        assert_eq!(sidecar_fields(&doc).narrators, Some(vec![]));
    }

    #[test]
    fn book_sidecar_rules() {
        let mut doc = Sidecar::new(SidecarKind::Audiobook);
        doc.tmdb_id = Some(5);
        doc.subtitle = Some(" ".into());
        doc.google_books_id = Some("../etc".into());
        doc.open_library_id = Some("OL1M".into());
        doc.people = Some(vec![SidecarPerson {
            name: "A".into(),
            role: "actor".into(),
            character_name: Some("C".into()),
            sort_order: None,
            tmdb_id: None,
        }]);
        let problems = doc.problems();
        assert_eq!(problems.len(), 6, "{problems:?}");
        // Book-only fields are refused on video sidecars.
        let mut movie = Sidecar::new(SidecarKind::Movie);
        movie.isbn = Some("9780000000002".into());
        assert_eq!(movie.problems(), ["isbn is only for an audiobook"]);
        // Video sidecars' bytes don't change: no new field is written.
        let bytes = crate::sidecar::to_bytes(&Sidecar::new(SidecarKind::Movie));
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\n  \"format\": \"parlour-sidecar\",\n  \"format_version\": 1,\n  \"kind\": \"movie\"\n}\n"
        );
    }

    fn candidate() -> parlour_metadata::books::BookCandidate {
        parlour_metadata::books::BookCandidate {
            provider: "google".into(),
            provider_id: "gBk0001-_x".into(),
            title: "The Lantern Keeper".into(),
            subtitle: Some("A Harbour Tale".into()),
            authors: vec!["Ada Fictional".into()],
            narrators: vec![],
            publisher: Some("Example Press".into()),
            year: Some(2011),
            description: Some("From the provider.".into()),
            genres: vec![],
            isbn: Some("9780000000002".into()),
            asin: None,
            language: Some("en".into()),
            cover_url: None,
        }
    }

    #[test]
    fn matches_overwrite_or_fill_gaps() {
        let current = BookFields {
            title: s("lantern keeper"),
            authors: list(&["Tag Author"]),
            narrators: list(&["Nell Reader"]),
            genres: list(&["Tagged"]),
            overview: None,
            year: Some(1999),
            ..Default::default()
        };
        let matched = candidate_fields(&candidate());
        assert_eq!(matched.google_books_id, s("gBk0001-_x"));
        assert_eq!(matched.open_library_id, None);

        let filled = apply_match(&current, &matched, MatchMode::FillGaps);
        assert_eq!(filled.title, s("lantern keeper"), "kept");
        assert_eq!(filled.authors, list(&["Tag Author"]), "kept");
        assert_eq!(filled.year, Some(1999), "kept");
        assert_eq!(filled.overview, s("From the provider."), "filled");
        assert_eq!(filled.subtitle, s("A Harbour Tale"), "filled");
        assert_eq!(filled.publisher, s("Example Press"), "filled");
        assert_eq!(filled.google_books_id, s("gBk0001-_x"), "id recorded");

        let over = apply_match(&current, &matched, MatchMode::Overwrite);
        assert_eq!(over.title, s("The Lantern Keeper"));
        assert_eq!(over.authors, list(&["Ada Fictional"]));
        assert_eq!(over.year, Some(2011));
        // The provider has no narrators or genres: the book keeps its.
        assert_eq!(over.narrators, list(&["Nell Reader"]));
        assert_eq!(over.genres, list(&["Tagged"]));
        // Matching the same record again changes nothing.
        assert_eq!(apply_match(&over, &matched, MatchMode::Overwrite), over);
        assert_eq!(apply_match(&filled, &matched, MatchMode::FillGaps), filled);
        assert_eq!(MatchMode::parse("fill_gaps"), Some(MatchMode::FillGaps));
        assert_eq!(MatchMode::parse("merge"), None);
    }

    #[test]
    fn source_files_are_picked_by_name() {
        let files: Vec<(String, String)> = [
            "B.opf",
            "a.nfo",
            "b.nfo",
            "Desc.TXT",
            "metadata.json",
            "reader.txt",
            "book.parlour.json",
            "notes.txt",
        ]
        .iter()
        .map(|n| (n.to_string(), format!("/b/{n}")))
        .collect();
        let picked = pick_source_files(&files);
        assert_eq!(picked.nfo.as_deref(), Some("/b/a.nfo"));
        assert_eq!(picked.opf.as_deref(), Some("/b/B.opf"));
        assert_eq!(picked.desc.as_deref(), Some("/b/Desc.TXT"));
        assert_eq!(picked.reader.as_deref(), Some("/b/reader.txt"));
        assert_eq!(picked.abs_json.as_deref(), Some("/b/metadata.json"));
        assert_eq!(picked.sidecar.as_deref(), Some("/b/book.parlour.json"));
        assert_eq!(picked.paths().len(), 6);
        assert!(is_source_name("X.NFO") && is_source_name("x.parlour.json"));
        assert!(!is_source_name("notes.txt") && !is_source_name("cover.jpg"));

        let a = sources_fingerprint(&DEFAULT_PRECEDENCE, false, &[("/b/a.nfo".into(), 3, 7)]);
        let b = sources_fingerprint(&[MetadataSource::Tags], false, &[("/b/a.nfo".into(), 3, 7)]);
        let c = sources_fingerprint(&DEFAULT_PRECEDENCE, false, &[("/b/a.nfo".into(), 4, 7)]);
        assert!(a != b && a != c && a.starts_with(MERGE_RULES_VERSION));
        let d = sources_fingerprint(&DEFAULT_PRECEDENCE, true, &[("/b/a.nfo".into(), 3, 7)]);
        assert_ne!(a, d, "the parse-subtitles setting re-merges a book");
    }

    /// (title, subtitle, sequence, year, narrators, asin)
    type Parsed<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<i32>,
        &'a [&'a str],
        Option<&'a str>,
    );

    fn parsed(name: &str, has_series: bool, subtitles: bool) -> TitleFolder {
        parse_title_folder(name, has_series, subtitles)
    }

    fn expect(t: &TitleFolder, want: Parsed<'_>, name: &str) {
        let got: Parsed<'_> = (
            t.title.as_str(),
            t.subtitle.as_deref(),
            t.sequence.as_deref(),
            t.year,
            &[],
            t.asin.as_deref(),
        );
        assert_eq!(
            (got.0, got.1, got.2, got.3, got.5),
            (want.0, want.1, want.2, want.3, want.5),
            "{name}"
        );
        assert_eq!(t.narrators, want.4, "{name}");
    }

    #[test]
    fn title_folders_are_parsed_like_audiobookshelf() {
        // (folder name, has a series folder, parse subtitles, expected)
        let cases: &[(&str, bool, bool, Parsed<'_>)] = &[
            ("Title", true, true, ("Title", None, None, None, &[], None)),
            (
                "Title [B0015T963C]",
                true,
                true,
                ("Title", None, None, None, &[], Some("B0015T963C")),
            ),
            (
                "[B0015T963C] Title",
                true,
                true,
                ("Title", None, None, None, &[], Some("B0015T963C")),
            ),
            (
                "Title {Jim Dale}",
                true,
                true,
                ("Title", None, None, None, &["Jim Dale"], None),
            ),
            (
                "Title {Ann Reader} [B0015T963C]",
                true,
                true,
                (
                    "Title",
                    None,
                    None,
                    None,
                    &["Ann Reader"],
                    Some("B0015T963C"),
                ),
            ),
            // Lower case, or not ten characters: not an ASIN.
            (
                "Title [b0015t963c]",
                true,
                true,
                ("Title [b0015t963c]", None, None, None, &[], None),
            ),
            (
                "Title [B0015T963]",
                true,
                true,
                ("Title [B0015T963]", None, None, None, &[], None),
            ),
            (
                "Title[B0015T963C]",
                true,
                true,
                ("Title[B0015T963C]", None, None, None, &[], None),
            ),
            (
                "1994 - Title",
                false,
                true,
                ("Title", None, None, Some(1994), &[], None),
            ),
            (
                "(1994) - Title",
                false,
                true,
                ("Title", None, None, Some(1994), &[], None),
            ),
            (
                "1994-Title",
                false,
                true,
                ("Title", None, None, Some(1994), &[], None),
            ),
            (
                "Title (2001)",
                false,
                true,
                ("Title (2001)", None, None, None, &[], None),
            ),
            (
                "2001 A Space Odyssey",
                false,
                true,
                ("2001 A Space Odyssey", None, None, None, &[], None),
            ),
            (
                "Book 2 - Title",
                true,
                true,
                ("Title", None, Some("2"), None, &[], None),
            ),
            (
                "Vol. 3 Title Here - Subtitle",
                true,
                true,
                ("Title Here", Some("Subtitle"), Some("3"), None, &[], None),
            ),
            (
                "Title - Subtitle - Vol 12",
                true,
                true,
                ("Title", Some("Subtitle"), Some("12"), None, &[], None),
            ),
            (
                "Volume 4 - Title",
                true,
                false,
                ("Title", None, Some("4"), None, &[], None),
            ),
            (
                "1. Title",
                true,
                true,
                ("Title", None, Some("1"), None, &[], None),
            ),
            (
                "1 - Title",
                true,
                true,
                ("Title", None, Some("1"), None, &[], None),
            ),
            (
                "0.5 - Book Title",
                true,
                true,
                ("Book Title", None, Some("0.5"), None, &[], None),
            ),
            (
                "2.25. Title",
                true,
                true,
                ("Title", None, Some("2.25"), None, &[], None),
            ),
            (
                "101 Dalmatians",
                true,
                true,
                ("101 Dalmatians", None, None, None, &[], None),
            ),
            (
                "101. Dalmatians",
                true,
                true,
                ("Dalmatians", None, Some("101"), None, &[], None),
            ),
            ("1984", true, true, ("1984", None, None, None, &[], None)),
            ("1984", false, true, ("1984", None, None, None, &[], None)),
            (
                "Vol 1 - 1994 - Title",
                true,
                true,
                ("Title", None, Some("1"), Some(1994), &[], None),
            ),
            (
                "1994 - Vol 1 - Title",
                true,
                true,
                ("Title", None, Some("1"), Some(1994), &[], None),
            ),
            // No series folder: no sequence, so the rest is a subtitle.
            (
                "Vol 1 - 1994 - Title",
                false,
                true,
                ("Vol 1", Some("1994 - Title"), None, None, &[], None),
            ),
            (
                "Book Two - Title",
                true,
                true,
                ("Book Two", Some("Title"), None, None, &[], None),
            ),
            // Subtitles off (the default): ` - ` stays in the title.
            (
                "Title - Subtitle",
                true,
                false,
                ("Title - Subtitle", None, None, None, &[], None),
            ),
            (
                "Book Two - Title",
                true,
                false,
                ("Book Two - Title", None, None, None, &[], None),
            ),
            // Several narrators split as folder authors are.
            (
                "Title {Ann Reader & Bob Voice}",
                false,
                false,
                (
                    "Title",
                    None,
                    None,
                    None,
                    &["Ann Reader", "Bob Voice"],
                    None,
                ),
            ),
            // Everything at once.
            (
                "Vol 2 - 1999 - The Middle - A Subtitle {Ann Reader} [B000000002]",
                true,
                true,
                (
                    "The Middle",
                    Some("A Subtitle"),
                    Some("2"),
                    Some(1999),
                    &["Ann Reader"],
                    Some("B000000002"),
                ),
            ),
            // Nothing left for a title: the name stays the title.
            (
                "Book 3",
                true,
                false,
                ("Book 3", None, Some("3"), None, &[], None),
            ),
            (
                "[B0015T963C]",
                false,
                false,
                ("[B0015T963C]", None, None, None, &[], Some("B0015T963C")),
            ),
        ];
        for (name, has_series, subtitles, want) in cases {
            expect(&parsed(name, *has_series, *subtitles), *want, name);
        }
    }

    #[test]
    fn the_folder_source_reads_the_nearest_folders_and_the_title_folder() {
        // Item 4: the nearest author folder, not the top-level one.
        let f = folder_fields(
            "Fantasy/Jane Doe/The Long Road/Book 2 - Title",
            false,
            false,
        );
        assert_eq!(f.authors, list(&["Jane Doe"]));
        assert_eq!(f.series, series(&[("The Long Road", Some("2"))]));
        assert_eq!(f.title, s("Title"));
        let f = folder_fields("A/B/Jane Doe/The Long Road/1. Title", false, false);
        assert_eq!(f.authors, list(&["Jane Doe"]));
        assert_eq!(f.series, series(&[("The Long Road", Some("1"))]));
        // Three folders: ABS reads the middle one as the series, so in
        // `Genre/Author/Title` the genre is the author, as in ABS.
        let f = folder_fields("Fantasy/Jane Doe/Title", false, false);
        assert_eq!(f.authors, list(&["Fantasy"]));
        assert_eq!(f.series, series(&[("Jane Doe", None)]));
        // Two: the author; one: nothing above the title.
        let f = folder_fields(
            "Jane Doe/1994 - Title {Ann Reader} [B0015T963C]",
            false,
            false,
        );
        assert_eq!(f.authors, list(&["Jane Doe"]));
        assert_eq!(f.series, None);
        assert_eq!(f.title, s("Title"));
        assert_eq!(f.year, Some(1994));
        assert_eq!(f.narrators, list(&["Ann Reader"]));
        assert_eq!(f.asin, s("B0015T963C"));
        // No series folder: `Book 2 -` is not a sequence.
        assert_eq!(
            folder_fields("Jane Doe/Book 2 - Title", false, false).title,
            s("Book 2 - Title")
        );
        // The library's subtitle setting.
        let f = folder_fields("Jane Doe/Title - The Subtitle", false, true);
        assert_eq!((f.title, f.subtitle), (s("Title"), s("The Subtitle")));
        let f = folder_fields("Jane Doe/Title - The Subtitle", false, false);
        assert_eq!((f.title, f.subtitle), (s("Title - The Subtitle"), None));
        // A root file's name is not parsed.
        let f = folder_fields("1994 - Loose [B0015T963C].m4b", true, true);
        assert_eq!(f.title, s("1994 - Loose [B0015T963C]"));
        assert_eq!((f.year, f.asin), (None, None));
    }
}
