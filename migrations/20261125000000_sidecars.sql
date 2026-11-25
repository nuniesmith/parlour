-- Sidecar metadata stored with the media files (Phase 8, XFER4,
-- D14; spec: docs/IMPORT-EXPORT.md §13).
--
-- libraries.write_sidecars — the per-library opt-in D14 requires
--                 before Parlour writes anything next to the media.
--                 Off by default: media mounts are read-only. XFER4
--                 only reads it; exposing and operating it (API,
--                 admin UI, the mount change) is XFER5's task.
-- item_sidecars — what the scanner last read from an item's
--                 sidecar files: their fingerprint (path, size,
--                 mtime of each file, so an unchanged sidecar is
--                 not applied again) and the document it applied,
--                 which the metadata matcher re-applies over a
--                 provider lookup. `overridden_at` is set when an
--                 admin's fix-match replaced the sidecar's values
--                 and the file could not be rewritten; it clears
--                 when the sidecar file changes or is rewritten.

ALTER TABLE libraries ADD COLUMN write_sidecars boolean NOT NULL DEFAULT false;

CREATE TABLE item_sidecars (
    item_id       uuid PRIMARY KEY REFERENCES media_items (id) ON DELETE CASCADE,
    fingerprint   text NOT NULL,
    document      jsonb NOT NULL,
    overridden_at timestamptz,
    read_at       timestamptz NOT NULL DEFAULT now()
);
