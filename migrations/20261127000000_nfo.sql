-- Kodi/Emby-style .nfo files beside the media (Phase 8, XFER6, D14;
-- spec: docs/IMPORT-EXPORT.md §13.13).
--
-- item_nfos   — what the scanner last read from an item's NFO files:
--               their fingerprint (path, size, mtime of each, so an
--               unchanged NFO is not applied again) and what Parlour
--               takes from them (title and provider ids), which the
--               matcher uses to pick the provider record when the
--               item's sidecar names none. `overridden_at` is set by
--               an admin's fix-match and clears when an NFO file
--               changes or Parlour rewrites its own.
-- sidecar_nfos — one row per .nfo that Parlour itself wrote next to
--               the media, with the SHA-256 of the bytes it wrote (as
--               sidecar_artwork does for posters). Parlour only ever
--               replaces an NFO whose bytes still hash to what it
--               recorded here, so an NFO another program wrote is
--               never overwritten.
--
-- No new opt-in: NFOs are written under libraries.write_sidecars
-- (XFER4/XFER5), off by default.

CREATE TABLE item_nfos (
    item_id       uuid PRIMARY KEY REFERENCES media_items (id) ON DELETE CASCADE,
    fingerprint   text NOT NULL,
    document      jsonb NOT NULL,
    overridden_at timestamptz,
    read_at       timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sidecar_nfos (
    path       text PRIMARY KEY,
    item_id    uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    sha256     text NOT NULL,
    written_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sidecar_nfos_item ON sidecar_nfos (item_id);
