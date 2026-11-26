//! Where every column of the current schema goes in a bundle: the
//! machine-checked twin of the tables in `docs/IMPORT-EXPORT.md`
//! ("What the bundle carries" / "What stays behind").
//!
//! The ignored flow test `tests/bundle_schema_flow.rs` compares
//! [`SCHEMA`] with a freshly migrated database, so a migration that
//! adds a table or a column fails it until this list — and the doc —
//! say where the new thing goes. XFER2's export reads exactly the
//! [`Column::Value`] columns, and resolves the [`Column::Reference`]
//! ones to names.

/// What the bundle does with one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// Carried as it is, at this place in the bundle.
    Value(&'static str),
    /// A database id: the bundle names the row instead, like this.
    Reference(&'static str),
    /// Not carried, and why.
    Left(&'static str),
}

/// What the bundle does with one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    /// Carried; every column is accounted for.
    Carried {
        name: &'static str,
        columns: &'static [(&'static str, Column)],
    },
    /// Not carried at all, and why.
    LeftOut {
        name: &'static str,
        why: &'static str,
    },
    /// Not in format v1 — an open proposal waiting for the owner's
    /// answer (`docs/IMPORT-EXPORT.md`, "Proposed").
    Proposed {
        name: &'static str,
        proposal: &'static str,
    },
}

impl Table {
    pub fn name(&self) -> &'static str {
        match self {
            Table::Carried { name, .. }
            | Table::LeftOut { name, .. }
            | Table::Proposed { name, .. } => name,
        }
    }
}

use Column::{Left, Reference, Value};

const REBUILT_BY_SCAN: &str = "rebuilt by the new host's scan from the media files themselves";

