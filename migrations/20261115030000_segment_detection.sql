-- SEG2 (Phase 9, D17/D18): the cheap segment-detection pass.
--
-- 1. A fourth source, `heuristic`: intro/credits spans guessed from
--    black-frame + silence runs at a file's head and tail. They are
--    stored at confidence 0.5, below the 0.6 playback floor, so a
--    guess never reaches a player (D17); they exist for the SEG4
--    editor and as starting points for SEG3. A guess must stay
--    distinguishable from a real chapter marker, hence its own
--    source rather than `chapters`.
--
-- 2. media_files.segments_detected_at: when the DetectSegments job
--    last looked at this exact file. NULL means "not looked at
--    yet": a new file, a changed one (the scanner clears it every
--    time it re-probes a file whose size or mtime moved), and every
--    file that existed before this migration. After a scan, the
--    job is queued for the episodes whose playback file is NULL
--    here, so unchanged episodes are never decoded twice.

ALTER TABLE media_segments DROP CONSTRAINT media_segments_source_check;
ALTER TABLE media_segments ADD CONSTRAINT media_segments_source_check
    CHECK (source IN ('chapters', 'fingerprint', 'manual', 'heuristic'));

ALTER TABLE media_files ADD COLUMN segments_detected_at timestamptz;
