# Audiobooks, podcasts and audio clients — plan (2026-11-23)

The owner's three new tracks, in his words (2026-11-23):

1. **Native audiobooks** — build an Audiobookshelf-style audiobook
   feature *into* Parlour as a first-class citizen (its own library,
   metadata and per-user listening progress), instead of Parlour
   leaning on Audiobookshelf-compat as its audiobook story. Keep the
   ABS-compat endpoints working.
2. **Podcasts** — the Parlour server manages RSS feeds in its
   interface, the *server* fetches new episodes, and clients (iOS
   app, Fire Stick, tablet, web) sync subscriptions and playback
   progress from the server.
3. **Client direction (API design only for now)** — the future iOS
   app's audiobook experience is modelled on the Audiobookshelf
   client the owner uses today; its podcast experience on Overcast.

These are Phases 10–12 in `ROADMAP.md`; the task checklists are in
`TODO.md`. Open product questions are DECISION NEEDED entries
**D26–D35** in `DECISIONS.md`. They are the owner's calls, and
nothing here answers them. Where the first phase must build
*something* before an answer (AB1–AB5 are built in the first
Phase 10 PR), this doc says what it builds, why that is the easiest
choice to reverse, and what changing it would cost.

**2026-11-24 — D26–D35 decided** (`DECISIONS.md`). In short:
**no ABS-compat bridge** — Parlour builds its own audiobook server
and its **own web and mobile clients**, and the owner stays on
Audiobookshelf (with the Audiobooth app) until they ship (D26); so
point 1's "keep the ABS-compat endpoints working" no longer applies.
**No ABS progress import** (D27, AB9 cancelled). Everything else
**mirrors what Audiobookshelf does** where ABS has an equivalent
(D28–D35), and anything without one is **deferred until the owner
asks**. The sections below are updated to match; where AB1–AB5
differ from ABS, see "Follow-up from D28–D35". The client UI,
derived from the owner's Audiobooth and Overcast screenshots, is
`UI-AUDIO-REFERENCE.md`.

**Clean-room rule, extended.** Audiobookshelf is GPL-3.0 and
Overcast is proprietary. The AGENTS.md rule for Emby, Jellyfin and
Silo applies to both: matching their behaviour and information
architecture is fine, but their code, assets, icons and wording are
not. Parlour stays MIT.

---

## Where things stand (read before planning)

- **There are no ABS-compat endpoints in Parlour today.** The only
  Audiobookshelf work on the books was Phase 5's X2 ("ABS surface so
  ABS apps work before Parlour grows native audiobook support"),
  which was never started. "Keep the ABS-compat endpoints working"
  therefore costs nothing now. Whether X2 is ever built, and for how
  long it lives, is **D26**. *2026-11-24: D26 — never built; X2 is
  cancelled.*
- The schema already allows `audiobooks` and `podcasts` library
  kinds, and the admin UI already offers them. Before Phase 10, an
  audiobooks library scanned **nothing**: the walker only knows
  video extensions.
- ARCHITECTURE §7 and §10 already settle the direction: libraries
  are typed but share **one catalog** (`media_items` +
  `media_files` + `media_streams`), and "every new library type is a
  `catalog` + `metadata` addition, not a new server". Phase 10
  follows that and does not add a second catalog.
