-- Parlour core identity + library schema (Milestone 1.1, task F5).
--
-- users      — one login per person. Invite-only (Milestone 1.2).
-- profiles   — people under a login (household members, kids).
-- sessions   — DB-backed sessions (decision D7): the cookie holds
--              a random token, this table stores only its SHA-256
--              hash, so a database leak does not leak live sessions.
-- libraries  — named roots of one kind; library_paths holds the
--              folders, library_access says which PROFILES may see
--              a library and up to what content rating.

CREATE TABLE users (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username      text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    is_admin      boolean NOT NULL DEFAULT false,
    created_at    timestamptz NOT NULL DEFAULT now(),
    disabled_at   timestamptz
);

CREATE TABLE profiles (
    id                       uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id                  uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name                     text NOT NULL,
    pin_hash                 text,
    avatar_color             text,
    audio_lang               text,
    subtitle_lang            text,
    subtitle_mode            text NOT NULL DEFAULT 'auto',
    remote_bitrate_cap_kbps  integer,
    created_at               timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);
CREATE INDEX profiles_user_id_idx ON profiles (user_id);

CREATE TABLE sessions (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   text NOT NULL UNIQUE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    user_agent   text,
    ip           text
);
CREATE INDEX sessions_user_id_idx ON sessions (user_id);
CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);

CREATE TABLE libraries (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL UNIQUE,
    kind       text NOT NULL CHECK (kind IN
                 ('movies', 'shows', 'audiobooks', 'ebooks', 'music', 'podcasts')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE library_paths (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    library_id uuid NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    path       text NOT NULL,
    UNIQUE (library_id, path)
);

CREATE TABLE library_access (
    profile_id uuid NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
    library_id uuid NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    max_rating text,
    PRIMARY KEY (profile_id, library_id)
);
