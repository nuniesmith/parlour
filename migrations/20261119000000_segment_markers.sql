-- SEG4 (Phase 9, D21): the segment editor's "no segment of this kind
-- here" marker.
--
-- Deleting a *detected* segment in the editor cannot simply remove its
-- row: every detection run rewrites its own source's rows (SEG2 a
-- file's chapters and heuristic rows when the file changes, SEG3 a
-- whole season's fingerprint rows whenever any episode in it is new or
-- changed), so a deleted row would come back. Instead the editor
-- stores the admin's answer as the item's *manual* row for that kind,
-- with no times: "this item has no <kind>". Like every manual row it
-- beats all automatic sources (D15) and no detector ever touches it,
-- so it survives re-runs; the detectors keep their own rows up to date
-- underneath, and deleting the marker hands the kind straight back to
-- them (D21).
--
-- A marker is the only kind of row without times, and only a manual
-- row may be one. Because there is one row per (item, kind, source),
-- an item holds either the admin's times for a kind or the admin's
-- "none", never both. The existing checks (start_ms >= 0,
-- end_ms > start_ms) still hold for every row that has times; a CHECK
-- passes on NULL. Readers must expect NULL times on manual rows from
-- here on (playback-info's read path and the editor do).

ALTER TABLE media_segments
    ALTER COLUMN start_ms DROP NOT NULL,
    ALTER COLUMN end_ms DROP NOT NULL;

ALTER TABLE media_segments ADD CONSTRAINT media_segments_marker_check
    CHECK ((start_ms IS NULL) = (end_ms IS NULL)
           AND (start_ms IS NOT NULL OR source = 'manual'));
