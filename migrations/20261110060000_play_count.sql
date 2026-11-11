-- Playback reporting (Milestone 1.6, P4): how many times a
-- profile has started an item, alongside the resume position
-- and played flag already on user_data.
ALTER TABLE user_data
    ADD COLUMN play_count integer NOT NULL DEFAULT 0;
