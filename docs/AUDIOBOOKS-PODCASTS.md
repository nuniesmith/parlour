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
  the earlier discs' files. *AB11: only `CD` / `Disc` / `Disk`,
  optional whitespace and 1–3 digits, as ABS reads them; `Part 01` is
  a folder of its own ("Aligning with Audiobookshelf (AB11)").*
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
| authors | `album_artist`, else `artist`; else the top-level folder when the book sits in one (`Author/Title`, `Author/Series/Title`). *AB11: the nearest author folder instead* |
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
  *POD1 (2026-11-26): the same table also holds podcast-episode
  progress (D29). A row has `item_id` (a book) or `episode_id` (an
  episode), never both; the primary key became `UNIQUE (profile_id,
  item_id)`. Book queries are unchanged ("Schema (POD1)").*
- `position_ms` is **book-global**: milliseconds from the start of
  track 0. Clients map it to (track, offset) with the track list's
  `start_ms` values. One number per book is what makes cross-device
  sync simple.
- **Finished is explicit:** a client sets `finished: true` (the web
  player does it when the last track ends). There is deliberately
  no percentage rule: when to auto-finish is part of D29.
  *AB11: ABS's rule — the library's auto-finish threshold (10 s, or a
  percentage), un-finishing by hand resets the position to 0, plus
  the hide flag ("Aligning with Audiobookshelf (AB11)").*
- Writes are **last-write-wins** in arrival order, as video
  reporting is today. The rule for offline clients is part of D33.
  *AB11: the client's own timestamp is stored too
  (`client_updated_at`); last arrival still wins.*

### Native API (AB3/AB4, built)

All under `/api/v1`, cookie session, scoped to the **active
profile's** library access (no access reads as 404, like items).

