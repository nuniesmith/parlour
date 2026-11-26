-- AB8 (Phase 10; D29/D35): listening sessions, kept server-side as
-- Audiobookshelf keeps them. Spec: docs/AUDIOBOOKS-PODCASTS.md
-- "Listening sessions and Home shelves (AB8)". The timestamp is only
-- an ordering key (it sorts after AB11's 20261201000000); written
-- 2026-11-26.
--
-- A listen is a session, not a play count: a book's listens never
-- touch user_data.play_count. A session belongs to one profile (D29)
-- and runs from the player's start report to its stop report, pauses
-- included. Open sessions (ended_at IS NULL) feed Now Playing; closed
-- ones feed the Activity feed; all of them feed the profile's
-- listening history and stats.
--
-- listening_sessions — one row per session:
--   start_position_ms / position_ms — book-global, where it started
--                   and where it is (or ended);
--   duration_ms   — the book's length as the player reported it;
--   listened_ms   — time actually spent listening (pauses excluded),
--                   as the player measures it, each report capped by
--                   the wall-clock time since the one before;
--   last_seen_at  — the last report; a session silent for an hour is
--                   closed at this time (end_reason 'timeout');
--   end_reason    — 'stopped' (the player said so), 'replaced' (a new
--                   start for the same book by the same profile),
--                   'timeout', or 'admin' (stopped from Now Playing);
--   client_*      — what the player said about itself, display only.

CREATE TABLE listening_sessions (
    id                uuid PRIMARY KEY,
    profile_id        uuid NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
    item_id           uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    started_at        timestamptz NOT NULL DEFAULT now(),
    last_seen_at      timestamptz NOT NULL DEFAULT now(),
    ended_at          timestamptz,
    end_reason        text CHECK (end_reason IN ('stopped', 'replaced', 'timeout', 'admin')),
    start_position_ms bigint NOT NULL CHECK (start_position_ms >= 0),
    position_ms       bigint NOT NULL CHECK (position_ms >= 0),
    duration_ms       bigint CHECK (duration_ms > 0),
    listened_ms       bigint NOT NULL DEFAULT 0 CHECK (listened_ms >= 0),
    client_name       text,
    client_version    text,
    device_name       text,
    CHECK ((ended_at IS NULL) = (end_reason IS NULL))
);

CREATE INDEX listening_sessions_profile_started_idx
    ON listening_sessions (profile_id, started_at DESC);
CREATE INDEX listening_sessions_started_idx
    ON listening_sessions (started_at DESC, id);
CREATE INDEX listening_sessions_open_idx
    ON listening_sessions (last_seen_at) WHERE ended_at IS NULL;