- Watch state is per **profile** (ARCHITECTURE §2: "watch_state is
  per profile + item"). The owner wrote "per-user listening
  progress". Phase 10 stores progress per profile, like video, so
  each household member under one login keeps their own place.
  D29 asks whether that is what he meant. *2026-11-24: D29 — yes;
  ABS's per-user progress maps onto Parlour's profiles.*

---

## Phase 10 — Native audiobooks

### What a book is on disk (AB2, built 2026-11-23)

The scanner follows the folder layouts audiobook tools and
downloaders already produce (`Author/Title/…`,
`Author/Series/Title/…`), so the owner's existing library should
scan without renaming:

- Audio files: `m4b m4a mp3 flac ogg oga opus aac wav wma`.
- **Every folder that directly holds audio files is one book.**
  Its tracks are those files.
- **Disc folders fold into their parent.** A folder named like
  `CD 1`, `Disc 2`, `Disk3` or `Part 01` (nothing else in the name)
  belongs to the book in the folder above, and its files come after
  the earlier discs' files.
- An audio file sitting directly in the library root is a book on
  its own (typical for single-file `.m4b` books).
- Track order: tag disc number, then the disc folder's number, then
  tag track number, then natural file-name order (`Chapter 2`
  before `Chapter 10`). The order is stored as
  `media_files.track_index`.
- A book's identity (`stable_key`) is its folder path relative to
  the library root. Re-scans are idempotent, and moving a book's
  folder makes it a new book, the same as a movie today.

### Metadata from tags (AB2, built)

ffprobe reads every track's format tags and the chapters. The first
track that has a tag wins:

*AB6 (2026-11-26) changed two things in this table:*
- tags now come from the **first track** only, as ABS reads them;
- tags are one source in a per-library precedence list, and the
  folder name is its own source.

Embedded cover art is now extracted too. See "Metadata sources,
matching and covers (AB6)" below.

| Field | Source, first that's present |
|---|---|
| title | `album`; for a single-file book, `title`; else the folder or file name |
| authors | `album_artist`, else `artist`; else the top-level folder when the book sits in one (`Author/Title`, `Author/Series/Title`) |
| narrators | `composer` (the usual audiobook convention; plenty of files leave it empty) |
| year | first four digits of `date` / `year` |
| overview | `description`, else `comment` |
| genres | `genre`, split on `;` / `,` / `/` |
| cover | `cover.jpg` / `folder.jpg` (or `.png`) in the book folder, served from the media path; since AB6, embedded cover art when there is no such image |

Several authors or narrators in one tag (`A, B`, `A & B`, `A; B`)
are split. They are stored as `people` linked through `item_people`
with roles `author` / `narrator`. That is the same table cast uses,
so the author page (AB7, built) is a query, not a migration.

### Chapters (AB2, built)

- A track with embedded chapters (m4b/m4a, some mp3s) contributes
  them, shifted onto the book's timeline by the track's start
  offset.
- A track without chapters becomes one chapter, titled with its
  `title` tag or its file name.
- Chapters live in `book_chapters (item_id, chapter_index, start_ms,
  end_ms, title)`, rebuilt whenever a book's tracks change.

### Progress (AB4, built — D29 decided 2026-11-24; follow-ups below)

- Stored in `book_progress (profile_id, item_id, position_ms,
  duration_ms, finished, finished_at, last_listened_at,
  updated_at)`. That is its own table, not `user_data`, so nothing
  in Phase 1–9 behaviour changes (Continue Watching, unwatched
  filters, the Emby importer and the export draft). D29 asks
  whether it should be folded into `user_data` instead. Switching is
  one migration.
- `position_ms` is **book-global**: milliseconds from the start of
  track 0. Clients map it to (track, offset) with the track list's
  `start_ms` values. One number per book is what makes cross-device
  sync simple.
- **Finished is explicit:** a client sets `finished: true` (the web
  player does it when the last track ends). There is deliberately
  no percentage rule: when to auto-finish is part of D29.
- Writes are **last-write-wins** in arrival order, as video
  reporting is today. The rule for offline clients is part of D33.

### Native API (AB3/AB4, built)

All under `/api/v1`, cookie session, scoped to the **active
profile's** library access (no access reads as 404, like items).

| Method + path | What |
|---|---|
| `GET /audiobooks?library_id=&q=&sort=title\|added\|recent` | Books the profile can reach, with authors, narrators, duration, track and chapter counts, cover flag, and this profile's progress inline |
| `GET /audiobooks/{id}` | One book: the above plus `tracks` (index, file id, title, `start_ms`, `duration_ms`, codec, size, a tokenised `stream_url`) and `chapters` |
| `GET /audiobooks/{id}/tracks/{index}/stream` | Track audio with HTTP Range (single range, 206/416), authorised by stream token or session, the same rules as `/stream/{file_id}` |
| `GET /audiobooks/{id}/progress` | This profile's progress (zeros when never started) |
| `PUT /audiobooks/{id}/progress` | `{position_ms?, finished?}`; position is clamped to the book's length; returns the stored row |
| `DELETE /audiobooks/{id}/progress` | Back to "not started" |
| `GET /audiobooks/progress?since=<rfc3339>` | Every progress row for this profile changed after `since`: the bulk pull a client does on launch |

`stream_url` points at the existing `/api/v1/stream/{file_id}`
with a 6-hour HMAC token. AVPlayer, ExoPlayer and `<audio>` can play
it without sending cookies.

### Web (AB5, built)

An audiobooks library's page lists books (cover, title, authors,
progress). `/book/[id]` plays the book in an `<audio>` element,
track after track, with a chapter list. It resumes from the stored
position and writes progress every 15 s, on pause, on a seek, and
when the page closes, but only once the page has actually played. It marks the book finished when the last track ends.
Items of kind `audiobook` anywhere else (Home rails, search) link to
`/book/[id]`.

### Metadata sources, matching and covers (AB6, built 2026-11-26)

Built to **D30** (mirror Audiobookshelf's metadata handling). ABS
behaviour was read from its docs and server source (behaviour only,
nothing copied). Code: `crates/catalog/src/book_meta.rs` (the pure
readers and the merge), `crates/catalog/src/book_sync.rs` (the
database half: the sidecar slot, applying a match, writing beside a
book, covers), `crates/metadata/src/books.rs` (the two providers),
`crates/server/src/book_match.rs` (the admin API),
`web/src/lib/BookMatch.svelte`. Migration
`20261129000000_book_metadata.sql`.

**Local sources, merged in a per-library order.** Each audiobooks
library has an ordered precedence list
(`libraries.metadata_precedence`). A book's sources are read and
laid over each other in that order, and a later source overwrites
what an earlier one gave, field by field. The default is ABS's order,
with Parlour's own sidecar as the last word:

| id | source | what it gives |
|---|---|---|
| `folder` | the book folder's name, and the top-level folder above it | title (a root file's name without its extension); authors, split ABS's way (below) |
| `tags` | the audio tags of the **first track** in disc/track order | title (`album`; for a single-file book also `title`), `subtitle`, authors (`album_artist`, else `artist`), narrators (`composer`), year (`date`, else `year`), `publisher`, `isbn`, `asin`, `language`, overview (`description`, else `comment`), genres (`genre`) |
| `nfo` | an `.nfo` in the book folder, ABS's `Key: value` style | `Title` (`Title: Subtitle` splits), `Author`, `Narrator` / `Read by`, `Genre`, `Publisher`, `ASIN`, `ISBN`, `Language`, the year from the date and copyright lines, and `Book Description` to the end of the file. An XML (Kodi-style) `.nfo` says nothing here |
| `txt` | `desc.txt` and `reader.txt` | the whole of `desc.txt` as the overview; the first line of `reader.txt` as the narrators |
| `opf` | an `.opf` package document (Calibre and others) | the Dublin Core title, creators (authors, and narrators by role), date, publisher, description, subjects, language, ISBN / ASIN identifiers |
| `abs_json` | ABS's own `metadata.json`, read as an input | `title`, `subtitle`, `authors`, `narrators`, `genres`, `publishedYear` (else `publishedDate`), `publisher`, `description`, `isbn`, `asin`, `language`; anything else in the file is not read |
| `sidecar` | Parlour's `book.parlour.json` (D14), or the server's copy of it that a match left (below) | every field it carries, authoritatively |