| Method + path | What |
|---|---|
| `GET /audiobooks?library_id=&q=&sort=title\|added\|recent` | Books the profile can reach, with authors, narrators, duration, track and chapter counts, cover flag, and this profile's progress inline |
| `GET /audiobooks/{id}?client=` | One book: the above plus `tracks` (index, file id, title, `start_ms`, `duration_ms`, codec, size, a tokenised `stream_url`) and `chapters`. *AB10:* with `client` (a profile name) each track has a `plan`, and a track that profile can't play has the AAC fallback as its `stream_url` |
| `GET /stream/{file_id}/aac.m4a` | *AB10:* a book track as AAC in MP4, with Range; 503 + `Retry-After` until it is made; same token as the file |
| `GET /audiobooks/{id}/tracks/{index}/stream` | Track audio with HTTP Range (single range, 206/416), authorised by stream token or session, the same rules as `/stream/{file_id}` |
| `GET /audiobooks/{id}/progress` | This profile's progress (zeros when never started) |
| `PUT /audiobooks/{id}/progress` | `{position_ms?, finished?, hidden?, client_updated_at?}` (the last two AB11); position is clamped to the book's length; returns the stored row |
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
| `folder` | the book folder's name, and the top-level folder above it (*AB11: the nearest author folder, and the title folder's name parsed*) | title (a root file's name without its extension); authors, split ABS's way (below); *AB11: subtitle, narrators, year, ASIN and the series sequence from the title folder* |
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
   (AB7)"*). *AB11 built both: "Aligning with Audiobookshelf
   (AB11)".*
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
| `folder` | the **series folder**: the folder right above the title folder, only when two or more folders sit above the title (`Author/Series/Title`), as D28 has it. `Author/Title` and a file in the library root have none. Only the three folders nearest the book count, so `Genre/Author/Series/Title` gives `Series`. No sequence (reading 2); *AB11: the sequence the title folder's name gives* |
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
   books, by title. *AB11 built it: a folder series takes the sequence
   the title folder's name gives.*
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

### Aligning with Audiobookshelf (AB11, built 2026-11-26)

Built to **D28** (the catalog model and the on-disk rules), **D29**
(finishing, the hide flag) and **D33** (the client's timestamp): the
"Follow-up from D28–D35" items that were still open after AB6 and AB7,
items 1, 3, 4, 8, 9 and 10. ABS behaviour was read from its server
source on 2026-11-26 (`scandir.js`, `MediaProgress`, `Library`,
`User`, `PlaybackSessionManager`, `MeController`; behaviour only,
nothing copied). Code: `crates/catalog/src/audiobooks.rs`
(`disc_folder_number`), `crates/catalog/src/book_meta.rs`
(`parse_title_folder`, `folder_fields`), `crates/server/src/audiobooks.rs`
(`apply_write`, `reaches_finish`), `crates/server/src/book_settings.rs`
(the library settings), `web/src/routes/admin/library`,
`web/src/routes/book/[id]`. Migration
`20261201000000_ab11_align_abs.sql` (the timestamp only orders it after
AB7's; written 2026-11-26).

**Disc folders (item 1).** A folder folds into the book above it only
when its whole name is `CD`, `Disc` or `Disk` (any case), optional
whitespace, and 1–3 digits: `CD 1`, `cd1`, `Disc 02`, `DISK 123`.
`Part 1` is a folder of its own, as ABS reads it, and so are `Disc-1`,
`Disc_1` and `CD 1234`, which AB2 also folded. Such a folder holding
audio is now a book of its own (`Author/Book/Part 1` is the book
`Part 1`, in the series `Book`, by `Author`). A book's identity is its
folder (`stable_key`), so **a book AB2 built from `Part` (or
`Disc-1` / `Disc_1`) folders becomes one new book per folder** at the
next scan. The old book is marked missing, not deleted, and keeps its
progress, which the new books don't inherit.

**The title folder's name (item 3).** The `folder` source parses it in
ABS's order:

1. an **ASIN** in square brackets, exactly ten capital letters or
   digits, standing at the start, at the end or between spaces:
   `Title [B0015T963C]`;
2. a **narrator** in braces at the very end, after a space:
   `Title {Jim Dale}` (several split as folder authors are);
3. a **series sequence**, only when the book has a series folder: the
   name is split on ` - `, and the first part that reads as a sequence
   is taken out. A part reads as one when it is an optional `Vol `,
   `Vol. `, `Volume ` or `Book ` (any case), up to three digits with an
   optional decimal of one or two digits, an optional trailing dot, and
   then either nothing or a space and more text. More text counts only
   after a label or a trailing dot: `Book 2 - Title`, `1. Title`,
   `1 - Title`, `0.5 - Title`, `Vol. 3 Title` are sequences;
   `101 Dalmatians` and `1984` are titles;
4. a **publish year** at the very start: four digits, optionally in
   parentheses, then a hyphen: `1994 - Title`, `(1994) - Title`,
   `1994-Title`. `Title (2001)` and `2001 A Space Odyssey` keep their
   numbers;
5. a **subtitle** after the first ` - `, only when the library's
   **"parse subtitles"** setting is on (off by default):
   `Title - Subtitle`.

So `Vol 2 - 1999 - The Middle - A Subtitle {Ann Reader} [B000000002]`
in a series folder, with subtitles on, is the title `The Middle`,
subtitle `A Subtitle`, book 2 of the series, 1999, read by Ann Reader,
ASIN `B000000002`. When nothing is left for a title, the name is the
title untouched. The values join the `folder` source, so the later
sources in the precedence list (tags, NFO, …) replace them as they
replace any folder value. **A folder series now has its sequence**
(AB7 reading 2): it is the one the title folder gave.

**Which folders count (item 4).** Only the three folders nearest the
book: the title, then — with two or more folders above it — the series
right above and the author above that; with one folder above, that one
is the author. Folders higher up are ignored. AB2 took the *top-level*
folder as the author, so `Genre/Author/Series/Title` credited `Genre`;
it now credits `Author`. As in ABS, `Genre/Author/Title` still reads
`Genre` as the author and `Author` as the series: with three folders
the middle one is the series position (AB7 already read it so).

**Library settings.** An audiobooks library has three settings of its
own, ABS's, beside the metadata precedence (AB6): `GET | PUT
/api/v1/admin/libraries/{id}/audiobook-settings` with
`{"parse_subtitles", "finish_remaining_seconds", "finish_percent"}`
(all three on a PUT; an audiobooks library only; admins only), edited
on Server → Library → "Audiobook settings".

| setting | default | what |
|---|---|---|
| `parse_subtitles` | off | a title folder's ` - Subtitle` is its subtitle. Part of every book's sources fingerprint, so a change merges the library's books again at the next scan |
| `finish_remaining_seconds` | 10 | finished once less than this much of the book is left; 0 is off; at most a day |
| `finish_percent` | off (null) | finished once the position is past this share of the book (1–100); when set it **replaces** the time rule |

**Finishing (item 8).** On every progress write (`PUT
/audiobooks/{id}/progress`):

- a write **with a position**, on a book whose length is known, is
  checked against the library's rule: less than
  `finish_remaining_seconds` left (strictly less, so exactly 10 s left
  is not finished), or with `finish_percent` set, a position strictly
  past that share. Reaching it finishes an unfinished book
  (`finished_at` is set). A finished book moved back **before** the
  point is unfinished again, as ABS does; one whose position didn't
  move stays finished. A write without a position leaves `finished`
  alone, and a book of unknown length never finishes by itself;
- **finishing by hand** (`finished: true`) finishes the book and leaves
  the position where it is (or where the same write puts it);
- **un-finishing by hand** (`finished: false` on a finished book) puts
  the position back to **0**, as ABS does, and ignores a position sent
  with it. The web player follows: "Mark as not finished" starts the
  book over;
- a `finished` the client sends always wins over the rule, both ways.

**Hide from Continue Listening (item 9).** `book_progress.hidden`, per
profile and book. `PUT …/progress` with `{"hidden": true}` hides the
book, `{"hidden": false}` shows it again, and neither moves anything
else. The next write that **moves the position** clears it (unless
that write sends `hidden: true` itself); finishing alone doesn't, and
un-finishing by hand does, because it moves the position to 0. Every
progress shape (`progress` in the book list and detail, the single
GET/PUT, the bulk sync rows) carries `hidden`. AB8's Continue
Listening shelf reads it; there is no web control before that shelf.

**The client's timestamp (item 10).** A write may send
`client_updated_at` (RFC 3339): when the client made the update, by its
own clock. It is stored as sent, in `book_progress.client_updated_at`,
and returned everywhere progress is; a write without one stores the
server's time there. The **last write to arrive still wins**: nothing
compares times or positions (D33). `updated_at` stays the server's
time, so the bulk sync's `?since=` cursor can't miss a write from a
client whose clock is behind. A time that doesn't parse is refused
(4xx) and nothing is stored. The web player sends its time with every
write.

**The bundle.** Format v1 is unchanged. The three library settings
stay behind (`coverage.rs`: an imported library starts with ABS's
defaults), and `hidden` / `client_updated_at` are part of
`book_progress`, which stays the open proposal of §9 of
`IMPORT-EXPORT.md` (D36): not carried.

**A re-scan.** The merge rules' version is now `ab11.1`, so the first
scan after this change merges (and probes) every book once, then
unchanged books are skipped again.

**Tests.** Unit (`audiobooks`, `book_meta`): disc folders and
grouping, ABS's worked title-folder examples (ASIN, narrator,
sequences with and without a series folder, years, subtitles on and
off, `101 Dalmatians`, `1984`), the nearest folders in deep layouts,
the fingerprint with the subtitle setting. Unit (`server`): the
finishing point (strict comparisons, the percentage replacing the
time, unknown length), every progress rule above, and the hide flag.
Flow: `crates/server/tests/abs_alignment_flow.rs` scans ffmpeg-made
books with no tags: `Part` folders as books and `Disc` folders folded,
a parsed title folder in `Genre/Author/Series/Title`, a folder series
in sequence order, the settings API and its checks, subtitles turned
on (one re-merge, then nothing), auto-finish at exactly the threshold
and with a percentage, both hand rules, the hide flag through moves and
marks, the client's time stored and the sync cursor unaffected.
`audiobook_flow.rs` was updated where AB4's explicit-only rules
changed.

#### Readings made in AB11 — for the owner's veto

Each is a reading inside D28, D29 or D33, not a new decision.

1. **Disc folders are exactly ABS's**, not just "without `Part`":
   only whitespace may sit between the word and the digits, and only
   1–3 digits, so `Disc-1`, `Disc_1` and `CD 0001` stop folding too
   (D28 says "only"). Each such book moves identity, as a `Part`
   book does.
2. **A file in a library root keeps its name unparsed**, as AB2 had
   it. Only the title *folder* is parsed; whether ABS parses a root
   file's name the same way wasn't checked.
3. **"Parse subtitles" is a per-library setting.** ABS keeps it as one
   server-wide scanner setting; Parlour follows its own per-library
   settings pattern (the precedence list is per library too), with the
   same default, off. On a server with one audiobooks library the two
   read the same. (ABS's off default comes from D28; its settings file
   wasn't readable on 2026-11-26 to check it again.)
4. **What the title folder gives is part of the `folder` source**, so
   tags and the later sources replace it field by field, with the
   default order. An admin who wants the folder's ASIN or narrator to
   win moves `folder` later in the precedence list.
5. **When parsing leaves no title**, the name stays the title
   (`Book 3` in a series folder is the title `Book 3`, sequence 3). ABS
   on that edge wasn't checked.
6. **The rule runs on every progress write with a position.** ABS
   applies its library settings only when a playback session syncs (a
   plain progress update falls back to 10 s or to values the client
   sends), and doesn't check a brand-new progress record at all.
   Parlour has no listening sessions yet (CL3), so its one write path
   uses the library's settings, and a first write can finish a book.
7. **A `finished` the client sends wins over the rule.** In ABS, a
   hand finish sent together with a position before the point is
   undone by the same write; here it stands.
8. **A finished book moved back before the point is unfinished** (as
   ABS does), reversing AB4's "a new position does not un-finish". So
   re-listening a finished book un-finishes it on its first saved
   position. `finish_percent` 100 is accepted and, with ABS's strict
   comparison, never finishes a book.
9. **Hiding goes through `PUT …/progress`** (`hidden`), with no
   separate endpoint, and showing again is `hidden: false`. ABS hides
   by its progress record's id, so a book with no progress can't be
   hidden there; here hiding a never-started book stores a progress
   row at 0 with the flag.
10. **The client's time has its own column.** ABS overwrites its
    record's update time with the client's; Parlour keeps `updated_at`
    as the server's time (the sync cursor) and stores the client's in
    `client_updated_at`, on new rows too (ABS ignores it on a new
    record). Nothing checks it against the server's clock, as in ABS;
    an unparseable one is refused here, where ABS logs it and uses its
    own time. `last_listened_at` and `sort=recent` stay on the server's
    time.
11. **The new settings and columns stay out of the bundle** (format v1
    unchanged): library settings start at ABS's defaults on an imported
    library, and the `book_progress` columns follow the table's open
    proposal (D36).
12. **The first scan after this change probes every book once**
    (merge rules `ab11.1`), as AB6's and AB7's did.

### Listening sessions and Home shelves (AB8, built 2026-11-26)

Built to **D29** (a listen is a session, no play count; sessions feed
Now Playing and the Activity feed; per-profile stats; ABS's book
shelves with the per-item hide) and **D35** (sessions and stats are
kept by the server). ABS behaviour was read from its server source on
2026-11-26 (`libraryFilters.js`, `libraryItemsBookFilters.js`,
`LibraryController.js`; behaviour only, nothing copied). Screens follow
`UI-AUDIO-REFERENCE.md` §4.2–§4.4. Code: `crates/server/src/listening.rs`
(sessions, history, stats), `crates/server/src/shelves.rs` (the
shelves), the audiobook branch of the playback reports in
`crates/server/src/playback.rs`, `HOME_RAILS` in `profiles.rs`;
web `lib/AudioShelf.svelte`, `lib/BookShelf.svelte` (the library's Home
view), `lib/ListeningHistory.svelte`, `routes/+page.svelte`,
`routes/book/[id]`, `routes/settings/listening`, `lib/NowPlaying.svelte`.
Migration `20261202000000_ab8_listening_sessions.sql` (the timestamp
only orders it after AB11's; written 2026-11-26).

**Sessions, through the video machinery.** A book's listening session
goes through the same three reports video uses —
`POST /api/v1/playback/{start,progress,stopped}` with the book's id as
`item_id` and book-global positions — and sits in the same live
registry, so **Now Playing** (`GET /api/v1/admin/sessions`) lists it
beside the viewing sessions (`item_kind: "audiobook"`, no stream plan;
*AB10: a plan when the book page named its profile*)
and the admin **Stop** works on it. The video session shape carries
audio without breaking video; what it can't carry (listening time, a
record that outlives the process) is in the new `listening_sessions`
table, one row per session, per profile:

- **start** opens a session at the player's position. Any session the
  same profile still has open **on the same book** closes first
  (`replaced`). No `user_data` row is written and `play_count` is never
  touched (follow-up item 13), and there is no Activity line yet;
- **progress** moves the position and adds listening time: what the
  player says it listened since its previous report (`listened_ms`,
  pauses excluded), or, when it sends none, how far the position moved
  forward — either way never more than the wall-clock time since the
  previous report plus 5 s. A report whose session was closed meanwhile
  opens a fresh one and answers with its id (`{"session_id"}`), so the
  player carries on in the new one;
- **stopped** closes it where the player left it (`stopped`);
- a session silent for **an hour** is closed at its last report
  (`timeout`) by the server's 15-minute tick; an admin stop closes it
  at once (`admin`) and revokes the stream tokens of **every track** of
  the book, so the next track is refused too.

The book's position is still written by `PUT /audiobooks/{id}/progress`
(AB4/AB11, unchanged): the session is the record of a listen, the
progress row is where the book is. The web player sends both: a
session report every 15 s while the book page is open (paused or not,
as the video player does, so the session stays in Now Playing), on a
pause, when the book ends, and a `keepalive` stop when the page goes
away.

**Activity.** A closed session — stopped, replaced, timed out or
stopped by an admin — writes one `playback.listened` line, in the
Playback category: "jordan listened to *Book* for 18 min", with the
session id, times, start and end positions, listening time and how it
ended in `detail`. Opening a session writes nothing. An admin stop also
writes the usual `playback.stopped_by_admin` line.

**The profile's own sessions and stats.**

| Method + path | What |
|---|---|
| `GET /api/v1/listening/sessions?before=&limit=` | This profile's sessions, newest first, open ones included; `live` while the player is still reporting (90 s, the viewing sessions' rule). Paged by `started_at` (`next_before`) |
| `GET /api/v1/listening/stats?tz=` | Totals from the same sessions: all time, today, sessions, days listened, per day (newest first) and per book (most first). A session counts on the day it **started**, in the IANA time zone given (UTC by default; an unknown zone is a 400) |
| `GET /api/v1/admin/listening/sessions?profile_id=&before=&limit=` | Every profile's sessions, for admins |

The profile's views only show books it can still reach (A6); the
admin's list shows everything. The web: **Preferences → Listening**
(the profile's "Listening now", stats and history) and **Server →
Sessions** (a listening-sessions table under Now Playing).

**Home shelves.** `GET /api/v1/audiobooks/home?library_id=` returns
ABS's seven book shelves for the active profile, over every audiobooks
library it can reach or the one named, at most **10** each (ABS's
default), books in the book list's shape (progress inline), series and
authors in AB7's shapes:

| shelf | what qualifies (as ABS) | order |
|---|---|---|
| Continue Listening | a position past 0, not finished, **not hidden** | progress last updated, newest first |
| Continue Series | per series: at least one book finished, none in progress (not finished with a position past 0 — hidden or not), one still unfinished; the book shown is the **first unfinished** in sequence order | the series' most recent progress update, newest first |
| Recently Added | books added in the last **60 days** | newest first |
| Recent Series | series created in the last **60 days**, with at least one book | newest first |
| Discover | books not finished with no position, that stand alone or are the **first book** of a series the profile hasn't started | random each time |
| Listen Again | finished books | progress last updated, newest first |
| Newest Authors | authors whose first book here arrived in the last **60 days** | newest first |

**Where they show.** On **Home**, each shelf is a Home Screen rail
(`continue_listening`, `continue_series`, `recently_added_books`,
`recent_series`, `discover_books`, `listen_again`, `newest_authors`),
after the video rails by default, ordered and hidden per profile in
**Preferences → Home Screen** (UI7) — except **Continue Listening,
which can be moved but not hidden** (a hide is refused with a 400, and
a stored one is ignored). An audiobooks library's page now opens on a
**Home** view (before Books, Series, Authors) with the same shelves for
that library alone, in the same order. Empty shelves aren't drawn.

**The hide control.** A Continue Listening card shows the percent
listened and the time left, and a "⋯" menu: **Hide from Continue
listening** (`PUT …/progress {"hidden": true}`) and **Mark as finished**.
The book page of a hidden book says so and offers **Show it there
again** (`{"hidden": false}`); the next position move clears the flag
anyway (AB11). AB11's write rules are untouched.

**The bundle.** `listening_sessions` is *Proposed* in `coverage.rs`, not
carried: per-profile state like `book_progress`, so it follows D36's
open answer and a format change (`IMPORT-EXPORT.md` §8–§9). The
`playback.listened` Activity lines travel with the Activity feed (D25).
The new Home Screen rail ids travel inside the profile's existing rail
preferences, so format v1 is unchanged.

**Not here.** The native-client side is still CL3/CL6: device tokens
(CL1), sessions a client recorded offline and uploads later, and the
documented contract. Bookmarks, playlists and collections (D35) are
CL2/POD work; per-user hidden series (ABS's "hide from Continue
Series") and the library settings `onlyShowLaterBooksInContinueSeries`
/ `hideSingleBookSeries` aren't built.

**Tests.** Unit: the listening-time cap, its phrase, the Home rails
(Continue Listening never hidden). Flow:
`crates/server/tests/listening_flow.rs` scans ffmpeg-made books and
drives the reports the web player sends — no `user_data` row, the
session in Now Playing and in the profile's list, the clock cap, the
stop into Activity (and a second stop adding nothing), a start
replacing an open session, an admin stop (409 to the player, the
track's token refused), an hour's silence closed and a fresh session
opened by the next report, stats in a time zone, paging, another
profile's empty view, the admin list — then every shelf's membership:
in progress / finished / hidden (and the hide clearing on a move),
Continue Series moving on through a series, Discover's series rule, the
60-day shelves, a library filter, a profile with nothing started and a
profile with no access, and the Home Screen rails.
`home_flow.rs` and `preferences_flow.rs` list the new rails. Checked in
headless Chromium against a private instance: the book page opens a
session on play, reports listening time, keeps it open through a pause
and closes it on leaving; the Activity line; Home's shelves; the hide
menu; "Show it there again"; the library's Home view; the Listening
page; Server → Sessions; the disabled Continue listening box.

#### Readings made in AB8 — for the owner's veto

Each is a reading inside D29, D35 or the UI reference, not a new
decision.

1. **Sessions reuse the playback reports**, not a new endpoint: the
   web player sends `start` / `progress` / `stopped` for a book as it
   does for a video, and the server branches on the item's kind. What
   CL3 still owns is the native client's side (tokens, offline upload,
   the contract); if CL3 wants its own endpoints, they can sit on the
   same `listening` module.
2. **A session runs from play to leaving the book**, pauses included,
   as ABS's does from opening the player to closing it. A pause is not
   a new session; leaving the page, the book ending, or a new start on
   the same book is.
3. **Listening time is the player's measure, capped by the clock**
   (wall time since the previous report + 5 s). ABS takes the client's
   `timeListened`; the cap is Parlour's guard. A client that sends no
   measure is credited with forward movement, under the same cap.
4. **One open session per profile per book.** A new start on the same
   book closes the old one (`replaced`); sessions on different books,
   or on two profiles, stay open side by side.
5. **An hour's silence closes a session** at its last report — the
   span after which the live registry already forgets a session. ABS's
   exact stale rule wasn't read.
6. **Only closed sessions reach the Activity feed** (D29: "the Activity
   feed (closed ones)"); opening one writes nothing, unlike a video's
   start line. An admin stop writes both its own line and the listen.
7. **Stats count a session on the day it started**, in the time zone
   the client names (the web sends the browser's), as ABS dates a
   session by its start. Days with no listening are left out of
   `days`.
8. **The profile's own history and stats show only books it can still
   reach**, as every item query does (A6); an admin sees all of them.
   Sessions are kept indefinitely; the Activity lines follow the
   feed's 90-day retention.
9. **The shelves follow ABS's queries**, including the 60-day window on
   Recently Added, Recent Series and Newest Authors, Listen Again
   ordered by the progress row's update (not `finished_at`), and
   Continue Series ignoring the hide flag. "An author's first book
   here" stands in for ABS's author record's creation time, since
   Parlour's authors are `people` rows (AB7).
10. **Shelf order and visibility are the profile's Home Screen rails**,
    one setting for Home and for each library's Home view, with
    Continue Listening always shown (UI-AUDIO-REFERENCE §4.3; that rule
    is from the Audiobooth app, not ABS's server). The book shelves
    follow the video rails by default.
11. **An audiobooks library opens on its Home view**, as ABS opens a
    library, so AB7's Books list moved to `?view=books`.
12. **Main Home's book shelves span every audiobooks library** the
    profile reaches (ABS's are per library; Parlour's Home spans
    libraries, as Continue Watching does). The existing Recently added
    rail still holds books too, as before.
13. **AB11's reading 6 stands**: the auto-finish rule still runs on
    every progress write. Sessions don't finish a book; the progress
    write does.

### Transcode fallback and audio profiles (AB10, built 2026-11-26)

Built to TODO AB10 ("transcode fallback for book codecs a client
profile can't play (AAC), using CL5's profiles") and ARCHITECTURE §4's
rule that an **explicit, named client profile** decides a stream, never
sniffing. This builds CL5's profiles on the server and uses them for
the web player; the native apps' side (naming their profile, testing
the contents on devices, the CL6 contract) stays with CL5/CL6. ABS
behaviour was read from its server source on 2026-11-26
(`PlaybackSessionManager.js`, `Stream.js`, `Book.js`, `constants.js`;
behaviour only, nothing copied). What each player decodes was read the
same day from MDN's audio codec and container guides, caniuse and
WebKit's Safari 18.4 notes (browsers), Android's Media3 and platform
supported-formats pages (ExoPlayer), and Apple's HLS authoring
specification (AVPlayer). Code: `crates/playback/src/lib.rs`
(`AudioProfile`, `decide_audio`), `crates/server/src/playback.rs` (the
cached-ffmpeg runner the remux now shares, `/aac.m4a`, the book's plan
for Now Playing), `crates/server/src/audiobooks.rs` (`?client=`); web
`routes/book/[id]`, `lib/NowPlaying.svelte`, `lib/types.ts`. No
migration and no new table: the profiles are code, like video's, and
the copies are cache files. The export bundle is untouched (format v1).

**The profiles.** A profile lists **pairs**: a container (ffprobe's
first format name, as the catalog stores it) with the codecs that play
in it. A pair goes in only when a source says the player decodes it in
that container on every version the profile covers; anything uncertain
converts, because a needless conversion costs a wait and a wrong
"direct" costs a book that doesn't play. Clients name a profile the way
they name one for video (`?client=`):

| Source (container / codec) | `web` (browsers) | `android`, `androidtv`, `firetv` (ExoPlayer) | `ios`, `tvos` (AVPlayer) |
|---|---|---|---|
| MP4 (`.m4a`/`.m4b`) / AAC | direct | direct | direct |
| MP3 / MP3 | direct | direct | direct |
| WAV / 16- or 8-bit PCM | direct | direct | direct |
| MP4 / ALAC | AAC | AAC | direct |
| FLAC / FLAC | AAC | direct | direct |
| Ogg (`.ogg`/`.oga`/`.opus`) / Opus, Vorbis | AAC | direct | AAC |
| Ogg / FLAC | AAC | direct | AAC |
| ADTS (`.aac`) / AAC | copied into MP4 | copied into MP4 | copied into MP4 |
| ASF (`.wma`) / WMA | AAC | AAC | AAC |
| anything else (AC-3, 24-bit WAV, MP3 inside MP4, …) | AAC | AAC | AAC |

**The decision** (`decide_audio`, pure, unit-tested against the table):
a pair the profile lists plays directly; AAC in a container it doesn't
list is **copied** into MP4 (`Remux`, reason `ContainerUnsupported`);
any other codec it doesn't list in that container is **converted to
AAC** (`Transcode`, reason `CodecUnsupported`, or `ContainerUnsupported`
when the profile plays that codec in some other container). A track
whose codec the probe didn't find plays as it is, as an unknown codec
does for video. AAC in MP4 is the fallback because every profile plays
it.

**The API.** `GET /api/v1/audiobooks/{id}?client=<profile>` decides
every track for that profile. Each track gains a `plan` (`action`,
`reason`, `mode` = `direct_play` | `copy` | `transcode`, `delivery` =
`file` | `aac`, `client`), and a track that doesn't play directly gets
the fallback as its `stream_url` (`content_type` `audio/mp4`); `codec`
and `container` stay the source's. Without `client` the answer is
AB3's — every `stream_url` the original file, `plan` null — so existing
callers see no change. An unknown profile name is a 400. `network` is
accepted as for video's playback-info and only recorded with the plan
(audio has no bit-rate cap).

**The fallback stream.** `GET /api/v1/stream/{file_id}/aac.m4a` serves
a book track as AAC-LC in MP4, made **once per track** by ffmpeg and
kept under `<PARLOUR_CACHE_DIR>/audio-aac/<file id>/audio.m4a`, with
its index first (`+faststart`), so it is served with HTTP Range like
any file and a player seeks anywhere in it by byte range. It is
authorised exactly as the file is — the same 6-hour stream token (or a
session with library access) — so an admin stop that revokes a book's
tracks revokes their copies too. Until the copy is made the answer is
**503 with `Retry-After: 2`**, as the video remux's playlist is. The
first request for a copy starts it; the book detail starts the copy of
the track the profile will resume in. The recipe: the first audio
stream only; ffmpeg's AAC encoder with its fast coder at 64 kb/s per
channel (mono stays mono, more than two channels fold to stereo); AAC
sources copied instead; tags, pictures and chapters dropped (the
catalog has them). Each copy is stamped with the recipe's version and
the source's size and modification time as the last scan saw them; a
re-scan that sees the file change makes the copy again (one ffmpeg per
track at a time: a copy still being made of the old file finishes
first). A video file's sound is never converted this way (404).

**Reusing the video machinery.** The remux and the fallback now share
one runner (one ffmpeg per output folder, run once, `.ready` / `.failed`
markers, 503 while running), the stream tokens, the plan registry that
Now Playing reads, the live session registry and the Range server. The
**HLS muxer itself is not shared**: the remux maps a video stream
unconditionally (`0:v:0`), so an audio-only file fails there; it writes
HLS, which the book page's `<audio>` would need hls.js for; and it waits
for the whole file before serving anyway, so a progressive `.m4a` gives
the same wait with simpler seeking for every player. The remux's ffmpeg
arguments and its retry after a failure are unchanged.

**Sessions, progress and Now Playing.** Nothing about AB4/AB11/AB8
changes: the web player still reports its listening session through
`POST /api/v1/playback/{start,progress,stopped}` and writes the
book-global position through `PUT /api/v1/audiobooks/{id}/progress`;
auto-finish, hand finish / un-finish, the hide flag, the client
timestamp, Activity and the Home shelves are as built. What's new is
that a book's session in **Now Playing** carries a stream plan when the
book page named its profile: the book detail remembers its plan for the
profile, and the session's start picks it up, as a video's start picks
up playback-info's. The card reads, for example, "Original file → AAC
file (transcode)", "Audio ALAC → Transcoded", "This codec is outside
the client's audio profile, so the audio is converted to AAC."

**Web.** The book page asks for `?client=web`. For a track that plays
through the fallback it waits for the copy (polling with `HEAD`, as the
video player polls the remux playlist), saying "Converting this part of
the book to AAC for this browser… It is kept once made" and showing the
seconds; play, seeks and chapter jumps made meanwhile are kept and
applied when the copy is ready. Playing a converted track, it says why
("Playing an AAC copy: ALAC isn't in the web player's profile."). As
each track starts it asks for the next converted track's copy, so a
multi-track book doesn't wait between tracks. Direct-play books behave
exactly as before.

**Measured on oryx (2026-11-26)** with the runtime image's ffmpeg 5.1,
16 cores: an hour of stereo audio converts in about 61 s with the
default AAC coder and **39 s with the fast coder** (decoding alone:
7 s). So the first play of a converted track waits roughly 40 s per
hour of that track; after that it starts at once.

#### Readings made in AB10 — for the owner's veto

Each is a reading inside TODO AB10/CL5 and ARCHITECTURE §4, not a new
decision.

1. **The browser profile is what every current browser plays**: MP3,
   AAC in MP4, and 16/8-bit WAV. Ogg Opus and Vorbis convert because
   Safari plays Ogg only from 18.4 (macOS 15.4 / iOS 18.4, March 2025);
   FLAC converts because sources disagree on iOS Safari (caniuse yes,
   MDN no); ALAC is Safari-only. Chrome and Firefox would play Ogg and
   FLAC as they are, so this costs them a conversion. The alternative is
   a generous `web` profile (adding FLAC and Ogg), at the price of books
   that fail on older Safari; there is no per-browser profile, because
   telling browsers apart is the sniffing §4 rules out. WAV is in on
   its long-standing support in all three; the pages read didn't settle
   it browser by browser.
2. **The Android profile is ExoPlayer with the platform decoders
   only** (no FFmpeg extension), one profile for phones and Fire TV:
   AAC in MP4, MP3, FLAC (as `.flac` and in Ogg), Opus and Vorbis in Ogg,
   16/8-bit WAV. Out: ALAC and WMA (no platform decoder); AC-3 / E-AC-3
   (no source confirms them, and Amazon's Fire TV spec pages list no
   audio codecs); FLAC, Opus or MP3 inside MP4 (Android 10+ only);
   ADTS (ExoPlayer seeks it only with constant-bitrate seeking turned
   on). ExoPlayer seeks MP3 by its Xing/VBRI index or with that same
   flag: a note for the Android app (CL5/Phase 3).
3. **The Apple profile is AVPlayer**: AAC and ALAC in MP4, MP3, FLAC,
   16/8-bit WAV; no Ogg container at all (so no Opus or Vorbis), no
   WMA. AC-3 / E-AC-3 are left out although Apple's HLS table lists
   them for A7 and later: no book uses them, and converting is harmless.
   **Lower confidence than the other two:** the Apple documents read
   cover HLS, not files; local MP3, WAV and `.flac` playback (FLAC since
   iOS 11) is from general knowledge. CL5 / S1 should confirm on a
   device.
4. **Codec and container pairs, not ABS's MIME type.** ABS decides by
   a MIME type taken from the file's extension, so an `.m4b` is
   `audio/mp4` whatever its codec and an ALAC book plays "directly" in
   Chrome and fails. Parlour decides by the probed codec in its
   container.
5. **The profile is named, not asked of the browser.** ABS's web
   client builds its list by asking the browser (`canPlayType`); §4
   says a named profile decides, so the book page names `web`. An
   unknown name is a 400 (video reads an unknown name as `web`; for
   books a typo would quietly pick a profile). No name at all keeps
   AB3's answer, so a native client must name its profile to get the
   fallback (the CL5/CL6 contract).
6. **Per track, not per book.** ABS converts the whole book into one
   stream when any track fails its check (and copies only when the
   first track allows it). Parlour decides each track and converts only
   the ones that need it; the player keeps playing track after track,
   and positions stay book-global. A book of mixed formats (rare) plays
   some tracks as they are.
7. **A kept `.m4a` per track, not a live HLS stream.** ABS streams HLS
   (MPEG-TS segments) from the session's position, restarts ffmpeg
   when a seek lands more than a minute past what it has made, and
   deletes the stream when the session closes. Parlour makes each
   track's copy once and keeps it: seeking anywhere is a byte range,
   every player plays a progressive `.m4a`, and the next listen costs
   nothing. The price is the **first-play wait** above, worst for a
   long single-file book in a format the profile can't play (a 10-hour
   Opus file is about 7 minutes before it starts). A stream made on
   demand around the play position, as ABS does, belongs with Phase 2's
   transcode engine.
8. **The recipe**: ffmpeg's own AAC encoder with its fast coder (1.5×
   the default's speed for speech), 64 kb/s per channel, mono kept,
   more than two channels folded to stereo, the first audio stream
   only, tags/pictures/chapters dropped. ABS sets no bit rate (ffmpeg's
   default) except for AC-3 sources.
9. **AAC in a container the profile doesn't list is copied, not
   re-encoded** (§4's remux), and every profile copies raw ADTS into
   MP4 for exact seeking (ExoPlayer can't seek ADTS by default; browsers
   estimate).
10. **Copies are kept until their source changes** (the stamp: recipe,
    size, modification time as scanned). There is no eviction, as for
    the remux cache; D9's LRU cache index arrives with Phase 2. A copy
    that failed is not retried until its source changes or the server
    restarts (the video remux still retries, as before).
11. **No limit on simultaneous conversions**, as for the remux (D8's
    admission control is Phase 2's). In practice: the resume track per
    book opened, plus the next track per playing page.
12. **Now Playing shows one plan per book**: the furthest-reaching
    decision among its tracks (convert, then copy, then direct), with
    that track's codec, channels and container, as decided when the
    book page loaded. It doesn't change per track: a session is per
    book. A session from a client that named no profile still has no
    plan, as in AB8.
13. **The fallback URL serves any book track to anyone authorised for
    it, whatever the decision** (a client may prefer AAC), and never a
    video file.

### Later in Phase 10 (blocked on decisions)

AB6 metadata providers and embedded covers (D30), AB7 series and
author browsing (D28), AB8 Now Playing, Activity and a
Continue-listening rail (D29/D35), AB9 an importer for progress from
the owner's running Audiobookshelf (D27), AB10 a transcode fallback
for codecs a client can't play (CL5).

*2026-11-24:* all unblocked except AB9, which is **cancelled**
(D27: no ABS import). AB6 follows D30, AB7 follows D28, AB8 follows
D29 + D35; the details are in the list below. *2026-11-26: AB6 is
built (above). AB7 is built (above, "Series and authors (AB7)").
AB11 is built (above, "Aligning with Audiobookshelf (AB11)"). AB8 is
built (above, "Listening sessions and Home shelves (AB8)"). AB10 is
built (above, "Transcode fallback and audio profiles (AB10)"); Phase 10
has no task left but the owner's real-library check in its "Done
when".*

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
   such book's identity (`stable_key`). *Done in AB11, with ABS's
   exact rule (whitespace only, 1–3 digits; AB11 reading 1). The
   library on oryx holds no audiobooks, so the owner's real library
   still needs the check before the change reaches it.*
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
   setting, off by default). AB2 uses the folder name raw. *Done in
   AB11; the subtitle setting is per library (AB11 reading 3).*
4. **Which folders count (AB2).** ABS looks only at the three
   folders nearest the book: title, series, author. AB2 takes the
   *top-level* folder as the author fallback, so a deeper layout
   (`Genre/Author/Title`) would credit "Genre". Use the nearest
   author position instead. *Done in AB11. As in ABS,
   `Genre/Author/Title` still reads `Genre` as the author (three
   folders: the middle one is the series); `Genre/Author/Series/Title`
   now credits `Author`.*
5. **Author splitting (AB2).** ABS splits folder-name authors on
   `,` `;` `&` and `and`, and accepts `Last, First`. AB2's
   `split_people` splits on `;` `&` ` / ` and on commas only when
   every part has a space. Match ABS for folder names; keep tag
   splitting under review in AB6. *Folder names: done in AB6. Tag
   splitting kept as AB2 has it (AB6 reading 9). AB11 changed nothing
   here; a narrator from the title folder splits as folder authors
   do.*
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
   *Done in AB11.*
9. **Hide from Continue Listening (AB4/AB8).** ABS keeps a per-user,
   per-item hide flag that clears when progress next changes. Add
   it to `book_progress` and the API. *Done in AB11 (API only); AB8
   added the shelf and the web control.*
10. **Client timestamp (AB4/CL4).** ABS keeps last-arrival-wins but
    stores the client's own last-update time. AB4 stamps the
    server's time. Accept and store the client's. *Done in AB11
    (`client_updated_at`; AB11 reading 10).*
11. **Podcast progress shares the table (POD1).** ABS keeps one
    progress model for books and episodes; POD1 puts episode
    progress in the same table and shape rather than a parallel
    one (a `kind` column or a generalised name, decided in POD1).
    *Done in POD1: neither — a nullable `episode_id` beside `item_id`,
    exactly one set, each unique per profile; the table keeps its name
    (POD1 reading 2).*
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
    sessions. AB8 must not bump `play_count` for books. *Done in AB8:
    a book's playback reports write no `user_data` row at all; listens
    are `listening_sessions` ("Listening sessions and Home shelves
    (AB8)").*

---

## Phase 11 — Podcasts (POD1's schema, POD2's feed checks, POD3's downloads and POD4's feed management built 2026-11-26; POD5's listener API, POD6's web pages and player, POD7's directory search and POD8's playlists, collections and bookmarks built 2026-11-27)

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
  *Built in POD7 ("Directory search (POD7)").*
- **The server downloads episodes** (ABS is not a pass-through). An
  episode not downloaded shows in the feed list and can be
  downloaded by hand. *As decided on 2026-11-24, only downloaded
  episodes played and the enclosure redirect was dropped.* **D32 was
  amended on 2026-11-27 (owner): an episode that isn't downloaded
  streams by redirecting to its enclosure**, with the consequences
  the owner accepted (a paid feed's tokenised enclosure URL reaches
  listeners through the redirect, the publisher sees their IP, and a
  redirected stream has no AB10 audio plan). Built in POD5 ("Native
  API (POD5)").
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
*POD1 (2026-11-26) built the schema; it is "Schema (POD1)" at the
end of this section, and replaces the table bullets below.*

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
*Built in POD2 (2026-11-26): "Feed fetch and parse (POD2)" at the end
of this section; N is 3 (POD2 reading 1).*

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
*Built in POD5 as `/api/v1/podcasts…` and
`/api/v1/podcast-episodes/{id}…` ("Native API (POD5)"); adding a
show stayed admin-only under `/api/v1/admin/` (POD4, D31).*

### Schema (POD1, built 2026-11-26)

Migration `20261203000000_pod1_podcasts.sql`: the timestamp is an
ordering key only, and the migration was written on 2026-11-26.
POD1 is schema only. Nothing fetches, parses or downloads yet, and
there is no API and no UI. POD2 (feed fetch and parse) and POD3
(download and retention) write these columns, and POD4–POD6 read
them. The flow `crates/server/tests/podcast_schema_flow.rs` checks
the shapes and rules below on a fresh database.

Audiobookshelf's behaviour quoted here was checked on 2026-11-26
against its server source on `master` (the podcast, episode and
media-progress models, the podcast manager and the cron manager).
That reading was for behaviour only, and nothing was copied.

**`podcasts`: one row per feed in a `podcasts` library.**

| Column | What |
|---|---|
| `library_id` | The podcasts library the show is in. Deleting the library deletes its shows. |
| `feed_url` | The RSS address. Unique per library: a feed is in a library once, and another podcasts library may hold it too. |
| `title`, `sort_title` | The feed's title. `sort_title` is made the way the catalog makes it (`parlour_catalog::parse::sort_title`). |
| `author`, `description`, `language`, `explicit`, `podcast_type`, `genres`, `itunes_id` | The show as its feed last described it, with the iTunes namespace used where RSS has no field. `podcast_type` is `episodic` or `serial`. |
| `image_url`, `image_path` | The feed's artwork address, and the copy in Parlour's artwork cache once one is fetched. |
| `folder_path` | Where the show's episodes are written (below). NULL until the show is given one. |
| `etag`, `last_modified` | The validators from the feed's last 200 response. They are kept as the header text and sent back as `If-None-Match` / `If-Modified-Since`. |
| `last_checked_at`, `last_error`, `consecutive_failures` | The refresh state: when the feed was last checked (successful or not), why the last check failed (NULL after a success), and how many checks in a row have failed. |
| `auto_download` | D32: download new episodes on the schedule. **Off** by default. |
| `check_schedule` | D32: a five-field cron expression. Default `0 * * * *` (hourly). |
| `max_episodes_to_keep` | D32: **0** = keep all (default). When over, POD3 deletes the oldest, at most one per check. |
| `max_new_episodes_per_check` | D32: default **3**. 0 means no cap, as in ABS. |

**`podcast_episodes`: one row per feed item that has an enclosure.**

| Column | What |
|---|---|
| `podcast_id` | The show. Deleting the show deletes its episodes. |
| `guid` | The feed's identity for the episode, or the enclosure URL when the item has no guid. Unique within the show. |
| `title`, `subtitle`, `description`, `published_at`, `season`, `episode_number`, `episode_type` | As the feed gives them. `episode_type` is `full`, `trailer` or `bonus`. |
| `duration_ms` | The length the feed gives (`itunes:duration`). |
| `enclosure_url`, `enclosure_type`, `enclosure_length` | The audio the feed points at. The URL is required: a feed item without an enclosure isn't an episode. |
| `chapters_url`, `transcript_url`, `transcript_type` | The Podcasting 2.0 links, stored as URLs. Showing transcripts is deferred (D35). |
| `file_path`, `file_size_bytes`, `file_duration_ms`, `file_container`, `file_codec`, `downloaded_at` | The local copy once POD3 downloads it: its absolute path, size, probed length, container and audio codec. All NULL until then. `file_path` and `downloaded_at` are set together. No two episodes share a file. |

**Where downloaded files live (D32).** An episode's `file_path` sits
under its show's `folder_path`, and that folder sits inside the
podcasts library's own folder (`library_paths`). That library folder
is the **writable podcast store** D32 describes. It is a folder
Parlour owns, set up as an explicit ops step like XFER5's sidecar
opt-in. It is never a video or book media mount, and never a
read-only mount flipped to writable. POD1 creates no store, changes
no mount and writes nothing to disk. Which disk on sullivan holds the
store is still waiting on the owner, and it must be answered before
POD3 runs in production.

**Subscriptions (D31): none.** There is no `subscriptions` table and
no per-profile follow. A show belongs to a podcasts library, and a
profile reaches it only through that library's `library_access`
grant, the same grant that admits it to an audiobooks library's
books. A profile with the grant sees every show in the library; a
profile without it sees none. That is the whole of "subscriptions
shaped by D31": nothing in the schema expresses it except the
existing grant. The grant path every podcast query takes is:

```sql
podcast_episodes e
JOIN podcasts p ON p.id = e.podcast_id
JOIN library_access a ON a.library_id = p.library_id AND a.profile_id = $profile
```

The per-profile follow / hide list is not built. It has no ABS
equivalent and is deferred until the owner asks.

**Episode progress lives in `book_progress` (D29; follow-up item
11).** ABS keeps one progress model for books and episodes, so
episodes don't get a parallel table:

- `book_progress` gains `episode_id`, which references
  `podcast_episodes` and is deleted with the episode. `item_id` may
  now be NULL.
- A row is a book's (`item_id`) or an episode's (`episode_id`), never
  both and never neither (`book_progress_one_target_check`).
- Each kind of row is unique per profile:
  - `UNIQUE (profile_id, item_id)` replaces the old primary key, so
    AB4's `ON CONFLICT (profile_id, item_id)` upsert is unchanged.
  - `UNIQUE (profile_id, episode_id)` is the episode key.
- Every other column means what it means for a book: `position_ms`,
  `duration_ms`, `finished` / `finished_at`, `last_listened_at`, the
  `hidden` flag, `client_updated_at`, and `updated_at` (the sync
  cursor). Progress is per profile per episode (D31).
- Every existing book query reaches the table through an inner join
  on `item_id`, so an episode's row never appears in a book list, a
  shelf or the book sync (`GET /api/v1/audiobooks/progress`).

**Progress writes for an episode follow AB11's rules unchanged.**
POD5 writes episode progress with the same pure `apply_write`, which
takes these inputs:

- **The episode's length:** `file_duration_ms`, or `duration_ms`
  when there is no file.
- **The finish rule:** the podcasts library's own
  `finish_remaining_seconds` / `finish_percent`, with ABS's 10 s by
  default.

The write behaves as for a book:

- **Position.** A position is clamped to the episode's length and
  stamps `last_listened_at`.
- **Auto-finish.** A position within the finish rule finishes the
  episode. A finished episode that is moved back is unfinished.
- **Manual finish.** Un-finishing by hand puts the position back
  to 0.
- **Hide flag.** A write that changes the position or the finished
  flag clears `hidden`.
- **Timestamps.** `client_updated_at` is stored as the client sent
  it (or the server's time). The last write to arrive wins.

The upsert is the book's, keyed on `(profile_id, episode_id)`. The
flow test makes exactly these writes.

**What later tasks need to know.**

- **POD2.** *Done in POD2, as written here ("Feed fetch and parse
  (POD2)").*
  - **New episodes.** A feed item is the same episode as a stored row
    when its guid matches, or (as ABS checks) when its enclosure URL
    matches.
  - **Schedule.** The scheduled check runs only while
    `auto_download` is on (reading 5).
  - **Failures.** Every check stamps `last_checked_at`. A failure
    sets `last_error` and adds one to `consecutive_failures`. The
    24th failure in a row turns `auto_download` off. A successful
    check clears `last_error` and the count. Feeds failing repeatedly
    raise the Dashboard notice.
- **POD3.** *Done in POD3 ("Episode download and retention (POD3)"):
  retention takes the second way, clearing the `file_*` columns
  (POD3 reading 1).*
  - **Store.** It writes into `folder_path` inside the writable
    store, and fills the `file_*` columns and `downloaded_at`.
  - **Retention.** When retention removes an episode, ABS deletes
    the file, the episode and its progress. Here a feed episode keeps
    its row, so POD3 must choose between two ways:
    - delete the row (the progress goes too, by cascade);
    - clear the `file_*` columns, leaving "not downloaded".

    If it deletes the row, the next feed check must not treat the
    episode as new and fetch it again.
- **POD5 / POD6.** *All four done in POD5 ("Native API (POD5)").*
  - **Playback.** AB10's audio plan reads `file_container` /
    `file_codec`. The AAC fallback serves only `kind = 'audiobook'`
    files today and needs widening. *POD5: widened to downloaded
    episodes.*
  - **Sessions.** `listening_sessions` is still book-only (`item_id`
    NOT NULL). An episode's listening session needs the same
    `episode_id` treatment. *POD5: given it.*
  - **Explicit shows.** `explicit` is stored, but nothing yet decides
    how a grant's `max_rating` treats an explicit show (ABS has a
    per-user "explicit content" permission instead). That is POD5's
    to raise. *Decided as D41 (2026-11-27): a per-profile flag, on by
    default; `max_rating` plays no part.*
  - **Settings API.** AB11's settings API (`…/audiobook-settings`)
    accepts audiobooks libraries only, so editing a podcasts
    library's finish rule needs it widened (POD4/POD5).
    *POD4 left it to POD5, which writes the first episode progress
    (POD4 reading 10). POD5 added a podcasts twin,
    `…/podcast-settings`.*
- **Bundle (format v1 unchanged).** `podcasts` and `podcast_episodes`
  are *left out* in `coverage.rs`:
  - The shows are an admin's deliberate work. Their place in the
    bundle is later work (a format change); until then they are added
    again on a new host, or carried by POD4's OPML export.
  - The episode list is rebuilt by the new host's feed checks, and
    downloaded files live in the old host's store.
  - Episode progress rows are part of `book_progress`, which stays
    *Proposed*. **D36 is still open**, and no episode progress is
    carried.

#### Readings made in POD1 — for the owner's veto

1. **Shows and episodes are their own tables, not `media_items`
   rows.**
   - *Why:*
     - TODO and the plan name `podcasts` / `podcast_episodes`.
     - The `media_items` kinds are mirrored by bundle format v1
       (`ItemKind`), and every `media_items` row travels in
       `catalog.json`. New kinds would change format v1, which this
       step may not do.
     - An episode isn't a file a scan found: until it is downloaded
       it has no file at all.
   - *Cost:* a show or episode gets none of the item machinery for
     free (search, the `images` table, Activity's item links). POD5
     and POD6 build their own paths. This is the one place podcasts
     part from ARCHITECTURE §7's "one catalog", which now says so.
2. **Episode progress shares `book_progress`, not a parallel table.**
   - *How:* a nullable `episode_id` beside `item_id`, exactly one of
     them set, each unique per profile.
   - *Why this mechanism:*
     - There is no `kind` column: which id is set says what the row
       is, the role of ABS's media-item type.
     - The table keeps its name. Renaming it (e.g. `audio_progress`)
       would touch every book query for no change in behaviour; it
       would be one migration later if wanted.
   - *Detail:* the old primary key became `UNIQUE (profile_id,
     item_id)`, which every existing upsert still finds.
3. **Every feed episode gets a row; only downloaded ones have a
   file.**
   - *ABS differs:* it stores only downloaded episodes and reads the
     rest from the feed live.
   - *Why rows here:*
     - The TODO line names the local file as a column of the episode.
     - The behaviour D32 describes is the same either way: a feed
       list in which undownloaded episodes are marked and can be
       downloaded by hand.
     - Rows also give CL2 "new episodes" to sync.
4. **An episode's identity is its `guid`, unique per show.**
   - The enclosure URL stands in when the item has no guid (the
     plan's sketch).
   - An item without an enclosure is not stored.
5. **The schedule is `auto_download`, as in ABS.**
   - *What ABS does:* it schedules a show's check only while
     auto-download is on, and the 24-failure rule turns
     auto-download off. So there is no separate "schedule on" flag,
     and `check_schedule` is kept when it turns off.
   - *Consequence:* with ABS's default (auto-download off), a show is
     not checked on its own until an admin turns auto-download on.
     Until then its list refreshes when an admin asks (POD4's
     "refresh now"). Phase 11's "Done when" needs auto-download on
     for its feed.
   - *If vetoed:* a separate flag is one column.
6. **The failure count is stored.**
   - *ABS differs:* it keeps the count in memory, so a restart resets
     it.
   - *Why stored:* Parlour's checks run as jobs on the Postgres
     queue, so the count lives with the show and survives restarts.
     It stays at its value after auto-download turns off, until the
     next successful check, so the Dashboard notice can show it.
7. **Per-show settings are columns on `podcasts`.** That is how
   per-library settings are stored (columns on `libraries`). The
   defaults are ABS's: off, hourly as cron, 0, 3. A cron expression,
   not an interval, because ABS's schedule is one.
8. **Conditional GET state is stored, though ABS has none.**
   - ABS fetches feeds with a plain GET.
   - Parlour's TODO asks for ETag / Last-Modified. Listeners see no
     difference, and publishers get fewer full downloads.
   - The validators are kept as raw header text and sent back
     verbatim.
9. **Two lengths per episode.**
   - `duration_ms` is the feed's, for the list before a download.
   - `file_duration_ms` is the downloaded file's, and is what
     progress clamps and auto-finishes against. Only downloaded
     episodes play, so that is the length a listener hears.
   - The feed's length is the fallback when there is no file (for a
     mark-finished on an undownloaded episode, if POD5 allows one).
     *Since D32's amendment (2026-11-27) an undownloaded episode plays
     through the redirect, so POD5 takes every write on it, against
     the feed's length (POD5 reading 5).*
10. **Episodes auto-finish under their podcasts library's rule.**
    - AB11's `finish_remaining_seconds` / `finish_percent` are on
      every library row, so ABS's 10 s applies to episodes by
      default.
    - ABS's progress code applies the same rule to both kinds.
11. **Files have absolute paths and one owner.**
    - `folder_path` and `file_path` are absolute, as
      `media_files.path` is.
    - A file belongs to one episode.
    - The show's folder is NULL until POD3/POD4 names it (ABS fixes a
      show's folder when it is added).
      *POD3 names it at the first download, and POD4 keeps that
      (POD4 reading 2).*
12. **More of the feed is kept than the plan's sketch listed.**
    - *Added, show:* `sort_title`, `language`, `explicit`,
      `podcast_type` (a serial show lists its episodes oldest
      first), `genres` and `itunes_id` (POD7 can mark a search result
      already added). ABS keeps the same.
    - *Added, episode:* `subtitle`, `season` and `episode_number` (as
      integers; ABS keeps text, and a feed's non-numeric value is
      dropped), and `episode_type`.
    - *Not kept:* episode artwork, the raw `pubDate` text, the feed's
      `<link>` and chapter bodies. A later task adds a column if its
      screen needs one.

### Feed fetch and parse (POD2, built 2026-11-26)

POD2 fills POD1's tables from the feeds. A show's feed is fetched
with a conditional GET and read by a pure parser. The show's row and
one row per feed episode are then brought in line with it. This runs
as a `refresh_podcasts` job on the Postgres queue at the show's D32
schedule, and feeds that keep failing raise a Dashboard notice.

POD2 needs no migration: every column it writes is POD1's. Nothing
is downloaded, no file is written, no mount changes, and there is no
API and no UI. POD3 downloads, and POD4–POD6 manage and show. The
flow `crates/server/tests/podcast_refresh_flow.rs` runs all of it
against a local stub feed host on a fresh database.

**The parser** (`parlour_catalog::podcast_feed`) is pure: bytes in,
POD1's shapes out. It is fixture-tested against
`crates/catalog/tests/fixtures/podcasts/`, which holds an episodic
feed, a serial feed, a feed of odd items and an HTML page.

- **What it reads.** RSS 2.0 with the iTunes namespace, plus the
  Podcasting 2.0 `podcast:chapters` / `podcast:transcript` links,
  stored as URLs. Elements are recognised by their usual prefixes
  (`itunes:`, `podcast:`, `content:`), ignoring case. The root must
  be `<rss>` with a `<channel>`. Anything else (Atom, an HTML page)
  is a failed check.
- **The show:**

  | Field | Where it comes from |
  |---|---|
  | `title` | `<title>` |
  | `author` | `itunes:author`, else `<author>` |
  | `description` | `<description>`, else `itunes:summary` |
  | `language` | `<language>` |
  | `explicit` | `itunes:explicit`: `yes` / `true` / `explicit` mean explicit |
  | `podcast_type` | `itunes:type`: `episodic` or `serial`, else none |
  | `genres` | every `itunes:category` `text`, sub-categories included, each once |
  | `image_url` | `itunes:image`'s `href`, else `<image><url>` |

- **An episode** is an `<item>` with an enclosure URL. An item
  without one is not stored (POD1 reading 4).

  | Field | Where it comes from |
  |---|---|
  | `guid` | `<guid>`, or the enclosure URL when the guid is missing or blank |
  | `title` | `<title>`, else `itunes:title`, else empty |
  | `subtitle` | `itunes:subtitle` |
  | `description` | `<description>`, else `content:encoded`, else `itunes:summary`, kept as the feed's HTML |
  | `published_at` | `pubDate` (below) |
  | `duration_ms` | `itunes:duration` (below) |
  | `season`, `episode_number` | `itunes:season` / `itunes:episode`: whole numbers 0 or more only; `Two`, `4a`, `-2` are dropped (POD1 reading 12) |
  | `episode_type` | `itunes:episodeType`: `full`, `trailer` or `bonus`, else none |
  | `enclosure_*` | the first `<enclosure>`'s `url`, `type` and `length`. A length of 0, or one that can't be read, is none |
  | `chapters_url` | `podcast:chapters`' `url` |
  | `transcript_*` | the first `podcast:transcript` the item lists |

  If a feed repeats a guid, the first item with it is kept.
- **Durations.** `H:MM:SS`, `M:SS` or plain seconds, the seconds
  possibly with a fraction. Minutes past 59 are read as written
  (`75:30` is 75½ minutes). Empty, 0, `abc`, `1:2:x` or four parts
  give no duration.
- **Dates.** RFC 822 first, then ISO 8601. A wrong weekday is ignored.
  A zone name RFC 822 doesn't know (`BST`) is read as UTC. A date that
  can't be read is no date.
- **Text.** Single-line fields have their white space folded. Feed
  bytes that aren't UTF-8 are read as Latin-1. The common HTML
  entities (`&nbsp;`, `&mdash;` …) are read, and unknown ones are
  kept as written.

**A check** (`parlour_catalog::podcasts::check_podcast`):

1. **Fetch.** The feed is fetched with the stored validators sent
   back as `If-None-Match` / `If-Modified-Since`. Only a 200 or a 304
   is an answer. Any other status, a network error, a timeout (60 s)
   or a body over 64 MB is a failed check. Redirects are followed,
   and the stored `feed_url` is not rewritten.
2. **304.** The show is left alone except for its refresh state:
   `last_checked_at` is stamped, and `last_error` and
   `consecutive_failures` are cleared.
3. **200.** The body is parsed. If it can't be read, the check fails
   and the response's validators are **not** kept (a later 304 must
   never hide a broken feed). If it reads, one transaction does
   this:
   - The show's fields are set as the feed now describes them. A
     feed without a title keeps the stored one, and a field the feed
     no longer has is cleared. `itunes_id`, `image_path`,
     `folder_path` and the D32 settings are never touched by a check.
   - The new validators are stored (a 200 without them clears them).
   - The check is stamped and the refresh state cleared.
   - `updated_at` moves only when a show field really changed.
4. **Episodes.** Each feed episode is matched to a stored row:
   - first by guid, across the whole feed;
   - then, as ABS checks, by enclosure URL, among rows no feed item
     claimed by guid;
   - a row is matched once.
   
   An unmatched feed episode gets a new row, with the file columns
   empty (POD1 reading 3). A matched row whose feed-described values
   changed (title, dates, enclosure, links …) is updated. It keeps
   its stored guid, and its file columns are never touched. A stored
   episode the feed no longer lists is left alone, because retention
   is POD3's.
5. **Failure.** `last_checked_at` is stamped, `last_error` is set
   (at most 500 characters), and `consecutive_failures` goes up by
   one. The failure that brings the count to **24** (D32) turns
   `auto_download` off, and that moves `updated_at`. `check_schedule`
   is kept.

**The feed URL is kept out of sight.** A paid feed's URL carries a
private token. So the URL is not written to the log, to a job's error
list or to a notice, and fetch errors drop it (`without_url`).
Failures are listed by the show's title.

**The schedule and the job** (`crates/server/src/podcasts.rs`):

- **Due.** A show is due when its `auto_download` is on (the schedule
  switch, POD1 reading 5) and its `check_schedule` has fired since
  `last_checked_at`. A show never checked is due at once. Firings
  missed while the server was down add up to one check, not several.
- **The cron expression** has five fields (minute, hour, day of
  month, month, day of week) with lists, ranges, steps and English
  names (`parlour_jobs::cron`). It is read in UTC, as the nightly
  tick is. An expression Parlour can't read is treated as the hourly
  default and logged; POD4 will refuse one at input.
  *POD4 does ("Managing feeds (POD4)").*
- **The tick.** Once a minute, if any show is due, a tick queues one
  `refresh_podcasts` job `{"trigger": "schedule"}`. It doesn't when a
  refresh job is already waiting, or when one started within the
  hour is still running (a restart can leave a `running` row behind).
- **The job.** It has a worker of its own, so a slow feed never holds
  up a scan. When it runs, it checks the shows due at that moment,
  one after another. The job's progress counts shows, and each feed
  failure is listed on the job under the show's title. A feed's
  failure doesn't fail the job; only a database error does. The
  result is a summary: checked, not modified, updated, failed, new
  and changed episodes, and schedules turned off.
- **One show by name.** A job queued as `{"podcast_id": …}` checks
  that one show whatever its schedule says. That is what POD4's
  "refresh now" will queue. A check by hand counts toward the
  failures like any other.

**The Dashboard notice** (`notices.rs`, reconciled once a minute
with the others):

- **When.** A show raises one notice, `podcast_feed_failing:{id}`,
  from its **3rd** failed check in a row.
- **What it says.** The summary is "The feed for *show* has failed
  *n* checks in a row". The detail holds the show's id, title and
  library, the count, the last error and `auto_download`.
- **Severity.** It is a warning, and becomes an error from the 24th
  failure. Once the 24th failure has turned the schedule off, the
  summary adds "its scheduled check is off". The severity change
  brings back a notice an admin dismissed.
- **Clearing.** A successful check (200 or 304) resets the count, and
  the next pass resolves the notice.
- **No link.** The Alerts rail has no link for it until POD4 gives
  shows a page. *POD4 did: the notice links to the show's page
  ("Open the show").*

**Unchanged.** The bundle stays at format v1, with no table change
and nothing new in `coverage.rs`. `podcasts`, `podcast_episodes` and
episode progress stay out of it. D36 is still open. The only web
change is the job's name on the Jobs page ("Podcast feed check").

#### Readings made in POD2 — for the owner's veto

1. **The notice starts at the 3rd failure in a row.**
   - *What:* a warning from the 3rd failure, an error from the 24th.
   - *Why:* the plan said "after N failures", and no decision names
     N. ABS has no such notice, so there is nothing to mirror. With
     the hourly default, 3 is about three hours of failing, which a
     host's one-off hiccup doesn't reach.
   - *If vetoed:* `FEED_FAILING_NOTICE_AT` is one constant.
2. **Schedules are read in UTC.**
   - *ABS differs:* it reads them in the server's local time.
   - *Why UTC:* the container has no `TZ` set, so its local time is
     UTC anyway, and the nightly tick already uses UTC. The hourly
     default is the same either way.
   - *If vetoed:* a daily or weekly schedule would follow a
     configured zone instead.
3. **A show never checked is due at once, and missed firings make
   one catch-up check.**
   - *ABS differs:* ABS fetches the feed when a show is added, and
     its cron doesn't catch up after downtime.
   - *Here:* turning auto-download on for a show checked days ago
     checks it on the next tick, once.
4. **Only five-field cron is read.**
   - An unreadable schedule is treated as hourly and logged, not
     treated as a failed check. Nothing can store one until POD4,
     which will refuse it at input.
   - ABS's cron library also accepts a sixth, seconds field. Its UI
     writes five fields.
5. **One job per tick checks every show that is due, not one job per
   show.**
   - *Why:* the Jobs page gets one row per round instead of one per
     show.
   - *The job:* it lists failures by show, and a feed failure doesn't
     fail it.
   - *By hand:* a job naming one show (`podcast_id`) is the hook for
     POD4's "refresh now".
6. **Each 200 rewrites the show's described fields.** POD1's table
   says "as its feed last described it". A missing title keeps the
   stored one, and other missing fields are cleared. Whether ABS's
   own scheduled check refreshes a show's details was not re-checked
   in this run. If the owner wants checks to add episodes only, it is
   one condition.
7. **Matched episodes are refreshed from the feed.**
   - *What:* when a publisher edits an episode (a title, a new
     chapters link, a moved enclosure), the stored row follows.
   - *Kept:* the row's guid stays as stored, so its identity holds
     even when it was matched by enclosure URL. The `file_*` columns
     are never touched.
   - *ABS:* it shows undownloaded episodes from the feed as it is
     now (POD1 reading 3), and this keeps the list the same.
8. **Matching order: guid first across the whole feed, then
   enclosure URL among the rows left.**
   - That order means a re-issued guid never steals a row another
     item holds by guid.
   - Each row is matched once, and a feed's repeated guid keeps its
     first item.
9. **Validators come only from a 200 that parsed.** A broken 200
   fails the check and leaves the old validators alone. A 304 counts
   as a successful check (it clears the error and the count).
10. **Fetch limits.**
    - 60 s per fetch, 15 s to connect, 64 MB per feed.
    - Up to 10 redirects are followed, and the stored URL is not
      rewritten on a permanent one (POD4 can offer that; it didn't,
      POD4 reading 11).
    - Parlour's own User-Agent, with no contact address.
    - Any status but 200/304 is a failure.
11. **The feed URL never leaves the database.**
    - It isn't written to logs, job errors or notices, because paid
      feeds put a private token in it.
    - Errors read "the feed answered HTTP 503", "could not fetch the
      feed: it took too long", "not an RSS feed (the document's root
      is <html>)".
12. **The parser is lenient where real feeds are untidy.**
    - *Dates:* a wrong weekday is ignored. A zone RFC 822 doesn't
      know (`BST`, `CEST`) is read as UTC, so it may be an hour or
      two off rather than missing.
    - *Lengths:* a duration or an enclosure length of 0 is none.
      Minutes past 59 are read as written.
    - *Text:* Latin-1 is read when the bytes aren't UTF-8, and common
      HTML entities are read.
    - *Descriptions:* kept as the feed's HTML. Showing them safely is
      POD6's.
    - *Transcripts:* the first one the item lists is stored. Showing
      transcripts is deferred (D35).
13. **After the 24th failure, the count keeps counting.**
    - Checks by hand still count.
    - Turning auto-download back on without a success gives the feed
      one scheduled check: one more failure turns it off again, and a
      success resets the count.
    - A success never turns auto-download back on by itself.

### Episode download and retention (POD3, built 2026-11-26)

POD3 downloads episodes into the writable podcast store and keeps each
show to its D32 limit. It rides on POD2's `refresh_podcasts` job: the
same worker and the same run, right after a show's check, so there is
no second scheduler. Nothing is added to the API or the UI. POD4 manages
feeds and episode lists, and POD5/POD6 play episodes.

There is no migration: every column POD3 writes is POD1's. The flow
`crates/server/tests/podcast_download_flow.rs` runs all of it on a
fresh database. It uses a local stub feed-and-enclosure host, real
ffprobe, and a throwaway folder as the store. The code is
`parlour_catalog::podcast_download` (store, download, retention),
`parlour_catalog::podcasts::refresh_podcasts` (the run), and
`crates/server/src/podcasts.rs` (the job).

Audiobookshelf's behaviour quoted here was checked on 2026-11-26
against its server source on `master` (its podcast manager and podcast
controller). That reading was for behaviour only, and nothing was
copied.

**The store (D32).**

- **What it is.** A podcasts library's folder (its `library_paths`
  row) is the writable podcast store. If a library has several
  folders, the first by path is used. Setting it up is the explicit
  ops step:
  1. Mount a writable folder that Parlour owns.
  2. Create a podcasts library with that folder.

  The store is never a video or book media mount, and never a
  read-only mount flipped to writable. Parlour never creates the
  store's own folder, only the show folders inside it.
- **Overlap guard.** A store that is, contains or sits inside any
  other library's folder is refused for every download, naming the
  library it overlaps. So a podcasts library pointed into `/media`
  writes nothing.
- **The oryx test instance.** `docker-compose.yml` mounts a named
  volume, `parlour-podcasts`, at `/podcasts`. It sits beside the
  artwork and cache volumes, under the instance's own data. After the
  owner's next `docker compose up -d`, an admin creates a podcasts
  library whose folder is `/podcasts` (Server → Library already allows
  it). This run did not restart or change the live instance.
- **Production.** Which disk on sullivan holds the store is **still
  to be decided** (D32: no ABS equivalent, deferred until Jordan
  asks), and it must be answered before POD3 runs there. The answer
  replaces the named volume with a bind mount such as
  `/srv/podcasts:/podcasts`.
- **A show's folder** (`podcasts.folder_path`) is named the first
  time an episode is downloaded for the show:
  - **The name** is the show's title as a safe name: no `/ \ : * ? " < > |`
    or control characters, white space folded, no leading or trailing
    dots, at most 150 bytes, and `Podcast` if nothing is left.
  - **Clashes.** If another show holds the name, or a folder of that
    name is already on disk (not Parlour's to share), the show gets
    ` (2)`, ` (3)` and so on.
  - **After that** the stored folder is used even when the title
    changes. If the folder disappears, it is made again inside the
    store.
- **Never scanned.** A podcasts library's rows come from its feeds,
  so a library scan (the nightly one scans every library) does nothing
  for it. Before POD3 a scan walked a podcasts library as if it held
  videos, and a video podcast's `.mp4` in the store would have become
  a film.

**New episodes (the scheduled path).** After a successful check (a
200 or a 304) of a show with `auto_download` on, the run downloads the
show's new episodes. It works out which ones as Audiobookshelf does:

- **The cutoff** is the publish date of the newest downloaded episode.
  When none is downloaded, it is the show's previous check
  (`last_checked_at` before this check). For a show never checked, it
  is the time the show was added (`created_at`).
- **New** means not downloaded and published strictly after the
  cutoff. An episode without a date is never new.
- **Order and limit.** Newest first (the guid breaks ties), at most
  the show's `max_new_episodes_per_check` (default 3; 0 = no cap).

So:

- **The back catalogue** of a newly added show is never downloaded on
  its own.
- **A later check** never works through older episodes: once the
  newest download moves the cutoff, everything older is old.
- **Retention.** An episode it took back to "not downloaded" is older
  than the newest download, so a check never fetches it again.

A check by hand (`{"podcast_id": …}`) downloads in the same way when
the show's `auto_download` is on, and only checks the feed when it is
off.

**A download** (`podcast_download::download_episode`):

1. **The show's folder** is found or named (above). If that fails,
   the download fails.
2. **Fetch.** The enclosure is fetched into a hidden temporary file,
   `.{episode id}.part`, in the show's folder. Only a 200 is an
   answer; redirects are followed. The download fails when:
   - nothing arrives for 60 s;
   - it takes over 3 hours;
   - it passes 4 GB;
   - it arrives empty.
3. **Probe.** The file is probed with ffprobe the way the scanner
   probes a book's tracks (`audiobooks::probe_audio`). A file with no
   audio stream ffprobe can read is a failure (an HTML paywall page,
   say).
4. **Name and rename.** The file is renamed into place as the
   episode's title, made safe as above (`Episode` if nothing is left).
   - **The extension** is the enclosure URL's, when it is a known
     audio one. Otherwise it comes from the probed container (`mov` →
     `m4a`), then the MIME type, else `audio`.
   - **Clashes.** If that name is on disk or is another episode's
     file, the name gets the episode's short id (` [1a2b3c4d]`), then
     its full id.
5. **The row.** Only then are `file_path` (absolute), `file_size_bytes`,
   `file_duration_ms`, `file_container`, `file_codec` and
   `downloaded_at` filled, in one update:
   - **Container** is ffprobe's first format name, as for book tracks.
   - **Codec** is the default audio stream's, else the first one's.

**A failure leaves no trace on disk.** On any failure the temporary
file is removed, nothing is renamed, and the row stays undownloaded
with every `file_*` column NULL. If the row can't be written after the
rename, the file is removed again. So a failed or partial download
never leaves a file an episode row points at.

**How failures are reported.**

- **On the job.** Each failure is listed under "*show* — *episode*",
  for example `the enclosure answered HTTP 404` or `the download could
  not be read: …`.
- **Never the URL.** The enclosure URL is never written to the log or
  the job, because paid feeds put a private token in it. Request
  errors drop it (`without_url`).
- **The job.** A download failure doesn't fail the job; only a
  database error does.
- **Retries.** A failed new episode is tried again by the next check
  only while it is still new, which means something newer had already
  been downloaded. Otherwise it waits to be downloaded by hand.

**Download by hand.** A `refresh_podcasts` job queued as
`{"episode_id": …}` downloads that one episode whatever its show's
schedule or `auto_download` says. That is the hook POD4's "download"
will queue, mirroring POD2's `{"podcast_id": …}` for "refresh now".

- **No feed check.** It doesn't check the feed.
- **Already downloaded.** An episode that already has a file is left
  alone (`already_downloaded` in the result).
- **No retention.** Retention doesn't run, as in Audiobookshelf, where
  only automatic downloads trigger it.
- **Bad ids.** A job whose `episode_id` isn't an id fails. One naming
  an episode that doesn't exist does nothing.

**Retention** (`podcast_download::apply_retention`):

- **When it runs.** Once after the downloads of every successful check
  of a show with `auto_download` on.
- **What it does.** When the show holds more downloaded episodes with
  a publish date than its `max_episodes_to_keep` (0 = keep all, the
  default), it removes the oldest: by `published_at`, then guid.
- **At most one per check.** Lowering the limit from 10 to 3 trims one
  episode per check until three are left.
- **What counts.** Episodes downloaded by hand count toward the limit.
  Episodes without a date are neither counted nor removed.
- **Removal:**
  1. The file is deleted from disk (a file already gone is fine).
  2. Then the row's `file_*` columns and `downloaded_at` are cleared.
     The row, its progress and everything the feed describes stay.
     The episode is "not downloaded" again and can be downloaded by
     hand.
- **If the file can't be deleted**, the row is left as it was and the
  failure is listed on the job under "*show* — *episode*".

**The job's result** adds these counts to POD2's: `downloaded`,
`download_failed`, `already_downloaded`, `removed` and
`remove_failed`. The Jobs page now calls the job "Podcast check and
download" (it was "Podcast feed check"). Its failure notice changed to
match.

**Feed client.** The client feed checks use now carries no overall
time limit, so long downloads can share it. The 60 s limit is set on
each feed request instead, so feed checks behave as before.

**Unchanged.**

- **Bundle.** It stays at format v1, with no table change and nothing
  new in `coverage.rs`. `podcasts`, `podcast_episodes` and episode
  progress stay out of it.
- **Downloaded files.** They stay in the old host's store. A show that
  moves comes back with its episodes not downloaded.
- **D36** is still open.

#### Readings made in POD3 — for the owner's veto

1. **Retention clears the file and keeps the episode (option b), with
   its progress.**
   - *ABS differs:* it deletes the file, the episode record and its
     listening progress.
   - *Why (b):*
     - Here every feed episode has a row whether or not it is
       downloaded (POD1 reading 3). ABS's feed list still shows a
       removed episode, not downloaded and downloadable by hand,
       which is exactly the row (b) leaves.
     - Deleting the row (a) would make POD2's next check insert it
       again as a new row. Stopping that needs a record of removed
       guids, which is a migration, for no difference a listener
       sees.
     - The kept progress means a finished episode still reads as
       played, and a re-download resumes.
   - *If vetoed:* deleting the episode's `book_progress` rows as well
     is one statement. Deleting the row needs that tombstone.
2. **"New" is ABS's rule: published after the newest download, else
   after the previous check, else after the show was added.**
   - *Why:* it is what ABS's scheduled check does. It means a
     retention-cleared episode, an older episode or a show's back
     catalogue is never fetched on its own.
   - *Cost:* if a show's first new episode fails to download, it isn't
     retried by the next check, because the cutoff has moved on with
     the check. It waits for a download by hand. An episode without a
     publish date is never downloaded automatically.
3. **Newest first.**
   - ABS takes the new episodes in the feed's own order, which is
     newest first in nearly every feed.
   - Sorting by date gives the same result on those feeds and the
     right one on the rest.
4. **Retention runs once per successful check, whether or not the
   check downloaded anything.**
   - *ABS differs:* its code removes at most one episode after each
     automatic download.
   - *Why:* D32's text is "at most one per check", and this follows
     it. It also means a lowered limit is reached one episode per
     check even when nothing new arrives.
   - *Not run:* after a failed check, or after a download by hand
     (ABS doesn't run it there either).
5. **What retention counts.**
   - Downloaded episodes with a publish date, including ones
     downloaded by hand. The oldest goes first, by date then guid.
   - Undated episodes are neither counted nor removed, as in ABS.
6. **The store is the podcasts library's folder.**
   - The first by path if there are several. Parlour never creates it.
   - A store overlapping any other library's folder is refused.
   - On oryx it is a `parlour-podcasts` volume at `/podcasts`.
   - Production's disk is still to be decided (D32).
7. **Folder and file names.**
   - The show's folder is named once, from its title, and kept when
     the title changes.
   - An episode's file is its title plus an extension.
   - Clashes get ` (2)` for folders and the episode's short id for
     files. ABS appends a random id.
   - A folder already on disk that Parlour didn't name is never
     shared.
8. **Temp file, probe, then rename.**
   - Only a file that probes as audio is kept.
   - If the server stops between the rename and the row update, the
     file can be left with no row pointing at it, never the reverse.
     The next attempt picks a fresh name. Nothing tidies such a file
     yet.
9. **Download limits.**
   - 60 s without data, 3 hours in all, 4 GB per file, 15 s to
     connect, redirects followed, only a 200 accepted.
   - D32 has no disk cap, and none is added. The 4 GB is a per-file
     sanity limit.
10. **Failures are on the job, not the episode.**
    - No column records a failed download (no migration).
    - The job lists each failure by show and episode title.
    - POD4's episode list will show "not downloaded", and the error is
      on the Jobs page.
11. **Download by hand rides on `refresh_podcasts`.**
    - The payload is `{"episode_id": …}`, the same worker as the
      checks, so a download by hand never races a check of the same
      show.
    - It skips an episode that already has a file. ABS would download
      a second copy.
12. **Podcasts libraries are never scanned.** The scanner returns an
    empty report for them. This also covers the file watcher and the
    nightly scan.

### Managing feeds (POD4, built 2026-11-26)

POD4 gives admins the screens and API to manage a podcasts library's
shows: add a show by its feed address, import and export OPML, refresh
a show now, download an episode by hand, and edit a show's D32
settings. Shows get a management page, and the Dashboard's
failing-feed notice links to it. Listening is not here: the listener
API is POD5's, and the podcast pages and player are POD6's.

There is no migration: every column POD4 writes is POD1's. The bundle
stays at format v1, with nothing new in `coverage.rs`. The flow
`crates/server/tests/podcast_manage_flow.rs` runs it all on a fresh
database, against a local stub feed and enclosure host (never the real
internet), with real ffprobe and a throwaway store.

- **The code:**
  - `crates/server/src/podcast_admin.rs` holds the routes.
  - `parlour_catalog::podcasts` gains `add_podcast`,
    `check_feed_url` and `ShowSettings`.
  - `parlour_catalog::opml` is the OPML reader and writer.
- **The pages:** Server → Podcasts (`/admin/podcasts`) and a show's
  page (`/admin/podcasts/{id}`).

Audiobookshelf's behaviour cited in this section and its readings
(what ABS does on add, on OPML import, on removal) is from general
knowledge of ABS. Unlike POD1–POD3, it was **not** re-checked against
ABS's source in this run, so treat those remarks as unconfirmed.
Nothing was copied. The D31/D32 rules themselves are as decided.

**Who (D31).** Every POD4 route is under `/api/v1/admin/` and answers
404 to anyone who isn't an admin, as the rest of the Server area does.
Admins manage every podcasts library, whatever their own profile's
grants say, as on Server → Library. Everyone with access to a podcasts
library sees every show in it: there are no subscriptions, and the
grant path is POD5/POD6's.

**The API.**

| Route | What |
|---|---|
| `GET /api/v1/admin/libraries/{id}/podcasts` | The library's shows, by sort title. For each: episodes, downloaded, last check, last error, failures in a row, `auto_download`, `check_schedule`. |
| `POST /api/v1/admin/libraries/{id}/podcasts` `{"feed_url"}` | Add a show (below). 201 with the show page's data. |
| `POST /api/v1/admin/libraries/{id}/podcasts/opml` | Import: the OPML file is the request body. 200 with a report. |
| `GET /api/v1/admin/libraries/{id}/podcasts/opml` | Export: the library's shows as an OPML file download. |
| `GET /api/v1/admin/podcasts/{id}` | A show, as on its page. It holds the details, the feed address, the folder and the check state. It also holds `settings` and ABS's `default`, whether a check is waiting, and the episode list. |
| `PUT /api/v1/admin/podcasts/{id}/settings` | D32's four settings, all four required. |
| `POST /api/v1/admin/podcasts/{id}/refresh` | "Refresh now". 202 with the job. |
| `POST /api/v1/admin/podcast-episodes/{id}/download` | "Download". 202 with the job. |

A library that isn't a podcasts library is a 400, and an unknown one
is a 404.

**Adding a show by its address.**

1. **The address** is trimmed. It must be an `http` or `https` URL
   with a host, at most 2048 characters. It is stored as typed.
2. **A duplicate** is refused with a 409 naming the show already there.
   A duplicate is the same address in the same library (POD1's
   uniqueness). Another podcasts library may hold the same feed.
3. **The feed is fetched and read at once**, with POD2's fetch and
   parser and POD2's limits (60 s, 64 MB, 200 only). If it fails, the
   add is refused with a 400 and nothing is stored, for example:
   - "That address can't be added: not an RSS feed (the document's
     root is \<html\>)."
   - "… the feed answered HTTP 404."
   - "… could not fetch the feed: could not connect."
4. **Stored with its first check.** A feed that reads is stored in one
   transaction together with its first check: the show as the feed
   describes it, its validators, `last_checked_at`, and a row per
   episode. A new show is therefore never empty, and never a row that
   fails every check. A feed without a title is named "Untitled
   podcast" (or, from OPML, after its outline).
5. **The new show** has ABS's defaults: auto-download off, hourly,
   keep all, 3 per check. Nothing is downloaded. As in POD3, the cutoff
   for "new" is this first check, so a show's back catalogue is never
   downloaded on its own. An episode is downloaded by hand, or the next
   new one arrives once auto-download is on.
6. **The folder** stays NULL until the first download, which names it
   as POD3 does (reading 2).

**OPML import.**

- **Reading the file.** The root must be `<opml>` with a `<body>`. Every
  `<outline>` with a non-blank `xmlUrl` is a feed, at any depth (apps
  nest them in folders), whatever its `type`. Its name is its `text`,
  else its `title`. A file that isn't OPML is refused with a 400, and
  so is an empty one. The body limit is the server's default, 2 MB.
- **Each feed** is added exactly as an add by address is: checked,
  fetched and read, then stored with its first check. Four are fetched
  at a time.
- **Skipped, never duplicated:**
  - a feed already in the library;
  - an address listed again in the same file (the first one counts).
- **The report** has the feed count, how many were added, skipped and
  not added, and one line per feed in the file's order. Each line has
  the feed's name (its outline's name, else the stored title, else
  "Feed *n* in the file"), the outcome, the show's id when there is
  one, and why a feed was not added. It never includes an address.

**OPML export.**

- **The format.** An OPML 2.0 file named after the library (ASCII
  letters, digits and dashes, e.g. `Podcasts.opml`). There is one
  `<outline type="rss">` per show, sorted by title, with the title as
  `text` and `title` and the feed address as `xmlUrl`.
- **Its use.** This is how shows move to a new host until the bundle
  carries them (POD1's note). The importer reads the file back: the
  flow does exactly that round trip into a second library.
- **Addresses.** The export includes each feed's full address, private
  token and all, because a feed can't be re-added without it. The page
  says so.

**Refresh now.** "Refresh now" queues POD2's hook, a `refresh_podcasts`
job `{"podcast_id": …}`. It checks the show whatever its schedule.

- **Failures.** It counts toward the failures in a row like any other
  check, so three failed refreshes raise the Dashboard notice.
- **Downloads.** When the show's `auto_download` is on, it also
  downloads new episodes and applies retention (POD3).
- **A second click** while a check is waiting returns the waiting job
  instead of queueing another.

**Download by hand.** "Download" queues POD3's hook, a
`refresh_podcasts` job `{"episode_id": …}`. It downloads that one
episode, whatever the show's settings say.

- **Already downloaded.** An episode that already has a file is refused
  with a 409 "This episode is already downloaded.", and nothing is
  queued. The job leaves such an episode alone anyway.
- **A second click** returns the waiting job.
- **Failures.** A failed download is listed on the job as POD3 lists
  it, and the episode stays "not downloaded".

**Per-show settings (D32).** `PUT …/settings` takes the four settings,
checked before anything is stored:

- **`auto_download`** turns the download of new episodes and the
  scheduled check on together (POD1 reading 5). Turning it back on
  after the 24-failure rule turned it off is allowed (POD2 reading 13).
- **`check_schedule`** must be a five-field cron expression
  (`parlour_jobs::cron`, UTC) that fires within the next five years.
  Its white space is folded.
  - **Refused:** an unreadable one (`every hour`, `@hourly`, four or six
    fields, `61 * * * *`), with "The check schedule can't be read: …".
    This is POD2 reading 4's promise: it is no longer only logged and
    run hourly.
  - **Also refused:** one that never fires (`0 0 31 2 *`), with "The
    check schedule never fires."
- **`max_new_episodes_per_check`** and **`max_episodes_to_keep`** must
  each be 0 to 100 000. 0 means no cap and keep all, as before.
- **Refused edits.** An unknown field is refused (422), and a refused
  edit changes nothing.
- **What the edit changes.** `updated_at` moves only when a value
  changes. The next scheduled tick reads the new schedule, and the next
  check reads the new limits.

**The pages.**

- **Server → Podcasts** (`/admin/podcasts`, in the Server sidebar):
  - **Choosing a library.** With several podcasts libraries, a picker
    chooses one (`?library=`). With none, the page says how to make
    one: a library of kind Podcasts whose folder is a writable folder
    kept for Parlour, never a media folder.
  - **The table.** It lists the shows, with episodes downloaded / total,
    last check, status (OK, not checked yet, or failing with the count
    and the last error), and auto-download and its schedule. Each show
    has "Manage" and "Refresh now".
  - **Below the table.** The "Add a show" form, and the OPML import (a
    file picker) and "Export OPML" link. The import's report is shown
    as a table that links to each added or existing show.
- **A show's page** (`/admin/podcasts/{id}`):
  - **Feed checks.** Last checked, failing (count and last error),
    scheduled checks on/off, and "Refresh now". The button reads
    "Check queued…" while one waits.
  - **Settings.** The four settings with their meanings, "Save
    settings" and "Back to the defaults".
  - **Details.** Feed address (admins only), folder (or "named at the
    first download"), type, language, genres, explicit, and when it
    was added.
  - **Episodes.** The episode list:
    - **Order.** A serial show's list runs oldest first, and an
      episodic one's newest first. Undated episodes come last.
    - **Columns.** Title with season/episode and trailer/bonus,
      published, length, and downloaded (date and size), "Queued…" or
      a "Download" button.
  - **Polling.** While a check or a download waits, the page looks
    again every 2 s (for at most ten minutes), so the result appears
    without a reload.
- **The Dashboard notice.** The failing-feed notice
  (`podcast_feed_failing`) now links to its show's page ("Open the
  show"), from the `podcast_id` in its detail. POD2 left it with no
  link.

**The feed address stays out of sight.**

- **Where it never appears:** in an add's or an import's error, in the
  import report, in the log, in a job (payload, errors or result) or in
  a notice. This covers both the flow test and the browser check.
- **Where admins see it:** on the show's page and in the OPML export.
- **Why the import isn't a job:** a job's payload is shown on the Jobs
  page and in its API, so the import runs in the request instead
  (reading 3).

**Not built here.**

- **Removing a show**, and what removing does to its downloaded files
  (ABS asks).
- **Rewriting a feed's address** after a permanent redirect (POD2
  reading 10).
- **Editing a show's title.**
- **Showing artwork or descriptions.**
- **Directory search.** That is POD7. *Built in POD7 ("Directory
  search (POD7)").*
- **The library's finish rule for episodes.** That is POD5 (reading
  10).

#### Readings made in POD4 — for the owner's veto

1. **An add fetches and checks the feed in the request, and stores the
   show with that first check.**
   - *Not a job:* it is neither queued as a `{"podcast_id"}` job nor
     stored first and checked later.
   - *Why:* validating at input needs the fetch anyway, so one fetch
     both refuses a non-feed and fills the show. A new show is never an
     empty row, and never a row that fails every check. ABS also reads
     the feed when a show is added.
   - *Cost:* the admin waits for the fetch, up to 60 s for a slow host.
2. **A show's folder is named at its first download, as POD3 does, not
   when it is added (ABS does it at add).**
   - *Why:*
     - ABS fixes the folder at add because its add form has the admin
       choose the folder. Parlour has no such choice: the store is the
       library's.
     - Adding needn't touch the disk, so shows can be listed before
       the store is mounted or while it is full. A show nobody
       downloads from (auto-download is off by default) leaves no
       empty folder.
     - There is one naming path, POD3's, with its clash rules and
       overlap guard.
   - *If vetoed:* `podcast_download::show_folder` is called at add.
3. **OPML import runs in the request, four feeds at a time, not as a
   job.**
   - *Why:* a job's payload and errors are shown on the Jobs page and in
     its API, and an import's input is a list of addresses that may hold
     tokens. In the request, nothing but the report leaves, and the
     report names feeds only.
   - *Cost:* a long file holds the request open. Ten feeds on good
     hosts take a few seconds, while a hundred dead hosts take minutes.
     ABS runs its import as a background task.
   - *If vetoed:* a job that reads the file from the cache folder, never
     from its payload.
4. **What counts as a feed in OPML.**
   - Every `outline` with an `xmlUrl`, at any depth, whatever its
     `type`. The name is `text`, else `title`.
   - Repeats in the file: the first counts. There is no cap on the
     number of feeds, and the file limit is 2 MB.
   - The export is OPML 2.0, `type="rss"`, with `text`, `title` and
     `xmlUrl`, sorted by title.
5. **A feed's address is stored as typed (trimmed), and duplicates are
   exact matches.**
   - Only `http`/`https`, at most 2048 characters.
   - It is not normalised, so `HTTPS://Host/feed` and
     `https://host/feed` would be two shows. Normalising could change a
     token's encoding, and ABS stores the address as given too.
6. **Admins see the feed address; nobody else, and no error, ever
   does.**
   - It is on the show page (with a note that it may hold a token) and
     in the OPML export, which is useless without it. ABS shows it to
     admins.
   - *If vetoed:* the page masks the query string.
7. **Settings checks.**
   - *The schedule:* a five-field cron that fires within five years,
     with its white space folded. Every-minute schedules are allowed,
     as ABS allows them.
   - *The limits:* 0–100 000. ABS has no upper bound, and this one only
     catches typos.
   - *The edit:* all four settings in one PUT, as AB11's settings are.
8. **A show's title and sort title can't be edited here.**
   - *Why:* every successful check rewrites them from the feed (POD2
     reading 6), so an edit would last until the next check. Nothing on
     the management page needs it.
   - *If wanted:* a "keep my title" flag is a migration, plus one
     condition in the check.
9. **A second click doesn't queue a second job.**
   - "Refresh now" and "Download" return the job already waiting for
     the same show or episode, if there is one. A job that is already
     running doesn't count, so a refresh after it starts queues a new
     one.
   - Downloading an episode that has a file is a 409, not a queued job
     that does nothing.
10. **The podcasts library's finish rule is POD5's, not POD4's.**
    - AB11's `…/audiobook-settings` still accepts audiobooks libraries
      only. Nothing writes episode progress until POD5's API, so
      widening it now would edit a rule nothing reads.
    - The rule applies with ABS's default (10 s) when POD5 starts
      writing progress. POD5 widens the settings route, or adds a
      podcasts twin, with the API that uses it.
11. **Left out of POD4:** removing a show, rewriting an address after a
    permanent redirect, artwork and descriptions.
    - *Removing a show:* it isn't in POD4's line. ABS asks whether to
      delete the files too, and that is the owner's call when the
      feature is wanted.
    - *Artwork:* it isn't hot-linked from the publisher, because that
      would hand every admin's address to the publisher. Caching it is
      POD6's, with the listener pages. *POD6 (2026-11-27): cached at
      each check and after an add ("Web pages and player (POD6)").*
    - *Descriptions:* they are the feed's HTML, which POD6 shows
      safely. *POD6: through an allowlist sanitiser in the page.*
12. **Admins manage every podcasts library, whatever their profile's
    grants.** This is the Server area's existing rule (Server → Library
    lists every library). The grant path (POD1's join through
    `library_access`) is for the listener API and pages, POD5/POD6.
13. **Episode order on the show page:** a serial show's runs oldest
    first and an episodic one's newest first. Undated episodes come
    last, and the guid breaks ties. This is what podcast apps do, and
    POD1 reading 12 kept `podcast_type` for it.

### Native API (POD5, built 2026-11-27)

POD5 is the listener side of podcasts: the API a profile's player (the
web UI in POD6, the apps in Phase 12) uses to list shows and episodes,
stream an episode, and keep its progress and listening sessions. Admin
management stays where POD4 put it, under `/api/v1/admin/`.

Two owner decisions of 2026-11-27 bind it:

- **D32, amended:** an episode that is **not** downloaded streams by
  redirecting to its enclosure. The owner accepted the consequences:
  - a paid feed's tokenised enclosure URL reaches every listener with
    access, through the redirect;
  - the publisher sees each such listener's IP;
  - a redirected stream gets no AB10 audio plan.
- **D41:** a per-profile "explicit content" flag, on by default, that
  only an admin sets.

The code:

- `crates/server/src/podcast_api.rs` holds the listener routes.
- `playback.rs` gains the episode AAC fallback, and the reports gain
  `episode_id`.
- `listening.rs` gains `Target::Episode`.
- `book_settings.rs` gains the podcasts twin.
- `profiles.rs` gains the admin D41 routes.

One migration, `20261204000000_pod5_explicit_episode_sessions.sql`, adds
`profiles.allow_explicit` and `listening_sessions.episode_id`. The
timestamp is an ordering key only; it was written on 2026-11-27. The
flow `crates/server/tests/podcast_api_flow.rs` runs all of it on a
fresh database, against a local stub feed and enclosure host, with real
ffmpeg/ffprobe.

Audiobookshelf's behaviour cited here (its per-user explicit
permission, its OPML export) is from general knowledge of ABS. It was
not re-checked against ABS's source in this run. Nothing was copied.

**Who sees what.** A profile reaches a show only through its
`library_access` grant on the show's podcasts library (D31, POD1's
join). It reaches an explicit show (`podcasts.explicit`, from the
feed's `itunes:explicit`) only while its `allow_explicit` is on (D41).
Anything it can't reach answers 404, the same not-found as a show that
doesn't exist. That covers:

- the show and its episodes;
- an episode's stream and AAC copy;
- its progress, read or written;
- a playback report naming the episode.

A stream token proves *who* asks. The grant and the flag are checked
again on every stream request, so taking either away stops playback at
once (reading 2).

**The API.**

| Route | What |
|---|---|
| `GET /api/v1/podcasts?library_id=&q=` | The shows the profile can reach, by sort title; `q` matches title or author. For each: the show as its feed describes it (title, author, description, language, `explicit`, `podcast_type`, genres), its last check, episodes, downloaded, newest episode date, and this profile's finished and in-progress counts. |
| `GET /api/v1/podcasts/{id}?client=` | One show and its episode list. A serial show's runs oldest first, every other show's newest first, undated last (POD4 reading 13). Each episode is as below. |
| `GET /api/v1/podcast-episodes/{id}?client=` | One episode, with its show's title. With a client that needs the AAC copy, it starts making the copy now, as the book detail does for its resume track. |
| `GET /api/v1/podcast-episodes/{id}/stream` | A downloaded episode's file with HTTP Range. An episode not downloaded is a **302 to its enclosure**. |
| `GET /api/v1/podcast-episodes/{id}/aac.m4a` | AB10's AAC-in-MP4 copy of a downloaded episode, with Range. It answers 503 + `Retry-After` while the copy is made. |
| `GET\|PUT\|DELETE /api/v1/podcast-episodes/{id}/progress` | This profile's progress on the episode (below). |
| `GET /api/v1/podcasts/opml?library_id=` | The reachable shows as OPML 2.0. **Admins only** (reading 10). |
| `GET\|PUT /api/v1/admin/libraries/{id}/podcast-settings` | A podcasts library's finish rule (below). |
| `GET /api/v1/admin/profiles`, `PUT /api/v1/admin/profiles/{id}/explicit` | Every profile with its D41 flag, and setting it (`{"allow_explicit": bool}`). |

The stream and AAC routes are authorised like a book track's: a stream
token (6 h, HMAC) for that episode, or a signed-in session. Without
either they answer 401, before saying whether the episode exists.

**An episode, as a listener sees it.** Each episode carries:

- the feed's fields (title, subtitle, description, published, season,
  episode number, type);
