-- POD8 (Phase 11; D35): playlists, collections and bookmarks — the
-- parts of D35's "what syncs" that are neither progress nor sessions.
-- Spec: docs/AUDIOBOOKS-PODCASTS.md "Playlists, collections and
-- bookmarks (POD8)". The timestamp is only an ordering key (it sorts
-- after POD5's 20261204000000); written 2026-11-27.
--
-- playlists / playlist_items — per profile, per library, ordered (D35,
--   as Audiobookshelf keeps them per user per library). A playlist lives
--   in one audiobooks or podcasts library, so its entries are that
--   library's books (item_id) or episodes (episode_id): exactly one set,
--   never mixed in practice because a library holds one kind (the API
--   checks the library). An entry is deleted with its book or episode.
--   `position` orders the entries; gaps are allowed, and the unique key
--   is deferrable so a reorder rewrites every position in one statement.
--
-- collections / collection_items — per library, visible to every
--   profile with a grant on it (D35). Books only, in an audiobooks
--   library, as in Audiobookshelf; only admins change them (the API).
--
-- bookmarks — per profile, per book: a time and a title (D35; ABS has
--   no episode bookmarks, so there is no episode_id). As in ABS a book
--   has at most one bookmark at a given time.
--
-- None of the five tables is in export format v1 (coverage.rs: left
-- out; D36 and the format-v2 step decide per-profile audio state).

CREATE TABLE playlists (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    profile_id  uuid NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
    library_id  uuid NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    name        text NOT NULL CHECK (length(btrim(name)) > 0),
    description text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX playlists_profile_library_idx ON playlists (profile_id, library_id);

CREATE TABLE playlist_items (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    playlist_id uuid NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
    item_id     uuid REFERENCES media_items (id) ON DELETE CASCADE,
    episode_id  uuid REFERENCES podcast_episodes (id) ON DELETE CASCADE,
    position    integer NOT NULL CHECK (position >= 0),
    added_at    timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT playlist_items_one_target_check CHECK (num_nonnulls(item_id, episode_id) = 1),
    UNIQUE (playlist_id, item_id),
    UNIQUE (playlist_id, episode_id),
    CONSTRAINT playlist_items_position_key UNIQUE (playlist_id, position) DEFERRABLE
);
CREATE INDEX playlist_items_item_idx ON playlist_items (item_id) WHERE item_id IS NOT NULL;
CREATE INDEX playlist_items_episode_idx ON playlist_items (episode_id) WHERE episode_id IS NOT NULL;

CREATE TABLE collections (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    library_id  uuid NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    name        text NOT NULL CHECK (length(btrim(name)) > 0),
    description text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX collections_library_idx ON collections (library_id);

CREATE TABLE collection_items (
    collection_id uuid NOT NULL REFERENCES collections (id) ON DELETE CASCADE,
    item_id       uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    position      integer NOT NULL CHECK (position >= 0),
    added_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (collection_id, item_id),
    CONSTRAINT collection_items_position_key UNIQUE (collection_id, position) DEFERRABLE
);
CREATE INDEX collection_items_item_idx ON collection_items (item_id);

CREATE TABLE bookmarks (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    profile_id  uuid NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
    item_id     uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    position_ms bigint NOT NULL CHECK (position_ms >= 0),
    title       text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (profile_id, item_id, position_ms)
);