- **How a field merges.** A source that has a field replaces it; a
  source that doesn't says nothing. Lists (authors, narrators,
  genres) are replaced whole, never merged. An empty list replaces
  only from the sidecar, where a present `[]` means "none" (§13.2 of
  `IMPORT-EXPORT.md`); from any other source it says nothing.
- **The title is never empty.** When no source gave one, the
  folder's (or file's) name is used.
- **A source left out of the list isn't read.** The admin edits the
  list per library (Server → Library → "Audiobook metadata sources":
  reorder, turn sources off). `GET | PUT
  /api/v1/admin/libraries/{id}/metadata-precedence` with
  `{"sources": [...]}`; the list needs at least one source, each a
  known id, none twice. An audiobooks library only.
- **Which files are a book's.** A folder book reads the files in its
  own folder (not its disc folders): the first `.nfo` and the first
  `.opf` by name, `desc.txt`, `reader.txt`, `metadata.json`,
  `book.parlour.json`, matched without regard to case. A book that is
  one file in a library root reads only `<stem>.parlour.json` beside
  it, because everything else in a root is shared.
- **Fields the shared item row has no column for** (`subtitle`,
  `publisher`, `isbn`, `asin`, `language`, the provider ids a match
  recorded) live in `book_metadata`, and `GET /api/v1/audiobooks/{id}`
  returns them, plus `match` (`provider`, `provider_id`, `mode`,
  `matched_at`; `null` when unmatched).
- **A re-scan stays idempotent.** `book_metadata` keeps a fingerprint
  of the book's metadata files (path, size, mtime), the precedence
  and the merge rules' version. An unchanged book with an unchanged
  fingerprint is skipped unprobed, as in AB2; a changed source file
  or a changed precedence list re-merges it. A book with no metadata
  files and no match merges to what AB2 gave it, apart from the
  first-track and author-splitting alignments listed below.

**Author splitting.** Folder names split ABS's way (follow-up item
5): on `;`, `&` and the word `and`, then on commas, where comma parts
that all lack a space are read as `Last, First` pairs (`Sanderson,
Brandon` → Brandon Sanderson) and a single-word surname before given
names is turned round (`Tolkien, J. R. R.`). Tags, NFO and
`reader.txt` keep AB2's tag splitting (`;`, `&`, ` / `, commas only
when every part has a space).

**Covers.** A book has one poster, in this order:

1. a cover an admin's match applied (in the artwork cache, variant
   `match`), while its file is there;
2. the folder's `cover.*` / `folder.*` image (variant `file`, served
   from the media path, as AB2 built);
3. **embedded cover art** from the first track in play order that
   carries a picture, extracted into the artwork cache as
   `poster-embedded.jpg` (`.png` for a PNG), variant `embedded`. Only
   when the book is probed; an earlier extraction is kept until then.

Cover bytes are sniffed (JPEG, PNG or WebP only), never trusted by
their name, and written atomically.

**Manual match (admins).** ABS matches by hand, and so does Parlour:

- `GET /api/v1/admin/audiobooks/{id}/match/search?provider=&title=&author=`
  searches one provider (`google`, the default, or `openlibrary`);
  title and author default to the book's own. Results keep the
  provider's order, each with a title `confidence` (0–1) for display.
- `POST /api/v1/admin/audiobooks/{id}/match` with `{"provider",
  "provider_id", "mode", "cover"}` applies one result. **The admin
  chooses `mode`** (D30): `overwrite` (every value the provider has
  replaces the book's) or `fill_gaps` (it fills only what the book
  lacks). The API has no default for it. `cover` (default true) takes
  the provider's cover: always with `overwrite`, and with `fill_gaps`
  only when the book has none.
- `DELETE /api/v1/admin/audiobooks/{id}/match` forgets the match.
  The server's copy and the matched cover go, and the next scan merges
  the book from its local sources again. Files written beside the
  book stay, because Parlour never deletes beside the media.
- In the web UI: a **Match** panel on `/book/[id]` for admins
  (provider, title, author, results with covers; "fill gaps" is
  preselected, "overwrite" one click away; "use the cover").

**Library-wide quick match.** `POST /api/v1/admin/libraries/{id}/metadata`
on an audiobooks library (the "Quick match" button on its card)
queues a job. For every available book it searches **Google Books** by
title and first author, takes the first result whose title is at
least 0.5 similar to the book's (also against "title subtitle"),
and **fills gaps only**, covers included (only where a book has
none). One book's failure goes on the job and the rest carry on. The
job waits 1 s between books, to stay inside the anonymous quota.
*ABS's default, checked 2026-11-26 in its server source:* its quick
match, both per book and library-wide, fills only empty fields and
sets a cover only where there is none, unless a server-wide "prefer
matched metadata" setting is on, and that setting is **off by
default**. The library-wide match passes no override, so it follows
that setting. D30's open question is answered by that, so quick match
is fill-gaps.

**Providers.**

