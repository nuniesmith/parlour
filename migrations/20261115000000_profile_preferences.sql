-- UI7: per-profile preferences that the Preferences pages edit.
--
-- Home Screen: `home_rail_order` is the order the profile chose
-- (it may be partial — rails it has never seen are appended in
-- the server's default order) and `home_rails_hidden` the rails
-- it turned off. Storing "hidden" rather than "shown" means a rail
-- added later (UI8) starts visible instead of silently missing.
--
-- Playback (D15, read by SEG5/SEG6): `segment_skip` is what the
-- player does inside an intro/credits segment — 'button', 'auto'
-- or 'off' — and `autoplay_next` the next-episode countdown
-- toggle (default on).
ALTER TABLE profiles
    ADD COLUMN home_rail_order   text[]  NOT NULL DEFAULT '{}',
    ADD COLUMN home_rails_hidden text[]  NOT NULL DEFAULT '{}',
    ADD COLUMN segment_skip      text    NOT NULL DEFAULT 'button',
    ADD COLUMN autoplay_next     boolean NOT NULL DEFAULT true;
