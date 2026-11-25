-- Poster and backdrop copies beside the media (Phase 8, XFER5, D14;
-- spec: docs/IMPORT-EXPORT.md §13.10).
--
-- sidecar_artwork — one row per `poster.jpg` / `backdrop.jpg` that
--                 Parlour itself wrote next to the media, with the
--                 SHA-256 of the bytes it wrote. Parlour only ever
--                 replaces a file whose bytes still hash to what it
--                 recorded here, so a poster someone else put in a
--                 folder (another media server, the owner by hand)
--                 is never overwritten. Losing a row (its item was
--                 deleted) only makes Parlour leave that file alone.
--
-- XFER5 adds no new column for the opt-in itself: it operates
-- libraries.write_sidecars, which XFER4 added (default off).

CREATE TABLE sidecar_artwork (
    path       text PRIMARY KEY,
    item_id    uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    kind       text NOT NULL CHECK (kind IN ('poster', 'backdrop')),
    sha256     text NOT NULL,
    written_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sidecar_artwork_item ON sidecar_artwork (item_id);
