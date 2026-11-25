# Parlour — the export bundle (`parlour-export` v1)

Written 2026-11-24 for Phase 8, task XFER1. This is the format spec
for the export bundle that D14 asks for: one file that carries the
server's users, profiles, libraries, catalog and per-profile state
from one install to another. It defines the zip layout, the six JSON
documents and their fields, and the rules a valid bundle follows.

**Status.** The format exists in code (`crates/server/src/bundle/`)
with fixtures and tests. Export exists too (XFER2, §11): an admin
endpoint and background job write a bundle, and the Jobs page
downloads it. Import exists as an API and a job (XFER3, §12):
upload, dry-run report, apply. The admin page that drives both is
XFER8, and nightly backups are XFER7. The per-item sidecars stored
with the media files are a separate format, `parlour-sidecar` v1,
specified in §13 (XFER4). The write opt-in and poster copy are XFER5,
and NFO interop is XFER6.

**What it rests on.**

- **D14:** the bundle carries users/logins (hashes only behind an
  explicit admin confirm), profiles and preferences, library
  definitions and access grants, catalog metadata, watch history
  and segments. It imports with a dry run, idempotently, and never
  deletes by default. Server secrets are never in it.
- **D25 (decided 2026-11-24):** each profile's recent searches travel
  next to its preferences. The Activity feed travels as the server
  holds it, keyed by usernames and item `stable_key`s, not database
  ids. Password and PIN hashes may sit in the plain, unencrypted zip,
  but D14's confirm and warning still stand.
- **One open proposal:** the audio tables (§9). They are not
  decided, so format v1 doesn't carry them yet.
- **Readings this spec made inside D14/D25:** listed in §10 for the
  owner to veto.

---

## 1. The bundle at a glance

A bundle is a **plain zip** with no encryption (D25). Its six JSON
documents sit at the top level. When the export opted in, artwork
files sit in one folder:

| entry | holds |
|---|---|
| `manifest.json` | format and server versions, export time, which options were taken |
| `users.json` | logins; password and PIN hashes only when the admin confirmed |
| `profiles.json` | profiles, their preferences, their recent searches |
| `libraries.json` | library definitions, root paths, access grants |
| `catalog.json` | every catalog item, with its people, segments and provider artwork |
| `user_data.json` | watch state per profile, and the Activity feed |
| `artwork/<sha256>.<ext>` | image bytes, only when the export included artwork |

Rules for the zip itself:

- The documents are stored deflated. Artwork is stored as it is,
  because images don't compress. Those two methods are the only
  ones a bundle uses, and the only ones the reader accepts. It
  supports no zip encryption either.
- Nothing else may be in the zip. The reader refuses any other entry
  and names it. It tolerates a bare `artwork/` folder entry, which
  some zip tools add.
- If the documents sit inside a folder (someone zipped the folder),
  the reader says so.
- No name may appear twice.
- Artwork names are exactly `artwork/` + 64 lowercase hex digits +
  `.` + 1–4 lowercase letters or digits. So no artwork name can
  point outside the folder it is extracted into.

## 2. Conventions

- **JSON:** UTF-8, pretty-printed with two-space indents and a final
  newline. A reader doesn't care about whitespace.
- **Document shape:** each document is an object holding its arrays
  (`{"users": […]}`), never a bare array.
- **Times:** RFC 3339 in UTC with a `Z`, at the precision Postgres
  keeps (microseconds), e.g. `2026-11-24T16:05:42.318406Z`. Dates are
  `YYYY-MM-DD`.
- **Nulls and absent fields:** `null` means the database holds NULL.
  Every field is always present, with two exceptions:
  - the hash fields in `users.json` are **absent** unless hashes
    were included;
  - an artwork `file` is `null` when there is no file.
- **Closed value sets** travel as their database names:
  - library `kind`: `movies` `shows` `audiobooks` `ebooks` `music`
    `podcasts`
  - item `kind`: `movie` `series` `season` `episode` `audiobook`
  - `metadata_status`: `unmatched` `matched` `needs_review` `failed`
  - segment `kind`: `intro` `credits` `recap`
  - segment `source`: `chapters` `fingerprint` `manual` `heuristic`
  - image `kind`: `poster` `backdrop` `logo` `still`
  - `subtitle_mode`: `auto` `always` `never`
  - `segment_skip`: `button` `auto` `off`

  The first six are the CHECK constraints in `migrations/`. The last
  two are what the profile API accepts. Tests hold all eight to
  those sources.
- **No database ids.** Every reference is by name. The one exception
  is ids that sit *inside* an Activity entry's `detail`, carried
  verbatim (§5.6).

**How each thing is identified.** These are also the keys the import
matches on, and no key repeats within a bundle:

| thing | identified by |
|---|---|
| user | `username` — unique **ignoring case**, like the database's `lower(username)` index |
| profile | (`username`, profile `name`) |
| library | `name` |
| catalog item | (`library`, `stable_key`) |
| person | `name` (people are one row per name server-wide, so a name carries one `tmdb_id` everywhere) |
| access grant | (library, `username` + `profile`) |
| library path | (library, path) |
| segment | (item, `kind`, `source`) |
| image | (item, `kind`, `variant`) |
| watch state row | (profile, `item`) |
| recent search | (profile, `query` ignoring case) |
| Activity entry | the whole entry (it has no key of its own) |

**`stable_key`** is the scanner's identity for an item inside its
library (`crates/catalog/src/scan.rs`, `audiobooks.rs`). The bundle
treats it as an opaque string. Today's forms are:

- `series:<title>:<year or 0>`
- a season: `<series key>:sNN`
- an episode: `<season key>:eNNN`
- `movie:<folder>:<title>:<year or 0>`
- `audiobook:<book folder, relative to the library root>`

Titles and folders are normalised: lowercase letters and digits,
with `-` between words.

## 3. Versioning

- **Identity:** `manifest.json` says `"format": "parlour-export"` and
  `"format_version": 1`.
- **Version check first:** the reader checks those two before it
  parses anything else. So a newer bundle gets "the bundle is format
  version 2; this server reads version 1", not an error about some
  field.
- **Strict:** an unknown field in any document is an error. This
  version never drops data it doesn't understand: an older server
  refuses a newer bundle instead of silently losing part of it.
  Any change to the shape is a new version.
- **Until XFER2 ships, v1 can still change in place.** Nothing writes
  bundles before then, so no v1 bundle exists yet. Changes made
  before XFER2 ships (the §9 proposal, say) amend v1 directly. From
  XFER2 on, every change bumps the version.

## 4. Consistency

A bundle is valid only if it is consistent. `Bundle::problems()`
lists everything wrong with it. The writer refuses to write an
inconsistent bundle, and the reader refuses to open one. So a bundle
that opens is one an import can apply as it stands. The checks are
the rules the database itself enforces, plus the references between
documents:

- **Keys:** no key from the table in §2 repeats.
- **References resolve:**
  - every profile belongs to a user;
  - every grant names a profile;
  - every item names a library;
  - every parent is an item in the same library, and no parent chain
    loops;
  - every watch state row, and every Activity user, profile and item,
    names something in the bundle.
- **Segments follow the database's CHECKs:**
  - `0 ≤ start_ms < end_ms`;
  - confidence is between 0 and 1;
  - times are null only together, and only on a `manual` row (a D21
    "no segment of this kind here" marker).
- **Hashes are all or nothing** (§6), and each one is an Argon2id PHC
  string.
- **Artwork:**
  - `file` is set only when the manifest says artwork is included,
    and only to a proper artwork name;
  - every named file is in the zip, and every file in the zip is
    named by an item;
  - a file's bytes hash to its name, which the writer checks as it
    adds a file and the reader as it copies one out.

Beyond the closed value sets in §2, server policy that the database
doesn't enforce (name lengths, the ten-search limit) isn't a format
rule and isn't checked.

## 5. The documents

The complete example is `crates/server/tests/fixtures/bundle-v1/`: a
consistent bundle, byte for byte what the writer produces. It uses
every item kind, every segment kind and source (a D21 marker
included) and every subtitle and skip mode, and it has an Activity
row whose profile belongs to someone other than its user. All of it is fictional: the people, the
titles and the `example.org` URLs are invented, the IPs are RFC 5737
documentation addresses, and the hashes decode to "fixture-only"
text that no password produces.

### 5.1 `manifest.json`

```json
{
  "format": "parlour-export",
  "format_version": 1,
  "server_version": "0.1.0",
  "exported_at": "2026-11-24T16:05:42.318406Z",
  "hashes_included": true,
  "artwork_included": false
}
```

| field | meaning |
|---|---|
| `format`, `format_version` | always `parlour-export`, `1` |
| `server_version` | the exporting server's version, as `/api/v1/version` reports it |
| `exported_at` | when the export ran |
| `hashes_included` | whether `users.json` carries password and PIN hashes (§6) |
| `artwork_included` | whether `artwork/` holds image files (§5.7) |

### 5.2 `users.json` — `users`

| field | from | notes |
|---|---|---|
| `username` | `users.username` | |
| `is_admin` | `users.is_admin` | |
| `created_at` | `users.created_at` | |
| `disabled_at` | `users.disabled_at` | honoured at sign-in; nothing sets it yet |
| `password_hash` | `users.password_hash` | **only when `hashes_included`**; absent otherwise |
| `pin_hashes` | `profiles.pin_hash` | **only when `hashes_included`**: `{profile name: hash}` for this user's profiles that have a PIN (`{}` if none); absent otherwise |

The TODO puts PIN hashes here, not in `profiles.json`. That keeps
every hash in the bundle in this one file.

### 5.3 `profiles.json` — `profiles`