- `duration_ms`, the feed's length, and `length_ms`, the length
  progress uses: the download's, else the feed's;
- `downloaded`, with the download's date, size, length, container and
  codec;
- `stream_url` (tokenised) and `delivery`, one of:
  - `file`: the download;
  - `aac`: its AAC copy;
  - `enclosure`: the stream route, which redirects to the publisher;
- `content_type`: for an enclosure, the type the feed announces;
- `plan`, with `?client=` (AB10's profiles), for a downloaded episode
  only;
- this profile's `progress`, in the book's shape.

No answer body carries the feed's address, the enclosure's, the
chapter or transcript links, or the publisher's artwork address
(reading 12).

**Streaming (D32 as amended).**

- **Downloaded:** the file in the podcast store, with Range, typed by
  its extension. A `?client=` the file doesn't suit gets
  `delivery: aac` and the AAC route. That is AB10's fallback, widened
  from `kind = 'audiobook'` to downloaded episodes (reading 4).
- **Not downloaded:** `302 Found` with:
  - `Location` set to the enclosure URL;
  - `Cache-Control: no-store`;
  - `Referrer-Policy: no-referrer`;
  - no body.

  Only an absolute `http`/`https` enclosure is followed. Anything else
  is a 404 with a plain message that never repeats the address. The
  redirect's `Location` is the one place the enclosure URL leaves the
  server. It is not in any JSON answer, error, log line, job, notice or
  Activity line. The flow checks the answers and the jobs, notices,
  Activity and sessions tables for the fixture token.
- **Admin stop.** An admin's stop on Now Playing revokes the episode's
  stream token, so the next request is refused (401). A player that has
  already followed a redirect holds the publisher's address, and that
  can't be stopped (reading 3).

**Progress.** Progress is AB11's rules, unchanged: the same pure
`apply_write`, upserted into `book_progress` on `(profile_id,
episode_id)` (POD1).

- **The length** is the download's `file_duration_ms`, else the feed's
  `duration_ms`. With neither, a position isn't clamped and never
  finishes the episode, as for a book of unknown length.
- **The finish rule** is the podcasts library's own
  `finish_remaining_seconds` / `finish_percent`, with ABS's 10 s by
  default.
- **The writes:**
  - A position is clamped to the length and stamps
    `last_listened_at`. A position within the rule finishes the
    episode, and a finished episode moved back before it is unfinished.
  - `finished: true` marks it finished, and `finished: false` on a
    finished episode puts the position back to 0.
  - A move of the position clears `hidden`.
  - `client_updated_at` is stored as sent (else the server's time). The
    last write to arrive wins.
- **Undownloaded episodes** take every write, with the feed's length
  (reading 5).
- **Not in the book sync.** Episode rows never appear in
  `GET /api/v1/audiobooks/progress`, and there is no episode bulk sync
  yet (reading 11).

**The finish rule (POD4 reading 10).** `GET|PUT
/api/v1/admin/libraries/{id}/podcast-settings` takes
`{"finish_remaining_seconds", "finish_percent"}`:

- the same columns and checks as AB11's (0–86 400 s, 0 = off; 1–100 %
  or null = off);
- a podcasts library only (any other kind is a 400, and
  `…/audiobook-settings` still refuses a podcasts library);
- an unknown field is refused (422);
- the next progress write reads it.

It is a twin rather than a widened route (reading 7). It is edited on
Server → Podcasts, under "When an episode counts as finished".

**Listening sessions (AB8, widened).**

- **The column.** `listening_sessions` gains `episode_id` beside
  `item_id`, exactly one set (`listening_sessions_one_target_check`),
  deleted with the episode.
- **The reports.** The player uses the same
  `POST /api/v1/playback/{start,progress,stopped}` with `episode_id`
  instead of `item_id`. Sending both, or neither, is a 400. Every AB8
  rule holds:
  - a new start replaces the profile's open session on the episode;
  - listening time is the player's measure, capped by the clock;
  - an hour's silence closes a session;
  - a report on a closed session opens a fresh one;
  - an admin stop answers the next report 409.
- **Where an episode's session shows:**
  - **Now Playing** shows it under its show (kind `podcast_episode`,
    the show where a series would be). It links to Server → Podcasts
    until POD6's pages exist. *POD6: it links to the episode's page,
    with the show's artwork.*
  - **Activity.** Its closed line reads "*user* listened to *Show ·
    Episode* for …". The `episode_id` and `podcast_id` are in the
    detail, with no item link, because an episode isn't a catalog item.
  - **The profile's history** (`/api/v1/listening/sessions`) gains
    `episode_id`, `podcast_id` and `show_title`. `item_id` is null for
    an episode.
  - **The stats' totals and days** count episodes. The per-book list
    stays books (reading 8).

**D41 in the UI.** Server → Users gains a **Profiles** table: each
account's profiles, with an "Explicit podcasts" checkbox (Allowed /
Hidden) and a note on what it does.

