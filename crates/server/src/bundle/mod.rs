//! The export bundle format, `parlour-export` version 1 (Phase 8,
//! XFER1; D14, D25). Spec: `docs/IMPORT-EXPORT.md`.
//!
//! A bundle is a plain zip (no encryption — D25) holding six JSON
//! documents and, optionally, artwork files:
//!
//! | entry            | type             | holds                                             |
//! |------------------|------------------|---------------------------------------------------|
//! | `manifest.json`  | [`Manifest`]     | format + server versions, export time, options    |
//! | `users.json`     | [`UsersFile`]    | logins; password + PIN hashes only when confirmed |
//! | `profiles.json`  | [`ProfilesFile`] | profiles, preferences, recent searches (D25)      |
//! | `libraries.json` | [`LibrariesFile`]| definitions, paths, access grants by name         |
//! | `catalog.json`   | [`CatalogFile`]  | items by library + stable_key, people, segments   |
//! | `user_data.json` | [`UserDataFile`] | watch state per profile, the Activity feed (D25)  |
//! | `artwork/…`      | image bytes      | only when the export opted in (XFER2)             |
//!
//! Nothing in a bundle is a database id: users are their usernames,
//! profiles are (username, profile name), libraries their names and
//! items (library name, `stable_key`). Server secrets (API keys, the
//! stream secret, the database URL) live only in the environment and
//! no type here has a field that could hold one.
//!
//! This module is the format only: the types, the zip layout
//! ([`BundleWriter`], [`BundleReader`]) and the consistency checks
//! ([`Bundle::problems`]). Reading the database into a bundle is
//! XFER2's job and applying one is XFER3's.

mod archive;
mod check;
pub mod coverage;

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

pub use archive::{
    artwork_file_name, parse_artwork_name, BundleReader, BundleWriter, MAX_DOCUMENT_BYTES,
};
pub use check::Problem;

/// `manifest.json`'s `format`: says "this zip is a Parlour bundle".
pub const FORMAT_NAME: &str = "parlour-export";
/// The one format version this server reads and writes.
pub const FORMAT_VERSION: u32 = 1;

pub const MANIFEST_JSON: &str = "manifest.json";
pub const USERS_JSON: &str = "users.json";
pub const PROFILES_JSON: &str = "profiles.json";
pub const LIBRARIES_JSON: &str = "libraries.json";
pub const CATALOG_JSON: &str = "catalog.json";
pub const USER_DATA_JSON: &str = "user_data.json";
/// Folder for artwork files: `artwork/<sha256 hex>.<ext>`.
pub const ARTWORK_DIR: &str = "artwork/";

/// The six documents, in the order a writer stores them.
pub const DOCUMENTS: [&str; 6] = [
    MANIFEST_JSON,
    USERS_JSON,
    PROFILES_JSON,
    LIBRARIES_JSON,
    CATALOG_JSON,
    USER_DATA_JSON,
];

