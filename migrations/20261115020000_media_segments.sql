-- SEG1 (Phase 9, D15): intro / credits / recap segments.
--
-- One row per (item, kind, source): each detector (SEG2 chapters,
-- SEG3 fingerprint) and the editor (SEG4, manual) owns its own row
-- for a kind, so an automatic re-run upserts only its own source
-- and can never overwrite a manual edit. Which row the player
-- actually gets is resolved at read time
-- (`parlour_playback::effective_segments`): a manual row always
-- wins over every automatic source for that kind.
--
-- Times are milliseconds into the item's file. `confidence` is
-- 0.0–1.0; manual rows are written at 1.0. A low-confidence
-- detection is no row at all (D15), so the table never holds a
-- guess the detector itself would not stand behind.
CREATE TABLE media_segments (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    item_id     uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    kind        text NOT NULL CHECK (kind IN ('intro', 'credits', 'recap')),
    start_ms    bigint NOT NULL CHECK (start_ms >= 0),
    end_ms      bigint NOT NULL,
    source      text NOT NULL CHECK (source IN ('chapters', 'fingerprint', 'manual')),
    confidence  real NOT NULL DEFAULT 1.0 CHECK (confidence >= 0 AND confidence <= 1),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (end_ms > start_ms),
    UNIQUE (item_id, kind, source)
);