- **Who.** Only an admin can change the flag. A profile's own
  `PATCH /api/v1/profiles/{id}` ignores it.
- **Activity.** Each change writes a `profile.explicit_changed` line.
- **While off:** progress rows on explicit episodes are kept, only
  unreachable. Turned back on, they return.
- **Also covered:** the history, the stats and the admin's OPML export
  follow the flag.

**The bundle.** It is unchanged at format v1:

- `profiles.allow_explicit` is *left out* (`coverage.rs`). An imported
  profile starts with the default (on). Its bundle home is the
  format-v2 step (D40).
- `listening_sessions` stays *Proposed*, and its new `episode_id` goes
  with it (**D36 is still open**).
- Episode progress stays inside `book_progress`'s open proposal.

**Not built here.**

- The listener pages and player (POD6). *Built in POD6.*
- Show artwork (POD6 caches it; POD4 reading 11). *Built in POD6.*
- Directory search (POD7). *Built in POD7.*
- Bookmarks, playlists and collections (D35, CL2/POD phases).
  *Built in POD8 ("Playlists, collections and bookmarks (POD8)").*
- A bulk episode progress sync (CL2).
- Removing a show.
- Enforcing `library_access.max_rating` (reading 14).

#### Readings made in POD5 — for the owner's veto

1. **Route names: `/api/v1/podcasts` and
   `/api/v1/podcast-episodes/{id}`.**
   - *Not* the sketch's bare `/episodes`: `episode` already names a
     TV episode (`media_items.kind`), and POD4's admin routes say
     `podcast-episodes`.
   - The list takes `library_id` like `/api/v1/audiobooks`. Adding a
     show stays admin-only under `/api/v1/admin/` (D31), so there is
     no listener `POST /podcasts`.