/// Closed value sets, carried as their database / wire names.
macro_rules! wire_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The database / wire name.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $wire),+
                }
            }

            /// The value for a database / wire name.
            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($wire => Some($name::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

wire_enum!(
    /// `libraries.kind` (its CHECK constraint).
    LibraryKind {
        Movies => "movies",
        Shows => "shows",
        Audiobooks => "audiobooks",
        Ebooks => "ebooks",
        Music => "music",
        Podcasts => "podcasts",
    }
);

wire_enum!(
    /// `media_items.kind` (its CHECK constraint).
    ItemKind {
        Movie => "movie",
        Series => "series",
        Season => "season",
        Episode => "episode",
        Audiobook => "audiobook",
    }
);

wire_enum!(
    /// `media_items.metadata_status` (its CHECK constraint).
    MetadataStatus {
        Unmatched => "unmatched",
        Matched => "matched",
        NeedsReview => "needs_review",
        Failed => "failed",
    }
);

wire_enum!(
    /// `media_segments.kind` (its CHECK constraint; `parlour_core::SegmentKind`).
    SegmentKind {
        Intro => "intro",
        Credits => "credits",
        Recap => "recap",
    }
);

wire_enum!(
    /// `media_segments.source` (its CHECK constraint; `parlour_core::SegmentSource`).
    SegmentSource {
        Chapters => "chapters",
        Fingerprint => "fingerprint",
        Manual => "manual",
        Heuristic => "heuristic",
    }
);

wire_enum!(
    /// `images.kind` (its CHECK constraint).
    ImageKind {
        Poster => "poster",
        Backdrop => "backdrop",
        Logo => "logo",
        Still => "still",
    }
);

wire_enum!(
    /// `profiles.subtitle_mode` (`profiles::SUBTITLE_MODES`).
    SubtitleMode {
        Auto => "auto",
        Always => "always",
        Never => "never",
    }
);

wire_enum!(
    /// `profiles.segment_skip` (`profiles::SEGMENT_SKIP_MODES`).
    SegmentSkip {
        Button => "button",
        Auto => "auto",
        Off => "off",
    }
);

impl From<SegmentKind> for parlour_core::SegmentKind {
    fn from(kind: SegmentKind) -> Self {
        match kind {
            SegmentKind::Intro => parlour_core::SegmentKind::Intro,
            SegmentKind::Credits => parlour_core::SegmentKind::Credits,
            SegmentKind::Recap => parlour_core::SegmentKind::Recap,
        }
    }
}

impl From<parlour_core::SegmentKind> for SegmentKind {
    fn from(kind: parlour_core::SegmentKind) -> Self {
        match kind {
            parlour_core::SegmentKind::Intro => SegmentKind::Intro,
            parlour_core::SegmentKind::Credits => SegmentKind::Credits,
            parlour_core::SegmentKind::Recap => SegmentKind::Recap,
        }
    }
}

impl From<SegmentSource> for parlour_core::SegmentSource {
    fn from(source: SegmentSource) -> Self {
        match source {
            SegmentSource::Chapters => parlour_core::SegmentSource::Chapters,
            SegmentSource::Fingerprint => parlour_core::SegmentSource::Fingerprint,
            SegmentSource::Manual => parlour_core::SegmentSource::Manual,
            SegmentSource::Heuristic => parlour_core::SegmentSource::Heuristic,
        }
    }
}

impl From<parlour_core::SegmentSource> for SegmentSource {
    fn from(source: parlour_core::SegmentSource) -> Self {
        match source {
            parlour_core::SegmentSource::Chapters => SegmentSource::Chapters,
            parlour_core::SegmentSource::Fingerprint => SegmentSource::Fingerprint,
            parlour_core::SegmentSource::Manual => SegmentSource::Manual,
            parlour_core::SegmentSource::Heuristic => SegmentSource::Heuristic,
        }
    }
}

// ------------------------------------------------------------ references

/// A profile, by name: `(username, profile name)` is unique.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRef {
    pub username: String,
    pub profile: String,
}

/// An item, by name: `(library name, stable_key)` is unique.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRef {
    pub library: String,
    pub stable_key: String,
}

// ------------------------------------------------------------ manifest.json

/// `manifest.json`: what the bundle is and how it was made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Always [`FORMAT_NAME`].
    pub format: String,
    /// Always [`FORMAT_VERSION`] for this server.
    pub format_version: u32,
    /// The exporting server's version (`/api/v1/version`).
    pub server_version: String,
    pub exported_at: DateTime<Utc>,
    /// Whether password and PIN hashes are in `users.json`. Only an
    /// export the admin explicitly confirmed, past a warning, sets
    /// this (D14); D25 lets them sit in the plain zip.
    pub hashes_included: bool,
    /// Whether artwork files are under `artwork/` (XFER2's opt-in).
    pub artwork_included: bool,
}

