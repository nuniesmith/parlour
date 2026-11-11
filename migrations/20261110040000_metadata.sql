-- Parlour metadata schema (Milestone 1.4, MD1–MD4).
--
-- media_items gains the fields a metadata provider fills in:
-- genres, vote_average, runtime, air date (episodes), and the
-- match state machine: unmatched -> matched (auto or manual) /
-- needs_review (candidates stored, a human picks) — the matcher
-- never silently guesses below the confidence bar (MD2).
--
-- metadata_candidates holds the scored shortlist for items in
-- needs_review, so the fix-match API (MD4) can show exactly what
-- the matcher was unsure about.
--
-- images gains `variant` (original / w500 / w300 / generated) and
-- `source_url` (where the cached file came from; the local path
-- is filled by the artwork job, MD3).
--
-- jobs gains `result`: a JSON summary a finished job reports
-- (e.g. matched/review/unmatched counts for a metadata run).

ALTER TABLE media_items ADD COLUMN genres text[] NOT NULL DEFAULT '{}';
ALTER TABLE media_items ADD COLUMN vote_average double precision;
ALTER TABLE media_items ADD COLUMN runtime_minutes integer;
ALTER TABLE media_items ADD COLUMN air_date date;
ALTER TABLE media_items ADD COLUMN metadata_status text NOT NULL DEFAULT 'unmatched'
    CHECK (metadata_status IN ('unmatched', 'matched', 'needs_review', 'failed'));
ALTER TABLE media_items ADD COLUMN metadata_provider text;
ALTER TABLE media_items ADD COLUMN last_metadata_at timestamptz;

CREATE TABLE metadata_candidates (
    item_id     uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    provider    text NOT NULL,
    provider_id bigint NOT NULL,
    title       text NOT NULL,
    year        integer,
    overview    text,
    poster_url  text,
    confidence  double precision NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (item_id, provider, provider_id)
);
CREATE INDEX metadata_candidates_item_idx ON metadata_candidates (item_id);

ALTER TABLE images ADD COLUMN variant text NOT NULL DEFAULT 'original';
ALTER TABLE images ADD COLUMN source_url text;

ALTER TABLE jobs ADD COLUMN result jsonb;