2. **A stream token is not enough on its own: the grant and D41's flag
   are checked again on every stream request.**
   - *Books differ:* there a valid token alone streams for its 6 h.
   - *Why:* D41 says an explicit episode's stream is a 404 once the
     flag is off, and a token minted while it was on must not keep
     playing. Taking a grant away works the same way.
   - *Cost:* one indexed query per Range request.
3. **The redirect.**
   - *The status:* `302`, not 301/308, because it must never be cached
     as permanent: the episode may be downloaded later and then
     streams from Parlour.
   - *The headers:* `no-store`, and `no-referrer` so the publisher
     isn't told Parlour's address.
   - *What is followed:* only absolute http(s) addresses.
   - *What an admin stop can do:* it refuses the next redirect but
     can't take back one already followed. There is no channel to the
     client (UI3's stop is server-side only).
   - *Logging:* the request log (tower-http) records request paths,
     not response headers, so `Location` is not logged.
4. **AB10's fallback for episodes is a route of its own**
   (`…/podcast-episodes/{id}/aac.m4a`), on the same cached-ffmpeg
   runner.
   - *The stamp:* the download's size and `downloaded_at`, so a
     re-download after retention makes a new copy.
   - *Channels:* POD3 doesn't store the episode's channel count, so
     the copy keeps the source's channels at 128 kb/s, where a book's
     uses 64 kb/s per channel.
   - *After retention:* once retention removes the file, the route is
     a 404. The kept copy stays in the cache folder, as a book's does.
   - *Undownloaded episodes:* no plan and no fallback. That is D32's
     accepted cost.
5. **An undownloaded episode takes every progress write, mark-finished
   included, against the feed's length.**
   - *Why:* POD1 reading 9 left this to POD5. Since D32's amendment,
     such an episode plays through the redirect, so its position has
     to be writable like any other. Refusing a mark-finished on it
     would leave a played episode unmarkable.
   - *With no length at all:* nothing is clamped and nothing
     auto-finishes. A by-hand finish still works.
6. **`apply_write` is used as AB11 built it.**
   - *What it does:* a move of the position clears `hidden`, and
     un-finishing clears it too (that moves the position to 0).
     Finishing alone (`finished: true` with no move) keeps it, as
     AB11's tests pin.
   - *The wording:* POD1's and the brief's "a write that changes the
     position or the finished flag clears `hidden`" says slightly more
     than the code does. POD5 follows the code rather than change
     AB11's rule for books too.
7. **The finish rule is a twin, `…/podcast-settings`, not a widened
   `…/audiobook-settings`.**
   - *Why:* the audiobook route's body requires `parse_subtitles`,
     which means nothing for a feed. Widening it would either make
     podcasts send a dead field or make a required field optional for
     books.
   - *What's shared:* both write the same `libraries` columns with the
     same checks. The form sits on Server → Podcasts, where an admin
     manages the library.
8. **Listening sessions are widened, with a migration, not deferred.**
   - *Why:* ABS keeps podcast listening sessions like book ones (D29),
     and without one an episode's listen would leave no history, stats
     or Now Playing line.
   - *What the stats count:* their totals and days count episodes, and
     the per-book list stays books. A per-show list is POD6's, if its
     screen wants one. *POD6: added (`shows` in the stats, POD6
     reading 5).*
   - *Activity:* an episode's line has no item link (Activity's item
     link is to `media_items`) and names the episode in its detail.
