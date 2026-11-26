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
//! - `segments` — the cheap intro/credits pass over new or changed
//!   episodes: chapter names, then black-frame + silence at the
//!   head and tail (SEG2, D17–D19).
//! - `audiobooks` — audiobooks libraries (Phase 10, AB2): audio
//!   folders grouped into books, ordered tracks, tag metadata,
//!   chapters on the book's timeline, folder covers.
//! - `book_meta` / `book_sync` — AB6 (D30): a book's metadata merged
//!   from its local sources in the library's precedence order, the
//!   admin's manual match against Google Books / Open Library (per
//!   book, or a library-wide quick match that fills gaps), embedded
//!   covers, and the `book.parlour.json` sidecar.
//! - `fingerprint` — the audio-fingerprint pass in the same job:
//!   chromaprint over each episode's head and tail, cached by file
//!   hash, matched across the whole season (SEG3, D20).
//! - `sidecar` — per-item metadata stored with the media files
//!   (`*.parlour.json`, XFER4, D14): read by the scanner before any
//!   provider lookup and authoritative over it; rewritten by the
//!   metadata job where a library opted in.
//!
//! Matching files to TMDB/TVDB metadata is `metadata_sync`
//! (Milestone 1.4, providers in `parlour-metadata`); artwork
//! caching is `artwork`. The Emby importer (watched state matched
//! by ProviderIds, never titles) is Milestone 1.7.

pub mod artwork;
pub mod audiobooks;
pub mod book_meta;
pub mod book_sync;
pub mod emby;
pub mod fingerprint;
pub mod metadata_sync;
pub mod parse;
pub mod probe;
pub mod scan;
pub mod segments;
pub mod sidecar;
pub mod walk;

pub use artwork::{download_item_artwork, download_library_artwork, ArtworkReport};
pub use emby::{run_import, EmbyClient, EmbyError, EmbySettings, ImportReport};
pub use metadata_sync::{apply_manual, match_item, match_library, MatchError, MatchOutcome};
pub use parse::{parse_file_name, ParsedName};
pub use probe::{probe_file, ProbeResult};
pub use scan::{scan_library, scan_library_with, ScanOptions, ScanReport};
pub use segments::{detect_library, needs_detection, DetectError, DetectReport};
pub use sidecar::{copy_item_artwork, write_item_sidecars, ArtworkCopyReport, SidecarWriteReport};
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
