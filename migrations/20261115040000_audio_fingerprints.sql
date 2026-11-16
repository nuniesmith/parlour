-- SEG3 (Phase 9, D20): the audio-fingerprint pass of segment detection.
--
-- 1. audio_fingerprints: the fingerprint cache. One row per distinct
--    file content, keyed by the SHA-256 of the file's size plus its
--    first and last 4 MiB (D20), so a file that is renamed, moved or
--    shared between libraries is never fingerprinted twice. A row
--    holds chromaprint's raw points (little-endian u32s, one every
--    1365/11025 s) for D20's two search windows: the head — the first
--    25 % of the episode, capped at 10 minutes, ending at
--    `head_end_ms` — and the tail — the last 600 s, starting at
--    `tail_start_ms`. About 40 KB per episode.
--
-- 2. media_files.file_hash: which cache row describes this file. The
--    scanner clears it whenever it re-probes a changed file (as it
--    does `segments_detected_at`), so a non-NULL hash always belongs
--    to the bytes the row describes. A cache row that no file row
--    references any more is deleted at the end of each detection job
--    (D20). An unavailable file's row still references its
--    fingerprint, so a disk that comes back is not fingerprinted
--    again (rows are never deleted, C8).
--
-- 3. Every file is marked as not yet looked at: the detection job
--    now also fingerprints, so each library's next scan queues it and
--    every season gets its first fingerprint match. SEG2's pass over
--    the same files is idempotent — it writes the rows it wrote
--    before.

CREATE TABLE audio_fingerprints (
    file_hash     bytea PRIMARY KEY CHECK (octet_length(file_hash) = 32),
    head_end_ms   bigint NOT NULL CHECK (head_end_ms >= 0),
    head_points   bytea NOT NULL CHECK (octet_length(head_points) % 4 = 0),
    tail_start_ms bigint NOT NULL CHECK (tail_start_ms >= 0),
    tail_points   bytea NOT NULL CHECK (octet_length(tail_points) % 4 = 0),
    created_at    timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE media_files ADD COLUMN file_hash bytea;
CREATE INDEX media_files_file_hash_idx ON media_files (file_hash)
    WHERE file_hash IS NOT NULL;

UPDATE media_files SET segments_detected_at = NULL
WHERE segments_detected_at IS NOT NULL;
