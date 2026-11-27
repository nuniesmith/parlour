-- POD5 (Phase 11; D41, D29/D35): the per-profile explicit-content
-- flag, and listening sessions on podcast episodes. Spec:
-- docs/AUDIOBOOKS-PODCASTS.md "Native API (POD5)". The timestamp is
-- only an ordering key (it sorts after POD1's 20261203000000); written
-- 2026-11-27.
--
-- profiles.allow_explicit — D41 (decided 2026-11-27, option a): as
--   Audiobookshelf's per-user "explicit content" permission. On by
--   default, so every existing profile keeps seeing what it sees. With
--   it off, a show whose feed marks it explicit (podcasts.explicit) and
--   all of its episodes read as not found to that profile. Only an
--   admin sets it. Left out of export format v1 (its bundle home is the
--   format-v2 step, D40).
--
-- listening_sessions.episode_id — AB8's sessions, widened the way
--   POD1 widened book_progress: a session is a book's (item_id) or an
--   episode's (episode_id), never both and never neither. Deleting the
--   episode deletes its sessions, as deleting a book does.

ALTER TABLE profiles
    ADD COLUMN allow_explicit boolean NOT NULL DEFAULT true;

ALTER TABLE listening_sessions
    ADD COLUMN episode_id uuid REFERENCES podcast_episodes (id) ON DELETE CASCADE;
ALTER TABLE listening_sessions ALTER COLUMN item_id DROP NOT NULL;
ALTER TABLE listening_sessions
    ADD CONSTRAINT listening_sessions_one_target_check
    CHECK (num_nonnulls(item_id, episode_id) = 1);
CREATE INDEX listening_sessions_episode_idx
    ON listening_sessions (episode_id) WHERE episode_id IS NOT NULL;
