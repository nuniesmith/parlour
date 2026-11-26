-- POD1 (Phase 11; D29/D31/D32): the podcast schema, mirroring
-- Audiobookshelf's podcast model. Spec: docs/AUDIOBOOKS-PODCASTS.md
-- "Schema (POD1)". The timestamp is only an ordering key (it sorts
-- after AB8's 20261202000000); written 2026-11-26.
--
-- No fetching, parsing or downloading happens yet: POD2 (feed fetch
-- and parse) and POD3 (download and retention) write these columns.
--
-- Access (D31): there is no subscriptions table and no per-profile
-- follow. A show belongs to a `podcasts` library, and a profile sees
-- every show in the libraries its `library_access` rows grant, as it
-- sees every book of an audiobooks library.
--
-- podcasts — one row per feed in a podcasts library:
--   feed_url        — the RSS address; a feed is in a library once;
--   title … itunes_id — the show as its feed last described it
--                   (iTunes namespace where RSS has no field);
--   image_url / image_path — the feed's artwork address, and the
--                   cached copy in Parlour's artwork cache once one
--                   is fetched;
--   folder_path     — where the show's episodes are written: a folder
--                   inside the podcasts library's own writable folder
--                   (D32), never a video or book media mount. NULL
--                   until the show is given one;
--   etag / last_modified — the feed's validators from its last 200,
--                   sent back as If-None-Match / If-Modified-Since;
--   last_checked_at / last_error / consecutive_failures — the refresh
--                   state: when the feed was last checked (successful
--                   or not), why the last check failed (NULL after a
--                   success) and how many checks in a row have failed;
--   auto_download   — D32: download new episodes on the schedule. Off
--                   by default. As in Audiobookshelf, the scheduled
--                   check runs only while it is on, and it is what
--                   turns itself off after 24 failed checks in a row;
--   check_schedule  — D32: when the scheduled check runs, as a
--                   five-field cron expression; hourly by default;
--   max_episodes_to_keep — D32: keep at most this many downloaded
--                   episodes, deleting the oldest (at most one per
--                   check); 0, the default, keeps all;
--   max_new_episodes_per_check — D32: download at most this many new
--                   episodes per check; 3 by default; 0 is no cap.
--
-- podcast_episodes — one row per feed item with an enclosure:
--   guid            — the feed's identity for the episode (the
--                   enclosure URL when the item has no guid), unique
--                   within its show;
--   duration_ms     — the length the feed gives (itunes:duration);
--   enclosure_*     — the audio the feed points at (URL, MIME type,
--                   length in bytes as the feed gives it);
--   chapters_url / transcript_* — Podcasting 2.0 links, kept as URLs;
--   file_*, downloaded_at — the local copy once downloaded (D32: only
--                   downloaded episodes play): its absolute path under
--                   the show's folder_path, size, the probed length,
--                   container and audio codec. All NULL until then.
--
-- book_progress — episode progress shares the book progress table and
--   its shape (D29; follow-up item 11): a row is either a book's
--   (item_id) or an episode's (episode_id), never both. Each is
--   unique per profile, as the primary key was for books. Every
--   existing book query reaches the table through an inner join on
--   item_id, so episode rows never show up there.

CREATE TABLE podcasts (
    id                         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    library_id                 uuid NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    feed_url                   text NOT NULL,
    title                      text NOT NULL,
    sort_title                 text NOT NULL,
    author                     text,
    description                text,
    language                   text,
    explicit                   boolean NOT NULL DEFAULT false,
    podcast_type               text CHECK (podcast_type IN ('episodic', 'serial')),
    genres                     text[] NOT NULL DEFAULT '{}',
    itunes_id                  bigint,
    image_url                  text,
    image_path                 text,
    folder_path                text,
    etag                       text,
    last_modified              text,
    last_checked_at            timestamptz,
    last_error                 text,
    consecutive_failures       integer NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    auto_download              boolean NOT NULL DEFAULT false,
    check_schedule             text NOT NULL DEFAULT '0 * * * *',
    max_episodes_to_keep       integer NOT NULL DEFAULT 0 CHECK (max_episodes_to_keep >= 0),
    max_new_episodes_per_check integer NOT NULL DEFAULT 3
                               CHECK (max_new_episodes_per_check >= 0),
    created_at                 timestamptz NOT NULL DEFAULT now(),
    updated_at                 timestamptz NOT NULL DEFAULT now(),
    UNIQUE (library_id, feed_url)
);
CREATE INDEX podcasts_library_sort_idx ON podcasts (library_id, sort_title);

CREATE TABLE podcast_episodes (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    podcast_id       uuid NOT NULL REFERENCES podcasts (id) ON DELETE CASCADE,
    guid             text NOT NULL,
    title            text NOT NULL,
    subtitle         text,
    description      text,
    published_at     timestamptz,
    duration_ms      bigint CHECK (duration_ms >= 0),
    season           integer,
    episode_number   integer,
    episode_type     text CHECK (episode_type IN ('full', 'trailer', 'bonus')),
    enclosure_url    text NOT NULL,
    enclosure_type   text,
    enclosure_length bigint CHECK (enclosure_length >= 0),
    chapters_url     text,
    transcript_url   text,
    transcript_type  text,
    file_path        text,
    file_size_bytes  bigint CHECK (file_size_bytes >= 0),
    file_duration_ms bigint CHECK (file_duration_ms >= 0),
    file_container   text,
    file_codec       text,
    downloaded_at    timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (podcast_id, guid),
    CHECK ((file_path IS NULL) = (downloaded_at IS NULL))
);
CREATE INDEX podcast_episodes_published_idx
    ON podcast_episodes (podcast_id, published_at DESC);
CREATE UNIQUE INDEX podcast_episodes_file_path_key
    ON podcast_episodes (file_path) WHERE file_path IS NOT NULL;

ALTER TABLE book_progress
    ADD COLUMN episode_id uuid REFERENCES podcast_episodes (id) ON DELETE CASCADE;
ALTER TABLE book_progress DROP CONSTRAINT book_progress_pkey;
ALTER TABLE book_progress ALTER COLUMN item_id DROP NOT NULL;
ALTER TABLE book_progress
    ADD CONSTRAINT book_progress_profile_item_key UNIQUE (profile_id, item_id);
ALTER TABLE book_progress
    ADD CONSTRAINT book_progress_profile_episode_key UNIQUE (profile_id, episode_id);
ALTER TABLE book_progress
    ADD CONSTRAINT book_progress_one_target_check
    CHECK (num_nonnulls(item_id, episode_id) = 1);
