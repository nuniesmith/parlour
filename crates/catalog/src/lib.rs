//! Parlour catalog (placeholder).
//!
//! Responsibilities when implemented:
//! - Watch library folders (`notify`) plus targeted rescans from
//!   *arr webhooks, and a periodic full scan.
//! - Skip unchanged files by (path, size, mtime) fingerprint.
//! - Run ffprobe and store streams (incl. every subtitle stream)
//!   via `parlour-db`.
//! - Match items to metadata providers (`parlour-metadata`) using
//!   *arr/TRaSH naming conventions; TMDB/TVDB ids in folder names
//!   are a primary signal.
//! - One-off Emby importer for watched state / progress / favourites
//!   so the household keeps Continue Watching at cutover.

use parlour_core::LibraryKind;

/// A library: a named root folder of one kind, owned by the server,
/// with per-user/profile access controlled outside this crate.
#[derive(Debug, Clone)]
pub struct Library {
    pub name: String,
    pub kind: LibraryKind,
    pub root_path: String,
}