| field | from |
|---|---|
| `username`, `name` | the owning user; `profiles.name` |
| `created_at` | `profiles.created_at` (orders Who's watching) |
| `has_pin` | whether `profiles.pin_hash` is set. Always carried; the hash itself is §6's |
| `avatar_color` | `profiles.avatar_color` |
| `preferences.audio_lang`, `.subtitle_lang`, `.subtitle_mode`, `.remote_bitrate_cap_kbps` | the A5 defaults |
| `preferences.home_rail_order`, `.home_rails_hidden` | UI7's Home Screen, **as stored**: the order may be partial, and the server fills in the rest when it reads them |
| `preferences.segment_skip`, `.autoplay_next` | UI7's Playback choices (SEG5/SEG6) |
| `recent_searches[]` | `{query, searched_at}`, newest first, at most what the server keeps (10) — D25 |

### 5.4 `libraries.json` — `libraries`

| field | from |
|---|---|
| `name`, `kind`, `created_at` | `libraries` |
| `paths[]` | `library_paths.path`: root folders **as the old host saw them** |
| `access[]` | `library_access`: `{username, profile, max_rating}`. Grants are per profile, so a grant names the profile's user and the profile |

### 5.5 `catalog.json` — `items`

Every `media_items` row, audiobooks included. Each row carries its
people, segments and provider artwork:

| field | from |
|---|---|
| `library`, `stable_key` | the item's identity |
| `parent` | the parent's `stable_key` (season → series, episode → season), or null |
| `kind`, `title`, `sort_title`, `year`, `season_number`, `episode_number`, `air_date`, `overview`, `genres`, `runtime_minutes` | the item's own columns |
| `content_rating`, `vote_average` | the "ratings" |
| `tmdb_id`, `tvdb_id`, `imdb_id` | the provider ids |
| `metadata_status`, `metadata_provider`, `last_metadata_at` | the matcher's state (MD2) |
| `created_at`, `updated_at` | `created_at` orders Recently Added and Latest |
| `people[]` | `item_people` + `people`: `{name, tmdb_id, role, character_name, sort_order}`; roles are `actor`, `author`, `narrator`, … |
| `segments[]` | every `media_segments` row, whatever its source: `{kind, source, start_ms, end_ms, confidence, created_at, updated_at}`. Times are milliseconds into the item's file; both are null on a manual marker (D21) |
| `images[]` | provider artwork: `{kind, variant, source_url, file}` for each `images` row that has a `source_url`. `file` is `artwork/<sha256>.<ext>` when artwork was included and the image was cached, else null |

Left out of each item: `available` (file state, which the new host's
scan decides — C8). Also left out: the `images` rows with no
`source_url`. Those are the generated tiles and audiobook folder
covers, and the new host makes them again.

### 5.6 `user_data.json` — `profiles` and `activity`

`profiles[]` holds one group per profile that has watch state:
`{username, profile, items[]}`. Each item row is one `user_data`
row:

| field | from |
|---|---|
| `item` | `{library, stable_key}` |
| `position_ms`, `duration_ms`, `played`, `is_favorite`, `play_count`, `last_played_at` | the row |
| `created_at`, `updated_at` | `updated_at` is Continue Watching's tie-break after `last_played_at` |

`activity[]` is the Activity feed (D25): every `activity` row the
server holds, oldest first. The 90-day purge already bounds it. Each
entry:

| field | from |
|---|---|
| `created_at`, `kind`, `summary` | the row (`kind` is the dotted name: `auth.login`, `playback.start`, `scan.completed`, …) |
| `username` | the user's **current** name, or null when the row has no user (jobs, or a deleted account) |
| `profile` | `{username, profile}`, or null. It names its own user, because a row's profile need not belong to the row's user: an admin's Stop puts the admin in `username` and the viewer in `profile`, and an Emby import row has a profile and no user |
| `item` | `{library, stable_key}`, or null |
| `actor` | the username **at the time**, as written then (it stays as it was after a rename) |
| `detail` | **verbatim** (D25 "as the server holds them"). It includes what the server put there, such as a sign-in's client IP and the old server's job, library, invite, session and user ids (§10, reading 2) |

### 5.7 `artwork/`

This folder exists only when `artwork_included` is true (XFER2's
opt-in, off by default, with a size warning).

- **Names:** one file per distinct image, named by the SHA-256 of
  its bytes, which is the value `images.hash` already holds. The
  extension is the cached file's.
- **Shared files:** several `images[]` entries can name the same file.
- **What's left out:** generated tiles and audiobook folder covers
  are never included.

## 6. Hashes and secrets

**Hashes leave the server only behind an explicit admin confirm with
a warning (D14).**

- **Default:** hashes are left out. They go in only when the admin,
  for that one export, explicitly asks for them past a warning: the
  flag is XFER2's API, the warning XFER8's page. D25 lets them sit in
  the plain zip; it removed the need for encryption, not the confirm.
- **All or nothing:** with the confirm, every login's password hash
  and every PIN hash goes in. Without it, none does: the fields are
  absent, and `profiles.json`'s `has_pin` still says which profiles
  have a PIN.
- **What a hash is:** the Argon2id PHC string exactly as stored, e.g.
  `$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`. The reader checks
  that each one parses as that.
- **What the warning should make plain:** anyone holding the file can
  try passwords against the hashes offline, at their own pace. A
  4-digit PIN's hash can be brute-forced in minutes. So a bundle with
  hashes is as sensitive as the logins themselves.

**Never in a bundle:**

- **Server secrets:**
  - the TMDB and TVDB API keys;
  - the Emby API key;
  - the stream-token secret;
  - the database URL and its password;
  - the first-admin bootstrap password.

  These live only in the environment. There is no settings table:
  config is environment-only, which is why UI6's General page is
  read-only. No type in the format has a field that could hold one.
- **Session tokens and invite tokens:** their tables stay behind (§8).
- **A future settings table:** if runtime settings ever move into the
  database, their secrets stay out too. Their non-secret settings
  need a decision before they join the bundle.

## 7. What the bundle carries

The column-by-column map is `crates/server/src/bundle/coverage.rs`.
The flow test `crates/server/tests/bundle_schema_flow.rs` checks it
against a freshly migrated database: every table and every column
must be accounted for, and the mirrored CHECK sets must match. A
migration that adds a table, a column or a value fails that test
until the format and this doc say where it goes.

At 2026-11-25 the schema has 27 tables, 13 of them carried:

| table | lands in |
|---|---|
| `users` | `users.json` (and `pin_hash` from `profiles`) |
| `profiles`, `recent_searches` | `profiles.json` |
| `libraries`, `library_paths`, `library_access` | `libraries.json` |
| `media_items`, `people`, `item_people`, `images` (rows with a `source_url`), `media_segments` | `catalog.json` |
| `user_data`, `activity` | `user_data.json` |

Columns of those tables that stay behind:

- every row's own `id`;
- `media_items.available` (file state);
- `libraries.write_sidecars` (the host's sidecar opt-in, §13.8);
- `images.path` (a path in the old host's cache; `file` replaces it
  when artwork is included);
- `images.width` / `images.height` (nothing writes them).

## 8. What stays behind

D14 lists what the bundle carries, and D25 added recent searches and
the Activity feed. The rest of the schema stays on the old host:

| table | why |
|---|---|
| `sessions` | live sign-ins on the old host (token hashes, IPs); everyone signs in again |
| `invites` | pending links point at the old host; used invites are already users |
| `jobs` | the old host's queue and history |
| `media_files`, `media_streams`, `subtitle_tracks` | rebuilt by the new host's scan from the media files themselves |
| `metadata_candidates` | the matcher's review shortlist. An imported `needs_review` item arrives without one, and the next metadata match rebuilds it (`match_library` re-matches every top-level item) |
| `notices`, `notice_dismissals` | reconciled from live conditions once a minute on the new host |
| `audio_fingerprints` | a cache keyed by file content; segment detection rebuilds it |
| `item_sidecars` | what this host's scanner last read from the sidecar files (§13); the new host reads the files |
| `_sqlx_migrations` | the new host keeps its own ledger |
| `book_chapters`, `book_progress` | **open — §9** |

## 9. Proposed — needs the owner's yes: the audio tables

D25 didn't cover the tables Phase 10 added. The proposal follows
D14's split between catalog and per-person state. **It is not
decided, and format v1 doesn't carry either table until the owner
says yes.**

- **`book_progress`: carry it, under `user_data.json`.** It is
  per-profile state, like watch history. D29 keeps it in its own table
  rather than in `user_data`, so in the bundle it would be a sibling
  list in each profile's group, not mixed into `items`:

  ```json
  {
    "username": "ada",
    "profile": "Ada",
    "items": [],
    "books": [
      {
        "item": {"library": "Audiobooks", "stable_key": "audiobook:M. R. Ellison/The Cartographer's Daughter"},
        "position_ms": 5423100,
        "duration_ms": 41870000,
        "finished": false,
        "finished_at": null,
        "last_listened_at": "2026-11-23T21:14:02.551870Z",
        "created_at": "2026-11-23T20:02:41.119036Z",
        "updated_at": "2026-11-23T21:14:02.551870Z"
      }
    ]
  }
  ```

  `position_ms` is book-global, as AB4 stores it.
- **`book_chapters`: leave it out.** Every scan of the book rebuilds
  it from the files' tags and chapters, as it does the file rows.
- **If the answer is yes:**
  - add the field to `ProfileUserData`;
  - move `book_progress` in `coverage.rs` from `Proposed` to `Carried`;
  - extend the fixture.

  Before XFER2 ships, that amends v1 in place (§3). **At XFER2
  (2026-11-24) the proposal was still open**, so the exporter writes
  v1 without either table: a yes before XFER2 merges amends v1 in
  place, a yes after it bumps the format to version 2. Podcast-episode
  progress (POD1) shares `book_progress`'s shape (D29), so it would
  follow the same answer when it exists.
- **Audiobook items themselves are not part of the question.** They
  are ordinary catalog rows (`media_items`, kind `audiobook`), already
  in `catalog.json` under D14 with their author and narrator credits.
  Only the two audio tables are open.

## 10. Readings of D14/D25 made here — for the owner's veto

None of these is a new decision. Each is how this spec read D14, D25
or the TODO where they don't spell the detail out, listed so the
owner can overturn any of them.

1. **Unlisted tables stay out.** These are sessions, invites, jobs,
   notices, metadata candidates, fingerprints and file rows (§8). D14
   names what the bundle carries; D25 added two things and nothing
   else.
2. **Activity `detail` is carried verbatim.** That includes the client
   IP of every sign-in, and ids from the old server (job, library,
   invite, session, user) that mean nothing on the new one. D25 says
   rows travel "as the server holds them"; its "keyed by username /
   stable_key" is read as the row's own user / profile / item
   references. Nothing in the web UI reads ids from `detail`. Say if
   the IPs should be stripped.
3. **Hashes are all or nothing** under one confirm (§6). PIN hashes
   sit in `users.json`, where the TODO puts them, and `has_pin`
   travels without the confirm.
4. **Access grants name a profile.** They are `{username, profile}`,
   because the schema grants access per profile; the TODO's "by
   username" can't express that alone.
5. **Fidelity fields are kept:**
   - `users.disabled_at`;
   - every `created_at` / `updated_at`, because they order Who's
     watching, Recently Added and Latest, and break Continue
     Watching's ties;
   - the metadata status fields.
6. **`media_items.available` stays behind.** It is file state.
7. **Segments: every stored row, from every source.** That includes
   chapter, heuristic and fingerprint rows a new scan would rebuild,
   and D21 markers. D14 says "segments", unqualified.
8. **Artwork travels as provider references,** always. Image files
   travel only through XFER2's opt-in, named by content hash.
   Generated tiles and audiobook folder covers stay behind.
9. **Audiobook items are in `catalog.json`.** They are catalog rows;
   only the audio *tables* are §9's question.
10. **People carry their `tmdb_id` on each credit.**
11. **The format is strict:** an unknown field is an error, and the
    version is checked first (§3).

## 11. Export (XFER2)

Built 2026-11-24 to the notes XFER1 left here. Code:
`crates/server/src/export/`.

**The API** (admin only; anyone else gets a 404, like every admin
route):

| route | does |
|---|---|
| `GET /api/v1/admin/export/estimate` | what the artwork opt-in would add right now: `artwork_files`, `artwork_bytes`, and `artwork_unavailable` (cached images whose file is gone or can't be named). The size warning's numbers |
| `POST /api/v1/admin/export` | queues an export job and starts it; 202 with the job. Body `{"include_hashes": bool, "include_artwork": bool}`, both **off by default**; an empty body means both off, and an unknown field is refused |
| `GET /api/v1/admin/export/{job}/download` | the finished bundle, `application/zip`, as `parlour-export-<UTC time>.zip`. 409 while the job runs, 404 for a failed job, a deleted bundle or a job that isn't an export |
| `DELETE /api/v1/admin/export/{job}` | removes the bundle from the server's disk; the job's result gains `"deleted": true` |

- **The hash confirm** is `include_hashes`, an explicit per-export
  flag, off by default. A job payload missing it, or holding
  anything but `true`, reads as off. The Jobs page never sets it;
  the warning that goes with it is XFER8's page (§6).
- **The job** (`export_bundle`) runs as soon as it is queued. A worker
  of its own picks up any export left queued, for example after a
  restart, so a long export with artwork never holds up scans or
  metadata. It:
  - reads every `Value` column of `coverage.rs` in one read-only
    `REPEATABLE READ` transaction, and takes `exported_at` from that
    transaction's `now()`;
  - turns ids into names, and fails with a message if a reference
    leads nowhere or the database holds a value outside a closed set;
  - writes the zip as `<PARLOUR_CACHE_DIR>/exports/<job id>.zip.part`,
    then renames it into place, so a download never sees half a
    bundle. The cache directory is the compose stack's `/cache`
    volume;
  - records the counts on the job's result: users, profiles,
    libraries, items, watch state rows, recent searches, Activity
    rows, artwork files, artwork left out, duplicate images left
    out, the file's name and its bytes;
  - adds an `export.completed` or `export.failed` Activity row by the
    requesting admin. A completed row's summary says when hashes
    went in.
- **The Jobs page** offers "Export the server" under *Run now*. It
  has one option, the artwork opt-in, and shows the estimate as its
  size warning. A finished export's row has **Download** and
  **Delete from server**, and is marked when the bundle holds hashes.
- **Order:** this is the order the notes asked for. Names compare
  byte by byte, so uppercase sorts before lowercase.
  - users by username;
  - profiles by username, then `created_at`, then name;
  - libraries by name, with paths sorted and grants by username, then
    profile;
  - items by library, then `stable_key`;
  - people by `sort_order`, then name, then role;
  - segments and images by kind (and source or variant), in the order
    §2 lists the values (`intro` `credits` `recap`; `poster`
    `backdrop` `logo` `still`), not alphabetically;
  - watch state by username, profile, then item;
  - recent searches newest first;
  - Activity oldest first, with the row id breaking a tie in the same
    microsecond.

  So two exports of an unchanged server write the same documents.
  Only the manifest's `exported_at` (and the zip entries' times)
  differ, plus the Activity rows the earlier export added. The
  XFER1 fixtures are hand-ordered; readers don't rely on order.
- **Artwork:**
  - **Which images can carry a file:** only an image with a real
    `path` (not empty, not `pending:…`) and a `hash`. Its name is
    `artwork_file_name(hash, lowercased extension of the path)`.
  - **Checks first:** before the bundle names a file, the job reads
    it and checks that its bytes still hash to `images.hash`. If the
    file is missing or changed, or its name doesn't fit the format,
    it is left out. The image then travels as a provider reference
    only, and the job's errors list it.
  - **Shared files:** cached files with the same bytes share one name.
    Each name is added once.

**Readings made in XFER2 — for the owner's veto.** None is a new
decision. Each is how this build read D14/D25, the TODO or these
notes where they don't say:

1. **A stale duplicate image is dropped, not fatal.** The database
   keys images by (item, kind, path), the bundle by (item, kind,
   variant). When a re-match leaves two rows for one key, the cached
   row is kept, otherwise the lowest path. The other row is listed
   on the job's errors and counted in `duplicate_images_left_out`.
   The rows are dropped so the rest of the server still exports.

   The known way to get such a row: re-matching a series deletes
   only the series' own provider images, then re-inserts each
   episode still as `pending:still:original`, next to the already
   downloaded one. That is a catalog bug, left for its own task.
2. **Bundles stay on the server until deleted.** Nothing removes
   them automatically; XFER7's retention will cover the nightly
   ones. A bundle holding hashes is as sensitive as the logins, so
   its job row says so, and Delete is one click.
3. **The Jobs page offers no hash option.** The API flag is the
   confirm. The warning in §6 belongs to XFER8's page.
4. **Exports are recorded in the Activity feed** (`export.*`, with a
   new "Exports" filter), so the audit trail shows who took the
   server's data, and whether the hashes went with it.

## 12. Import (XFER3)

Built 2026-11-25 to the notes XFER1 and XFER2 left here, and to
**D37** and **D38** (decided 2026-11-25; recorded in
`docs/DECISIONS.md`). Code: `crates/server/src/import/`.

**The API** (admin only; anyone else gets a 404, like every admin
route):

| route | does |
|---|---|
| `POST /api/v1/admin/import` | the bundle as the request body (`application/zip`). Streamed to `<PARLOUR_CACHE_DIR>/imports/<id>.zip`, refused past **4 GiB**, then opened with `BundleReader`, which checks the whole bundle. 201 with `{id, bytes, report}`, where `report` is the dry run. Not a bundle, or an inconsistent one: 400 with the reader's reason, and nothing is kept |
| `POST /api/v1/admin/import/{id}/dry-run` | the dry run again, for the server as it is now, with the options in the body |
| `POST /api/v1/admin/import/{id}/apply` | queues an `import_bundle` job with the options and starts it; 202 with the job. Its result is the same report, for what it did |
| `DELETE /api/v1/admin/import/{id}` | removes an upload |

The options are `{"library_paths": {"<library name>": ["/path", …]}}`.
They give a library folders on this host in place of the bundle's,
which are the old host's. The body is strict: an unknown field, a
library the bundle doesn't have, or a path that isn't absolute is a
400.

**Bounds.** The upload is capped at 4 GiB, which leaves room for
artwork. Each JSON document is also capped at 512 MiB unpacked
(`MAX_DOCUMENT_BYTES`, checked by `BundleReader`), because the
reader parses each one whole in memory. The reader also never reads
past that bound, even when a zip header understates the size.

**One walk, two endings.** The dry run and the job run the same
code (`import::apply::run`) inside one transaction. The dry run rolls
it back; the job commits it. So the report the admin reads is what
the apply does, unless the server changes in between. That's why
the apply recomputes rather than replaying the dry run. Imports
take a transaction-level advisory lock, so two never run at once.
A failed apply rolls back whole and changes nothing.

**Insert order** is the one these notes asked for: users; then
profiles; then libraries, paths and access; then items, parents
first; then people, segments and images; then watch state, recent
searches and Activity.

**Never delete; change only what's newer.** Nothing in the import
issues a `DELETE`. Rows that carry their own clock change only when
the bundle's copy is newer by that clock:

- watch state and segments by `updated_at`;
- recent searches by `searched_at`;
- an item's metadata by `last_metadata_at`. A freshly scanned,
  never-matched item takes the bundle's metadata; an item matched
  here more recently keeps its own.

Rows with no clock are only ever added. These are accounts,
profiles, libraries, folders, grants and people. A person's missing
`tmdb_id` is filled in, never replaced. Re-importing a bundle
therefore changes nothing and duplicates nothing.

**How each thing matches:**

| thing | matched by | when it's already here |
|---|---|---|
| user | `username`, ignoring case | **the same account** when its `created_at` matches too, to the microsecond (an earlier import of this bundle, or the server it came from): kept as it is. **Otherwise a conflict**: reported, never merged. The user is skipped with their profiles, grants, watch state and recent searches. The one exception is D38's adoption, below |
| profile | (user, name) | kept as it is |
| library | `name` | kept as it is, if it has the same kind. Another kind is a conflict, and its items, grants and watch state are skipped. It gains only the folders the admin names in the options |
| item | (library, `stable_key`); failing that, a provider id (`tmdb_id`, `tvdb_id` or `imdb_id`) within the same library and kind, among items here whose key the bundle doesn't use | the item here keeps its key and its place in the tree. Its metadata changes only by the newer-wins rule. Two or more provider-id candidates is a conflict, and the item is skipped. So is the same key with another kind. A child of a skipped item is skipped too |
| credit | (item, person, role) | added. Updated only on an item whose metadata came from the bundle |
| segment | (item, kind, source) | newer `updated_at` wins |
| image | (item, kind, variant) | added, or re-pointed at the bundle's `source_url`, only on an item whose metadata came from the bundle. A row still waiting for its download gets the bundle's file when there is one |
| watch state | (profile, item) | newer `updated_at` wins |
| recent search | (profile, query ignoring case) | newer `searched_at` wins |
| Activity entry | its own fields: `created_at`, `kind`, `actor`, `summary` and `detail` | present: not added again |

**New items arrive unavailable.** `available` is file state (§5.5),
so an imported item stays unavailable until a scan of its library
finds its files. The keys match only when the folders under each
library root are laid out as on the old host: movie keys include the
folder, and audiobook keys the relative path. When they don't, the
provider-id fallback catches what it can.

**The two problems these notes flagged, as decided:**

1. **A user imported without a hash (D37(b)).** The user is created
   **locked**. The stored hash is a well-formed Argon2id PHC string
   whose "hash" is random bytes, so no password matches it. It
   passes the format's hash check and travels through a later export
   unchanged. Its salt starts with a fixed marker (`parlourLocked000`),
   so the server can tell a lock apart from a real hash
   (`auth::locked_hash` / `is_locked_hash`). The new admin action
   **`PUT /api/v1/admin/users/{id}/password`** (`{"password": …}`,
   with the sign-up rules) gives any account a working password. Like a
   self-service change, it ends that account's other sessions. It
   records `user.password_set` in the Activity feed. On Server →
   Users, `GET /api/v1/admin/users` now says `password_locked`; the
   page marks those accounts "No password" and offers **Set
   password** on every row. If a later import carries the hash, a
   locked account takes it; a real password here is never replaced.
2. **The fresh install's first admin (D38(b)).** When the bundle's
   admin clashes by name with this server's **pristine** bootstrap
   admin, it adopts that account instead of being a conflict. Every
   other clash is still a conflict. The account is pristine when all
   of these hold:
   - it is the server's first account, taken before the import adds
     any (the bootstrap only ever creates the first user);
   - it is an admin and not disabled, and so is the bundle's user;
   - it still has exactly the one profile the bootstrap gave it,
     with no PIN, and that profile still bears the account's name (a
     rename leaves the profile's name behind);
   - that profile has no watch state, audiobook progress or recent
     searches.

   Adoption keeps the account's id, so the admin running the import
   stays signed in. The account takes the bundle's username,
   `created_at` (so a re-import sees the same account) and password
   hash. Without a hash in the bundle, it keeps the bootstrap
   password. Its profile takes the bundle's same-named profile's
   settings.

**The rest of these notes, as built:**

- **Users are created directly,** not through `create_user`, so no
  extra default profile appears: the bundle carries every profile.
- **Library paths:** the dry run lists each library's bundle folders
  and whether each exists on this host, and which folders the import
  adds.
- **Segment times** are positions in the old host's files. The report
  says so whenever it carries timed segments.
- **`needs_review` items** arrive without candidates (§8), and the
  report says how many there are.
- **Artwork:** the job extracts only the names `BundleReader` lists,
  through `copy_artwork`, which checks each file's hash. It stages
  them under `<cache>/imports/<job>.artwork/` and copies each one into
  the artwork cache as
  `<PARLOUR_ARTWORK_DIR>/<item id>/<kind>-<variant>-<first 16 of sha256>.<ext>`,
  written beside its place and renamed in. A file that fails its check
  is left out and listed on the job's errors, and its image waits for
  its download like any other. An image whose variant isn't safe in a
  file name (`[A-Za-z0-9_-]`, 1–32 characters) travels as a
  provider reference only.
- **Activity entries** whose user, profile or item wasn't imported
  keep their text, without that link, like a deleted account's. The
  report counts them.
- **The audio tables** are not imported. Format v1 doesn't carry them,
  and §9's proposal is still open.

**The report** (`import::Report`, the dry run's and the job's
`result`):

- `dry_run`, and the bundle's `manifest`;
- `users[]`: `{username, is_admin, action, sign_in,
  existing_username, note}`. `action` is `create`, `existing`,
  `adopt` or `conflict`. `sign_in` says how the account signs in
  afterwards: `from_bundle` (the old password), `locked` (D37),
  `kept` (as it is here) or `none` (not imported);
- `libraries[]`: `{name, kind, action, bundle_paths[], paths_added[],
  note}`, each path with `exists_here`;
- `conflicts[]`: `{what, key, reason}`, listing at most 500, with
  `conflicts_total` counting them all;
- a tally `{created, updated, existing, skipped}` for each of profiles,
  access, items, people, credits, segments, images, artwork files,
  watch state, recent searches and Activity. There is also
  `items_matched_by_provider_id`;
- `notes[]`, plain sentences for the admin.

Once applied, the upload is **deleted**, because it may hold hashes. A
failed apply keeps it for another try. An `import.bundle_applied` or
`import.bundle_failed` Activity row records the requesting admin;
these fall under the existing "Imports" filter. The Jobs page lists
the job as "Server import". Its result renders as the page renders
any job's; XFER8's page is where the report gets a real view.

**Readings made in XFER3 — for the owner's veto.** None is a new
decision. Each is how this build read D14, D25, D37, D38, the TODO or
these notes where they don't spell it out:

1. **"Updates" means newer wins, by each row's own clock.** Rows with
   no clock are added, never changed. So an import never rolls back
   anything newer here, and a re-import is a no-op. An item's clock is
   `last_metadata_at`, not `updated_at`, because a scan bumps
   `updated_at` without making the metadata any better.
2. **"The same account" is the same username plus the same
   `created_at`.** That is how a re-import, or an import back onto the
   server the bundle came from, avoids reporting every user as a
   conflict. A different account with the name is still a conflict
   (TODO).
3. **Existing accounts' credentials are never replaced.** That covers
   the password hash, the PIN, `is_admin` and `disabled_at`. The only
   change is a D37 lock (password or PIN) taking the bundle's hash.
4. **PINs follow D37 too.** A profile with `has_pin` but no hash in
   the bundle gets a locked PIN, so it isn't left open. Its owner sets
   a new PIN when editing the profile, which already needs no old PIN.
5. **The D37 lock carries a visible marker** in its salt, so the
   admin's user list can flag "No password". It is still a valid
   Argon2id string that no password matches.
6. **D38's "pristine"** is the checklist above. "No state of its own"
   is read as no watch state, audiobook progress, recent searches,
   PIN or extra profile. Preferences and library grants don't count.
   The bundle's user must be an admin and not disabled. The adopted
   account keeps its bootstrap password when the bundle has no hash.
7. **An existing library keeps its own folders.** It gains only the
   ones the admin names. A created library gets the bundle's folders
   unless the admin names others.
8. **Activity identity leaves out the links.** An entry's user,
   profile and item are where it attaches on this server, resolved by
   name each time. So an entry imported once without a link isn't
   duplicated after a later import resolves it.
9. **Imported items are created unavailable,** so they appear only
   once a scan finds their files (§5.5: `available` is file state).
10. **The upload is deleted once applied,** and kept when the apply
    fails.
11. **The provider-id fallback** looks only at items of the same
    library and kind whose `stable_key` the bundle doesn't use itself,
    and needs exactly one candidate. Two or more is a conflict.

## 13. Sidecars with the media files (XFER4)

Built 2026-11-25 for D14's second half: per-item metadata stored
**with the media files**, so it travels when the files move. Code:
`crates/catalog/src/sidecar/`. The bundle (§1–§12) and the sidecars
split the record the way D14 does: *item* metadata goes in sidecars,
*per-user* state (watch history, logins, audiobook progress) goes
only in the bundle. A sidecar never holds anything about a user.

### 13.1 Where sidecars live

| item | sidecar | where |
|---|---|---|
| movie | `<video stem>.parlour.json` | beside each of the movie's video files: `Arrival (2016).mkv` → `Arrival (2016).parlour.json` |
| episode | `<video stem>.parlour.json` | beside each of the episode's video files |
| series | `series.parlour.json` | in the **series folder**: the nearest folder above an episode that isn't a season folder (`Season 01`, `S01`, `Specials`) |

- Seasons have no sidecar. Nothing the matcher sets lives on a season.
- An episode directly in the library root has no series folder, so
  its series gets no sidecar from it.
- A video whose stem is `series` gets no per-file sidecar, because
  that name belongs to the series folder.
- Two videos with the same stem (`Movie.mkv`, `Movie.mp4`) share one
  sidecar. They are one item anyway.
- Sidecars belong to video libraries for now. Audiobooks libraries
  have their own scanner and no provider lookup yet. AB6 brings
  books in (D30).
- Only **available** files' sidecars are read or written.

### 13.2 The document

A sidecar is one JSON object (UTF-8, the §2 conventions: two-space
indents, a final newline, dates `YYYY-MM-DD`). Examples, byte for
byte what the writer produces, are in
`crates/catalog/tests/fixtures/sidecar-v1/`: a movie (with credits
and a manual segment), a series, and an episode (with a D21 marker).
All of them are fictional.

| field | type | for | holds |
|---|---|---|---|
| `format` | string | all | always `"parlour-sidecar"` |
| `format_version` | integer | all | always `1` |
| `kind` | string | all | `movie`, `series` or `episode`. It must match the item the file belongs to |
| `title` | string | all | the item's title (`sort_title` is derived from it) |
| `year` | integer | all | release or first-air year |
| `season_number`, `episode_number` | integer | episode | **checks, not values**: when present they must equal the numbers parsed from the file name, or the sidecar is refused (it belongs to another episode). They are never applied, because they are part of the item's `stable_key` |
| `air_date` | date | all | |
| `overview` | string | all | |
| `genres` | array of strings | all | |
| `content_rating` | string | all | e.g. `PG-13`, `TV-14` |
| `vote_average` | number | all | 0–10 |
| `runtime_minutes` | integer | all | |
| `tmdb_id`, `tvdb_id` | integer | all | provider ids |
| `imdb_id` | string | all | |
| `people` | array | all | credits: `{name, role, character_name, sort_order, tmdb_id}`. `name` and `role` are required, the rest optional. `sort_order` defaults to the entry's position in the list |
| `segments` | object | movie, episode | `{size_bytes, entries}`. `size_bytes` is the size of the video the times were measured on. `entries` is a list of `{kind, start_ms, end_ms}`: `kind` is `intro`, `credits` or `recap`; times are milliseconds into that video, or both absent for a D21 "no segment of this kind here" marker |

**Present means authoritative; absent means not carried.** Every
field after `kind` is optional. A field that is present wins over the
provider. A field that is absent, or `null`, isn't carried, and still
comes from the provider as before. An array that is present is
carried even when empty: `"genres": []` means "no genres", not
"ask the provider". The writer leaves out what the database holds as
NULL or empty, so a Parlour-written sidecar never pins an empty
value.

### 13.3 Rules a valid sidecar follows

The reader refuses a sidecar that breaks any of these. A refused
sidecar is reported on the scan job (path and reason) and skipped,
and the item keeps whatever it had.

- **Version first.** `format` and `format_version` are read before
  anything else, so a newer sidecar is refused for its version
  ("the sidecar is format version 2; this server reads version 1"),
  not for a field.
- **Strict.** An unknown field is an error, anywhere: at the top, in
  a credit, or in a segment. So is a field given twice, or a value of
  the wrong type. Any change to the shape is a new version.
- Text fields, when present, aren't empty or blank. Leave a field
  out instead.
- `year`, `runtime_minutes`, `tmdb_id`, `tvdb_id` and a credit's
  `tmdb_id` are above 0. `vote_average` is between 0 and 10.
  `season_number` and `episode_number` are 0 or more, and appear only
  on an episode.
- `genres` has no empty entry and no repeats.
- A credit's (`name`, `role`) appears once. The same person in two
  roles is fine.
- `segments` appears only on a movie or an episode, and `size_bytes`
  is above 0. Each `kind` appears once. Times are both present with
  `0 ≤ start_ms < end_ms`, or both absent (a marker).
- **One item, one story.** When an item reads several sidecars (a
  movie in two files, a series spread over two folders), they must
  agree on everything except `segments`. If they don't, all of them
  are refused for that scan.

### 13.4 Which segments a sidecar carries

**Only `manual` rows:** the times an admin set in the editor (SEG4),
and D21 markers. Detected rows (`chapters`, `heuristic`,
`fingerprint`) stay out. Segment detection rebuilds them from the
video on any host, and it never touches manual rows. So a sidecar's
segments and detection can't fight.

Segment times are positions in one encode. As with a bundle import,
they hold only where the host has the same file. The sidecar says
which file, by `size_bytes`:

- The scanner applies a sidecar's segments only beside a video of
  exactly that size. Otherwise it leaves them out, keeps the rest of
  the sidecar, and counts the item in the scan's
  `sidecar_segments_skipped`.
- The writer puts the item's segments only in the sidecar of its
  **playback file** (the first available file by path, the one the
  player and the editor use), with that file's size.
- Applying a sidecar's segments makes the item's manual rows exactly
  the listed ones. A kind the sidecar doesn't list loses its manual
  row.

### 13.5 Reading: the scanner

At the end of each scan of a video library, after files are
catalogued and vanished ones marked (C8), the scanner reads every
item's sidecars. A provider lookup happens only later, in the
metadata job, so the sidecar is read **before** any lookup.

- **New or changed sidecars are applied.** "Changed" means the path,
  size or mtime of any of the item's sidecar files differs from the
  last read (`item_sidecars.fingerprint`). Applying writes the
  carried fields, credits and segments in one transaction, and
  records the document. An unchanged sidecar is not applied again.
  So an edit made in Parlour after the read (a segment edit, say) is
  not undone by the next nightly scan.
- **A re-read video gets its sidecar back.** A new or changed video
  re-creates its item's row from the file name, which resets the
  title and year. The scanner then re-applies that item's recorded
  sidecar, unless an admin's fix overrode it (§13.6).
- **A sidecar that is gone stops counting.** Its record is dropped.
  The item keeps its values, and the next metadata match may change
  them. This happens only when every library root walked cleanly, so
  an unmounted disk never looks like deleted sidecars.
- The scanner doesn't change `metadata_status`. An item with a full
  sidecar still reads "unmatched" until the metadata job matches it,
  usually at once, by the sidecar's id.
- The scan report adds `sidecars_applied` and
  `sidecar_segments_skipped`. Refusals go in its `errors`.
- **A scan never writes.** Sidecars are written only by the metadata
  job (§13.7).

### 13.6 Precedence: sidecar, provider, admin

- **Sidecar over automatic lookups.** The matcher (MD2) gains a
  rule 0: when the item's recorded sidecar names a provider id for
  the active provider (`tmdb_id` for TMDB, `tvdb_id` for TVDB), that
  record is fetched directly, at confidence 1.0, with no search.
  After any automatic match applies a record (rule 0, 1 or 2), the
  sidecar's carried fields and credits are put back over it, on the
  item and, for a series, on its episodes (a series lookup enriches
  them too). Fields the sidecar doesn't carry keep the provider's
  values. Artwork always comes from the provider; poster and backdrop
  files beside the media are XFER5's.
- **The admin's fix-match over the sidecar.** A manual apply (MD4)
  is an explicit choice, not a lookup, so it wins:
  - In a library that writes sidecars, the fix rewrites them at once,
    so the sidecar and the item agree again.
  - In a library that doesn't, the item's sidecar records (and its
    episodes', for a series) are marked overridden
    (`item_sidecars.overridden_at`). The matcher then ignores them,
    and the scanner doesn't re-apply them, until the sidecar file
    changes. A changed file is read and wins again.
- **What the provider can no longer change.** Once a sidecar carries
  a field, a later automatic lookup can't change it, even if the
  provider's value drifts (a new vote average, say). To take fresh
  provider values, fix-match the item (the same id is fine), or edit
  or delete the sidecar.

### 13.7 Writing: the metadata job

After the `fetch_metadata` job matches an item, whether for a whole
library or one item, and after an admin's manual apply, Parlour
rewrites that item's sidecars from what the database now holds:

- **Only where the library opted in.** The opt-in is
  `libraries.write_sidecars`, off by default (D14: media mounts stay
  read-only). With it off, nothing is written, and the media folders
  aren't even looked at for writing. XFER4 adds the column and obeys
  it. Turning it on (API, admin UI, and the mount change) is XFER5's
  task, so until then no library writes.
- **Only matched items.** The item must be `matched`: a movie, or a
  series with its series-folder sidecars and its episodes' sidecars.
  An unmatched or needs-review item gets nothing, so a file-name guess
  is never pinned.
- **Only when something changed.** The writer is deterministic (§2's
  JSON, fields in a fixed order, absent ones left out). A file that
  already holds exactly the new bytes isn't touched. So re-running
  the job writes nothing, and the job's result counts
  `sidecars_written`.
- **Atomically.** Each file is written to a hidden temp file in the
  same folder (`.<name>.<random>.tmp`), synced, then renamed over the
  sidecar, so a reader sees the old file or the new one, never part
  of one. On any failure the temp file is removed and the old
  sidecar stays. A read-only mount fails here.
- **A failed write doesn't fail the job.** Each file that can't be
  written is listed on the job (path and reason), and the rest carry
  on. A value the database holds that no sidecar may carry (a vote
  above 10, say) is left out. A document that would still break a
  §13.3 rule isn't written at all, so Parlour never writes a sidecar
  its own reader refuses.
- After writing, the item's record is updated to the new files'
  fingerprint, so the next scan doesn't re-read Parlour's own
  writes. The filesystem watcher still sees the write and queues a
  scan, which finds nothing new.

### 13.8 What the bundle does with the new schema

- `libraries.write_sidecars` **stays behind.** Whether a library may
  write depends on the host's mounts, so an imported library starts
  with it off.
- `item_sidecars` **stays behind.** It is what one host read from its
  own files; the new host reads the files.

Format v1 of the bundle doesn't change.

### 13.9 Readings of D14 made here — for the owner's veto

None of these is a new decision. Each is how XFER4 read D14 and the
TODO where they don't spell the detail out.

1. **The gate is a column now, operated later.** XFER4 adds
   `libraries.write_sidecars` (default off) and obeys it. XFER5 owns
   turning it on (API, UI, the documented mount change) and the
   poster/backdrop copy. Until XFER5, the writer only runs in tests.
2. **"Authoritative" means over automatic lookups.** An admin's
   fix-match beats the sidecar, and rewrites it where the library
   writes (§13.6). Without that, a wrong id in a sidecar could never
   be fixed from the UI.
3. **Applied when changed, not on every scan.** A sidecar is applied
   when its files change, or when its item's video is re-read.
   Otherwise every nightly scan would undo segment edits and fixes
   made in Parlour since.
4. **Parlour writes everything the database holds** for a matched
   item. So after a first write, provider drift no longer reaches
   that item until a fix-match or a sidecar edit (§13.6). This is
   what "authoritative" plus "the job rewrites sidecars" adds up to.
5. **Only matched items are written.** "Matched metadata" in the
   TODO is read literally.
6. **Segments: manual rows only**, tied to the video's byte size
   (§13.4). The bundle carries every source (§10, reading 7). A
   sidecar carries only what detection can't rebuild.
7. **Season and episode numbers are checks, not values,** because
   they are part of the item's identity (`stable_key`).
8. **Absent and `null` both mean "not carried."** A present array,
   even empty, is carried.
9. **The scanner leaves `metadata_status` alone.** "Matched" stays the
   matcher's word.
10. **A removed sidecar leaves its values behind.** The item isn't
    rolled back to the file name. The provider takes over at the next
    match.
11. **Video libraries only, for now.** Audiobooks have their own
    scanner and no provider lookup yet. D30 already puts the
    `.parlour.json` sidecar last in AB6's audiobook precedence list,
    so AB6 extends this format to books. XFER4 doesn't build that.
12. **The audio proposal (§9, D36) is still open.** It doesn't touch
    sidecars either way: audiobook progress is per-user state, so it
    lives only in the bundle, never in a sidecar.

## 14. In the code

- **`crates/server/src/bundle/`:**
  - `mod.rs` — the types;
  - `archive.rs` — `BundleWriter` and `BundleReader`;
  - `check.rs` — `Bundle::problems()`;
  - `coverage.rs` — the schema map and the CHECK constraints mirrored.
- **`crates/server/src/export/`** (XFER2) — the endpoints, the job
  and its worker; unit tests in `export/tests.rs` (run in CI).
- **`crates/server/src/import/`** (XFER3) — `mod.rs`: the upload,
  dry-run, apply and delete endpoints, the job and its worker;
  `apply.rs`: the walk and the report; unit tests in
  `import/tests.rs` (run in CI). The D37 lock and the admin's set
  password are in `auth.rs` / `invites.rs`.
- **`crates/catalog/src/sidecar/`** (XFER4) — `mod.rs`: the
  `parlour-sidecar` types, the strict reader, the writer's bytes, where
  sidecars live and the atomic write; `sync.rs`: the scanner's pass,
  applying, the matcher's re-apply and override, and the metadata
  job's rewrite. Unit tests in `sidecar/tests.rs` (run in CI). Hooks:
  `scan.rs` (the pass), `metadata_sync.rs` (rule 0, re-apply,
  override), and `crates/server/src/metadata.rs` (the job and manual
  apply write).
- **Fixtures:** `crates/server/tests/fixtures/bundle-v1/`;
  `crates/catalog/tests/fixtures/sidecar-v1/`.
- **Unit tests** in `bundle/tests.rs` (run in CI):
  - each fixture round-trips and is byte for byte what the writer
    writes;
  - whole bundles go through the zip and back, with and without
    hashes, with artwork;
  - every refusal the reader and writer make;
  - the value sets match the server's.
- **Flow tests**, ignored, each on its own throwaway Postgres, run
  locally:
  - `tests/bundle_schema_flow.rs`: the schema against `coverage.rs`;
  - `tests/export_flow.rs`: export a seeded database over HTTP and
    open the downloads with `BundleReader` (hashes off and on,
    artwork off and on, duplicates, delete, the worker);
  - `tests/import_flow.rs`: export a seeded database and import it
    into fresh databases the test creates beside it. It checks that
    the target exports what the source did, that the dry run equals
    the apply, and that a re-import duplicates nothing (Activity
    included). It covers D38's adoption, and, without hashes, D37's
    lock and set password. It also covers a user conflict, a library
    of another kind, a provider-id match, replacement folders, and a
    later import that fills in a locked PIN. Its database role must be
    allowed to create databases.
  - `tests/sidecar_flow.rs` (XFER4): stub TMDB and ffmpeg-made files.
    It checks that a folder without sidecars scans as before and that
    a library that hasn't opted in is never written to. It checks the
    job's sidecars (movie, series folder, episode, a manual segment),
    that a re-run writes nothing, and that a copy of the folder,
    scanned as a new library, gets identical metadata with no lookup.
    It also covers the sidecar beating the provider (and naming the
    record), a re-probe, a refused sidecar, segments for another
    encode, a fix-match with and without writing, and a removed
    sidecar.
