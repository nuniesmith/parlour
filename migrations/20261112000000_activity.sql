-- Activity feed (Phase 7, UI4): an append-only audit trail of
-- things people and the server did — sign-ins, playback start /
-- stop, scan and import results, invite and user admin actions.
-- Shown on the Server Dashboard and its Activity page; rows older
-- than 90 days are removed by the nightly purge_activity job.
--
-- `actor` is the username at the time of the event, kept as text
-- so the line still reads correctly after the account is renamed
-- or removed (user_id is then set NULL).

CREATE TABLE activity (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at  timestamptz NOT NULL DEFAULT now(),
    kind        text NOT NULL,
    user_id     uuid REFERENCES users (id) ON DELETE SET NULL,
    profile_id  uuid REFERENCES profiles (id) ON DELETE SET NULL,
    item_id     uuid REFERENCES media_items (id) ON DELETE SET NULL,
    actor       text,
    summary     text NOT NULL,
    detail      jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX activity_created_idx ON activity (created_at DESC, id);