impl Manifest {
    /// The manifest of an export this server makes: this format and
    /// version, and this server's version.
    pub fn new(exported_at: DateTime<Utc>, hashes_included: bool, artwork_included: bool) -> Self {
        Self {
            format: FORMAT_NAME.to_string(),
            format_version: FORMAT_VERSION,
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            exported_at,
            hashes_included,
            artwork_included,
        }
    }
}

// ------------------------------------------------------------ users.json

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsersFile {
    pub users: Vec<User>,
}

/// One login (`users`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    /// Unique ignoring case, like the database's `lower(username)` index.
    pub username: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
    /// The Argon2id PHC string exactly as stored. Present only when
    /// [`Manifest::hashes_included`]; absent (not null) otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// PIN hashes of this user's profiles that have a PIN, by profile
    /// name (the TODO puts PIN hashes here, so every hash in the
    /// bundle sits in this one file). Present only when
    /// [`Manifest::hashes_included`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_hashes: Option<BTreeMap<String, String>>,
}

// ------------------------------------------------------------ profiles.json

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilesFile {
    pub profiles: Vec<Profile>,
}

/// One profile (`profiles`), with its preferences and, per D25, its
/// recent searches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub username: String,
    /// Unique per user.
    pub name: String,
    /// Orders Who's watching.
    pub created_at: DateTime<Utc>,
    /// Whether the profile has a PIN. Always carried: the PIN hash
    /// itself is in `users.json`, and only behind the confirm.
    pub has_pin: bool,
    pub avatar_color: Option<String>,
    pub preferences: Preferences,
    /// Newest first, as `GET /api/v1/search/recent` lists them.
    pub recent_searches: Vec<RecentSearch>,
}

/// A profile's settings (A5 defaults + the UI7 Preferences pages).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub audio_lang: Option<String>,
    pub subtitle_lang: Option<String>,
    pub subtitle_mode: SubtitleMode,
    pub remote_bitrate_cap_kbps: Option<i32>,
    /// Rail ids as stored: the profile's chosen order (may be partial).
    pub home_rail_order: Vec<String>,
    /// Rail ids as stored: the rails the profile turned off.
    pub home_rails_hidden: Vec<String>,
    pub segment_skip: SegmentSkip,
    pub autoplay_next: bool,
}

/// One recent search (`recent_searches`, UI9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentSearch {
    /// Unique per profile ignoring case.
    pub query: String,
    pub searched_at: DateTime<Utc>,
}

// ------------------------------------------------------------ libraries.json

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibrariesFile {
    pub libraries: Vec<Library>,
}

/// One library (`libraries` + `library_paths` + `library_access`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    /// Unique.
    pub name: String,
    pub kind: LibraryKind,
    pub created_at: DateTime<Utc>,
    /// Root folders as the exporting host saw them.
    pub paths: Vec<String>,
    /// Which profiles may see the library, and up to what rating.
    pub access: Vec<AccessGrant>,
}

/// One `library_access` row. Grants are per profile, so a grant
/// names the profile's user and the profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessGrant {
    pub username: String,
    pub profile: String,
    pub max_rating: Option<String>,
}

// ------------------------------------------------------------ catalog.json

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFile {
    pub items: Vec<Item>,
}

/// One catalog item (`media_items`) with its people, segments and
/// provider artwork.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub library: String,
    /// The scanner's identity for the item inside its library.
    pub stable_key: String,
    /// The parent's `stable_key`, in the same library.
    pub parent: Option<String>,
    pub kind: ItemKind,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i32>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub air_date: Option<NaiveDate>,
    pub overview: Option<String>,
    pub genres: Vec<String>,
    pub runtime_minutes: Option<i32>,
    pub content_rating: Option<String>,
    pub vote_average: Option<f64>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub metadata_status: MetadataStatus,
    pub metadata_provider: Option<String>,
    pub last_metadata_at: Option<DateTime<Utc>>,
    /// Orders Recently Added and Latest.
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub people: Vec<Credit>,
    pub segments: Vec<Segment>,
    pub images: Vec<Image>,
}

