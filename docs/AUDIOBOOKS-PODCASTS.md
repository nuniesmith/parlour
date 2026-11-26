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
