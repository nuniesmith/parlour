-- Audiobook series (Phase 10, AB7, D28; spec:
-- docs/AUDIOBOOKS-PODCASTS.md "Series and authors (AB7)").
--
-- D28 mirrors Audiobookshelf: a series is its own record, joined to
-- books with a sequence on the link, so one book can sit in several
-- series. Authors need no table here: they are already `people`
-- linked through `item_people` with role `author` (AB1).
--
-- series      — one per (library, name), the name compared without
--               regard to case. The first spelling seen is kept.
--               `sort_name` drops a leading article, as sort_title
--               does for items. A scan removes a series once no book
--               links to it.
-- book_series — a book's place in a series. `sequence` is text, as
--               the sources give it (`1`, `2.5`, `1-3`), NULL when
--               none was given. `sort_order` is the position of the
--               series in the book's own list. Rebuilt whole with the
--               book's other merged fields, so a re-scan never
--               duplicates a link.

CREATE TABLE series (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    library_id uuid NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    name       text NOT NULL,
    sort_name  text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX series_library_name ON series (library_id, lower(name));

CREATE TABLE book_series (
    item_id    uuid NOT NULL REFERENCES media_items (id) ON DELETE CASCADE,
    series_id  uuid NOT NULL REFERENCES series (id) ON DELETE CASCADE,
    sequence   text,
    sort_order integer NOT NULL DEFAULT 0,
    PRIMARY KEY (item_id, series_id)
);

CREATE INDEX book_series_series ON book_series (series_id);