- **Google Books** (D30: ABS's default for books): the public volumes
  API. **A key is optional.** Matching works without one on Google's
  anonymous quota. With `PARLOUR_GOOGLE_BOOKS_API_KEY` set in the
  server's environment (passed through by `docker-compose.yml`), it is
  sent in Google's `X-Goog-Api-Key` header, never in a URL, and
  provider errors are stripped of their URL, so the key can't reach a
  job error, an API error or a log. No key appears in the repo, its
  fixtures or its tests: the flow test makes one up at run time.
- **Open Library:** `search.json` and its covers service. No key.
  Requests carry Parlour's User-Agent
  (`Parlour/<version> (+https://github.com/nuniesmith/parlour)`, no
  contact address), as Open Library asks of applications.
- **Audnexus: left out.** It was checked on 2026-11-26, as asked
  on 2026-11-23. Its code is GPL-3.0, but neither its README nor its
  API reference publishes any terms of use for the public instance.
  Its data is assembled from Audible's API and product pages, and the
  project states no permission to redistribute it. That doesn't
  clearly permit a third-party server to depend on it, so Parlour
  doesn't. The search API answers 400 for `provider=audnexus`.
  Audible itself, and covers-only sources, are not built either:
  D30 names Google Books and Open Library first.

**Where a match is kept.** D30: in the server's own metadata area by
default, beside the book only under the per-library opt-in.

- **The server's copy.** A match leaves `book_sidecars`: a
  `parlour-sidecar` document (kind `audiobook`) holding everything the
  book then holds, with the provider, id and mode. It fills the
  merge's `sidecar` slot, so a re-scan keeps the match, as ABS keeps
  a match in the `metadata.json` it saves in its own metadata folder.
- **Which wins in the slot.** The book's own `book.parlour.json`
  wins when there is no server copy, or when the file has changed
  since the match. Otherwise the server copy wins: an admin's match
  beats the file until the file changes, as §13.6 of
  `IMPORT-EXPORT.md` has it for video.
- **Beside the book** only where the library has `write_sidecars` on
  (XFER5's flag, reused, now accepted for audiobooks libraries; no
  second flag). A match then writes `book.parlour.json` into the
  book's folder, and a matched cover as `cover.jpg` beside it. D14's
  sidecar rules hold:
  - the writes are atomic;
  - only into a folder that belongs to this book alone: never a
    library root (so a single-file root book never gets one), never
    a folder with another book's available audio under it;
  - `cover.jpg` is written only for a JPEG, never over a file
    Parlour didn't write, and never when the folder already has its
    own cover image under another name.

  Parlour writes **no `.nfo`** beside a book, because ABS's book NFO
  is a different format and is read only. The file formats are in
  §13.15 of `IMPORT-EXPORT.md`.

**Tests.** Unit: the precedence lists, the field-by-field merge,
folder author splitting, first-track tags, each local reader against
fixtures (`crates/catalog/tests/fixtures/books/`), overwrite vs fill
gaps, the sidecar slot, cover sniffing, quick match's pick, and both
providers' JSON against recorded responses
(`crates/metadata/tests/fixtures/books/`). Flow:
`crates/server/tests/book_match_flow.rs` scans a library of
ffmpeg-made books against a stub Google Books and Open Library (no
network, no key). It covers:

- the merged sources and embedded covers;
- search, fill-gaps and overwrite;
- the precedence API;
- writing beside books under the opt-in;
- quick match, and forgetting a match;
- a re-scan after each step, which must change nothing;
- a configured key travelling only in the header, never in a URL
  or an error.

#### Readings made in AB6 — for the owner's veto

Each is a reading inside D30, not a new decision.

1. **Quick match always fills gaps**, as ABS's does by default. There
   is no equivalent of ABS's server-wide "prefer matched metadata"
   switch. To overwrite, an admin matches the book by hand.
2. **Quick match uses Google Books only** (ABS's default provider
   for a book library). Open Library is offered per book.
3. **Quick match visits every available book.** ABS has per-library
   options to skip books that already have an ASIN or ISBN; their
   defaults weren't confirmed, and they aren't built.
4. **Quick match's bar is a title similarity of 0.5**; below it a
   book is counted as "no match" and left alone.
5. **The per-book API has no default mode.** The admin must send
   `overwrite` or `fill_gaps`. The web panel preselects fill gaps,
   the non-destructive choice.
6. **A matched cover beats the folder's image** when the admin chose
   overwrite with the cover; with fill gaps it applies only to a book
   with no poster. This follows ABS, where an override cover replaces
   the book's.
7. **Embedded cover: the first track in play order that has a
   picture**, not strictly the first track. Tags come strictly from
   the first track (D30).
8. **The `folder` source keeps AB2's author folder (the top level)**
   and reads the title folder's name raw. Nearest-folder authors and
   title-folder parsing (ASIN, narrator, sequence, year, subtitle)
   are AB11 (follow-up items 3–4), and the series folder is AB7
   (*built: it is part of the `folder` source, "Series and authors
   (AB7)"*).
9. **Tag author splitting stays as AB2 has it** (follow-up item 5
   asked for this to be reviewed here); only folder names split ABS's
   way. The precedence work didn't need a change.
10. **A source left out of the precedence list is not read at all**,
    and the admin may leave out or move `sidecar` too. An imported
    library starts with the default order (see D40 below).
11. **Matches and precedence changes are not in the Activity feed.**
    A video fix-match isn't either; XFER5's audit covers only the
    write-beside-media flag, which these reuse.
12. **Forgetting a match leaves files written beside the book**,
    because Parlour never deletes beside the media; delete them by
    hand to drop them.

**Open: D40 (DECISION NEEDED, `docs/DECISIONS.md`).** Should a book's
match (`book_sidecars`) and a library's precedence list travel in the
export bundle? Today neither does (format v1 is unchanged). On a new
host a match survives only as a `book.parlour.json` written beside
the book under the opt-in. **D36 (the audio tables) is also still
open:** AB6 doesn't touch `book_progress` / `book_chapters`, and the
bundle doesn't gain them.

### Series and authors (AB7, built 2026-11-26)

Built to **D28** (mirror Audiobookshelf's catalog model: series are
their own records joined to books with a sequence on the link, so a
book can sit in several series; authors are records linked to books).
ABS behaviour was read from its server source on 2026-11-26
(behaviour only, nothing copied). Code: `crates/catalog/src/book_meta.rs`
(`folder_series`, `tag_series`, the `series` field of the merge),
`crates/catalog/src/book_sync.rs` (`write_series`,
`remove_empty_series`), `crates/server/src/series.rs` (the API),
`web/src/lib/BookShelf.svelte` / `BookGrid.svelte`,
`web/src/routes/series/[id]`, `web/src/routes/author/[id]`. Migration
`20261130000000_series.sql`.

**What's stored.**

- `series (id, library_id, name, sort_name, created_at)`: one per
  library and name, the name compared without regard to case;
  `sort_name` drops a leading article, as items' `sort_title` does.
- `book_series (item_id, series_id, sequence, sort_order)`: a book's
  place in a series. `sequence` is text as the source gave it (`1`,
  `2.5`, `1a`), NULL when there was none; `sort_order` is the series'
  position in the book's own list. The primary key is
  `(item_id, series_id)`, so a book sits in a series once.
- **Authors need no table.** They are the `people` linked through
  `item_people` with role `author` (AB1, AB6's merge), so an author
  page is a query.

**Where a book's series come from.** A series is a book-level fact
like the others AB6 merges: the merge gained a `series` list, and two
of the existing sources give it. There is **no new source** in the
precedence list, and nothing AB6 already merged changed.

| source | series it gives |
|---|---|
| `folder` | the **series folder**: the folder right above the title folder, only when two or more folders sit above the title (`Author/Series/Title`), as D28 has it. `Author/Title` and a file in the library root have none. Only the three folders nearest the book count, so `Genre/Author/Series/Title` gives `Series`. No sequence (reading 2) |
| `tags` | from the **first track** (D30), ABS's tag names: the name from `series`, else `show`, else `mvnm` (MP4's movement name); the sequence from `series-part`, else `episode_id`, else `mvin`, else `part`. When the name and the sequence both hold the same number (more than one) of `;`-separated parts, they pair up into several series; otherwise it is one series with both values taken whole. Only when there is no name tag: `grouping` (else `grp1`), split on `;`, each part `Name #sequence` (the sequence is the one word after the last ` #`) or just `Name` |

- **Merge.** A list, like authors: a later source that names any
  series replaces the book's list whole; one that names none says
  nothing. With the default order, a book's series tags therefore
  replace its series folder. A source left out of a library's
  precedence list isn't read, so its series aren't either.
- **Clean-up.** Names and sequences are trimmed; a blank name is
  dropped; a name repeated in one book's list (without regard to
  case) is kept once, the first time.

**A re-scan stays idempotent.** A book's series links are rebuilt with
its other merged fields, whenever AB6 re-merges it (new, changed or
missing tracks, a changed metadata file, a changed precedence list):
its links are deleted and written again from the merge, each series
found by library and name or created. A re-scan therefore never
duplicates a series or a link, and a book that isn't merged again
keeps its links. The merge rules' version went to `ab7.1`, so the
first scan after this change merges every book once (and probes it,
as AB6's own first scan did) to fill in its series. At the end of
every scan of a library, a series no book links to any more is
removed. A book whose files went missing keeps its links, as it keeps
its progress, and is shown unavailable on its series page.

**API.** All under `/api/v1`, cookie session, scoped to the active
profile's library access. A series or author the profile can't reach
reads as 404.

| Method + path | What |
|---|---|
| `GET /series?library_id=&q=` | Series the profile can reach, by `sort_name`: `id`, `library_id`, `name`, `sort_name`, `book_count`, `finished_count` (this profile's finished books in it), `cover_book_id` (the first book in sequence order that has a cover) |
| `GET /series/{id}` | The above plus `books`: the book list's shape (AB3) in **sequence order** |
| `GET /authors?library_id=&q=` | People credited as author on an audiobook the profile can reach, by name: `id` (the person), `name`, `book_count` |
| `GET /authors/{id}?library_id=` | The above plus `books` (by title) and `series` (the series those books sit in, each with how many of the author's books it holds) |

The book list and detail (`GET /audiobooks`, `GET /audiobooks/{id}`)
gained `series` (`id`, `name`, `sequence`, in the book's own order)
and `author_refs` (`id`, `name`, in the order of `authors`), and the
book list's `q` now matches series names too.

**Sequence order** is ABS's: the sequence read as a number (its
leading number, so `1a` reads as 1; text with no leading number reads
as 0, as ABS's cast does), books with no sequence last, then by title.

**Web.** An audiobooks library's page has three views, **Books**,
**Series** and **Authors** (`?view=series`, `?view=authors`), each with
a search. A series tile shows a cover, the book count and how many
this profile has finished; `/series/[id]` lists the series' books in
sequence order, each marked "Book N", with links to its authors.
`/author/[id]` lists the author's series and books. On `/book/[id]`
the authors link to their pages, and each series reads "Book N of
Series", linked.

**Tests.** Unit (`book_meta`): the series folder, ABS's series tags
(fallbacks, pairing, `grouping`), `Name #sequence` parsing, a later
source replacing the list. Flow: `crates/server/tests/series_flow.rs`
scans a library of ffmpeg-made books. It covers series from the folder
and from tags, a name differing only in case, one book in two series,
sequence order, the author pages, search, library access, a missing
book, a precedence change, empty series removed, and re-scans that
duplicate nothing.

#### Readings made in AB7 — for the owner's veto

Each is a reading inside D28 (and D30's merge), not a new decision.

1. **Series ride in the existing `folder` and `tags` sources.** They
   don't get a precedence entry of their own, and a later source's
   series replace an earlier one's whole, as ABS does and as AB6
   merges authors.
2. **A folder series has no sequence yet.** ABS reads the sequence from
   the title folder's name (`Book 2 - Title`, `Vol 1 -`, `1.`) when
   there is a series folder. That is title-folder parsing, AB11's
   follow-up item 3. Until then, a book whose series is known only
   from its folder has no sequence, so it comes after the sequenced
   books, by title.
3. **Only the folder and the tags give series.** ABS also reads them
   from an OPF (`calibre:series`) and from its `metadata.json`.
   Parlour reads neither yet (a follow-up, if wanted). Parlour's
   sidecar format (§13.15 of `IMPORT-EXPORT.md`) has no series, so a
   sidecar says nothing about them. Neither match provider gives
   series either, so a match leaves them as they are.
4. **One series per library and name, without regard to case.** The
   first spelling seen is kept. ABS's unique index is on the exact
   name. Case-only variants as two series seemed the worse reading.
5. **A book's links are rebuilt whole from its sources.** ABS edits
   them in place and keeps a book's series when its sources go silent,
   because ABS lets a user edit series by hand. Parlour has no series
   editing, so the sources are the whole truth, and the links are
   rebuilt the way AB6 rebuilds a book's other merged fields.
6. **Empty series go at the end of every scan.** ABS removes series
   that lost their last book in a scan. A missing book keeps its
   links, so a series of only missing books stays, its books shown
   unavailable.
7. **Sequence order follows ABS's numeric cast**, including text with
   no leading number reading as 0 (so a `Prequel` sorts first). Books
   with no sequence come last, then ties go by title.
8. **Authors are not per library.** ABS's authors are. Parlour's
   `people` are shared across libraries (and with video cast), so an
   author page spans every audiobooks library the profile can reach,
   unless `?library_id=` narrows it (the library's Authors view
   passes it). Lists sort by name as stored (first name first). ABS's
   default author sort wasn't confirmed, and there is no last-first
   sort. There are no author photos or bios: ABS takes those from
   Audnexus, which AB6 left out.
9. **Narrators stay `people` rows with role `narrator`** (follow-up
   item 7, decided here as asked). ABS keeps a plain list of names on
   the book, and nothing a listener sees differs. Keeping the rows
   avoids a migration that would move data for no behaviour. Search
   by narrator already works, a narrator filter would be a query, and
   the export bundle already carries them through `item_people`. So
   the decision implies no change. No narrator browse view is built:
   it isn't in AB7's scope.
10. **`series` and `book_series` stay out of the export bundle**
    (`coverage.rs`: left out). They are rebuilt by the new host's scan
    from the same folders and tags, like `book_metadata`, so format v1
    is unchanged. If series ever come from a match or a hand edit, it
    becomes the same question as D40.
11. **The book list's search matches series names too**, as ABS's
    search finds series.
12. **The first scan after this change probes every book once**
    (merge rules `ab7.1`), as AB6's did. On a large library that scan
    is slow once, then unchanged books are skipped again.

### Later in Phase 10 (blocked on decisions)

AB6 metadata providers and embedded covers (D30), AB7 series and
author browsing (D28), AB8 Now Playing, Activity and a
Continue-listening rail (D29/D35), AB9 an importer for progress from
the owner's running Audiobookshelf (D27), AB10 a transcode fallback
for codecs a client can't play (CL5).

*2026-11-24:* all unblocked except AB9, which is **cancelled**
(D27: no ABS import). AB6 follows D30, AB7 follows D28, AB8 follows
D29 + D35; the details are in the list below. *2026-11-26: AB6 is
built (above). AB7 is built (above, "Series and authors (AB7)").*

### Follow-up from D28–D35 (where AB1–AB5 differ from ABS)

D28 and D29 say to mirror Audiobookshelf and to write down where the
first slice already differs. Checked against ABS's docs and server
source on 2026-11-24 (behaviour only). Each item is follow-up work,
not yet scheduled in a PR:

1. **Disc folders (AB2).** ABS folds only `CD` / `Disc` / `Disk` +
   1–3 digits directly inside the book folder. AB2 also folds
   `Part N`. Drop `Part` from `disc_folder_number`, so a `Part 1`
   folder is its own folder, as ABS reads it. Check the owner's
   real library for `Part` folders first: the change moves any
   such book's identity (`stable_key`).
2. **Series from the folder (AB2/AB7).** ABS reads
   `Author/Series/Title`'s middle folder as the series. AB2 ignores
   it. AB7 adds `series` records and a book ↔ series join with a
   sequence on the link (a book can be in several series), filled
   from that folder and from tags. *Done in AB7: the series folder
   and ABS's series tags; the sequence from the title folder's name
   waits for item 3.*
3. **Title-folder parsing (AB2).** ABS parses the title folder for
   an ASIN `[…]`, a narrator `{…}`, a series sequence (`Vol 1 -`,
   `Book 2 -`, `1.`, decimals), a publish year (`1994 -`,
   `(1994) -`), and a subtitle after ` - ` (subtitle only behind a
   setting, off by default). AB2 uses the folder name raw.
4. **Which folders count (AB2).** ABS looks only at the three
   folders nearest the book: title, series, author. AB2 takes the
   *top-level* folder as the author fallback, so a deeper layout
   (`Genre/Author/Title`) would credit "Genre". Use the nearest
   author position instead.
5. **Author splitting (AB2).** ABS splits folder-name authors on
   `,` `;` `&` and `and`, and accepts `Last, First`. AB2's
   `split_people` splits on `;` `&` ` / ` and on commas only when
   every part has a space. Match ABS for folder names; keep tag
   splitting under review in AB6. *Folder names: done in AB6. Tag
   splitting kept as AB2 has it (AB6 reading 9).*
6. **Which track's tags (AB2/AB6).** ABS reads tags from the first
   track in disc/track order. AB2 takes each field from the first
   track that *has* it. Small difference; align in AB6 together
   with the precedence list (D30). *Done in AB6.*
7. **Narrators (AB1).** ABS stores narrators as a plain list of
   names on the book; AB1 stores them as `people` with role
   `narrator`. The browsing behaviour (list and filter by narrator)
   is the same either way. Decide in AB7 whether to keep `people`
   rows; nothing user-visible depends on it. *Decided in AB7: keep the
   `people` rows (AB7 reading 9); nothing changes.*
8. **Auto-finish (AB4).** ABS marks finished when the remaining time
   drops below a per-library threshold (default 10 s), or at a
   per-library percentage when set (off by default). AB4 finishes
   only when a client says so. Add the server-side rule with those
   library settings; keep manual mark finished / not finished, and
   reset the position to 0 on a manual un-finish, as ABS does.
9. **Hide from Continue Listening (AB4/AB8).** ABS keeps a per-user,
   per-item hide flag that clears when progress next changes. Add
   it to `book_progress` and the API.
10. **Client timestamp (AB4/CL4).** ABS keeps last-arrival-wins but
    stores the client's own last-update time. AB4 stamps the
    server's time. Accept and store the client's.
11. **Podcast progress shares the table (POD1).** ABS keeps one
    progress model for books and episodes; POD1 puts episode
    progress in the same table and shape rather than a parallel
    one (a `kind` column or a generalised name, decided in POD1).
12. **Metadata precedence and match (AB6, D30).** Per-library
    precedence, ABS's order (folder → tags → NFO → `desc.txt` /
    `reader.txt` → OPF → sidecar, later wins), with Parlour's
    `.parlour.json` (D14) in the sidecar slot and ABS's
    `metadata.json` read as an input; manual match; embedded covers
    when the folder has no image. Before building library-wide
    quick match, confirm ABS's default for "overwrite vs fill gaps"
    (not confirmed on 2026-11-24). *Done in AB6. ABS's default was
    confirmed as fill gaps on 2026-11-26.*
13. **No play count (AB8).** ABS keeps no play count; listens are
    sessions. AB8 must not bump `play_count` for books.

---

## Phase 11 — Podcasts (plan only; no code yet)

The server owns the feeds. Clients never fetch RSS themselves, which
is what lets a Fire Stick, a tablet and the phone agree.

### Decided 2026-11-24: mirror Audiobookshelf's podcast model (D31, D32)

This supersedes the "to be confirmed" parts of the original sketch
kept below it.

- **Podcasts are a library.** A `podcasts` library (the kind already
  exists) holds shows; profiles reach them through the existing
  per-library access grants. **No per-profile subscriptions**:
  everyone with access sees every show in the library, as in ABS.
  The sketch's "Subscriptions" table is dropped. Progress is per
  profile per episode, in the book progress table (follow-up 11).
- **Only admins add shows** — by searching Apple's (iTunes)
  directory (no key), by RSS URL, or by OPML import (admin-only);
  OPML export too. POD7 becomes Apple search, not Podcast Index.
- **The server downloads episodes, and only downloaded episodes
  play** (ABS is not a pass-through). The "redirect to the
  enclosure if it isn't downloaded" idea is dropped. An episode not
  downloaded shows in the feed list and can be downloaded by hand.
- **Per-show settings**, with ABS's defaults: auto-download new
  episodes (**off**), check schedule (cron, **hourly**), max
  episodes to keep (**0 = all**; when over, delete the oldest, at
  most one per check, regardless of who has heard it), max new
  episodes to download per check (**3**). A show's schedule turns
  itself off after **24** failed checks in a row (with the
  Dashboard notice the sketch already planned). No disk cap.
- **Where episodes go.** ABS writes into the show's folder inside
  the library folder. Parlour's media mounts are read-only, so a
  podcasts library's folder is a **writable folder Parlour owns**,
  set up as an explicit ops step like XFER5's sidecar opt-in, never
  a flipped video/book mount. Which disk on sullivan holds it has
  no ABS equivalent and waits for the owner; it must be answered
  before POD3 runs in production.
- **No ABS equivalent — deferred until Jordan asks:** a
  per-profile follow/hide list (so a show one profile ignores stops
  counting as current for it), showing transcripts (POD2 still
  stores the link), and server-side Smart Speed / Voice Boost.

**Original shape sketch (2026-11-23, to be confirmed by D31/D32):**

- `podcasts` — feed URL, title, author, description, artwork,
  `etag` / `last_modified`, refresh state and the last error.
- `podcast_episodes` — `guid` (the feed's identity, falling back to
  the enclosure URL), title, published-at, duration, enclosure URL /
  type / size, description, and a local file once downloaded.
- Subscriptions — who follows what (per profile, or server-wide
  with per-profile following; **D31**).
- Episode progress — the same shape as `book_progress` (position,
  finished, last listened), so clients reuse one sync path.

**Fetching.** A `refresh_podcasts` job on the existing Postgres
queue (no new scheduler) uses a conditional GET (ETag /
If-Modified-Since). The RSS 2.0 + iTunes namespace parse is a pure
function, fixture-tested like ffprobe parsing. The Podcasting 2.0
`podcast:chapters` / `podcast:transcript` links are stored as URLs.
Feed errors raise a Dashboard notice after N failures in a row.
Cadence: **D32**.

**Episodes.** Whether the server downloads audio or clients stream
from the publisher, how many it keeps, and **where** they go are
all **D32**. The constraint, whatever the answer: media mounts are
read-only (AGENTS.md), so downloaded episodes need a writable store
of their own (like the artwork and cache dirs), never a flipped
media mount.

**Interface.** Admin or user UI (who may add feeds is **D31**):
add by URL, import/export OPML, refresh now, per-feed settings,
episode lists, and the shared audio player from AB5.

**API (sketch, mirrors Phase 10):** `GET /podcasts`, `POST
/podcasts` `{feed_url}`, `GET /podcasts/{id}` with episodes, `GET
/episodes/{id}/stream` (Range over the local file, or a redirect to
the enclosure if it isn't downloaded, per D32), `GET|PUT|DELETE
/episodes/{id}/progress`, `GET /podcasts/opml`, and the bulk sync
in Phase 12.

---

## Phase 12 — Audio clients: the sync contract (API design only)

The iOS app is still Phase 6 and Android/Fire TV is Phase 3. This
phase fixes the **server API** those apps will sync against, so the
audiobook and podcast endpoints don't need a v2 the day a native
client lands. Client code is out of scope here.

### Client scope, decided 2026-11-24 (D26, D33–D35)

- **Parlour's own clients only** (D26): the web UI and Parlour's
  mobile apps. No ABS-compat surface, so the API below is the only
  one a client uses. Audiobooks and podcasts live in the same app
  (as ABS's app handles both). The screens are specified in
  `UI-AUDIO-REFERENCE.md`: audiobooks modelled on Audiobooth,
  podcasts on Overcast.
- **Offline is in** (D33): the mobile app downloads books and
  episodes, so CL4 goes ahead. Progress pushed later is last
  arrival wins with the client's own timestamp stored, as ABS does.
- **What syncs** (D35): progress + finished + the hide flag,
  **bookmarks** (books), **listening sessions and stats** (CL3),
  **playlists** (per profile per library, ordered) and
  **collections** (per library, shared). The CL2 sync feed carries
  those, plus shows and new episodes from the libraries a profile
  can reach, instead of "subscriptions". **On the device only:** the
  Up Next queue (POD8's synced queue is replaced by playlists),
  playback speed (per book or per show), sleep timer, skip
  intervals.
- **No ABS equivalent — deferred until Jordan asks** (D34/D33/D35):
  one app with video or a separate audio app; iOS before Android
  for audio (D12's order stands meanwhile); audio on the Fire
  Stick; a tablet-specific layout; Smart Speed / Voice Boost.

The original text of this phase follows; where it says "only if
D33/D35 …", read the decisions above.

**Reference experiences** (behaviour only; clean-room rule above):

- *Audiobooks, like the Audiobookshelf client:* library browsing by
  title, author and series; a book page with chapters; a player
  with a chapter scrubber, speed control, sleep timer and bookmarks;
  progress following the owner between phone, tablet and web;
  downloaded books playing offline and syncing back.
- *Podcasts, like Overcast:* a subscription list; per-show episode
  lists; an Up Next queue; per-show settings (speed, how many to
  keep); Smart-Speed / Voice-Boost-style processing. **Those are
  on-device DSP in Overcast.** Parlour's server serves original
  audio, and doing that processing server-side would cost a
  transcode per listen (D35).

**What the server API needs** (CL tasks in TODO):

1. **Device auth for native apps** (CL1): today's auth is a 30-day
   HttpOnly cookie. Native clients get a long-lived, revocable
   device token (`Authorization: Bearer`), listed under Settings →
   Devices. It is stored hashed, as sessions are (D7).
2. **One sync endpoint** (CL2): `GET /api/v1/sync?cursor=` returns
   every change since the cursor: book and episode progress,
   subscriptions, new episodes, deletions (tombstones), and a new
   cursor. A client that was offline for a week catches up in one
   call. The per-resource endpoints of Phases 10–11 stay for simple
   clients and the web.
3. **Listening sessions** (CL3): start/progress/stop for audio, so
   Now Playing and Activity show listeners like viewers, with
   listening-time totals. Whether to keep stats is part of D35.
4. **Offline** (CL4, only if D33 says yes): a download manifest per
   book or episode (files, sizes, hashes), and replay of progress
   recorded offline, carrying the client's own timestamps under the
   D33 conflict rule.
5. **Audio capability profiles** (CL5): which codecs AVPlayer,
   ExoPlayer (Fire OS) and browsers play directly, so the server can
   fall back to an AAC stream only when it must. This is the same
   explicit-profile rule as video (ARCHITECTURE §4).
6. **Contract doc** (CL6): `docs/API-AUDIO.md`, with request and
   response examples, written when CL2 lands.

What a client stores on its own (speed per book, sleep timer, skip
intervals) and what syncs through the server is **D35**.

---

## Interplay with other phases

- **Export bundle (Phase 8).** The new tables (`book_chapters`,
  `book_progress`, and later the podcast tables) are tables D14
  doesn't list. Whether and how the bundle carries them follows the
  owner's still-open **D25** for XFER1. Nothing in Phases 10–12
  decides it.
  *2026-11-24:* **D25 is decided** (recent searches and the Activity
  feed are in the bundle; hashes may sit in a plain zip), which
  **unblocks the XFER chain** — XFER1 (the bundle spec) can start,
  then XFER2–XFER8 in order. No XFER work is started by this
  update. D25 did not cover the audio tables. By D14's split,
  `book_progress` is per-profile state like watch history, and
  `book_chapters` is rebuilt by every scan, so the natural XFER1
  proposal is to carry the former under `user_data` and skip the
  latter. That is XFER1's to propose and the owner's to confirm,
  not decided here.
  *2026-11-24 (XFER1):* the proposal is written up in
  `docs/IMPORT-EXPORT.md` §9, with the exact shape
  (`user_data.json` → each profile's `books[]`), and waits for the
  owner's yes; format v1 carries neither table until then.
  Audiobook *items* are ordinary catalog rows and are already in
  the bundle's `catalog.json`.
- **Sidecars (Phase 8, D14).** ABS-style `metadata.json` /
  `desc.txt` / `reader.txt` next to books are a possible metadata
  input (D30). Writing sidecars into audiobook folders needs the
  same per-library opt-in as video.
- **Compat (Phase 5).** X2 is reframed by D26. If built, it is a
  view over Phase 10's tables and never owns data (ARCHITECTURE §10).
  *2026-11-24: D26 — X2 is cancelled.*
- **Client order (D12).** Whether audio pulls the iOS app ahead of
  Android is **D34**. *2026-11-24: no ABS equivalent; deferred until
  the owner asks. D12's order stands.*
