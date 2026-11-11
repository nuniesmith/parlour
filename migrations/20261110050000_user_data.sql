-- Per-profile watch state (Milestone 1.5 groundwork; written by
-- playback reporting in Milestone 1.6, read by the home rails
-- and the unwatched filter now).
--
-- One row per (profile, item): resume position, played flag,
-- favourite, and when it was last played. Episodes carry their
-- own rows; "continue watching" for a series surfaces the
-- episode row (the UI groups by series).

CREATE TABLE user_data (
    profile_id      uuid NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
    item_id         uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    position_ms     bigint NOT NULL DEFAULT 0,
    duration_ms     bigint,
    played          boolean NOT NULL DEFAULT false,
    is_favorite     boolean NOT NULL DEFAULT false,
    last_played_at  timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (profile_id, item_id)
);
CREATE INDEX user_data_profile_recent_idx
    ON user_data (profile_id, last_played_at DESC);