/// Every table of the current schema (`migrations/`), in migration
/// order.
pub const SCHEMA: &[Table] = &[
    Table::Carried {
        name: "users",
        columns: &[
            ("id", Reference("a user is its username everywhere")),
            ("username", Value("users.json users[].username")),
            (
                "password_hash",
                Value("users.json users[].password_hash — only behind the admin's confirm"),
            ),
            ("is_admin", Value("users.json users[].is_admin")),
            ("created_at", Value("users.json users[].created_at")),
            ("disabled_at", Value("users.json users[].disabled_at")),
        ],
    },
    Table::Carried {
        name: "profiles",
        columns: &[
            (
                "id",
                Reference("a profile is (username, profile name) everywhere"),
            ),
            ("user_id", Reference("profiles.json profiles[].username")),
            ("name", Value("profiles.json profiles[].name")),
            (
                "pin_hash",
                Value("users.json users[].pin_hashes — only behind the admin's confirm; profiles[].has_pin always"),
            ),
            ("avatar_color", Value("profiles.json profiles[].avatar_color")),
            (
                "audio_lang",
                Value("profiles.json profiles[].preferences.audio_lang"),
            ),
            (
                "subtitle_lang",
                Value("profiles.json profiles[].preferences.subtitle_lang"),
            ),
            (
                "subtitle_mode",
                Value("profiles.json profiles[].preferences.subtitle_mode"),
            ),
            (
                "remote_bitrate_cap_kbps",
                Value("profiles.json profiles[].preferences.remote_bitrate_cap_kbps"),
            ),
            ("created_at", Value("profiles.json profiles[].created_at")),
            (
                "home_rail_order",
                Value("profiles.json profiles[].preferences.home_rail_order"),
            ),
            (
                "home_rails_hidden",
                Value("profiles.json profiles[].preferences.home_rails_hidden"),
            ),
            (
                "segment_skip",
                Value("profiles.json profiles[].preferences.segment_skip"),
            ),
            (
                "autoplay_next",
                Value("profiles.json profiles[].preferences.autoplay_next"),
            ),
        ],
    },
    Table::LeftOut {
        name: "sessions",
        why: "live sign-ins on the old host (token hashes, IPs); everyone signs in again",
    },
    Table::Carried {
        name: "libraries",
        columns: &[
            ("id", Reference("a library is its name everywhere")),
            ("name", Value("libraries.json libraries[].name")),
            ("kind", Value("libraries.json libraries[].kind")),
            ("created_at", Value("libraries.json libraries[].created_at")),
            (
                "write_sidecars",
                Left(
                    "the sidecar-writing opt-in (XFER4/XFER5) depends on the host's mounts; \
                     an imported library starts with it off",
                ),
            ),
            (
                "metadata_precedence",
                Left(
                    "an audiobooks library's merge order for its books' local metadata \
                     sources (AB6); not in format v1 — an imported library starts with \
                     D30's default order",
                ),
            ),
            (
                "parse_subtitles",
                Left(
                    "an audiobooks library's parse-subtitles setting (AB11); not in format \
                     v1 — an imported library starts with it off, ABS's default",
                ),
            ),
            (
                "finish_remaining_seconds",
                Left(
                    "an audiobooks library's auto-finish time rule (AB11); not in format v1 \
                     — an imported library starts with ABS's 10 s",
                ),
            ),
            (
                "finish_percent",
                Left(
                    "an audiobooks library's auto-finish share (AB11); not in format v1 — \
                     an imported library starts with it off, ABS's default",
                ),
            ),
        ],
    },
    Table::Carried {
        name: "library_paths",
        columns: &[
            ("id", Left("database id; a path is its library + path")),
            (
                "library_id",
                Reference("libraries.json libraries[] — the library it sits under"),
            ),
            ("path", Value("libraries.json libraries[].paths[]")),
        ],
    },
    Table::Carried {
        name: "library_access",
        columns: &[
            (
                "profile_id",
                Reference("libraries.json libraries[].access[].username + .profile"),
            ),
            (
                "library_id",
                Reference("libraries.json libraries[] — the library it sits under"),
            ),
            (
                "max_rating",
                Value("libraries.json libraries[].access[].max_rating"),
            ),
        ],
    },
    Table::LeftOut {
        name: "invites",
        why: "pending invite links point at the old host; used invites are already users",
    },
    Table::LeftOut {
        name: "jobs",
        why: "the old host's queue and job history",
    },
    Table::Carried {
        name: "media_items",
        columns: &[
            (
                "id",
                Reference("an item is (library name, stable_key) everywhere"),
            ),
            ("library_id", Reference("catalog.json items[].library")),
            (
                "parent_id",
                Reference("catalog.json items[].parent — the parent's stable_key"),
            ),
            ("kind", Value("catalog.json items[].kind")),
            ("stable_key", Value("catalog.json items[].stable_key")),
            ("title", Value("catalog.json items[].title")),
            ("sort_title", Value("catalog.json items[].sort_title")),
            ("year", Value("catalog.json items[].year")),
            ("season_number", Value("catalog.json items[].season_number")),
            ("episode_number", Value("catalog.json items[].episode_number")),
            ("overview", Value("catalog.json items[].overview")),
            ("content_rating", Value("catalog.json items[].content_rating")),
            ("tmdb_id", Value("catalog.json items[].tmdb_id")),
            ("tvdb_id", Value("catalog.json items[].tvdb_id")),
            ("imdb_id", Value("catalog.json items[].imdb_id")),
            (
                "available",
                Left("file state, not metadata: the new host's scan decides it (C8)"),
            ),
            ("created_at", Value("catalog.json items[].created_at")),
            ("updated_at", Value("catalog.json items[].updated_at")),
            ("genres", Value("catalog.json items[].genres")),
            ("vote_average", Value("catalog.json items[].vote_average")),
            ("runtime_minutes", Value("catalog.json items[].runtime_minutes")),
            ("air_date", Value("catalog.json items[].air_date")),
            ("metadata_status", Value("catalog.json items[].metadata_status")),
            (
                "metadata_provider",
                Value("catalog.json items[].metadata_provider"),
            ),
            (
                "last_metadata_at",
                Value("catalog.json items[].last_metadata_at"),
            ),
        ],
    },
    Table::LeftOut {
        name: "media_files",
        why: REBUILT_BY_SCAN,
    },
    Table::LeftOut {
        name: "media_streams",
        why: REBUILT_BY_SCAN,
    },
    Table::LeftOut {
        name: "subtitle_tracks",
        why: REBUILT_BY_SCAN,
    },
    Table::Carried {
        name: "people",
        columns: &[
            ("id", Left("database id; people are unique by name")),
            ("name", Value("catalog.json items[].people[].name")),
            ("tmdb_id", Value("catalog.json items[].people[].tmdb_id")),
        ],
    },
    Table::Carried {
        name: "item_people",
        columns: &[
            (
                "item_id",
                Reference("catalog.json items[] — the item it sits under"),
            ),
            ("person_id", Reference("catalog.json items[].people[].name")),
            ("role", Value("catalog.json items[].people[].role")),
            (
                "character_name",
                Value("catalog.json items[].people[].character_name"),
            ),
            ("sort_order", Value("catalog.json items[].people[].sort_order")),
        ],
    },
    Table::Carried {
        name: "images",
        columns: &[
            ("id", Left("database id")),
            (
                "item_id",
                Reference("catalog.json items[] — the item it sits under"),
            ),
            ("kind", Value("catalog.json items[].images[].kind")),
            (
                "path",
                Left("a path in the old host's artwork cache; with artwork included, images[].file names the bytes in the zip"),
            ),
            (
                "hash",
                Value("catalog.json items[].images[].file (artwork/<hash>.<ext>) — only when artwork is included"),
            ),
            ("width", Left("never written by the current code")),
            ("height", Left("never written by the current code")),
            ("variant", Value("catalog.json items[].images[].variant")),
            (
                "source_url",
                Value("catalog.json items[].images[].source_url — rows without one (generated tiles, audiobook folder covers) are rebuilt by the new host and stay behind"),
            ),
        ],
    },
    Table::LeftOut {
        name: "metadata_candidates",
        why: "the matcher's review shortlist; the next metadata match on the new host rebuilds it",
    },
    Table::Carried {
        name: "user_data",
        columns: &[
            (
                "profile_id",
                Reference("user_data.json profiles[].username + .profile"),
            ),
            ("item_id", Reference("user_data.json profiles[].items[].item")),
            (
                "position_ms",
                Value("user_data.json profiles[].items[].position_ms"),
            ),
            (
                "duration_ms",
                Value("user_data.json profiles[].items[].duration_ms"),
            ),
            ("played", Value("user_data.json profiles[].items[].played")),
            (
                "is_favorite",
                Value("user_data.json profiles[].items[].is_favorite"),
            ),
            (
                "last_played_at",
                Value("user_data.json profiles[].items[].last_played_at"),
            ),
            (
                "created_at",
                Value("user_data.json profiles[].items[].created_at"),
            ),
            (
                "updated_at",
                Value("user_data.json profiles[].items[].updated_at"),
            ),
            (
                "play_count",
                Value("user_data.json profiles[].items[].play_count"),
            ),
        ],
    },
    Table::Carried {
        name: "activity",
        columns: &[
            ("id", Left("database id")),
            ("created_at", Value("user_data.json activity[].created_at")),
            ("kind", Value("user_data.json activity[].kind")),
            ("user_id", Reference("user_data.json activity[].username")),
            ("profile_id", Reference("user_data.json activity[].profile")),
            ("item_id", Reference("user_data.json activity[].item")),
            ("actor", Value("user_data.json activity[].actor")),
            ("summary", Value("user_data.json activity[].summary")),
            (
                "detail",
                Value("user_data.json activity[].detail — verbatim (D25)"),
            ),
        ],
    },
    Table::LeftOut {
        name: "notices",
        why: "reconciled from live conditions once a minute on the new host",
    },
    Table::LeftOut {
        name: "notice_dismissals",
        why: "belongs to the notices, which stay behind",
    },
    Table::Carried {
        name: "recent_searches",
        columns: &[
            ("id", Left("database id; a search is its profile + query")),
            (
                "profile_id",
                Reference("profiles.json profiles[] — the profile it sits under"),
            ),
            (
                "query",
                Value("profiles.json profiles[].recent_searches[].query"),
            ),
            (
                "searched_at",
                Value("profiles.json profiles[].recent_searches[].searched_at"),
            ),
        ],
    },
    Table::Carried {
        name: "media_segments",
        columns: &[
            (
                "id",
                Left("database id; a segment is its item + kind + source"),
            ),
            (
                "item_id",
                Reference("catalog.json items[] — the item it sits under"),
            ),
            ("kind", Value("catalog.json items[].segments[].kind")),
            ("start_ms", Value("catalog.json items[].segments[].start_ms")),
            ("end_ms", Value("catalog.json items[].segments[].end_ms")),
            ("source", Value("catalog.json items[].segments[].source")),
            (
                "confidence",
                Value("catalog.json items[].segments[].confidence"),
            ),
            (
                "created_at",
                Value("catalog.json items[].segments[].created_at"),
            ),
            (
                "updated_at",
                Value("catalog.json items[].segments[].updated_at"),
            ),
        ],
    },
    Table::LeftOut {
        name: "audio_fingerprints",
        why: "a cache keyed by file content; segment detection on the new host rebuilds it",
    },
    Table::Proposed {
        name: "book_chapters",
        proposal: "not carried — rebuilt by every scan of the book; needs the owner's yes",
    },
    Table::Proposed {
        name: "book_progress",
        proposal: "carried under user_data.json profiles[].books[] like watch state; needs the \
                   owner's yes (D36). AB11's columns (hidden, client_updated_at) are part of \
                   the same open question and are not carried either. POD1's podcast-episode \
                   rows (episode_id set, item_id NULL) share the table (D29) and are not \
                   carried: the episodes they point at stay behind (podcast_episodes)",
    },
    Table::LeftOut {
        name: "sidecar_artwork",
        why: "which poster.jpg / backdrop.jpg files this host wrote beside its own media \
              (XFER5); the new host's media mounts are its own",
    },
    Table::LeftOut {
        name: "item_sidecars",
        why: "what the scanner last read from the sidecar files (XFER4); the new host reads \
              the files themselves",
    },
    Table::LeftOut {
        name: "item_nfos",
        why: "what the scanner last read from the .nfo files (XFER6); the new host reads the \
              files themselves",
    },
    Table::LeftOut {
        name: "sidecar_nfos",
        why: "which .nfo files this host wrote beside its own media (XFER6); the new host's \
              media mounts are its own",
    },
    Table::LeftOut {
        name: "backup_settings",
        why: "this host's nightly backup folder, count and hash choice (D39, XFER8); a \
              server setting, not state, and the new host's folders are its own",
    },
    Table::LeftOut {
        name: "book_metadata",
        why: "a book's merged book-only fields and its sources fingerprint (AB6); the new \
              host's scan rebuilds them from the book's files and its match",
    },
    Table::Proposed {
        name: "book_sidecars",
        proposal: "a book's match — the server's copy of its book.parlour.json (AB6); \
                   carrying it in catalog.json needs the owner's yes; until then a match \
                   travels only as the book.parlour.json written beside the book",
    },
    Table::LeftOut {
        name: "series",
        why: "audiobook series (AB7); the new host's scan rebuilds them from each book's \
              series folder and tags",
    },
    Table::LeftOut {
        name: "book_series",
        why: "a book's place in its series (AB7); the new host's scan rebuilds it from the \
              book's series folder and tags",
    },
    Table::Proposed {
        name: "listening_sessions",
        proposal: "a profile's listening sessions — its listening history and stats (AB8, \
                   D29/D35); per-profile state like book_progress, so whether and how they \
                   travel is the same open question (D36) and a format change; not carried. \
                   A closed session's Activity line travels with the Activity feed (D25)",
    },
    Table::LeftOut {
        name: "podcasts",
        why: "a podcasts library's shows and their per-show settings (POD1, D31/D32); not \
              in format v1 — the shows' place in the bundle is later work (a format \
              change), so until then an admin adds them again on the new host",
    },
    Table::LeftOut {
        name: "podcast_episodes",
        why: "a show's episodes as its feed lists them, and where this host downloaded \
              them (POD1, D32); the new host's feed checks rebuild the list, and the \
              downloaded files live in this host's own podcast folder",
    },
    Table::LeftOut {
        name: "_sqlx_migrations",
        why: "the new host keeps its own migration ledger",
    },
];

/// The closed value sets the format mirrors from CHECK constraints:
/// (table, constraint, the format's values). The flow test reads each
/// constraint's definition and compares.
pub fn check_constraints() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    use super::{ImageKind, ItemKind, LibraryKind, MetadataStatus, SegmentKind, SegmentSource};
    vec![
        (
            "libraries",
            "libraries_kind_check",
            LibraryKind::ALL.iter().map(|v| v.as_str()).collect(),
        ),
        (
            "media_items",
            "media_items_kind_check",
            ItemKind::ALL.iter().map(|v| v.as_str()).collect(),
        ),
        (
            "media_items",
            "media_items_metadata_status_check",
            MetadataStatus::ALL.iter().map(|v| v.as_str()).collect(),
        ),
        (
            "media_segments",
            "media_segments_kind_check",
            SegmentKind::ALL.iter().map(|v| v.as_str()).collect(),
        ),
        (
            "media_segments",
            "media_segments_source_check",
            SegmentSource::ALL.iter().map(|v| v.as_str()).collect(),
        ),
        (
            "images",
            "images_kind_check",
            ImageKind::ALL.iter().map(|v| v.as_str()).collect(),
        ),
    ]
}