9. **D41 as built.**
   - *Show-level only:* POD1 keeps no per-episode explicit flag (a
     feed's item-level `itunes:explicit` isn't stored), so an episode
     follows its show.
   - *Who sets it:* only admins, through new admin routes. ABS sets
     its permission on the user, and Parlour's unit is the profile
     (D29), so a kids' profile under a parent's account can differ.
   - *While off:* progress is kept, only hidden.
   - *Activity:* a change is logged in it.
10. **OPML: export only, and admins only.**
    - *Export only:* import stays POD4's admin import, because only
      admins add shows (D31).
    - *Admins only:* an OPML file is nothing without each feed's
      address, and POD4 reading 6 keeps those addresses (a paid feed's
      token) to admins. D32's amendment exposes *enclosure* addresses
      through the redirect, not feed addresses. A non-admin gets the
      admin surface's 404.
    - *What it lists:* unlike POD4's library export, this one follows
      the admin's own profile (its grants and D41's flag).
    - *If the owner wants listeners to export:* it is one extractor
      change, and the owner should know it hands out feed addresses.
11. **No bulk episode progress sync (`?since=`).**
    - *What exists:* the book sync stays books only, and per-episode
      GET is enough for Phase 11's "resumes on another client".
    - *Why not here:* the combined books-and-episodes sync is CL2's
      ("the sync endpoint"), and a second one-off endpoint now would be
      replaced there.
12. **What a listener's answers leave out.**
    - *Addresses:* the feed's and the enclosure's, both possibly
      tokenised.
    - *Links:* the Podcasting 2.0 chapter and transcript links, which
      are publisher addresses too. Showing transcripts is deferred
      (D35), and chapters are POD6's to fetch server-side if its player
      wants them. *POD6: read server-side by a route of their own, only
      start times and titles sent (POD6 reading 4).*
    - *Artwork:* POD4 reading 11 keeps it from being hot-linked, and
      caching it is POD6's. *POD6: `artwork_url`, Parlour's own route.*
    - *Descriptions:* they are passed as the feed gives them (HTML),
      for POD6 to show safely. *POD6: sanitised in the page (POD6
      reading 6).*
13. **Shows sort by sort title, and `q` matches title or author**, as
    the book list does for books. The episode order is POD4 reading
    13's.
14. **Known gap, not POD5's: `library_access.max_rating` is stored but
    enforced nowhere.**
    - It is written by `grant_access` (`libraries.rs`, "will enforce in
      1.3+"), yet TODO A6 is checked "enforced in SQL", and no query
      reads it.
    - POD5's run 1 found this, and D41 replaced the idea of tying
      explicit shows to it. Building the enforcement is outside POD5;
      it is noted for the review and fix loop.

### Web pages and player (POD6, built 2026-11-27)

POD6 is the listener's web UI for podcasts: a podcasts library's list
of shows, a show's page with its episode list, and an episode's page
with AB5's player. It is built on POD5's listener API. Three server
additions serve it:

- the show's artwork, cached by Parlour (POD4 reading 11);
- an episode's chapters, read server-side (POD5 reading 12);
- a per-show listening list in the stats (POD5 reading 8).

Two owner decisions bind it, as they bound POD5:

- **D32, amended:** an undownloaded episode plays through the redirect
  to its enclosure.
- **D41:** a profile with the explicit flag off never sees an explicit
  show.

There is **no migration**. `podcasts.image_path` is POD1's column,
filled here for the first time. The bundle stays at format v1. The
flow `crates/server/tests/podcast_pages_flow.rs` runs the server side
on a fresh database, against a local stub feed, artwork and chapter
host, with real ffmpeg/ffprobe and a throwaway artwork cache. The pages
and the player were checked in headless Chromium against a private
instance of this branch (below).

Audiobookshelf's and Overcast's behaviour cited here is from
`UI-AUDIO-REFERENCE.md` and general knowledge, behaviour only; nothing
was copied.

**The code:**

- `parlour_catalog::podcast_artwork` caches a show's artwork.
- `parlour_catalog::podcast_chapters` reads the JSON chapter list and
  a download's own chapter marks.
- `podcast_api.rs` gains the artwork and chapters routes, plus
  `artwork_url` on shows and episodes.
- `podcasts.rs` runs the artwork step after each check, and after an
  admin's add or OPML import.
- `listening.rs` gains the per-show stats and history artwork.
- `playback.rs` gains the show and artwork on Now Playing.
- `discover.rs` counts a podcasts library's shows for its Home tile.

**The pages:**

- `web/src/lib/PodcastShelf.svelte`: a podcasts library's view at
  `/library/{id}`.
- `web/src/routes/podcast/[id]`: the show page.
- `web/src/routes/episode/[id]`: the episode page and its player.
- `web/src/lib/feedHtml.ts`: the description sanitiser.
- `ShowArt.svelte` and `EpisodeMark.svelte`: the artwork tile and the
  episode-state mark.

**Who sees what (D31, D41).** Every page reads POD5's listener API, so
a profile sees only the shows its grant reaches. With D41's flag off it
sees no explicit show anywhere:

- **The shows list:** not listed, and not found by the filter (POD5's
  `q`).
- **Counts:** not counted on the Home library tile (reading 11).
- **Listening:** not in the stats or the history.
- **Its artwork and chapters:** 404.
- **Its show and episode pages:** "Not found", worded exactly as for a
  show that doesn't exist. Nothing of the show is on the page.

Shows and episodes are not in the global search (reading 14).

**The shows list** (`/library/{id}` for a podcasts library;
UI-AUDIO-REFERENCE §4.4 "Shows list").

- **The cards.** One card per show, by sort title. Each card has:
  - the artwork, else an initial tile;
  - the title, on two lines;
  - the author, in small capitals;
  - the episode count, and this profile's "in progress" and "played"
    counts.
- **The filter.** A box that filters by title or author on the
  server.
- **Current / All.** A switch: "current" shows are those with an
  episode this profile hasn't finished (reading 10). An empty library
  points admins to Server → Podcasts.

**The show page** (`/podcast/{id}`).

- **The header:**
  - the artwork;
  - the title and author;
  - the episode count, "Serial" for a serial show, the genres and the
    language;
  - an "Explicit" badge, which only a profile allowed explicit content
    can ever see;
  - this profile's played / in-progress counts;
  - the feed's description, shown safely (below) and folded when long;
  - for admins, a link to Server → Podcasts' page for the show.
- **The episode list.** The order is POD5's: a serial show runs oldest
  first, any other newest first. The list is shown 50 at a time.
- **Each row** has:
  - this profile's state (reading 7): a dot for new, a progress ring
    for part-played, a check for played;
  - the title, which links to the episode page;
  - the date, and the length, or for a part-played episode the time
    left;
  - the season, episode number and type (trailer / bonus);
  - a one-line summary;
  - an undownloaded episode's note: "Not downloaded to Parlour — plays
    from the publisher."
- **The buttons:**
  - **Play** (Resume for a part-played episode) opens the episode page
    and starts it.
  - **Mark as played** or **Mark as unplayed** writes progress through
    POD5. Unplaying by hand goes back to 0 (AB11).
  - For admins on an undownloaded episode, **Download** queues POD4's
    `{"episode_id"}` hook. The row says the download is queued, and
    the page looks again every 3 s until the file is in.

**The episode page and player** (`/episode/{id}`;
UI-AUDIO-REFERENCE §4.4 "Player"). It is AB5's book player, for one
episode.

- **The header:**
  - the show's name in small capitals, linking to the show;
  - the title, date, length, and season and episode;
  - this profile's state;
  - for an undownloaded episode, "plays straight from the publisher's
    server";
  - for admins, "Download to Parlour".
- **The player** (UI-AUDIO-REFERENCE C3–C5, C7):
  - The current chapter's title (else the episode's) sits over an
    episode-wide scrubber, with "Next chapter" beside it.
  - Elapsed and remaining time sit under the scrubber.
  - The transport is back 15 s, play / pause, forward 30 s.
  - Under it: Mark as played or unplayed, and Start over.
- **Below the player:**
  - the chapters, each with a check once played; the current one is
    filled to the playhead and shows the time left in it, and a click
    plays from it;
  - the show notes, shown safely.
- **Where the audio comes from** is POD5's `delivery` for the `web`
  profile:
  - `file` streams the download with Range.
  - `aac` waits for AB10's AAC copy, which answers 503 until it is
    made. The page says it is converting and why, exactly as the book
    page does.
  - `enclosure` sets the `<audio>` to the stream route, which
    redirects to the publisher (D32 as amended). The page says so.
    Because the redirect carries `Referrer-Policy: no-referrer`, the
    publisher's request has no Referer. If the publisher's copy can't
    be played, the page says the publisher's server may be down.
- **Progress** follows AB11's rules through POD5's routes, exactly as
  the book page writes it:
  - every 15 s while playing, on pause, on a seek, and with
    `keepalive` when the page goes away;
  - only once the page has played, so opening an episode changes
    nothing;
  - each write carries `client_updated_at`;
  - the audio's end writes `finished: true`.
- **The length** is the one the browser measures once it knows it;
  until then it is POD5's `length_ms` (the download's, else the
  feed's).
- **Listening sessions** use AB8's reports, widened by POD5, with
  `episode_id`:
  - the first play opens one;
  - a report goes every 15 s while the page is open;
  - the end, or leaving the page, closes it;
  - an admin's stop (409) pauses the player and says why.