/// One person on an item (`item_people` + `people`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credit {
    /// People are unique by name server-wide.
    pub name: String,
    pub tmdb_id: Option<i64>,
    /// `actor`, `author`, `narrator`, …
    pub role: String,
    pub character_name: Option<String>,
    pub sort_order: i32,
}

/// One `media_segments` row, whatever its source (SEG1–SEG4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub kind: SegmentKind,
    pub source: SegmentSource,
    /// Milliseconds into the item's file; both null on a manual
    /// "no segment of this kind here" marker (D21).
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One provider artwork reference (an `images` row with a
/// `source_url`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub kind: ImageKind,
    /// `original`, `w300`, …
    pub variant: String,
    pub source_url: String,
    /// `artwork/<sha256>.<ext>` when the export included artwork and
    /// this image was cached; null otherwise.
    pub file: Option<String>,
}

// ------------------------------------------------------------ user_data.json

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserDataFile {
    /// Watch state, one group per profile that has any.
    pub profiles: Vec<ProfileUserData>,
    /// The Activity feed (D25), oldest first.
    pub activity: Vec<ActivityEntry>,
}

/// One profile's watch state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileUserData {
    pub username: String,
    pub profile: String,
    pub items: Vec<ItemUserData>,
}

/// One `user_data` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemUserData {
    pub item: ItemRef,
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub played: bool,
    pub is_favorite: bool,
    pub play_count: i32,
    pub last_played_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Continue Watching's tie-break after `last_played_at`.
    pub updated_at: DateTime<Utc>,
}

/// One `activity` row as the server holds it (D25), with names in
/// place of its user / profile / item ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityEntry {
    pub created_at: DateTime<Utc>,
    /// `auth.login`, `playback.start`, `scan.completed`, …
    pub kind: String,
    /// The user's current name; null when the row has no user.
    pub username: Option<String>,
    pub profile: Option<ProfileRef>,
    pub item: Option<ItemRef>,
    /// The username at the time of the event, as written then.
    pub actor: Option<String>,
    pub summary: String,
    /// Verbatim, including whatever it holds (a sign-in's client IP,
    /// the old server's job / library / invite / session ids).
    pub detail: serde_json::Value,
}

// ------------------------------------------------------------ the bundle

/// The six documents of one bundle.
#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    pub manifest: Manifest,
    pub users: UsersFile,
    pub profiles: ProfilesFile,
    pub libraries: LibrariesFile,
    pub catalog: CatalogFile,
    pub user_data: UserDataFile,
}

impl Bundle {
    /// Everything that makes this bundle inconsistent — what the
    /// database would refuse (duplicate keys, broken CHECKs) and
    /// references between documents that lead nowhere. Empty means
    /// consistent. [`BundleWriter`] and [`BundleReader`] refuse a
    /// bundle with problems.
    pub fn problems(&self) -> Vec<Problem> {
        check::problems(self)
    }

    /// The artwork files the catalog refers to.
    pub fn artwork_files(&self) -> BTreeSet<&str> {
        self.catalog
            .items
            .iter()
            .flat_map(|item| item.images.iter())
            .filter_map(|image| image.file.as_deref())
            .collect()
    }
}

/// Why a bundle could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("not a Parlour export bundle: {0}")]
    NotABundle(String),
    #[error(
        "the bundle is format version {0}; this server reads version {supported}",
        supported = FORMAT_VERSION
    )]
    UnsupportedVersion(u32),
    #[error("the bundle has no {0}")]
    MissingEntry(&'static str),
    #[error("the bundle holds a file it shouldn't: {0}")]
    UnexpectedEntry(String),
    #[error("{file} could not be read: {source}")]
    Json {
        file: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("{file} is larger than this server reads ({limit} bytes unpacked)")]
    TooLarge { file: &'static str, limit: u64 },
    #[error("the bundle is inconsistent: {}", list(.0))]
    Invalid(Vec<Problem>),
    #[error("artwork {name}: {reason}")]
    Artwork { name: String, reason: String },
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
}

fn list(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(Problem::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests;
