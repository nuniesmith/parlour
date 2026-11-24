-- Native audiobooks (Phase 10, AB1; docs/AUDIOBOOKS-PODCASTS.md).
--
-- A book is a top-level media_items row of kind 'audiobook' in an
-- audiobooks library — the shared catalog (ARCHITECTURE §7/§10),
-- not a second one. Its tracks are its media_files, in
-- `track_index` order (NULL for every non-audiobook file).
--
-- book_chapters — the book's chapters on the book-global timeline
--                 (ms from the start of track 0): a track's
--                 embedded chapters shifted by its start offset,
--                 or one chapter per track. Rebuilt by the scanner
--                 whenever a book's tracks change.
-- book_progress — per profile + book: one book-global position,
--                 an explicit finished flag, and when the profile
--                 last listened. Its own table so no Phase 1–9
--                 query (Continue Watching, unwatched filters, the
--                 Emby importer) changes; whether it should fold
--                 into user_data, and whether progress is per
--                 profile or per login, is the open D29.

ALTER TABLE media_items DROP CONSTRAINT media_items_kind_check;
ALTER TABLE media_items ADD CONSTRAINT media_items_kind_check
    CHECK (kind IN ('movie', 'series', 'season', 'episode', 'audiobook'));

ALTER TABLE media_files ADD COLUMN track_index integer;

CREATE TABLE book_chapters (
    item_id       uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    chapter_index integer NOT NULL,
    start_ms      bigint NOT NULL CHECK (start_ms >= 0),
    end_ms        bigint NOT NULL,
    title         text NOT NULL,
    PRIMARY KEY (item_id, chapter_index),
    CHECK (end_ms >= start_ms)
);

CREATE TABLE book_progress (
    profile_id       uuid NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
    item_id          uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    position_ms      bigint NOT NULL DEFAULT 0 CHECK (position_ms >= 0),
    duration_ms      bigint,
    finished         boolean NOT NULL DEFAULT false,
    finished_at      timestamptz,
    last_listened_at timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (profile_id, item_id)
);
CREATE INDEX book_progress_profile_updated_idx
    ON book_progress (profile_id, updated_at);
