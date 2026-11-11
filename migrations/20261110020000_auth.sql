-- Milestone 1.2: sessions learn the active profile; invites table;
-- case-insensitive username uniqueness.

ALTER TABLE sessions
    ADD COLUMN active_profile_id uuid REFERENCES profiles(id) ON DELETE SET NULL;

CREATE TABLE invites (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash text NOT NULL UNIQUE,
    created_by uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    used_at    timestamptz,
    used_by    uuid REFERENCES users(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX users_username_lower_idx ON users (lower(username));
