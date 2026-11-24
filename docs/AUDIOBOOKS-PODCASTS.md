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
  long it lives, is **D26**.
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
  D29 asks whether that is what he meant.

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

| Field | Source, first that's present |
|---|---|
| title | `album`; for a single-file book, `title`; else the folder or file name |
| authors | `album_artist`, else `artist`; else the top-level folder when the book sits in one (`Author/Title`, `Author/Series/Title`) |
| narrators | `composer` (the usual audiobook convention; plenty of files leave it empty) |
| year | first four digits of `date` / `year` |
| overview | `description`, else `comment` |
| genres | `genre`, split on `;` / `,` / `/` |
| cover | `cover.jpg` / `folder.jpg` (or `.png`) in the book folder, served from the media path; embedded cover art waits for AB6 |

Several authors or narrators in one tag (`A, B`, `A & B`, `A; B`)
are split. They are stored as `people` linked through `item_people`
with roles `author` / `narrator`. That is the same table cast uses,
so a later author page (AB7) is a query, not a migration.

### Chapters (AB2, built)

- A track with embedded chapters (m4b/m4a, some mp3s) contributes
  them, shifted onto the book's timeline by the track's start
  offset.
- A track without chapters becomes one chapter, titled with its
  `title` tag or its file name.
- Chapters live in `book_chapters (item_id, chapter_index, start_ms,
  end_ms, title)`, rebuilt whenever a book's tracks change.

### Progress (AB4, built — provisional until D29)

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

### Later in Phase 10 (blocked on decisions)

AB6 metadata providers and embedded covers (D30), AB7 series and
author browsing (D28), AB8 Now Playing, Activity and a
Continue-listening rail (D29/D35), AB9 an importer for progress from
the owner's running Audiobookshelf (D27), AB10 a transcode fallback
for codecs a client can't play (CL5).

---

## Phase 11 — Podcasts (plan only; no code yet)

The server owns the feeds. Clients never fetch RSS themselves, which
is what lets a Fire Stick, a tablet and the phone agree.

**Shape (to be confirmed by D31/D32):**

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
- **Sidecars (Phase 8, D14).** ABS-style `metadata.json` /
  `desc.txt` / `reader.txt` next to books are a possible metadata
  input (D30). Writing sidecars into audiobook folders needs the
  same per-library opt-in as video.
- **Compat (Phase 5).** X2 is reframed by D26. If built, it is a
  view over Phase 10's tables and never owns data (ARCHITECTURE §10).
- **Client order (D12).** Whether audio pulls the iOS app ahead of
  Android is **D34**.