- **`?play=1`** (from a row's Play) starts the episode once it has
  loaded.

**Show artwork (POD4 reading 11).** A page never loads the publisher's
artwork address. That address is the publisher's (it would hand them
every listener's IP), and a paid feed's may hold its token.

- **The copy.** The server fetches the image into the artwork cache,
  `PARLOUR_ARTWORK_DIR/podcasts/<show>/cover-<key>.<ext>`, and records
  it in `podcasts.image_path`.
  - `<key>` is the first 16 hex digits of the address's SHA-256, so it
    says nothing of the address.
  - Only a JPEG, PNG, WebP or GIF is kept, recognised by its first
    bytes, at most 20 MB. Anything else is refused, an SVG or an HTML
    error page included, because the copy is served from Parlour's own
    origin.
- **When** (reading 1):
  - after every check of the show, scheduled or "refresh now", in the
    `refresh_podcasts` job;
  - in the background, right after an admin adds the show by URL or
    by OPML import.

  A copy already made for the feed's current address is left alone,
  so a check costs nothing once the image is cached.
- **When the feed changes it:**
  - **A new address** gets a new copy at the next check, and the old
    file is removed.
  - **No artwork any more:** the copy is removed and `image_path`
    cleared.
  - **A fetch that fails** keeps the older copy. It is listed on the
    job as "*Show*: the artwork wasn't cached (*reason*)", by title,
    never by address (`without_url`). It doesn't fail the job. The
    job's result gains `artwork: {cached, failed}`.
- **Serving.** `GET /api/v1/podcasts/{id}/artwork` serves the copy:
  - to a signed-in profile that can reach the show (its grant and
    D41's flag); anyone else gets 404, and no sign-in gets 401;
  - typed by its extension, with `X-Content-Type-Options: nosniff`
    and `Cache-Control: private, max-age=604800`;
  - with no copy, 404.

  Shows, episodes, Now Playing, the history and the stats carry
  `artwork_url` (`…/artwork?v=<key>`, so a new copy is a new address),
  or null. A page shows an initial tile when it is null or the image
  fails, as the book pages do.

**Descriptions (POD5 reading 12).** Show and episode descriptions are
the feed's HTML, written by whoever publishes the feed. The pages never
insert it as it comes (`web/src/lib/feedHtml.ts`, reading 6):

- **How.** It is read into an inert document (`DOMParser`: no script
  runs and nothing is fetched) and written out again from an
  allowlist.
- **Kept:** paragraphs, line breaks, lists, quotes, bold, italic,
  underline and code. Headings become bold paragraphs.
- **Links:** only absolute `http`, `https` and `mailto` addresses.
  They open in a new tab with `rel="noopener noreferrer nofollow"`.
  Any other link (e.g. `javascript:`) keeps its text and loses the
  link.
- **Dropped with their contents:** scripts, styles, frames, forms,
  media and **images**. An image would hot-link the publisher.
- **Unwrapped:** every other element keeps its text and loses the
  element.
- **Attributes:** none survives except a link's checked `href`.
- **Plain text:** a description with no markup is escaped, and its
  line breaks are kept. A row's one-line summary is the same text,
  stripped of markup.

**Chapters (POD5 reading 12; reading 4).**
`GET /api/v1/podcast-episodes/{id}/chapters` returns
`{source, chapters: [{start_ms, title}], error}`. It is read
server-side when the player asks:

1. **The download's own chapter marks** (ID3 `CHAP`, MP4 chapters,
   read with ffprobe), when it has titled ones. No publisher is asked.
   `source: "file"`.
2. **Else, the feed's Podcasting 2.0 chapter list**
   (`podcast:chapters`, `chapters_url`), fetched now from the
   publisher, and read as JSON. `source: "feed"`.
   - **Limits:** 15 s and 1 MB.
   - **What is kept:** only `startTime` and `title`. A chapter's
     `img` and `url` are dropped, because both are publisher
     addresses. A `"toc": false` chapter is dropped too.
   - **The order:** by start time, the first of two at the same time
     kept, at most 1 000 chapters.
   - **When it fails:** the answer is an empty list and an `error` in
     plain words ("The publisher's chapter list couldn't be read: the
     chapter list answered HTTP 404."), never the address, so the
     player plays on without chapters.
3. **Neither:** `source: null`, no chapters.

The route answers 404 to a profile that can't reach the episode, as
POD5's routes do. Nothing is stored.

**Listening stats, history, Now Playing (reading 5).**

- **Stats.** `GET /api/v1/listening/stats` gains `shows`:
  - each show with this profile's listening time, sessions, distinct
    episodes and last listen, most listened first
    (UI-AUDIO-REFERENCE §4.4, "total time per show");
  - only shows the profile can reach (its grant, and D41's flag).

  Preferences → Listening shows it as "By show", under "By book".
- **History.** A profile's history and the admin's session list link
  an episode's session to its page, with the show's artwork.
- **Now Playing.** An episode's card links to the episode page
  (POD5's placeholder was Server → Podcasts) and shows the show's
  artwork. `/api/v1/admin/sessions` gains `podcast_id` and
  `artwork_url` (reading 12).

**Home (reading 11).** A podcasts library's Home tile counts the shows
the profile can reach ("2 shows"), D41's flag included. It used to
count the library's `media_items`, which for podcasts is always 0.

**The bundle.** It is unchanged at format v1:

- `podcasts` (and with it `image_path`) stays *left out*. A new host's
  first check of a show fetches its artwork again into the new host's
  own cache.
- `listening_sessions` and episode progress stay with **D36, still
  open**.

**Checked in a browser (2026-11-27).**

- **Why not the test instance.** The live test instance still runs a
  build from before POD1: its migrations stop at 2026-11-10 and it has
  no podcast tables. It wasn't redeployed, because that isn't this
  task's to do.
- **The setup.** The checks ran instead against a private instance of
  this branch on oryx's loopback:
  - its own throwaway Postgres;
  - a local stub host serving two feeds with tokenised addresses;
  - MP3, ALAC and PNG fixtures made with ffmpeg;
  - a Podcasting 2.0 chapter list;
  - a description with a `<script>`, an `<img onerror>`, an `onclick`
    and a `javascript:` link.
- **The run.** chrome-headless-shell 151 over CDP: **68 checks, 68
  passed**. They cover:
  - the shows list (order, artwork from Parlour, initial tile, filter,
    counts);
  - the show page (order, states, not-downloaded rows, admin Download,
    a safe description in which nothing injected ran);
  - the player on a download: play, ±seek, chapter tracking, pause
    saving the position, a chapter click, playing to the end marking
    it played;
  - an undownloaded episode playing through the redirect;
  - the ALAC download playing as its AAC copy;
  - the feed's chapter list;
  - Now Playing linking to the episode with the show's artwork;
  - the show page's ring, check and counts updating;
  - Mark played / unplayed;
  - a download landing;
  - Listening's "By show" and history links;
  - the Home tile;
  - a D41-off profile, which saw one show, found nothing by filtering
    and got "Not found" on the explicit show's pages.
- **What left Parlour.** The browser's only requests off Parlour's
  origin were the two redirected enclosures, both sent with no
  Referer. The stub host's log agrees. Artwork, chapter list and feeds
  were fetched by the server only, and the token never appeared in the
  server's log.

**Not built here:**

- **The persistent player bar** (UI-AUDIO-REFERENCE §4.2): a separate
  web task for books and episodes together (reading 8).
- **The podcast Home screen** (§4.4 "Home"): the episode cards and the
  Playlists row. Playlists are D35's, in CL2/POD phases. *POD8 put the
  profile's own playlists in a row on the podcasts library's page; the
  episode cards and the row's built-in views are still the podcast
  Home's (POD8 reading 13).*
- **Device settings:** speed, sleep timer and skip intervals (D35; the
  reference's C5/C6 defaults are used), and keyboard shortcuts.
- **Deferred (D35):** transcripts.
- **Later tasks:** the Up Next queue (POD8 / D35), directory search
  (POD7), and bookmarks (books only, D35). *POD7 built the search.
  POD8 built bookmarks, and playlists in place of a server-side queue,
  which D35 keeps on the device ("Playlists, collections and bookmarks
  (POD8)").*
- **Not stored:** episode artwork (POD1 reading 12 kept none).
- **Search:** shows and episodes in the global search (reading 14).
- **Still a gap:** `library_access.max_rating`, still enforced nowhere
  (POD5 reading 14).

#### Readings made in POD6 — for the owner's veto

1. **Artwork is cached when a show is checked (and right after an
   admin adds it), not when a page first asks for it.**
   - *Why:* a page view never makes the server reach out to a
     publisher, and a broken artwork address is tried once per check,
     not once per listener per page. The check already holds the
     address, and the job already lists per-show trouble.
   - *Cost:* a show added before POD6 gets its artwork at its next
     check. With auto-download off (the default), that is the next
     "Refresh now".
   - *If vetoed:* the artwork route fetches a missing copy on first
     request.
2. **The original image is kept and served as fetched, at most 20 MB,
   and only as JPEG, PNG, WebP or GIF.**
   - *Size:* Parlour has no image library, so there is no resizing.
     Feed artwork is commonly 1400–3000 px, and a list page downloads
     each once, then the browser keeps it for 7 days (versioned by
     `?v=<key>`).
   - *SVG is refused:* it can carry script, and the copy is served
     from Parlour's origin.
   - *If wanted:* a thumbnail size is a later task with an image
     dependency.
3. **A failed artwork fetch keeps the older copy, names the show on the
   job, and raises no notice.**
   - *Why:* artwork is cosmetic, unlike a failing feed (POD2's notice).
     It is tried again at the next check.
4. **Chapters are read server-side, per request: the download's own
   marks first, else the feed's JSON chapter list. Nothing is stored.**
   - *Why the download first:* it needs no publisher and is exactly
     the audio being played.
   - *Why the feed's list next:* it covers undownloaded episodes.
   - *Why not stored:* nothing in POD1 holds chapters (POD1 reading
     12), and storing them would be a migration for something read
     once per listen.
   - *Cost:* the publisher of an undownloaded episode with a chapter
     list sees Parlour's address (not the listener's) once per episode
     page opened, and a download is probed by ffprobe each time.
   - *Left out:* a chapter's image and link are dropped (publisher
     addresses), and so is `"toc": false`.
   - *If vetoed:* deferring chapters is removing the route and the
     list. Storing them is one table plus a step in the download.
5. **The stats gain a per-show list (POD5 reading 8 left it to POD6).**
   - UI-AUDIO-REFERENCE §4.4 asks for "total time per show and per
     book", and Overcast's stats (§2.8) are per show.
   - It follows D41's flag, as the totals do.
   - The book list stays books.
6. **Descriptions are sanitised in the page, from an allowlist, not on
   the server.**
   - *Why the page:* POD5 passes the feed's HTML as given (POD5
     reading 12), so a native client (Phase 12) must sanitise too. CL6
     should say so.
   - *What it drops:* images, because they would hot-link the
     publisher.
   - *If vetoed:* sanitising on the server (e.g. an HTML-sanitiser
     crate) changes POD5's answers, which is the owner's call for the
     API contract.
7. **An episode's state.**
   - *The states:*
     - **New:** at 0 and not finished, which includes one un-played
       by hand.
     - **In progress:** its position is past 0.
     - **Played:** finished.
   - *The ring:* the position over the length, the download's else the
     feed's. With no length known, a small sliver shows that it has
     started.
   - *Not checked:* ABS's and Overcast's exact "new" rules (Overcast
     also ages episodes).
8. **The player lives on the episode page, as AB5's does on the book
   page.**
   - Leaving the page stops playback (saving progress and closing the
     session).
   - UI-AUDIO-REFERENCE §4.2's persistent player bar is "a
     prerequisite web task" for books and episodes alike, not POD6's
     line.
   - A row's Play opens the episode page with `?play=1`.
9. **The controls are the reference's podcast defaults, with no
   settings.**
   - *What there is:* back 15 s and forward 30 s (C5), next chapter,
     and the chapter list below the player.
   - *Not built:* speed, a sleep timer and skip-interval settings.
     These are device settings (D35) and not in POD6's line. There are
     no keyboard shortcuts yet.
10. **Current / All on the shows list defaults to All.**
    - The choice is remembered per browser.
    - "Current" is a show with an episode this profile hasn't
      finished. Under D31 every show counts for every profile, so
      "All" is the honest default (UI-AUDIO-REFERENCE §4.4 notes the
      same limit).
11. **A podcasts library's Home tile counts the shows this profile
    can reach, D41 included.**
    - It used to count `media_items`, which was always 0 for podcasts.
    - Counting every show would leak an explicit show's existence to a
      profile with D41 off.
12. **Now Playing links an episode to the listener's episode page.**
    - That page is read with the admin's own active profile, so an
      admin whose profile can't reach the show sees "Not found" there.
    - Server → Podcasts stays the admin's management view of any show.
    - The card's artwork follows the same rule, and falls back to the
      initial tile.
13. **An admin's Download on the episode page doesn't swap the source
    of an episode that is playing.**
    - It keeps playing from the publisher, and the page says to open
      it again to play Parlour's copy.
    - *Why:* a mid-play swap would reload the audio and move the
      playhead.
14. **Shows and episodes are not in the global search.**
    - They are not `media_items` (POD1 reading 1), and the search runs
      over the catalog.
    - The shows list's filter (POD5's `q`) is their search for now.
    - *If wanted:* adding them is a search-side change that must apply
      the grant and D41's flag.
15. **A show's page lists 50 episodes at a time, with "Show more".**
    The API sends the whole list (POD5), and a long-running show can
    have more than a thousand episodes.

### Directory search (POD7, built 2026-11-27)

POD7 is the third way an admin adds a show, beside POD4's add by
address and OPML import: an admin searches **Apple's podcast directory**
(the iTunes Search API, which needs no key) and adds a show from the
results. D31 decided it this way: Apple's search, not Podcast Index, and
only admins add shows.

There is **no migration**. `podcasts.itunes_id` is POD1's column (POD1
reading 12 kept it for this), filled here for the first time. The bundle
stays at format v1 (`podcasts` stays *left out*, and with it
`itunes_id`). The flow `crates/server/tests/podcast_search_flow.rs` runs
the server side on a fresh database. A local stub on the loopback stands
in for both Apple's API and the feed host, so the test never reaches the
real Apple API or the internet. The page was checked in headless
Chromium against a private instance of this branch (below).

Audiobookshelf's behaviour cited here is from general knowledge
(behaviour only), as in POD4; it was not re-checked against ABS's
source. Nothing was copied. Apple's answer shape (`collectionId`,
`collectionName`, `artistName`, `feedUrl`, `artworkUrl600`, `genres`,
`trackCount`, `collectionExplicitness`, `releaseDate`) was confirmed on
2026-11-27 with one request made by hand, outside any test.

**The code:**

- `parlour_catalog::podcast_directory` holds the query checks, a pure
  reader for Apple's answer (fixture-tested against
  `crates/catalog/tests/fixtures/podcasts/directory-search.json`, which
  is made-up data), and the client.
- `podcast_admin.rs` gains the search route and the "already added"
  marking. The add route takes an optional `itunes_id`, and the show
  page's data carries it.
- `web/src/routes/admin/podcasts/+page.svelte` holds the search.
- `AppError::Upstream` is a 502 for an outside service that failed on
  the caller's behalf.

**Who (D31).** The search is an admin route under `/api/v1/admin/`, and
it answers 404 to anyone who isn't an admin (401 when signed out), as
POD4's routes do. The search box is on Server → Podcasts, which
non-admins never see. Listeners never search the directory, and no
listener page or answer carries anything of Apple's.

**The API.**

| Route | What |
|---|---|
| `POST /api/v1/admin/libraries/{id}/podcasts/search` `{"query", "country"?}` | Search Apple's directory. 200 with `{country, results}`. |
| `POST /api/v1/admin/libraries/{id}/podcasts` `{"feed_url", "itunes_id"?}` | POD4's add. With `itunes_id`, the show keeps Apple's id (below). |

A library that isn't a podcasts library is a 400, and an unknown one is
a 404, as in POD4.

**The search.**

1. **The query** has its white space folded. These are refused with a
   400, and **nothing is sent to Apple**:
   - an empty query;
   - one over 200 characters;
   - one that looks like a web address (reading 7): "That looks like a
     feed's address, so it wasn't sent to Apple. Add it under "Add a
     show by its address" instead." A paid feed's address carries a
     private token, and Apple must never be handed it.
2. **The store country** is two letters, upper-cased, and `US` when it
   is blank or absent (reading 9). Anything else is a 400.
3. **The request.** Only the server asks Apple. It sends
   `GET {base}/search` with `term`, `media=podcast`, `entity=podcast`,
   `country` and `limit=25`, using Parlour's User-Agent (the feed
   client's, with no contact address).
   - **The base** is `https://itunes.apple.com`.
     `PARLOUR_APPLE_SEARCH_BASE` overrides it for tests and stubs, and
     is never set in production.
   - **The limits:** 15 s all told, and an answer of at most 2 MB.
     Anything but a 200 is a failure.
