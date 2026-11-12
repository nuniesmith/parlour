-- Server notices / Alerts (Phase 7, UI5): things the server wants
-- an admin to know about — a failed job, items waiting for a
-- metadata match, low disk space where Parlour writes, a restart
-- needed to apply configuration.
--
-- `key` identifies the condition ('job_failed:<job id>',
-- 'metadata_review', 'disk_low:cache', 'restart_required') so a
-- condition that is still true updates its row instead of adding
-- another. `resolved_at` is set when the condition clears.
-- Dismissal is per admin: one row here per (notice, user).

CREATE TABLE notices (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    key         text NOT NULL UNIQUE,
    kind        text NOT NULL,
    severity    text NOT NULL CHECK (severity IN ('info', 'warning', 'error')),
    summary     text NOT NULL,
    detail      jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    resolved_at timestamptz
);
CREATE INDEX notices_active_idx ON notices (created_at DESC) WHERE resolved_at IS NULL;

CREATE TABLE notice_dismissals (
    notice_id    uuid NOT NULL REFERENCES notices (id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    dismissed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (notice_id, user_id)
);
