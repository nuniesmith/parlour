-- XFER8 / D39: nightly backups are set up in the web UI (Server →
-- Import/Export), stored here. One row at most. No row means the
-- environment's bootstrap default (PARLOUR_BACKUP_DIR /
-- PARLOUR_BACKUP_KEEP, XFER7) applies; once an admin saves the
-- setting, this row is what counts, and a NULL folder means nightly
-- backups are off. `include_hashes` is D14's confirm for the standing
-- job, given past the same warning a manual export shows; off by
-- default. Not carried in the export bundle (IMPORT-EXPORT.md §8).
CREATE TABLE backup_settings (
    id             boolean PRIMARY KEY DEFAULT true CHECK (id),
    folder         text CHECK (folder IS NULL OR left(folder, 1) = '/'),
    keep           integer NOT NULL CHECK (keep BETWEEN 1 AND 365),
    include_hashes boolean NOT NULL DEFAULT false,
    updated_at     timestamptz NOT NULL DEFAULT now(),
    updated_by     uuid REFERENCES users (id) ON DELETE SET NULL
);
