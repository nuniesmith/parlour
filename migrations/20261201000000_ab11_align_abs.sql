-- AB11 (Phase 10; D28/D29/D33): align the first audiobook slice with
-- Audiobookshelf. Spec: docs/AUDIOBOOKS-PODCASTS.md "Aligning with
-- Audiobookshelf (AB11)". The timestamp is only an ordering key (it
-- sorts after AB7's 20261130000000_series.sql); written 2026-11-26.
--
-- libraries.parse_subtitles — an audiobooks library's "parse
--                 subtitles" setting: when on, a title folder named
--                 `Title - Subtitle` gives the subtitle after ` - `.
--                 Off by default, as ABS has it. Part of every book's
--                 sources fingerprint, so a change re-merges the
--                 library at the next scan.
-- libraries.finish_remaining_seconds — auto-finish: a book is marked
--                 finished when a progress write leaves less than this
--                 many seconds of it to go. ABS's default is 10; 0
--                 turns the time rule off.
-- libraries.finish_percent — auto-finish at this share of the book
--                 instead (1–100); NULL, the default, is off. When set
--                 it replaces the time rule, as in ABS.
-- book_progress.hidden — the profile hid the book from Continue
--                 Listening. Cleared by the next write that changes
--                 the position or the finished flag.
-- book_progress.client_updated_at — when the writer says it made the
--                 last update (the client's own clock; the server's
--                 time when a write carried none). Last arrival still
--                 wins; `updated_at` stays the server's time and the
--                 sync cursor.

ALTER TABLE libraries ADD COLUMN parse_subtitles boolean NOT NULL DEFAULT false;
ALTER TABLE libraries ADD COLUMN finish_remaining_seconds integer NOT NULL DEFAULT 10
    CHECK (finish_remaining_seconds >= 0);
ALTER TABLE libraries ADD COLUMN finish_percent integer
    CHECK (finish_percent BETWEEN 1 AND 100);

ALTER TABLE book_progress ADD COLUMN hidden boolean NOT NULL DEFAULT false;
ALTER TABLE book_progress ADD COLUMN client_updated_at timestamptz;
