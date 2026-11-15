-- UI9: recent searches, per profile. Stored on the server like the
-- other per-profile state (UI7), so they follow the profile to
-- every device. A search is recorded only when the profile commits
-- to it (Enter, or opening a result) — not on every keystroke.
--
-- One row per distinct query, compared case-insensitively:
-- searching again moves it to the top and keeps the latest casing.
-- The server keeps only the newest few per profile.

CREATE TABLE recent_searches (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    profile_id  uuid NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
    query       text NOT NULL,
    searched_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX recent_searches_profile_query_idx
    ON recent_searches (profile_id, lower(query));
CREATE INDEX recent_searches_profile_time_idx
    ON recent_searches (profile_id, searched_at DESC);
