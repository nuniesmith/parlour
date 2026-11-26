-- Audiobook metadata sources, matching and covers (Phase 10, AB6,
-- D30; spec: docs/AUDIOBOOKS-PODCASTS.md "Metadata sources, matching
-- and covers (AB6)").
--
-- libraries.metadata_precedence — the per-library order in which a
--                 book's local metadata sources are merged, later
--                 winning (D30: Audiobookshelf's order). Only
--                 audiobooks libraries read it. The ids are:
--                 folder, tags, nfo, txt, opf, abs_json, sidecar.
--                 `sidecar` is Parlour's own `.parlour.json` (or the
--                 server's copy of it, below), the last word by
--                 default. A source left out of the list isn't read.
-- book_metadata — the book-only fields the shared media_items row has
--                 no column for (subtitle, publisher, ISBN, ASIN,
--                 language, the provider ids a match recorded), and
--                 the fingerprint of the book's metadata files plus
--                 the precedence they were merged in, so a re-scan
--                 re-merges a book only when one of those changed.
-- book_sidecars — the server's own copy of a book's sidecar: what an
--                 admin's match left the book holding, kept in
--                 Parlour's metadata area (D30's default). It fills
--                 the `sidecar` slot of the merge when the book has
--                 no `.parlour.json` of its own, or when the file is
--                 unchanged since the match (an admin's match beats
--                 the file until the file changes, as §13.6 has it
--                 for video). `file_fingerprint` is the file's
--                 (path, size, mtime) at the match, NULL when there
--                 was none.

ALTER TABLE libraries ADD COLUMN metadata_precedence text[] NOT NULL
    DEFAULT '{folder,tags,nfo,txt,opf,abs_json,sidecar}';

CREATE TABLE book_metadata (
    item_id             uuid PRIMARY KEY REFERENCES media_items (id) ON DELETE CASCADE,
    subtitle            text,
    publisher           text,
    isbn                text,
    asin                text,
    language            text,
    google_books_id     text,
    open_library_id     text,
    sources_fingerprint text,
    updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE book_sidecars (
    item_id          uuid PRIMARY KEY REFERENCES media_items (id) ON DELETE CASCADE,
    document         jsonb NOT NULL,
    provider         text NOT NULL,
    provider_id      text NOT NULL,
    mode             text NOT NULL CHECK (mode IN ('overwrite', 'fill_gaps')),
    file_fingerprint text,
    matched_at       timestamptz NOT NULL DEFAULT now()
);
