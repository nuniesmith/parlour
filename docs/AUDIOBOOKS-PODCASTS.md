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

### Later in Phase 10 (blocked on decisions)

AB6 metadata providers and embedded covers (D30), AB7 series and
author browsing (D28), AB8 Now Playing, Activity and a
Continue-listening rail (D29/D35), AB9 an importer for progress from
the owner's running Audiobookshelf (D27), AB10 a transcode fallback
for codecs a client can't play (CL5).

*2026-11-24:* all unblocked except AB9, which is **cancelled**
(D27: no ABS import). AB6 follows D30, AB7 follows D28, AB8 follows
D29 + D35; the details are in the list below.

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
   from that folder and from tags.
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
   splitting under review in AB6.
6. **Which track's tags (AB2/AB6).** ABS reads tags from the first
   track in disc/track order. AB2 takes each field from the first
   track that *has* it. Small difference; align in AB6 together
   with the precedence list (D30).
7. **Narrators (AB1).** ABS stores narrators as a plain list of
   names on the book; AB1 stores them as `people` with role
   `narrator`. The browsing behaviour (list and filter by narrator)
   is the same either way. Decide in AB7 whether to keep `people`
   rows; nothing user-visible depends on it.
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
    (not confirmed on 2026-11-24).
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