4. **The answer.** Results are listed in Apple's order. Each has:
   - `itunes_id` (Apple's `collectionId`, else `trackId`);
   - `title` and `author`;
   - `feed_url`;
   - `artwork_url` (reading 5);
   - `genres` (Apple's list without its catch-all "Podcasts", else the
     primary genre);
   - `episode_count` (`trackCount`);
   - `explicit` (Apple's marking);
   - `latest_episode_at` (`releaseDate`);
   - `added` (below).

   What is skipped or blanked:
   - **Skipped:** a result Apple marks as something other than a
     podcast, one without a positive id or a title, and an id seen
     before.
   - **No feed address:** a `feedUrl` that is missing or isn't an
     `http`/`https` address (POD4's check) becomes `null`. Such a
     result can't be added, and says so.
5. **Failures** are a **502** that names Apple and the reason. The
   errors drop the request's URL (`without_url`), which holds the query.
   They never repeat the query or an address. For example:
   - "Apple's podcast directory answered HTTP 503."
   - "Apple's podcast directory couldn't be reached: it took too long."
   - "Apple's podcast directory sent an answer Parlour can't read."
   - "Apple's podcast directory sent an answer larger than 2 MB."
6. **The log** records the library, the country and the counts, never
   the query (reading 6).

**Already added (reading 2).** A result is marked `added: {podcast_id,
title}` when its show is already in **this** library:

- **by Apple's id:** a show added from a search keeps it, even if its
  feed has moved since;
- **else by its feed address**, exactly as Apple lists it: a show added
  by address or OPML has no Apple id.

The page shows "Already added" and an **Open** link to the show's
Server → Podcasts page instead of Add. A show in another podcasts
library doesn't mark the result, because that library may hold it too
(POD4's rule).

**Adding from a result.** "Add" sends POD4's add with the result's feed
address and Apple's id. It is POD4's add path, unchanged:

- **The first check.** The feed is fetched and read in the request, and
  the show is stored with its first check in one transaction (POD4
  reading 1). It gets ABS's default settings, and nothing is
  downloaded.
- **The artwork** is the feed's own, cached in the background
  afterwards (POD6). It is not Apple's.
- **Duplicates.** The same address in the library is a **409** naming
  the show. So is **the same Apple id** in the library (reading 3), so
  what the search marks "already added" can't be added twice.
- **Refusals.** A result whose feed fails the check is refused with a
  400 and the reason ("That address can't be added: not an RSS feed
  (the document's root is <html>)."), and nothing is stored. An
  `itunes_id` that isn't a positive number is refused too.
- **Where the id goes.** `podcasts.itunes_id` is written only here.
  POD2's check never touches it, and an add by address or OPML leaves
  it NULL. The show's page (Details → Added) says "from Apple's podcast
  directory (id …)".

POD4's duplicate message now reads "This library already has that show
(…)" (it said "that feed"), because the duplicate may be found by
Apple's id.

**The page (reading 1).** The search is on Server → Podcasts
(`/admin/podcasts`), under the shows table and above "Add a show by its
address" (POD4's form, renamed from "Add a show"), for the library the
picker has chosen.

- **The box.** A search box and a two-letter "Store" box (default US).
  The search runs when the admin presses Search, never while typing.
- **The results.** One row per result, with:
  - Apple's artwork, else an initial tile;
  - the title, with an "Explicit" badge where Apple marks one;
  - the author;
  - the genres, the episode count and the latest episode's date;
  - the feed address, small, for the admin.
- **The action** on each row:
  - **Add**, which reads "Checking the feed…" while it works. One add
    runs at a time.
  - **Already added** with **Open**.
  - "Apple lists no feed address for this show, so it can't be added."
- **Outcomes**, handled per result as the add form handles them:
  - **A successful add** turns its row into "Already added", says
    "Added "*Show*" with *n* episodes", and reloads the shows table.
  - **A refused or duplicate add** shows its error on that result's
    row, and the row keeps its Add.
- **Errors.** A refused query or a failed search shows its error above
  the results. Changing the library clears the results.
- **On a phone** a row's actions sit under its text.

**Checked in a browser (2026-11-27).**

- **Why not the test instance.** The live test instance still runs a
  build from before POD1 and has no podcast tables. It was not
  redeployed or restarted.
- **The setup.** The checks ran against a private instance of this
  branch on oryx's loopback, with:
  - its own throwaway Postgres;
  - `PARLOUR_APPLE_SEARCH_BASE` pointed at a Python stub on the
    loopback, which answered as Apple and also served feeds with
    tokenised addresses, an HTML page and PNG artwork.
- **The run.** chrome-headless-shell 151 over CDP: **49 checks, 49
  passed**. They cover:
  - the search box's place and default country;
  - five results in Apple's order, with author, genres, count, date,
    feed address and Apple's artwork (sent without a Referer);
  - a publisher's artwork address dropped for an initial tile;
  - the explicit badge;
  - the show added by address marked "Already added", linking to its
    page;
  - the no-feed result saying it can't be added;
  - Add from a result: the row turns, the notice, the table, the Apple
    id stored, and the show's artwork fetched by the server;
  - a refused feed's error on its row, with no token or address, and
    nothing stored;
  - the show page's "from Apple's podcast directory";
  - a new search marking the added show;
  - a feed address typed as a query: refused, never sent to Apple, the
    token not repeated;
  - Apple answering 503, reported without the query;
  - no results;
  - store country CA, sent and shown;
  - switching library: results cleared, and nothing marked in the
    second library;
  - a phone-width layout with no sideways scroll;
  - a non-admin: the search route a 404, the page "Page not found", and
    Apple never reached.
- **What left Parlour.** The browser's only requests off Parlour's
  origin were the result artwork images (Apple's CDN, played by the
  stub), sent with no Referer. Every search the stub saw came from the
  server (Parlour's User-Agent). The server's log held no token and no
  query.
- **Not exercised:** Apple timing out (15 s) or refusing the
  connection. The 2 MB limit, an unreadable answer and a 503 are in the
  flow test.

**Not built here:**

- **Searching from the listener pages** (UI-AUDIO-REFERENCE §4.1's
  Search button for shows and episodes): that is global search (POD6
  reading 14), not the directory.
- **Podcast Index.** D31 chose Apple.
- **Remembering the store country** between visits.
- **Removing a show**, still not built (POD4).
- **Showing Apple's description or episode list** before adding: the
  add reads the feed itself.

#### Readings made in POD7 — for the owner's veto

1. **The search is on Server → Podcasts' list page, not a view of its
   own.**
   - *Where:* under the shows table, above the add-by-address form, for
     the library the picker has chosen.
   - *Why:* the three ways to add a show are then on one page, and
     "already added" is a property of the chosen library.
   - *If vetoed:* moving the section to `/admin/podcasts/search` is a
     page move, not an API change.
2. **"Already added" means in this library, matched by Apple's id
   first, else by the exact feed address.**
   - *Why this library:* a feed is unique per library (POD1), and
     another podcasts library may hold the same show (POD4). Marking a
     show held only by another library would block a legitimate add.
   - *Why exact:* addresses are stored as typed and compared exactly
     (POD4 reading 5). A show added by address whose address differs
     from Apple's listing (`http` vs `https`, a token) isn't marked.
     Adding it then either works (another address of the same show) or
     is refused if the feed is the same address.
   - *If vetoed:* marking across libraries is one condition in the
     query, plus a label naming the other library.
3. **Apple's id is stored at add only, and an id already in the library
   is a duplicate.**
   - *What:* `itunes_id` is written by the add from a search, never by
     a check, an add by address or OPML.
   - *Why the duplicate:* it makes the add agree with the search's
     mark. A show whose feed has moved is still "already added".
   - *Cost:* there is no unique index (that would be a migration), so
     two admins adding the same Apple show at the same moment under two
     different addresses could both succeed. The search marks the
     first.
4. **The page sends the feed address and Apple's id back to the add;
   the server doesn't ask Apple again to confirm the pair.**
   - *Why:* the add route is admin-only, the id is used only for the
     mark and the duplicate check, and a second Apple request per add
     would add a failure point.
   - *If vetoed:* the add looks the id up (`/lookup?id=`) and uses
     Apple's address, ignoring the page's.
5. **Result artwork is Apple's CDN address, shown in the admin-only
   results.**
   - *What:* only `https` addresses on `*.mzstatic.com` are kept. Any
     other address, such as a publisher's, is dropped for an initial
     tile.
   - *Why:* AB6's admin match results show the providers' cover
     addresses the same way. The address is Apple's, not a feed's
     tokenised one, and only an admin's browser loads it, with
     `referrerpolicy="no-referrer"`.
   - *Cost:* Apple's CDN sees the admin's IP. No listener page ever
     loads it.
   - *After the add:* the show's artwork is the feed's, through POD6's
     cache.
   - *If vetoed:* initial tiles only, which is dropping `artwork_url`
     from the answer.
6. **The search is a POST with the query in its body, and the query is
   never logged.**
   - *Why:* a GET would put the query in URLs: the reverse proxy's
     access log, the browser's history. A pasted feed address with its
     token would land there before reading 7 could refuse it.
   - *What Parlour logs:* the library, the country and the counts.
7. **A query that looks like a web address is refused before anything
   is sent to Apple.**
   - *What counts:* one starting `http://`, `https://`, `feed:`,
     `itpc://`, `pcast://` or `www.`, or a single word holding a dot
     and a `/` or `?` (`feeds.example.com/show?token=…`).
   - *What doesn't:* ordinary searches with dots or slashes and spaces
     ("Mr. Robot recap", "AC/DC stories").
   - *Why:* a paid feed's address carries a private token, and Apple
     must not be handed it. The add-by-address form is the place for an
     address.
8. **The limits are 25 results, 15 s, 2 MB, and 200 characters for a
   query.**
   - *The term:* Apple's `term` search, without `attribute`, matches
     titles, authors and more. That is the "title / author search" the
     task asks for.
   - *The pace:* the search runs only when the admin presses Search, so
     Apple is never asked per keystroke.
9. **The store country is a two-letter box, US by default, not
   remembered.**
   - Apple's directory differs by country, and ABS lets an admin choose
     a region.
   - Apple decides which two-letter codes it knows. An unknown one
     gets Apple's own answer (usually no results).
10. **An Apple failure is a 502 with the reason.**
    - *How:* a new `AppError::Upstream` (code `upstream_error`), whose
      message is shown to the admin. `Internal` hides its message, and
      a 400 would blame the admin's input.
    - *In the log:* the message is logged like any 5xx. It holds no
      query and no address.
11. **Results show each feed's address to the admin.** It is Apple's
    public listing, and POD4 shows a show's address on its page by
    design (POD4 reading 6). It appears nowhere else: not in an error,
    the log, a job or a notice.
12. **The "Explicit" badge in the results is Apple's marking.** Once a
    show is added, D41 follows the feed's own `itunes:explicit`
    (POD2). The two usually agree, and where they differ the feed wins.
13. **`PARLOUR_APPLE_SEARCH_BASE` exists for tests and stubs.**
    - When it is set, result artwork on the same origin as that base is
      also kept, so a stub can serve Apple's images.
    - It is unset in production, where only Apple's CDN passes.

### Playlists, collections and bookmarks (POD8, built 2026-11-27)

POD8 builds the server side and the web pages of D35's "what syncs",
apart from progress and sessions, which AB4/AB11/AB8 already built.
That means playlists, collections and bookmarks. Per D35, the **Up
Next queue stays on the device**, so there is no server-side queue,
and playlists replace POD8's original "synced queue". Speed, the sleep
timer and skip intervals stay on the device too. Episode bookmarks
aren't built: ABS has none.

The TODO line names playlists only, but D35 lists all three as what
syncs, and POD6's "Later tasks" names bookmarks as a later task of
this phase. So all three are here (reading 1). CL2 is what later
carries them to the native clients in its sync feed.

Audiobookshelf's behaviour cited here was checked on 2026-11-27 against
its server source on `master` (its collection, playlist and "me"
controllers, its user model and its API router). That reading was for
behaviour only, and nothing was copied.

One migration, `20261205000000_pod8_playlists_collections_bookmarks.sql`,
adds five tables. The timestamp is an ordering key only; it was
written on 2026-11-27. The flow
`crates/server/tests/playlists_flow.rs` runs the API on a fresh
database.

**The code:**

- `crates/server/src/playlists.rs`: a profile's playlists;
- `collections.rs`: a library's collections;
- `bookmarks.rs`: a profile's bookmarks in a book;
- `podcast_api.rs`: POD5's episode query now also takes a list of ids,
  so a playlist reads its episodes with exactly POD5's grant and D41
  rules.

**The tables.**

| Table | What |
|---|---|
| `playlists` | One profile's playlist in one library: name, description, times. Deleted with the profile or the library. |
| `playlist_items` | An entry: a book (`item_id`) or an episode (`episode_id`), exactly one, unique per playlist, with a `position`. Deleted with its book or episode. |
| `collections` | A library's collection: name, description, times. Deleted with the library. |
| `collection_items` | A book in a collection, unique, with a `position`. Deleted with the book. |
| `bookmarks` | One profile's bookmark in one book: `position_ms` (book-global, as progress) and a `title`, unique per profile, book and time. Deleted with the profile or the book. |

Positions may have gaps. Their unique key is deferrable, so a reorder
rewrites every position in one statement.

**Playlists (D35: per profile, per library, ordered).**

| Route | What |
|---|---|
| `GET /api/v1/playlists?library_id=&item_id=\|episode_id=` | This profile's playlists in the libraries it can reach, by name. Each carries its entry count and length (counting only entries it can reach), up to four covers (Parlour's own addresses), and, when an item or an episode is named, `contains`. |
| `POST /api/v1/playlists` `{library_id, name, description?}` | A new, empty playlist (201) in an audiobooks or podcasts library the profile can reach. Any other library kind is a 400; a library without a grant is a 404. |
| `GET /api/v1/playlists/{id}?client=` | The playlist with its entries in order. Each entry has its own `id`, a `kind` (`book` / `episode`), and the book as the book list shows it or the episode as POD5 serves it (this profile's progress, a tokenised `stream_url`). |
| `PATCH /api/v1/playlists/{id}` `{name?, description?}` | Rename or describe it. An empty description clears it. |
| `DELETE /api/v1/playlists/{id}` | 204. |
| `POST /api/v1/playlists/{id}/items` `{item_id}` or `{episode_id}` | Append (201). Neither or both is a 400. A book or episode the profile can't reach is a 404. One from another library is a 400, and a duplicate is a 409. |
| `PUT /api/v1/playlists/{id}/items` `{items: [entry ids]}` | The new order (reading 6). |
| `DELETE /api/v1/playlists/{id}/items/{entry}` | Remove an entry. The answer says `playlist_deleted` when it was the last (reading 5). |

- **Whose.** A playlist is its profile's alone, as ABS keeps one per
  user. Any other profile gets 404, including another profile of the
  same login and an admin. It is reached only while the profile keeps
  its grant on the playlist's library. Without the grant, the playlist
  isn't listed and reads as 404; it comes back with the grant.
- **What it shows (reading 4).** An entry is shown only while the
  profile can reach what it points at:
  - a book through the library grant;
  - an episode through the grant and D41's flag, exactly as POD5.

  An entry out of reach is kept but unseen:
  - it isn't counted or timed;
  - it can't be added, removed or named in a reorder;
  - nothing in an answer mentions it.

  It returns in its own place when it is reachable again. An entry
  goes for good with its book or episode. POD3's retention keeps an
  episode's row, so a retained-out episode stays in a playlist and
  plays through the enclosure redirect (D32 as amended).

**Collections (D35: per library, visible to everyone with access).**

| Route | Who | What |
|---|---|---|
| `GET /api/v1/collections?library_id=&item_id=` | any profile | The collections of the libraries it can reach, by name, with book count, covers and `contains`. |
| `GET /api/v1/collections/{id}` | any profile | One collection with its books in order, each as the book list shows it, with this profile's progress. |
| `POST /api/v1/admin/collections` `{library_id, name, description?}` | admins | A new collection in an audiobooks library (another kind is a 400). |
| `PATCH\|DELETE /api/v1/admin/collections/{id}` | admins | Rename / describe it, or delete it. |
| `POST /api/v1/admin/collections/{id}/items` `{item_id}` | admins | Append a book of the collection's library. Another library's book is a 400, and a duplicate a 409. |
| `PUT /api/v1/admin/collections/{id}/items` `{items: [book ids]}` | admins | The new order, naming every book once (else 409). |
| `DELETE /api/v1/admin/collections/{id}/items/{item_id}` | admins | Remove a book (204). An empty collection stays. |

The admin routes follow POD4's pattern: they live under
`/api/v1/admin/` and are a 404 to anyone else. A profile without a
grant on the library gets 404 for its collections and doesn't see
them listed.

**Bookmarks (D35: per book, a time and a title; books only).**

| Route | What |
|---|---|
| `GET /api/v1/audiobooks/{id}/bookmarks` | This profile's bookmarks in the book, by time. |
| `POST /api/v1/audiobooks/{id}/bookmarks` `{position_ms, title?}` | A bookmark at a book-global time, clamped onto the book (a negative time is a 400). A new one is 201. At a time that already has one, the answer is that bookmark (200), retitled only when a title is sent (reading 11). |
| `PATCH /api/v1/audiobooks/{id}/bookmarks/{bookmark}` `{title}` | Rename. The time stays. |
| `DELETE /api/v1/audiobooks/{id}/bookmarks/{bookmark}` | 204. |

They are reached through the book's grant, like its progress (404
otherwise). They are kept while the grant is away, and go with the
book or the profile. An episode's id is not a book, so these routes
answer 404 for it.

**The web pages.**

- **An audiobooks library** gains a **Collections** view beside Home,
  Books, Series and Authors (UI-AUDIO-REFERENCE §4.2's Collections
  tab). It holds:
  - the library's collections, with "New collection" for admins;
  - "Your playlists", with "New playlist".
- **A podcasts library's page** gains the **Playlists row** above the
  shows (§2.2 / §4.4): this profile's playlists of episodes as round
  buttons, and "New playlist" (reading 13).
- **`/playlist/{id}`.** The entries in order, each with:
  - cover or show art, title, author or show, date, and the length or
    time left;
  - Play, ↑ / ↓ and Remove.

  The page also has Rename and Delete. Removing the last entry warns
  first, then returns to the library.
- **`/collection/{id}`.** The books in order, as the book grid. Admins
  also get Rename, Delete and "Arrange books" (↑ / ↓ / Remove).
- **"Add to playlist"** is on the book page, the episode page and each
  episode row of a show page. It lists this profile's playlists in that
  library, marks the ones already holding the item, and can start a new
  playlist with it. Admins also get **"Add to collection"** on the book
  page.
- **The bookmarks sheet** is in the book player (C7/C11), opened by
  "Bookmarks (n)" under the transport. It has:
  - **Add at** the current time, with an optional title;
  - the list by time: a click on a time plays from there, and each
    has Rename and Delete;
  - "Bookmark at 1:23" shown for an untitled one.

  The episode player has no bookmarks.
- **`?play=1`.** The book page now honours it, as the episode page
  already did, so a playlist's Play starts the book.

**The bundle.** All five tables are *left out* of format v1
(`coverage.rs`; `IMPORT-EXPORT.md` §8):

- playlists and bookmarks are per-profile audio state like
  `book_progress`, so they wait on **D36, still open**, and the
  format-v2 step;
- collections are an admin's work, and their place is the format-v2
  step.

**Checked in a browser (2026-11-27).** The live test instance still
runs a build from before POD1 and wasn't touched. The checks ran
against a private instance of this branch on oryx's loopback:

- **The setup:**
  - its own throwaway Postgres;
  - three Opus books scanned from disk;
  - two shows (one explicit) whose rows were written straight into
    the database, with tokenised fake addresses;
  - an admin with one profile, and a second, non-admin user.
- **The run.** chrome-headless-shell 151 over CDP: **56 checks, 56
  passed**. They cover:
  - a playlist made, filled from three book pages, reordered,
    renamed, an entry removed, each kept across reloads;
  - Play from the playlist starting the book;
  - bookmarks:
    - added at the playing time;
    - an untitled one shown by its time;
    - the list by time;
    - a jump that moved the player and played;
    - a rename and a delete, kept after a reload;
  - a collection made, filled with "Add to collection", arranged and
    renamed;
  - the podcasts Playlists row, episodes added from show rows, and
    D41 turned off and on again hiding and restoring the explicit
    episode and its count;
  - no bookmarks on the episode player;
  - a playlist deleted;
  - the last entry's removal deleting its playlist;
  - the other user seeing the collection in the admin's order, with
    no admin controls, none of the admin's playlists (and "Not found"
    on one), no "Add to collection" and no bookmarks of the admin's.
- **What left Parlour.** The browser's one request off Parlour's
  origin was the episode page's audio metadata following POD5's
  redirect for an undownloaded episode (D32 as amended), which isn't
  POD8's. The token never appeared in the server's log.

**Not built here:**

- a server-side Up Next queue, and playing a playlist through in
  order in the web player (reading 14);
- the podcast Home's built-in filtered views (reading 13);
- episode bookmarks (D35);
- "playlist from a collection" (ABS has one; nothing asked for it);
- the CL2 sync of all three, with tombstones for deletions;
- `library_access.max_rating`, still enforced nowhere (POD5 reading
  14).

#### Readings made in POD8 — for the owner's veto

1. **Bookmarks are built here, not in CL2.**
   - *Why:* D35 lists bookmarks among what the server keeps, and
     POD6's "Later tasks" names them as a later task of this phase.
     The book player is web work Parlour already has.
   - *What CL2 keeps:* carrying them, with playlists and collections,
     to native clients in its one sync call.
2. **A playlist lives in one library, so it never mixes books and
   episodes.**
   - *Why:* D35 says "per profile per library". An audiobooks library
     holds books and a podcasts library holds episodes, so the library
     decides the kind.
   - *ABS differs a little:* its create takes the kind from the items
     sent, and its add doesn't check. In practice its libraries are
     one media type too.
   - *If vetoed:* a mixed playlist needs `library_id` to become
     optional (a migration) and the add check dropped.
3. **A playlist is its profile's alone: no admin view of anyone
   else's.**
   - As in ABS, where only the owner reaches a playlist, admins
     included.
   - Another profile under the same login is another listener (D29),
     so it gets a 404 too.
4. **An entry out of reach is kept and unseen, not deleted.**
   - *Out of reach* means the grant is gone, or it is an explicit
     show's episode with D41's flag off. It isn't counted, timed,
     addable or removable, and a reorder mustn't name it.
   - In a reorder, the visible entries take the places they held among
     themselves, and hidden ones keep theirs. So turning the flag back
     on puts an episode back where it was.
   - *Why:* D41 already keeps a profile's progress on explicit episodes
     while hidden. A playlist that lost entries whenever an admin
     toggled a flag would be worse.
   - *ABS differs:* it filters nothing (it has no such flag on
     playlists).
   - A deleted book or episode takes its entries with it.
5. **Removing a playlist's last entry deletes the playlist, as in ABS.
   An empty playlist can still be made.**
   - *ABS:* its remove deletes a playlist left empty, and its create
     accepts an empty list. Both are mirrored.
   - *The web:* the page warns before removing the last entry.
   - *Not checked:* whether ABS also deletes a playlist emptied by an
     item's deletion (that model method wasn't read). Here a cascade
     leaves the playlist in place, empty.
   - *If vetoed:* dropping the delete is one statement.
6. **A reorder sends the full list of entry ids; a stale list is a
   409.**
   - *ABS:* its update takes the full items array and refuses a
     length mismatch.
   - *Here:* the list must name every entry the profile can see,
     exactly once. Anything else (a stale page, a repeat, a stranger)
     is a 409, "reload it and try again", and nothing moves.
   - Collections use the same shape with book ids.
7. **Collections hold books only, in an audiobooks library. A podcasts
   library has none.**
   - *ABS:* its create and batch add keep books only.
   - *The UI reference agrees:* the podcasts library's Collections tab
     is "Playlists (D35)" only (UI-AUDIO-REFERENCE §4.2).
   - *If vetoed:* a `podcast_id` / `episode_id` column on
     `collection_items` (a migration).
8. **Only admins create, change and delete collections.**
   - *ABS:* changing one needs its "update" permission (root and admin
     by default), and deleting one its "delete" permission (root only
     by default).
   - *Here:* Parlour has one admin role and no per-user permissions,
     so admins do all three.
   - *Routes:* the admin routes are POD4's pattern, a 404 to anyone
     else. Like POD4's, they don't ask whether the admin's own profile
     has a grant on the library.
9. **Everyone with a grant on the library sees its collections, each
   book with their own progress.**
   - A grant reaches every book of the library, so a collection never
     shows a book its viewer can't reach.
   - `library_access.max_rating` is still enforced nowhere (a known
     gap, not POD8's).
10. **An empty collection stays; names needn't be unique.**
    - ABS removes neither.
    - Names are trimmed, 1–200 characters. Descriptions are up to
      4 000, and empty clears them. The same holds for playlists.
11. **One bookmark per book per time, as ABS. A second add at that
    time answers the first.**
    - *ABS:* its create returns the existing bookmark at that time,
      retitling it if the title differs. Its rename finds a bookmark by
      time, so time is its key and can't be changed.
    - *Here:*
      - The second add answers 200 with the existing bookmark,
        retitled only when a non-empty title is sent.
      - Rename is by the bookmark's id and changes only the title.
      - Times are milliseconds, book-global, clamped onto the book.
        ABS leaves the unit to its client.
      - The title may be empty, and the player then shows "Bookmark
        at *time*".
12. **Bookmarks follow the book's row.** They are kept while the grant
    is away, and deleted with the book. A re-scan that makes the book a
    new item (a moved folder) loses them, as it loses its progress.
13. **The podcast Home's Playlists row: built-in views first, then
    the profile's own. Only the own ones are built here.**
    - *The reference (§2.2 / §4.4):* the row is filtered views first
      (All episodes, Downloaded, In progress), then the profile's
      playlists.
    - *Why not the filtered views:* they need a list of episodes
      across shows, which is the podcast Home task, with its episode
      cards (POD6's "not built"). "Downloaded" means on this device,
      which the web doesn't have.
    - *Where the row sits:* above the shows on the podcasts library's
      page. When the podcast Home is built, the filtered views go
      before these buttons.
14. **The web player doesn't play a playlist through; each entry has
    its own Play.**
    - *Why:* playback still lives inside `/book/[id]` and
      `/episode/[id]`, so "next in playlist" would mean a page change
      per entry. The persistent player bar (UI-AUDIO-REFERENCE §4.2) is
      where a queue-like "play this playlist" belongs, and it is a
      separate web task.
    - *The API is ready for it:* the entries carry everything a
      player needs, `stream_url` included.
15. **Nothing here writes an Activity line.** Playlists and bookmarks
    are a listener's own state, as progress writes are. A collection
    change is an admin's edit, like POD4's show settings, which write
    none either.
16. **No sync cursor or tombstones yet.** Every change stamps the
    playlist's or collection's `updated_at`, and a bookmark has its
    own, ready for CL2's cursor. A deleted row leaves nothing behind
    until CL2 designs tombstones.

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
  can reach, instead of "subscriptions". *POD8 (2026-11-27) built
  playlists, collections and bookmarks on the server and the web
  ("Playlists, collections and bookmarks (POD8)"); carrying them in
  the sync feed is CL2's.* **On the device only:** the
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
   *AB8 (2026-11-26) built the server side for the web player: the
   playback reports carry a book's session, `listening_sessions`, Now
   Playing, the Activity line, history and stats ("Listening sessions
   and Home shelves (AB8)"). CL3 is what a native client needs on top:
   its device token, sessions recorded offline and uploaded later, and
   the contract in CL6.*
4. **Offline** (CL4, only if D33 says yes): a download manifest per
   book or episode (files, sizes, hashes), and replay of progress
   recorded offline, carrying the client's own timestamps under the
   D33 conflict rule.
5. **Audio capability profiles** (CL5): which codecs AVPlayer,
   ExoPlayer (Fire OS) and browsers play directly, so the server can
   fall back to an AAC stream only when it must. This is the same
   explicit-profile rule as video (ARCHITECTURE §4).
   *AB10 (2026-11-26) built the profiles and the AAC fallback on the
   server, and the web player uses the `web` profile ("Transcode
   fallback and audio profiles (AB10)"). CL5 is what a native client
   needs on top: naming its profile (`?client=ios` / `android` /
   `firetv`), confirming the profile contents on real devices (AB10
   readings 2–3), and the contract in CL6.*
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
  *2026-11-26 (POD1):* `podcasts` and `podcast_episodes` are left out
  of format v1 (`coverage.rs`); their place in the bundle is later
  work. Episode progress rows sit in `book_progress`, so they follow
  its open proposal (D36) and are not carried.
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
