//! Parlour catalog (Milestone 1.3).
//!
//! - `parse` — *arr/TRaSH file-name parsing: titles, years,
//!   `SxxExx` episodes, sample/extra rules (C3, C5). Pure.
//! - `walk` — the folder walker: video allowlist, sidecar
//!   subtitles, (path, size, mtime) fingerprints (C3). Pure I/O
//!   over the filesystem, no database.
//! - `probe` — ffprobe as a subprocess; the JSON parse is a pure
//!   function so it is fixture-tested (C4).
//! - `scan` — the scanner itself: turns a walk into catalog rows,
//!   idempotently, as a background job (C6), and marks vanished
//!   files unavailable instead of deleting anything (C8).
//!
//! Matching files to TMDB/TVDB metadata is Milestone 1.4
//! (`parlour-metadata`); the Emby importer (watched state matched
//! by ProviderIds, never titles) is Milestone 1.7.

pub mod parse;
pub mod probe;
pub mod scan;
pub mod walk;

pub use parse::{parse_file_name, ParsedName};
pub use probe::{probe_file, ProbeResult};
pub use scan::{scan_library, ScanReport};
pub use walk::{walk_library, DiscoveredVideo, SidecarSubtitle, WalkResult};

use parlour_core::LibraryKind;

/// A library: a named root folder of one kind, owned by the server,
/// with per-user/profile access controlled outside this crate.
#[derive(Debug, Clone)]
pub struct Library {
    pub name: String,
    pub kind: LibraryKind,
    pub root_path: String,
}

/// Map the database `libraries.kind` text to the core enum.
pub fn kind_from_str(kind: &str) -> Option<LibraryKind> {
    match kind {
        "movies" => Some(LibraryKind::Movies),
        "shows" => Some(LibraryKind::Shows),
        "audiobooks" => Some(LibraryKind::Audiobooks),
        "ebooks" => Some(LibraryKind::Ebooks),
        "music" => Some(LibraryKind::Music),
        "podcasts" => Some(LibraryKind::Podcasts),
        _ => None,
    }
}
