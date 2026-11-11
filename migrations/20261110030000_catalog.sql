-- Parlour catalog + jobs schema (Milestone 1.3, tasks C1 + C6).
--
-- media_items   — the browsable catalog: movies stand alone;
--                 series -> season -> episode is a parent chain.
--                 `stable_key` is the scanner's identity for an
--                 item inside its library (derived from the parsed
--                 name/structure, never from a database id), which
--                 is what makes re-scans idempotent (C6).
-- media_files   — physical files. One item can have several
--                 (multi-part movies). Fingerprint = (size_bytes,
--                 mtime): a scan skips probing files whose
--                 fingerprint is unchanged (C3). A vanished file is
--                 marked unavailable (C8) — never deleted, so watch
--                 state (Milestone 1.6) survives a missing disk.
-- media_streams — video/audio streams from ffprobe (C4).
-- subtitle_tracks — embedded streams AND sidecar files (C4);
--                 `source` says which. Picture/text distinction is
--                 the codec (hdmv_pgs_subtitle vs subrip, ...).
-- people/item_people/images — metadata lands in 1.4; the tables
--                 exist now so the catalog shape is stable.
-- jobs          — the Postgres job queue behind parlour-jobs (C6):
--                 claimed with FOR UPDATE SKIP LOCKED, progress and
--                 per-file errors are visible to admins while a
--                 scan runs.

CREATE TABLE jobs (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind            text NOT NULL,
    library_id      uuid REFERENCES libraries (id) ON DELETE CASCADE,
    payload         jsonb NOT NULL DEFAULT '{}'::jsonb,
    status          text NOT NULL DEFAULT 'queued'
                    CHECK (status IN ('queued', 'running', 'succeeded', 'failed')),
    progress_done   integer NOT NULL DEFAULT 0,
    progress_total  integer NOT NULL DEFAULT 0,
    errors          jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at      timestamptz NOT NULL DEFAULT now(),
    started_at      timestamptz,
    finished_at     timestamptz
);
CREATE INDEX jobs_status_created_idx ON jobs (status, created_at);
CREATE INDEX jobs_library_idx ON jobs (library_id, created_at DESC);

CREATE TABLE media_items (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    library_id     uuid NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    parent_id      uuid REFERENCES media_items (id) ON DELETE CASCADE,
    kind           text NOT NULL CHECK (kind IN ('movie', 'series', 'season', 'episode')),
    stable_key     text NOT NULL,
    title          text NOT NULL,
    sort_title     text NOT NULL,
    year           integer,
    season_number  integer,
    episode_number integer,
    overview       text,
    content_rating text,
    tmdb_id        bigint,
    tvdb_id        bigint,
    imdb_id        text,
    available      boolean NOT NULL DEFAULT true,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (library_id, stable_key)
);
CREATE INDEX media_items_library_parent_idx ON media_items (library_id, parent_id);
CREATE INDEX media_items_sort_idx ON media_items (library_id, sort_title);

CREATE TABLE media_files (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id       uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    library_id    uuid NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    path          text NOT NULL,
    relative_path text NOT NULL,
    size_bytes    bigint NOT NULL,
    mtime         timestamptz NOT NULL,
    container     text,
    duration_ms   bigint,
    probe_error   text,
    available     boolean NOT NULL DEFAULT true,
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (library_id, path)
);
CREATE INDEX media_files_item_idx ON media_files (item_id);

CREATE TABLE media_streams (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id     uuid NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
    stream_index integer NOT NULL,
    kind        text NOT NULL CHECK (kind IN ('video', 'audio')),
    codec       text,
    width       integer,
    height      integer,
    hdr_format  text,
    language    text,
    title       text,
    channels    integer,
    bit_rate    bigint,
    is_default  boolean NOT NULL DEFAULT false,
    UNIQUE (file_id, stream_index)
);

CREATE TABLE subtitle_tracks (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id     uuid NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
    stream_index integer,
    source      text NOT NULL CHECK (source IN ('embedded', 'sidecar')),
    path        text,
    format      text,
    language    text,
    title       text,
    is_forced   boolean NOT NULL DEFAULT false,
    is_default  boolean NOT NULL DEFAULT false,
    is_sdh      boolean NOT NULL DEFAULT false,
    UNIQUE (file_id, source, stream_index, path)
);
CREATE INDEX subtitle_tracks_file_idx ON subtitle_tracks (file_id);

CREATE TABLE people (
    id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name    text NOT NULL UNIQUE,
    tmdb_id bigint
);

CREATE TABLE item_people (
    item_id       uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    person_id     uuid NOT NULL REFERENCES people (id) ON DELETE CASCADE,
    role          text NOT NULL,
    character_name text,
    sort_order    integer NOT NULL DEFAULT 0,
    PRIMARY KEY (item_id, person_id, role)
);

CREATE TABLE images (
    id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    kind    text NOT NULL CHECK (kind IN ('poster', 'backdrop', 'logo', 'still')),
    path    text NOT NULL,
    hash    text,
    width   integer,
    height  integer,
    UNIQUE (item_id, kind, path)
);
