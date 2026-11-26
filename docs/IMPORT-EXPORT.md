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
upload, dry-run report, apply. Nightly backups (XFER7, §15) write the
same bundle to a folder the admin sets, with retention; restoring one
is §12's import. The admin page that drives export and import is
XFER8. The per-item sidecars stored
with the media files are a separate format, `parlour-sidecar` v1,
specified in §13 (XFER4). The per-library write opt-in, the mount
step and the poster / backdrop copy are XFER5 (§13.10–§13.12). NFO
interop, reading Kodi/Emby-style `.nfo` files as an input and writing
them beside the sidecars, is XFER6 (§13.13–§13.14).

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
  decided, so format v1 doesn't carry them yet. *2026-11-26: a
  second open question, D40: whether a book's match and a library's
  metadata precedence (AB6) travel (§13.15). Format v1 doesn't carry
  them either.*
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

  These live only in the environment. Config is environment-only,
  which is why UI6's General page is read-only; the one exception is
  the nightly backup setting (D39, §16.4), which holds no secret and
  stays out of the bundle. No type in the format has a field that
  could hold a secret.
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

At 2026-11-26 the schema has 32 tables (AB6 added `book_metadata`
and `book_sidecars`, AB7 `series` and `book_series`), 13 of them
carried:

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
- `libraries.metadata_precedence` (an audiobooks library's merge
  order, AB6): **open, D40** (§13.15). Until then an imported
  library starts with D30's default order;
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
| `sidecar_artwork` | which `poster.jpg` / `backdrop.jpg` files this host wrote beside its own media (§13.10) |
| `item_nfos`, `sidecar_nfos` | what this host's scanner last read from `.nfo` files, and which ones it wrote (§13.13) |
| `backup_settings` | this host's nightly backup folder, count and hash choice (D39, §16.4): a server setting, and the new host's folders are its own |
| `_sqlx_migrations` | the new host keeps its own ledger |
| `book_metadata` | a book's merged book-only fields and its sources fingerprint (AB6); the new host's scan rebuilds them from the book's files and its match |
| `book_chapters`, `book_progress` | **open — §9** |
| `book_sidecars` | a book's match, the server's copy of its `book.parlour.json` (AB6): **open — D40, §13.15** |
| `series`, `book_series` | audiobook series and each book's place in them (AB7); the new host's scan rebuilds them from each book's series folder and tags. Nothing but a scan writes them, so nothing is lost |

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
- **The Jobs page** offered "Export the server" under *Run now*, with
  the artwork opt-in only. Since XFER8 the export is started on
  Server → Import/Export (§16.2), which owns the hashes warning. A
  finished export's row on the Jobs page still has **Download** and
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
   them automatically. XFER7's retention (§15) covers only the
   nightly bundles in the backup folder, never these. A bundle holding hashes is as sensitive as the logins, so
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
| audiobook (AB6) | `book.parlour.json` | in the **book folder**; a book that is one file in a library root reads `<stem>.parlour.json` beside it but never gets one written (§13.15) |

- Seasons have no sidecar. Nothing the matcher sets lives on a season.
- An episode directly in the library root has no series folder, so
  its series gets no sidecar from it.
- A video whose stem is `series` gets no per-file sidecar, because
  that name belongs to the series folder.
- Two videos with the same stem (`Movie.mkv`, `Movie.mp4`) share one
  sidecar. They are one item anyway.
- Sidecars belong to video libraries for now. Audiobooks libraries
  have their own scanner and no provider lookup yet. AB6 brings
  books in (D30). *Built 2026-11-26: §13.15.*
- Only **available** files' sidecars are read or written.
- Kodi/Emby-style `.nfo` files beside the media are read as an input
  and, where a library writes, written beside the sidecar (XFER6,
  §13.13).

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
| `kind` | string | all | `movie`, `series`, `episode` or (AB6) `audiobook`. It must match the item the file belongs to |
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
| `subtitle`, `publisher`, `isbn`, `asin`, `language` | string | audiobook | AB6, §13.15 |
| `google_books_id`, `open_library_id` | string | audiobook | the ids a match recorded (AB6): a Google Books volume id, an Open Library work id (`OL…W`) |
| `people` | array | all | credits: `{name, role, character_name, sort_order, tmdb_id}`. `name` and `role` are required, the rest optional. `sort_order` defaults to the entry's position in the list. An audiobook's people are `author` / `narrator` with a name and a `sort_order` only |
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
- After this pass, the scanner reads the items' `.nfo` files (XFER6,
  §13.13). They fill only what the sidecar doesn't carry.

### 13.6 Precedence: sidecar, provider, admin

- **Sidecar over automatic lookups.** The matcher (MD2) gains a
  rule 0: when the item's recorded sidecar names a provider id for
  the active provider (`tmdb_id` for TMDB, `tvdb_id` for TVDB), that
  record is fetched directly, at confidence 1.0, with no search.
  After any automatic match applies a record (rule 0, 1 or 2), the
  sidecar's carried fields and credits are put back over it, on the
  item and, for a series, on its episodes (a series lookup enriches
  them too). Fields the sidecar doesn't carry keep the provider's
  values. Artwork always comes from the provider. Parlour doesn't
  read `poster.jpg` / `backdrop.jpg` beside the media; it only
  writes them, where the library opted in (§13.10).
- **The admin's fix-match over the sidecar.** A manual apply (MD4)
  is an explicit choice, not a lookup, so it wins:
  - In a library that writes sidecars, the fix rewrites them at once,
    so the sidecar and the item agree again.
  - In a library that doesn't, the item's sidecar records (and its
    episodes', for a series) are marked overridden
    (`item_sidecars.overridden_at`). The matcher then ignores them,
    and the scanner doesn't re-apply them, until the sidecar file
    changes. A changed file is read and wins again.
- **NFO files rank below all of these** (XFER6, §13.13): below the
  sidecar and the provider, above the file name. The matcher's rule
  0b fetches an NFO's id when the sidecar names none.
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
  aren't even looked at for writing. XFER4 added the column and obeys
  it. XFER5 lets an admin turn it on, and documents the mount change
  that must go with it (§13.11).
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
- **A `.nfo` goes beside each sidecar** (XFER6). It is written in the
  same step, under the same opt-in, scope and rules, and never over
  an NFO Parlour didn't write (§13.13).

### 13.8 What the bundle does with the new schema

- `libraries.write_sidecars` **stays behind.** Whether a library may
  write depends on the host's mounts, so an imported library starts
  with it off.
- `item_sidecars` **stays behind.** It is what one host read from its
  own files; the new host reads the files.
- `sidecar_artwork` (XFER5) **stays behind.** It records which poster
  and backdrop files one host wrote beside its own media. On a new
  host, files it finds there are treated as not Parlour's until it
  writes them itself (§13.10).

- `item_nfos` and `sidecar_nfos` (XFER6) **stay behind**, for the
  same reasons as `item_sidecars` and `sidecar_artwork`. The new host
  reads the NFO files itself, and treats NFOs it finds as not
  Parlour's until it writes them itself, or finds Parlour's exact
  bytes there (§13.13).

Format v1 of the bundle doesn't change.

### 13.9 Readings of D14 made here — for the owner's veto

None of these is a new decision. Each is how XFER4 read D14 and the
TODO where they don't spell the detail out.

1. **The gate is a column now, operated later.** XFER4 adds
   `libraries.write_sidecars` (default off) and obeys it. XFER5 owns
   turning it on (API, UI, the documented mount change) and the
   poster/backdrop copy. Until XFER5, the writer only runs in tests.
   *(XFER5 has since added those: §13.10–§13.12.)*
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
    *AB6 built it on 2026-11-26 (§13.15).*
12. **The audio proposal (§9, D36) is still open.** It doesn't touch
    sidecars either way: audiobook progress is per-user state, so it
    lives only in the bundle, never in a sidecar.

### 13.10 Poster and backdrop beside the media (XFER5)

Where a library has opted in, the artwork step copies a matched
item's cached poster and backdrop next to its media, so they travel
with the files like its sidecar does. Code:
`crates/catalog/src/sidecar/artwork.rs`.

**When.** The copy runs at the end of the artwork step for an item,
`download_item_artwork`: the `download_artwork` job, for one item or
a whole library, and an admin's manual apply (MD4), which caches the
new artwork at once. That is when the cache holds the item's current
artwork. A metadata match on its own doesn't copy, because a match
resets the item's artwork to "to be downloaded" (MD3). So after a
match, run **Download artwork** on the library.

**What and where.**

| item | files | folder |
|---|---|---|
| movie | `poster.jpg`, `backdrop.jpg` | the folder of each of its video files |
| series | `poster.jpg`, `backdrop.jpg` | each of its series folders (where `series.parlour.json` goes, §13.1) |

- Seasons and episodes get none.
- **Only a folder that belongs to the item.** Every available video
  anywhere under the folder must belong to that item (for a series:
  to its episodes), and the folder must not be a library root. So a
  movie loose in the root, or two movies sharing a folder, get no
  copy: a single `poster.jpg` there can't say whose it is. Their
  sidecars are still written.
- The bytes are the cached file, unchanged: the original size, not
  the w300 grid copy (unless only that is cached). The generated
  placeholder tile is never copied.
- **JPEG only.** The files are named `.jpg`. Cached artwork that
  isn't a JPEG (it doesn't start with `FF D8 FF`) isn't copied, and
  the job lists it as left alone. TMDB and TVDB serve JPEGs.

**The same rules as the sidecar writer (§13.7):**

- **Only where the library opted in**, and only for **matched**
  movies and series in a video library. With the opt-in off, the
  media folders aren't looked at.
- **Atomic.** Written to a hidden temp file in the same folder
  (`.poster.jpg.<random>.tmp`), synced, then renamed. On a failure
  the temp file is removed and the old file stays.
- **A file that already holds the same bytes is left alone**, mtime
  included. So re-running the job writes nothing.
- **A failed write doesn't fail the job.** Each file that can't be
  written is listed on the job (path and reason), and the rest carry
  on.

**Never over someone else's file.** Parlour records each file it
writes, with the SHA-256 of its bytes, in `sidecar_artwork`. It
replaces a file only while the file still holds exactly what Parlour
recorded. A `poster.jpg` that Parlour didn't write (another media
server saved it, or someone put it there by hand) is left alone and
listed on the job's result as `copies_left_alone`, with the reason.
The same goes for one of Parlour's copies that someone has since
replaced: from then on it's theirs. To hand a folder back to
Parlour, delete the file; the next artwork run writes its own.

**No artwork.** An item with no cached poster (or backdrop) gets no
file of that kind. A copy Parlour wrote earlier stays where it is:
Parlour never deletes anything beside the media.

**Refreshed when the cached artwork changes.** After a fix-match or a
provider's new image, the next artwork run downloads the new bytes
and replaces Parlour's own copies with them (they still hold the
recorded hash). A file that isn't Parlour's is not refreshed.

**The scanner ignores them.** `.jpg` isn't a video extension, and
the files aren't `*.parlour.json`, so a scan neither catalogues nor
reads them. The filesystem watcher sees the write and queues a scan,
which finds nothing new.

### 13.11 Turning it on: the flag and the mount step (XFER5)

Two things must both be true before Parlour writes anything next to
a library's media: **the library's `write_sidecars` is on**, and
**the folders are mounted writable** for the Parlour process. Either
one alone writes nothing. The flag is off for every library until an
admin turns it on, and no other step turns it on.

**The flag.**

- `GET /api/v1/admin/libraries/{id}` answers with `write_sidecars`
  and `folders`: each root path with `writable` and, when it isn't,
  `problem` (the system's reason, e.g. `Read-only file system (os
  error 30)`). Writability is asked of the kernel (`access(2)`,
  `W_OK`) without writing anything.
- `PUT /api/v1/admin/libraries/{id}/write-sidecars` with
  `{"enabled": true}` or `{"enabled": false}` is the **only** way the
  flag changes. Admins only, and the body is strict (an unknown field
  is refused). It answers with the library as above. It can be turned
  on only for `movies` and `shows` libraries, the ones whose items
  get sidecars today. Turning it off is always allowed. Each change
  goes in the Activity feed (`library.sidecars_on` /
  `library.sidecars_off`, category "Library settings") with the
  admin's name. Setting it to what it already is changes nothing and
  records nothing.
- Creating a library (`POST /api/v1/admin/libraries`) or editing one
  (`PATCH …/{id}`) with a `write_sidecars` field is refused (400). A
  new library starts with it off. An import never sets it (§13.8).
- **Server → Library** has a "Writing next to the media" card per
  movie or show library. It shows the flag, and each folder as
  writable or not writable (with the reason). Turning it on asks for
  a confirmation that says what will be written, that nothing is
  deleted, and that the mount must also be writable. The table has a
  **Download artwork** action per library next to Match metadata.
- **A read-only mount fails cleanly.** The flag can be on while the
  mount is still read-only, for example while the ops change is
  pending. Then:
  - each write fails onto its job (metadata or artwork) as a listed
    error, and the job still succeeds;
  - the old files stay and no temp files are left;
  - the library card shows the folders that can't be written;
  - a warning notice ("Films may write sidecars, but Parlour can't
    write in /media/films") appears on the Dashboard and in Alerts.
    It is reconciled every minute, and it clears when the folders can
    be written or the flag is turned off.

**The mount step (ops, outside Parlour).** The compose file mounts
all media read-only (`${PARLOUR_MEDIA_PATH}:/media:ro`), and it
stays that way. For each library that opts in, the operator:

1. **Mounts that library's folder writable, and only that folder.**
   Add a second, writable bind mount for it on top of the read-only
   one, in the `parlour` service's `volumes:`, e.g. for a library
   whose root is `/media/films`:

   ```yaml
   volumes:
     - ${PARLOUR_MEDIA_PATH:-./media}:/media:ro
     - /srv/media/films:/media/films:rw   # Films writes sidecars (XFER5)
   ```

   A nested bind mount like this is writable while the rest of
   `/media` stays read-only. Don't flip the whole `/media` mount to
   `:rw` (AGENTS.md: "don't just flip the mount"). The library's root
   path in Parlour doesn't change.
2. **Lets the container's user write there.** The image runs as root
   today, so files appear on the host owned by root. If the host
   folder belongs to another user, set the service's `user:` to that
   uid:gid, or grant write access to the folder; otherwise the
   writes fail with "Permission denied", cleanly as above.
3. **Recreates the container** (`docker compose up -d parlour`) so
   the new mount applies. This is the deliberate, owner-run step;
   Parlour can't change its own mounts.
4. **Checks Server → Library.** The library's folders should now
   read "writable". Then turn the library on, run **Match metadata**
   (writes the sidecars and, since XFER6, the `.nfo` files beside
   them), then **Download artwork** (writes the copies). Each job
   lists what it wrote or couldn't write.

To stop: turn the flag off first (nothing more is written), then
remove the extra mount if wanted. Files already written stay. They
are ordinary files, and the scanner keeps reading the `.parlour.json`
ones (§13.5) and the `.nfo` ones (§13.13).

### 13.12 Readings made in XFER5 — for the owner's veto

None of these is a new decision. Each is how XFER5 read D14 and the
TODO where they don't spell the detail out.

1. **"Optionally copies" = part of the same opt-in.** The copy has no
   switch of its own: a library that writes sidecars also gets
   poster / backdrop copies. A separate toggle would be a second
   decision.
2. **The copy runs with the artwork step, not the metadata job.** A
   match resets artwork to "to be downloaded", so only the artwork
   step has the current image. A library-wide **Download artwork**
   action (`POST /api/v1/admin/libraries/{id}/artwork`, the existing
   `download_artwork` job) was added so the copy can be run per
   library; before XFER5 only per-item artwork runs had an endpoint.
3. **Only folders that belong to one item get a copy:** never a
   library root, never a folder shared by two movies (or two
   series). Emby / Kodi-style `<name>-poster.jpg` names for shared
   folders are not written. The TODO names only `poster.jpg` /
   `backdrop.jpg`.
4. **Never over a file Parlour didn't write.** The TODO says a file
   with the same bytes is left alone. XFER5 also leaves alone a file
   with *different* bytes unless Parlour wrote it (the
   `sidecar_artwork` hash). An existing Emby-saved `poster.jpg` is
   therefore kept, never replaced.
5. **Refresh yes, delete never.** Parlour's own copies follow the
   cached artwork; a missing image leaves the old copy in place.
6. **JPEG only**, because the names say `.jpg`. Other formats are
   listed as left alone, not converted.
7. **The flag can be on while the mount is read-only.** Refusing it
   would force the ops step to come first. Instead the library card
   shows each folder's state, every failed write is listed on its
   job, and a notice stays up until the mount is fixed or the flag is
   off.
8. **Only `movies` and `shows` libraries can turn it on.** These are
   the ones the writer covers (§13.9 reading 11). AB6 widens this for
   audiobooks. *Done 2026-11-26: `audiobooks` libraries take the
   same flag (§13.15).*
9. **Changes are audited** in the Activity feed under a new
   "Library settings" category, because the flag lets Parlour write
   on the media disks.
10. **The audio proposal (§9, D36) is still open.** XFER5 doesn't
    touch it: no `book_progress` / `book_chapters` anywhere, and
    audiobook progress, being per-user state, never goes in a sidecar.

### 13.13 NFO files: read as an input, written beside the sidecar (XFER6)

Built 2026-11-25. An NFO is the plain XML file other media software
(Kodi-style players, Emby-style servers, the *arr tools) keeps next
to a video: a `<movie>`, `<tvshow>` or `<episodedetails>` element
with a title, provider ids and usually much more. Parlour **reads**
an NFO only for what identifies the item, its title and ids, as an
input below the sidecar. Where a library writes sidecars, Parlour
also **writes** a small NFO of its own beside each `.parlour.json`,
so other software can identify the same files. Code:
`crates/catalog/src/sidecar/nfo.rs` (the format and file names) and
`nfo_sync.rs` (the scanner's pass, the matcher's rule, the writer's
step).

**Which NFO belongs to which item.**

| item | NFO files read | NFO written |
|---|---|---|
| movie | `<video stem>.nfo` beside each of its videos; `movie.nfo` in a video's folder, **only when that folder belongs to this movie alone** (§13.10's rule: not a library root, and every available video under it is this movie's) | `<video stem>.nfo` beside each video |
| episode | `<video stem>.nfo` beside each of its videos | the same |
| series | `tvshow.nfo` in each series folder (the folder of `series.parlour.json`, §13.1) | the same |

- `season.nfo` is not read: nothing the matcher sets lives on a
  season. A video whose stem is `tvshow` or `season` has no per-file
  NFO, because those names belong to a folder's NFO.
- A `movie.nfo` in a folder shared by two movies is ignored: it can't
  say whose it is. Parlour never writes `movie.nfo` as a folder NFO.
  The one exception is a video literally named `movie.<ext>`: its
  per-file NFO is `movie.nfo`, so that file is read and written as the
  video's own.
- As with sidecars: only **available** files' NFOs, and video
  libraries only. Audiobooks get their own NFO handling in AB6 (D30).
  *AB6 reads an ABS-style `Key: value` `.nfo` in a book folder as one
  of its local sources and writes no NFO beside a book (§13.15).*
- `.nfo` is neither a video nor a subtitle extension, so the scanner
  never catalogues one as either.

**What is read.** The root element gives the kind, which must match
the item's: `<movie>`, `<tvshow>` (a series) or `<episodedetails>`.
Of the root's **direct children**, Parlour reads:

| element | read as |
|---|---|
| `<title>` | the item's title (`sort_title` is derived from it) |
| `<uniqueid type="tmdb">` or `<tmdbid>` | the TMDB id |
| `<uniqueid type="tvdb">` or `<tvdbid>` | the TVDB id |
| `<uniqueid type="imdb">` or `<imdbid>` | the IMDb id |
| `<season>`, `<episode>` (episode only) | **checks, not values**: when present they must equal the numbers parsed from the file name, or the NFO is refused (it belongs to another episode), as with a sidecar (§13.2) |

**Deliberately not read:** everything else. That covers `<year>`,
`<plot>`, `<originaltitle>`, `<sorttitle>`, genres, cast, ratings,
artwork, `<id>` (what it holds depends on which program wrote it),
`<uniqueid>` of any other type or with no type, and anything nested
deeper (a collection's `<set><title>` is not the movie's title). The
item's year comes from the file name, or from the provider when the
name has none. Descriptive metadata comes from the provider, or from
the sidecar where there is one.

**Lenient about what it skips, strict about what it reads.**

- The file is UTF-8 (a byte-order mark is fine), at most 4 MiB, and
  well-formed XML up to the end of its root element. Reading stops
  there. A link after the root (a "combined" NFO) is never read, and
  neither is a second `<episodedetails>`: in a multi-episode file's
  NFO, only the first episode is read.
- Element names match without regard to case. Values are trimmed,
  and an empty element is absent. The five predefined entities and
  character references are resolved. Any other entity is an error.
- An id must be a whole number above 0, and an IMDb id must be `tt`
  followed by digits. A field given twice must agree, for example
  `<uniqueid type="tmdb">` and `<tmdbid>`.
- A link-only NFO (a URL with no XML root) is refused.
- **One item, one story.** When an item reads several NFOs (a movie
  in two files, a `<stem>.nfo` beside a `movie.nfo`, a series in two
  folders), they must agree on the title and the ids. If they don't,
  all of them are refused for that scan.

**A bad NFO never breaks a scan.** An NFO that breaks any of these
rules is refused. The scan job lists it (path and reason) in its
`errors`, the item keeps whatever it had, and the scan carries on
with everything else. Like a refused sidecar, it is reported again on
every scan until it is fixed or removed.

**When the scanner applies it.** The NFO pass runs after the sidecar
pass (§13.5), so the sidecar's fields are already in place.

- **New or changed NFOs are applied.** "Changed" means the path, size
  or mtime of any of the item's NFO files differs from the last read
  (`item_nfos.fingerprint`). The scan report counts them in
  `nfos_applied`. An unchanged NFO isn't applied again.
- **A re-read video gets its NFO's title back**, as with a sidecar,
  unless an admin's fix overrode it.
- **An NFO that is gone stops counting.** Its record is dropped and
  the item keeps its values. As with sidecars, this happens only when
  every library root walked cleanly.
- The scanner doesn't change `metadata_status`.

**Precedence.** For an item's title and provider ids, highest first:

1. **An admin's fix-match** (MD4).
2. **The sidecar** (`.parlour.json`), authoritative over automatic
   lookups (D14, §13.6).
3. **The provider's record** from an automatic match, for what the
   provider sets. That covers the ids, and the episode titles a
   series lookup brings. The provider never sets a movie's or a
   series' title, so the NFO's title stays.
4. **The NFO.**
5. **The file name:** its title and year, and a `{tmdb-603}`-style id
   written into it.

In practice:

- **At the scan**, the NFO's title and each of its ids are written
  onto the item **field by field**, each only where the sidecar
  doesn't carry that field.
- **At the match**, the matcher has a new rule 0b. After rule 0 (the
  sidecar's id), if the NFO names an id for the active provider
  (`tmdb_id` for TMDB, `tvdb_id` for TVDB), that record is fetched
  directly at confidence 1.0, with no search. Rule 1 (an id in the
  file name) and rule 2 (the search) come after. A search uses the
  item's title, which by then is the NFO's (unless the sidecar
  carries one).
- **After a match**, the sidecar's fields are put back as before.
  Nothing of the NFO is put back, because it is an input, not an
  authority. The provider's ids replace the NFO's where the provider
  has one. An IMDb id from the NFO stays when the provider has none.

**When the NFO and the sidecar disagree,** the sidecar wins, field by
field and without a report. The NFO fills only fields the sidecar
leaves out. When the sidecar names the active provider's id, the
NFO's id is never consulted. An older NFO from another program sitting
beside a newer sidecar is ordinary, not an error. In a library that
writes, Parlour's own NFO is written from the same values as the
sidecar, so the two agree.

**An admin's fix-match beats the NFO.** The fix marks the item's NFO
record overridden (`item_nfos.overridden_at`), its episodes' too,
just as it does the sidecar's. Until the NFO file changes, the matcher
ignores it and the scanner doesn't re-apply it. Without this, the next
library match would fetch the NFO's record again and undo the fix. In
a library that writes, the fix also rewrites Parlour's own NFO.

**Writing.** The NFO is written by the same step as the sidecar
(§13.7). Wherever `.parlour.json` files are written, the NFO goes
beside them:

- **Same opt-in, no switch of its own:** `libraries.write_sidecars`
  (§13.11). With it off nothing is written, and the media folders
  aren't looked at for writing.
- **Same scope:** matched movies (an NFO beside each video), and
  matched series (`tvshow.nfo` in each series folder, plus an NFO
  beside each episode's video).
- **Same rules:** each write is atomic (a hidden temp file in the same
  folder, synced, then renamed), and a file that already holds exactly
  the new bytes is left alone, mtime included. A file that can't be
  written is listed on the job (path and reason), and the job still
  succeeds.
- **Never over someone else's NFO.** This is the rule XFER5 uses for
  posters (§13.10). Parlour records each NFO it writes, with the
  SHA-256 of the bytes, in `sidecar_nfos`, and replaces a file only
  while it still holds exactly those bytes. An NFO that another
  program wrote, or that someone wrote by hand, is left alone and
  listed. A Parlour NFO that someone has since edited becomes theirs.
  The job result lists them as `nfos_left_alone` (the first 100), and
  counts what was written in `nfos_written`.
- **Claimed when identical.** An NFO that already holds exactly the
  bytes Parlour would write is recorded as Parlour's. One case is an
  NFO that came with the media from another host. Parlour keeps it up
  to date from then on.
- To hand a folder's NFO back to Parlour, delete the file. The next
  metadata match writes Parlour's own.

**What a written NFO holds:** only the fields that identify the item,
taken from the same database values the sidecar is written from:

- `<title>`;
- `<year>` (movies and series only);
- `<season>` and `<episode>` (episodes only);
- each provider id the item has, written twice: once as
  `<uniqueid type="…">` (the first one marked `default="true"`), and
  once as `<tmdbid>` / `<tvdbid>` / `<imdbid>`. Readers differ in
  which form they look for, and Parlour's reader requires the two to
  agree.

Plot, genres, cast and ratings are not written. The item's full
record is the sidecar, and the NFO only helps other software identify
the files. The writer is deterministic: UTF-8, two-space indents, a
final newline, elements in a fixed order, absent ones left out. Byte
for byte examples are in `crates/catalog/tests/fixtures/nfo/`
(`movie.nfo`, `tvshow.nfo`, `episode.nfo`; fictional), next to an
example of the richer kind of NFO other software writes
(`other-software-movie.nfo`), of which Parlour reads only the title
and ids.

**Parlour's own NFO is not new input.** After writing, when every NFO
the reader would read for that item is one Parlour wrote (its hash
matches), the item's `item_nfos` record is moved to the new files'
fingerprint. The next scan then finds nothing new (`nfos_applied`
stays 0). If another program's NFO sits beside it (a `movie.nfo` in
the folder, say), the record is left for the scan to read, and to
report if the two disagree. The filesystem watcher still sees the
write and queues a scan, which finds nothing new.

### 13.14 Readings made in XFER6 — for the owner's veto

None of these is a new decision. Each is how XFER6 read D14 and the
TODO ("scanner also reads Kodi/Emby-style `.nfo` (ids + title) as an
input source; XFER4 can optionally write `.nfo` alongside the JSON")
where they don't spell the detail out.

1. **An input, not an authority.** D14 makes only the sidecar
   authoritative, and the TODO calls the NFO "an input source". So the
   NFO ranks below the sidecar and below the provider's record, and
   above the file name (§13.13, precedence). Nothing of it is put back
   after a match.
2. **Ids and title only, read literally.** `<year>`, `<plot>` and the
   rest are deliberately not read. `<season>` / `<episode>` are only
   checks, like a sidecar's (§13.9, reading 7).
3. **The NFO's id beats an id in the file name.** Rule 0b runs before
   rule 1, because the NFO is a document about this item and the name
   is only a naming convention.
4. **The sidecar beats the NFO field by field, without a report.** A
   stale NFO beside a sidecar is normal. Refusing or reporting it
   would turn every library that kept another program's NFOs into a
   wall of errors.
5. **Which files:** `<stem>.nfo`; `movie.nfo` only in a folder that
   belongs to one movie; `tvshow.nfo` in the series folder.
   `season.nfo` is ignored. A multi-episode NFO gives only its first
   episode. A trailing link is ignored, and a link-only NFO is refused
   (it has no title or ids to read).
6. **Writing is part of the same opt-in,** on for every library that
   writes sidecars, with no switch of its own. This follows XFER5's
   reading of "optionally copies" (§13.12, reading 1). A separate
   toggle would be a second decision.
7. **The written NFO identifies; it doesn't describe.** It holds the
   title, the year (or the season and episode) and the ids. It doesn't
   carry the full matched metadata, because that is the sidecar's job,
   and a second full copy would be a second record to keep in step.
8. **Never over another program's NFO,** recorded by hash as XFER5
   does for posters (§13.12, reading 4). It matters more here, because
   Emby-style servers and the *arr tools write NFOs under exactly
   these names. One addition: an NFO already holding Parlour's exact
   bytes is claimed, so an NFO that moved hosts with the media stays
   up to date.
9. **Parlour never writes `movie.nfo` as a folder NFO.** It only reads
   it, and only in a folder that belongs to one movie. A video named
   `movie.<ext>` is the exception: `movie.nfo` is its own per-file NFO.
10. **A fix-match overrides the NFO too,** or the next library match
    would fetch the NFO's record again and undo the admin's choice.
11. **The audio proposal (§9, D36) is still open.** XFER6 doesn't
    touch it. An NFO holds no per-user state, and nothing about
    `book_progress` / `book_chapters`.

### 13.15 Audiobooks: `book.parlour.json` (AB6)

Built 2026-11-26 with AB6 (D30). The full rules for a book's metadata
(the per-library precedence list, matching, covers) are in
`docs/AUDIOBOOKS-PODCASTS.md`, "Metadata sources, matching and covers
(AB6)". What the sidecar side adds:

- **The same v1 document**, `kind` `audiobook`, with book-only fields
  (`subtitle`, `publisher`, `isbn`, `asin`, `language`,
  `google_books_id`, `open_library_id`; §13.2). `format_version`
  stays 1: the reader was strict, so an older server refuses a book's
  sidecar for its kind, never misreads it. Video-only fields
  (`air_date`, `content_rating`, `vote_average`, `runtime_minutes`,
  `tmdb_id`, `tvdb_id`, `imdb_id`, `segments`) are refused on a book,
  and the book-only fields are refused on video. An audiobook's
  `people` are `author` / `narrator` entries with a name and a
  `sort_order` only.
- **Where:** `book.parlour.json` in the book's folder. A book that is
  one audio file in a library root reads `<stem>.parlour.json` beside
  it. Parlour never writes there, because a library root is shared.
- **Read** as the `sidecar` source, the last word in the default
  precedence. Every field it carries is authoritative (a present `[]`
  means "none"), as for video.
- **The match's server copy** (`book_sidecars`) is the same document
  kept in Parlour's database. A match always leaves one (D30's
  default: the server's own metadata area). It fills the `sidecar`
  slot unless the book's own file changed after the match. That is
  the §13.6 rule: an admin's match beats the file until the file
  changes.
- **Written** only where the library has `write_sidecars` on (XFER5's
  flag, now accepted for `audiobooks` libraries; no second flag), at
  match time, atomically, only into a folder that belongs to that
  book alone (not a library root, no other book's available audio
  under it). A matched cover goes beside it as `cover.jpg`: only a
  JPEG, never over a file Parlour didn't write, never when the folder
  already has its own cover image.
- **No NFO is written beside a book.** ABS's book `.nfo` is a
  `Key: value` text format, not XFER6's XML, and is read only.
- **The bundle:** `book_metadata` stays behind (rebuilt by the new
  host's scan). Whether `book_sidecars` and
  `libraries.metadata_precedence` should travel is **open, D40**
  (`docs/DECISIONS.md`). Until the owner answers, format v1 is
  unchanged. A match reaches a new host only as a `book.parlour.json`
  written beside the book, and an imported audiobooks library starts
  with D30's default order.
- **The audio proposal (§9, D36) is still open.** AB6 doesn't touch
  `book_progress` / `book_chapters`, and no sidecar ever holds
  progress, which is per-user state.

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
- **XFER5:** `crates/catalog/src/sidecar/artwork.rs` — the poster /
  backdrop copy, called from `artwork.rs`'s `download_item_artwork`;
  `crates/server/src/libraries.rs` — `write_sidecars` on the admin
  library view, the folders' writability, and
  `PUT …/write-sidecars`; `crates/server/src/notices.rs` — the
  "can't write" notice; `crates/server/src/metadata.rs` — the
  library-wide artwork route; `web/src/routes/admin/library/` — the
  card and the toggle. Migration `20261126000000_sidecar_artwork.sql`.
- **XFER6:** `crates/catalog/src/sidecar/nfo.rs` — the NFO reader
  (streaming, via `quick-xml`), the writer's bytes and the file
  names; `sidecar/nfo_sync.rs` — the scanner's NFO pass (called from
  `scan.rs` after the sidecar pass), `stored_nfo` for the matcher's
  rule 0b (`metadata_sync.rs`), and the writer's NFO step (called from
  `write_item_sidecars`). The fix-match override covers NFOs in
  `sidecar/sync.rs`'s `mark_overridden`. Migration
  `20261127000000_nfo.sql` (`item_nfos`, `sidecar_nfos`). Unit tests
  are in `sidecar/tests.rs` (run in CI).
- **XFER7:** `crates/server/src/backup.rs` — the settings, the file
  name, retention, the folder check and the nightly check
  (`enqueue_nightly`, called from `catalog.rs`'s `nightly_tick`);
  `crates/server/src/export/mod.rs` — `run_backup_job` (the same
  `export` as a manual one, written to the backup folder), the
  download from the backup folder and the refused delete;
  `config.rs` — `PARLOUR_BACKUP_DIR` / `PARLOUR_BACKUP_KEEP`;
  `notices.rs` — "Nightly backup failed" and the folder's free-space
  check; `dashboard.rs` + `web/src/routes/admin/general/` — the
  setting shown on Server → General; `web/src/routes/admin/jobs/` —
  the "Nightly backup" row. Unit tests in `backup.rs` and `config.rs`
  (run in CI).
- **XFER8:** `web/src/routes/admin/import-export/+page.svelte` (the
  page) and `web/src/lib/ImportReport.svelte` (the report);
  `crates/server/src/backup.rs` — the saved setting
  (`saved` / `resolve` / `effective`) and `GET`/`PUT`/`DELETE
  /api/v1/admin/backup`; `crates/server/src/sidecars.rs` — the
  sidecar summary; `import/mod.rs` — the uploads list;
  `catalog.rs` + `crates/jobs` — the jobs `kind` filter. Migration
  `20261128000000_backup_settings.sql`. Unit tests in `backup.rs`,
  `import/tests.rs` and `crates/jobs` (run in CI).
- **AB6:** the book sidecar's kind and fields in
  `crates/catalog/src/sidecar/mod.rs`; reading, merging and the
  document Parlour writes in `crates/catalog/src/book_meta.rs`; the
  server copy, the slot and the writes beside a book in
  `crates/catalog/src/book_sync.rs`; `book_metadata` /
  `book_sidecars` in `bundle/coverage.rs`. Migration
  `20261129000000_book_metadata.sql`. Flow:
  `crates/server/tests/book_match_flow.rs`.
- **Fixtures:** `crates/server/tests/fixtures/bundle-v1/`;
  `crates/catalog/tests/fixtures/sidecar-v1/`;
  `crates/catalog/tests/fixtures/nfo/`;
  `crates/catalog/tests/fixtures/books/` (AB6).
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
  - `tests/sidecar_optin_flow.rs` (XFER5): over HTTP with stub TMDB.
    It checks that the flag is off by default and can't be set by
    creating or editing a library, and that with it off neither job
    writes. Turned on, it checks the sidecars and the copies (not in a
    root, not in a shared folder, not over someone else's
    `poster.jpg`), that re-runs touch nothing, the refresh of
    Parlour's own copies, a folder that can't be written (job
    succeeds, paths listed, no temp files), the library's folder
    report and the notice, and that turning it off stops all writes.
  - `tests/nfo_flow.rs` (XFER6): stub TMDB and ffmpeg-made files. It
    checks that an NFO next to a movie is read for its title and ids
    (not its year or plot) and that its id picks the record a search
    would miss. It checks that `movie.nfo` counts only in a folder of
    its own, that `tvshow.nfo` and episode NFOs are read, and that an
    episode NFO naming another episode is refused. A malformed NFO is
    reported and the scan carries on. It checks that the sidecar beats
    the NFO field by field, that a re-probe brings the NFO's title
    back, that a fix-match beats the NFO, and that a removed NFO stops
    counting. With the opt-in on, it checks the NFOs written beside the
    sidecars (and `tvshow.nfo`), that a re-run touches nothing, that
    the next scan doesn't re-read Parlour's own NFOs, and that another
    program's NFO, or one edited by hand, is left alone and listed.
    With the opt-in off, nothing is written.
  - `tests/backup_flow.rs` (XFER7): with no folder set, nothing is
    queued or written. A night writes `parlour-backup-<time>.zip`
    (format v1, no hashes, no artwork), keeps the newest `keep`,
    deletes the older ones and a stale `.part`, and leaves every
    other file alone; the old job stops offering its download, the
    new one downloads from the folder and refuses Delete. A missing
    folder and an unwritable one each fail the job with the path,
    create and delete nothing, raise a notice, and are retried the
    next night (not the same day). The bundle the nightly job wrote
    imports through §12 into a fresh database with the same users,
    profiles and watch history.
  - `tests/import_export_page_flow.rs` (XFER8), through the page's
    endpoints, and the Phase 8 acceptance: every endpoint the page
    uses is a 404 to a non-admin and a 401 signed out. The source is
    exported with hashes from the page and found on its job list; a
    fresh install (a database the test creates) uploads it, lists it
    as waiting, and reads a dry run that shows D38's take-over and the
    counts and changes nothing; a hash-less bundle shows D37's locked
    account before apply and is deleted from the page. Apply, with a
    replacement folder, reports exactly what the dry run said and
    deletes the upload; the target then exports the same users,
    profiles and watch history as the source, and the old passwords
    work. A later bundle shows and does a create, an update and a
    conflict. Sidecar status follows the opt-in, the folder check and
    the notice, and counts the ledgers. Backup settings round-trip;
    saved off beats the environment, nothing saved and no environment
    queues nothing, a saved folder beats the environment's, hashes go
    in only when turned on (and not if turned off before the job runs),
    the earlier nights still download after the folder changes, the
    reset brings the environment back, and every change is audited.

## 15. Nightly backups (XFER7)

Built 2026-11-25 to D14 and D25, with format v1 as it stands (the
§9 audio proposal, D36, is still open, so audiobook progress is not
in these bundles either). Code: `crates/server/src/backup.rs`, and
`run_backup_job` in `crates/server/src/export/mod.rs`. **Restore is
§12's import:** upload the nightly bundle, read the dry run, apply.

**What runs.** There is no second export. The nightly tick that
already queues the C7 scans and the Activity purge (after 07:30 UTC,
checked every 15 minutes) also queues one `export_bundle` job a day,
its payload `{"trigger": "nightly", "include_hashes": false,
"include_artwork": false}`. The export worker (§11) runs it through
the same `export` a manual export uses: one `REPEATABLE READ`
snapshot, the same documents, the same order, the same checks. The
only differences are where the zip goes, what it is called, and the
retention step after it.

**The setting.** As built in XFER7 it was environment-only, set in
the server's `.env` and taking effect at the next start. **Since D39
(XFER8) it is set on Server → Import/Export (§16.4)**; the variables
below are the bootstrap default until an admin saves it there:

| variable | meaning | when unset |
|---|---|---|
| `PARLOUR_BACKUP_DIR` | the folder nightly bundles are written to, an **absolute** path as the server sees it (in the container, e.g. `/backups`) | **no nightly backups.** Nothing is queued and nothing is written anywhere. Parlour never picks a folder on its own |
| `PARLOUR_BACKUP_KEEP` | how many nightly bundles to keep, 1–365 | 7 |

A relative path, or a keep outside 1–365, stops the server at
startup with a message naming the variable, as a bad access URL
does. Server → General shows both values read-only ("Off — no
nightly backups" when the folder is unset). In the compose stack the
folder is a bind mount, writable by the container's user, for
example:

```yaml
    environment:
      PARLOUR_BACKUP_DIR: /backups
      PARLOUR_BACKUP_KEEP: "14"
    volumes:
      - /srv/backups/parlour:/backups
```

`docker-compose.yml` passes both variables through, empty by
default, so the stack behaves as before until `.env` sets them.

**The file.** `parlour-backup-YYYYMMDD-HHMMSS.zip`, the bundle's own
`exported_at` in UTC (the same instant as its manifest), for example
`parlour-backup-20261126-073012.zip`. The fixed-width stamp sorts in
time order, and `parlour-backup-` tells a nightly bundle apart from a
manual download (`parlour-export-…`). The zip is written as
`<name>.part` in the same folder and renamed into place once complete
and synced, so the folder never holds half a bundle under a real
name.

**Retention: by count.** After tonight's bundle is in place, the job
keeps the newest `PARLOUR_BACKUP_KEEP` nightly bundles (tonight's
included), by the stamp in the name, and deletes the rest. It also
deletes a leftover `parlour-backup-….zip.part` from a run that died
mid-write (only one export job runs at a time, so no other run's
part can be live). It only ever touches plain files whose names match
`parlour-backup-` + 8 digits + `-` + 6 digits + `.zip` (or that plus
`.part`) exactly; manual downloads, the admin's own files and folders
are never touched. Count, not age, because an age rule would delete
every bundle after a long outage, while a count always leaves the
last good ones. Retention runs **only after a successful write**, so a
failed night never costs a good bundle. A bundle that can't be
deleted is listed on the job's errors; the job still succeeds,
because tonight's backup is good. The job result lists what
retention removed (`removed_by_retention`), and the jobs that wrote
those bundles are marked `deleted`, so the Jobs page stops offering
their download.

**A bad folder.** Before any export work, the job checks the folder:

- **missing** — it fails with "the backup folder `<path>` doesn't
  exist (Parlour doesn't create it: is its disk mounted?)". Parlour
  never creates the folder: a missing folder usually means a backup
  disk that isn't mounted, and creating it would fill the disk
  underneath the mount point;
- **not a folder**, or **not writable** by the server (permissions,
  a read-only mount; asked with `access(2)` as XFER5's folders are) —
  it fails with the path and the reason;
- **a write that fails part-way** (a full disk, say) — it fails with
  the path; the `.part` is removed and nothing older is deleted.

**Retry.** The nightly check queues a job only if no nightly job was
queued since the start of the UTC day, failed or not. So a failed
night is not retried every 15 minutes or at a restart that day; the
next night queues a new job, which succeeds once the folder is back.
No run is lost silently.

**Where an admin sees it.**

- **Jobs page:** the row is "Nightly backup". A finished one offers
  **Download** (served from the backup folder as configured now) and
  says where it was saved; there is no **Delete** (the API refuses
  it with 409), because retention owns these files. A removed one
  says "Removed by retention". A failed one shows its error with the
  path.
- **Dashboard notices (UI5):** a failed nightly backup raises an
  error notice, "Nightly backup failed", with the message, as any
  failed job does. The backup folder also joins
  the D16 free-space check (warning under 5 GB, error under 1 GB),
  labelled `backups`.
- **Activity feed:** `export.completed` "Nightly backup written" or
  `export.failed` "Nightly backup failed", recorded as the system's
  (no user), under the existing "Exports" filter. The detail has the
  file name, size and what retention removed, or the error.

**No hashes, no artwork.** The nightly job always exports with both
off, whatever its payload says:

- **Hashes:** D14 lets password and PIN hashes leave the server only
  behind an admin's explicit confirm with a warning, and §6 reads
  that confirm as per export ("for that one export"). A standing
  nightly job has no confirm to give, so its bundles carry none; the
  same is true of a bundle sitting unattended in a folder every
  night. **The consequence on restore:** every user arrives locked
  (D37) and needs an admin's **Set password**, PIN'd profiles get a
  locked PIN, and the new install's bootstrap admin, adopted under
  D38, keeps its bootstrap password. A nightly backup with hashes
  would need a new decision from the owner; it isn't built.
- **Artwork:** off, as it is by default for manual exports (§11): it
  can make every bundle many times larger. Images travel as provider
  references and are downloaded again after a restore.

**Is the bundle a whole backup? No — say so plainly.** A nightly
bundle restores users, profiles and their preferences and recent
searches, libraries and grants, the catalog's metadata and segments,
watch history and the Activity feed (§7). It does **not** restore,
on its own:

- **logins:** no password or PIN hashes (above), so everyone needs a
  new password from an admin;
- **audiobook progress** (`book_progress`): format v1 doesn't carry
  it until the owner answers §9 (D36);
- what §8 leaves behind on purpose: sessions, invites, job history,
  notices, file rows (a scan rebuilds them), fingerprints, each
  library's `write_sidecars` opt-in and the sidecar ledgers;
- the server's `.env` (secrets, settings) and the artwork cache.

A database dump covers the first two. The TODO's cross-cutting item
puts a Postgres dump into sullivan's existing backup run on the day
Parlour holds real watch state, with the host's backup script
updated in the same change. That is production work and is **not**
part of XFER7; nothing here touches a host backup script. Until it
exists, the nightly bundle is the portable, format-checked copy of
everyone's state, and it restores everything above except logins
and audiobook progress.

**Readings made in XFER7 — for the owner's veto.** None is a new
decision. Each is how this build read D14, D25, the TODO or these
notes where they don't spell it out:

1. **The setting is environment-only**, like every other setting
   (§6: there is no settings table), and **unset means off**: no
   folder is ever picked on the admin's behalf.
2. **The folder must be absolute** and is **never created**: missing
   means "not mounted", and the job fails rather than writing under
   the mount point.
3. **Retention is by count, default 7**, counted over nightly
   bundles only, applied only after a successful write.
4. **Nightly bundles carry no hashes** (D14's confirm is per export)
   and **no artwork** (§11's default). Restoring one locks every user
   until an admin sets a password (D37).
5. **One try per UTC day.** A failed night is retried the next night,
   not within the same day; the failure is on the Jobs page, the
   Dashboard notices and the Activity feed.
6. **The schedule is the existing nightly tick** (after 07:30 UTC),
   not a setting of its own.
7. **Nightly bundles can be downloaded from the Jobs page but not
   deleted there**; retention owns them. Manual exports keep their
   Download / Delete.

**Superseded in part by D39 (XFER8, §16):** the setting is no longer
environment-only. It is set on Server → Import/Export and stored in
`backup_settings`; `PARLOUR_BACKUP_DIR` / `PARLOUR_BACKUP_KEEP` are the
bootstrap default until an admin saves it. Reading 1 above is replaced
by that, reading 4's "no hashes" now means "no hashes unless the admin
turned them on there", and reading 7's Download now reads from the
folder the job wrote to (§16.4). The rest of §15 stands.

## 16. Server → Import/Export (XFER8)

Built 2026-11-25 to D14, D25, D37, D38 and **D39** (decided 2026-11-25,
recorded in `docs/DECISIONS.md`), with format v1 as it stands. The §9
audio proposal (D36) is still open, so nothing here carries
`book_progress` or `book_chapters`. This section sits after §15
because the page is the admin surface over §11, §12, §13.10–§13.12 and
§15: it adds no export, import or sidecar logic of its own.

**The page** is `/admin/import-export` (UI6 put it in the Server tree;
`web/src/routes/admin/import-export/+page.svelte`). It has four parts,
in this order: Export, Import, Nightly backups, Metadata files beside
the media.

### 16.1 The endpoints it uses

All admin-only: anyone else gets a 404, and nobody signed in a 401.

| part | endpoint | new in XFER8? |
|---|---|---|
| Export | `GET /api/v1/admin/export/estimate`, `POST /api/v1/admin/export`, `…/{job}/download`, `DELETE …/{job}` | no (§11) |
| Export, backups, imports | `GET /api/v1/admin/jobs?kind=export_bundle` / `?kind=import_bundle` | **the `kind` filter**: one job kind only; an unknown kind is a 400 |
| Import | `POST /api/v1/admin/import`, `…/{id}/dry-run`, `…/{id}/apply`, `DELETE …/{id}` | no (§12) |
| Import | `GET /api/v1/admin/import` | **yes**: the uploads waiting here (`{id, bytes, uploaded_at}`, newest first). Only finished `<uuid>.zip` uploads; a `.part` still arriving or a job's staging folder isn't one |
| Nightly backups | `GET` / `PUT` / `DELETE /api/v1/admin/backup` | **yes** (§16.4) |
| Metadata files | `GET /api/v1/admin/sidecars` | **yes** (§16.5) |

The switch for writing sidecars stays where XFER5 put it:
`PUT /api/v1/admin/libraries/{id}/write-sidecars`, from Server → Library.

### 16.2 Export

- **Options:** artwork (with §11's estimate as its size warning) and
  **password and PIN hashes**. Both off whenever the page loads.
- **The hashes warning (§6, D14).** Ticking hashes shows the warning
  in the page, in red, and turns the button into "Export with
  hashes…". Pressing it asks once more in a confirm dialog that
  repeats the warning. Only then does the page send
  `include_hashes: true`. After the export starts the box is unticked
  again, so the next export is hash-free unless asked again.
- **Bundles on this server:** the manual export jobs, newest first,
  with Download / Delete from server, a red "holds password and PIN
  hashes" chip, an "artwork" chip, progress while running and the
  error when failed. Nightly bundles are listed under Nightly backups
  instead.

### 16.3 Import

1. **Upload and check.** The admin picks a `.zip`; the page streams it
   to `POST /api/v1/admin/import` and renders the dry run that comes
   back. Uploads that are still waiting (from an earlier visit, or a
   failed apply) are listed with **Check** (re-runs the dry run) and
   **Delete**.
2. **The report** (`web/src/lib/ImportReport.svelte`) renders §12's
   `Report` as it is, with no second calculation:
   - the bundle's origin: server version, export time, format version,
     and a chip for hashes (red) or "no password hashes";
   - **"Before you apply"**, a bordered box at the top, shown whenever
     any of these hold:
     - **D38:** for each `adopt`, "This server's first admin account,
       "jordan", was never used, so "jordan" from the bundle takes it
       over", whether it then signs in with the old password
       (`from_bundle`) or keeps the one it has (`kept`);
     - **D37:** when any account's `sign_in` is `locked`, how many and
       which will have no usable password until an admin sets one
       under Server → Users (linked), and that PIN'd profiles get an
       unknown PIN;
     - user conflicts: how many accounts are skipped, with what;
     - the total number of conflicts;
   - **Accounts:** username, what happens (new / already here / takes
     over the first admin / conflict — skipped), how it signs in
     afterwards, and the report's note;
   - **Libraries:** what happens, the old host's folders and whether
     each exists here, and the folders the import adds. Each library
     that isn't a conflict has a box for **folders on this server
     instead** (one per line). **Check again with these folders**
     re-runs the dry run with them as §12's `library_paths`;
   - **Everything else:** one row per tally (profiles, library access,
     catalog items, people, credits, segments, images, artwork files,
     watch history, recent searches, Activity) with New / Updated /
     Already here / Skipped, plus the provider-id match count;
   - **Conflicts** (what, key, reason; "…and N more" past the 500
     listed) and the report's **notes**.
3. **Apply import…** asks for confirmation (it repeats the locked
   count and the conflict count), then sends the same folders to
   `…/apply`. The page follows the job; when it succeeds the upload is
   gone (§12 deletes it), the page says to scan the imported
   libraries, and the result appears under **Recent imports**,
   rendered with the same component ("What happened" instead of
   "Before you apply"). A failed apply says nothing changed and keeps
   the upload.

### 16.4 Nightly backups (D39)

**The setting lives in the web UI.** The form has the folder, how many
to keep (1–365), and "Include password and PIN hashes". Saving sends
`PUT /api/v1/admin/backup` `{"folder": …, "keep": …, "include_hashes": …}`
(strict: an unknown field is refused). The answer, and `GET`, is the
setting in force:

```
{ "source": "saved" | "environment", "folder": "/backups" | null,
  "keep": 7, "include_hashes": false,
  "folder_problem": null | "the backup folder … doesn't exist (…)",
  "updated_at": …, "updated_by": "jordan",
  "environment": { "folder": … | null, "keep": 7 } }
```

- **Stored server-side** in `backup_settings`, one row at most
  (migration `20261128000000_backup_settings.sql`).
- **The environment is the bootstrap default.** With no row,
  `PARLOUR_BACKUP_DIR` / `PARLOUR_BACKUP_KEEP` apply as in §15, never
  with hashes. Once an admin saves, the row is what counts, folder and
  all: a saved blank folder is **off even if the environment sets
  one**. **Use the environment default** (`DELETE`) forgets the row.
- **Unset folder = no nightly backup queued**, whichever way it is
  unset. Parlour never picks a folder.
- **Read fresh, no restart.** The nightly check, the export worker,
  the Download, the free-space notice (§15) and the Dashboard all read
  the setting in force when they run (`backup::effective`).
- **Checks:** the folder must be absolute, as for the environment
  variable (400 otherwise; blank means off); keep is 1–365. A folder
  that doesn't exist or can't be written **is saved** — a backup disk
  may simply not be mounted yet — and the page shows "Tonight's backup
  would fail: …" with the nightly job's own message (§15's check).
  Parlour still never creates it.
- **Hashes (D14/D25/D39).** Off by default and off for the environment
  default. Ticking the box shows the same warning as the export, and
  saving with it newly on asks for confirmation first. A nightly job
  carries hashes only if the setting had them on when the job was
  queued (its payload says so) **and** still has them on when it runs;
  turning them off in between wins. A nightly bundle with hashes is
  marked on its job ("holds password and PIN hashes") and in the
  Activity feed ("Nightly backup written, with password and PIN
  hashes").
- **Audit:** every save and reset is an Activity row,
  `export.backup_settings`, by the admin (under "Exports"), with the
  folder, keep and hash choice.
- **Recent nights:** the last seven nightly jobs, with Download (no
  Delete: retention owns them), "Removed by retention", or the error.
- **Download** reads a nightly bundle from the folder its job recorded
  on its result (the server wrote it there), not from the folder set
  now, so changing the folder doesn't orphan older nights. The name
  must still look like a nightly bundle's.
- **Server → General** no longer lists the two variables as its own
  settings; it says whether nightly backups are on, where, how many,
  whether with hashes, and links here.
- **Nothing is switched on by this task.** No folder is set on any
  host; a backup exists only once an admin saves a folder here (or
  sets the variable).

### 16.5 Metadata files beside the media

`GET /api/v1/admin/sidecars`, one row per library, from what already
exists — no second accounting and no walk of the disk:

| field | from |
|---|---|
| `supported` | whether the kind gets sidecars (`movies`, `shows` and, since AB6, `audiobooks`) |
| `write_sidecars` | XFER5's opt-in |
| `folders[]` | each root and whether it can be written now (XFER5's `access(2)` check) |
| `unwritable_notice` | whether XFER5's "can't write" notice is up |
| `items` | available movies, series, episodes and (AB6) books: what a sidecar can belong to |
| `sidecars`, `sidecars_overridden` | rows in `item_sidecars` (read by a scan or written by a match) plus, for books, `book_sidecars` (a match's server copy, AB6); and those a fix-match overrode |
| `nfos_read`, `nfos_written` | rows in `item_nfos`, `sidecar_nfos` |
| `artwork_written` | rows in `sidecar_artwork` |

The page shows Writing as "Off — Parlour only reads", "On", "On, but a
folder can't be written" (red) or "Not for this kind of library yet",
each folder as writable or not (with the reason), and links to Server →
Library for the switch.

### 16.6 What else changed

- **Server → Jobs** no longer has the "Export the server" form; it
  says exports, imports and nightly backups are started on Import /
  Export and links there. Finished export, import and backup jobs
  still appear there with Download / Delete as before, and in the
  Activity feed.
- **§8:** `backup_settings` stays behind (a server setting, not state;
  the new host's folders are its own). §6's "future settings table"
  rule applies: if any of its fields ever joins the bundle, that is a
  decision for the owner.

### 16.7 Readings made in XFER8 — for the owner's veto

None is a new decision beyond D39. Each is how this build read D14,
D25, D37, D38, D39, the TODO or these notes where they don't spell it
out:

1. **The Jobs page loses its export form.** One place owns the hashes
   warning (§11's reading 3 deferred it here); two export forms, one
   without the option, would only confuse. Its rows keep Download /
   Delete.
2. **The dry run renders the report as it is**, with D38's take-over
   and D37's no-password line in a "Before you apply" box above the
   tables, and the apply asks for confirmation repeating the locked
   and conflict counts.
3. **Waiting uploads are listed** (new `GET /api/v1/admin/import`) so a
   reload or a failed apply doesn't strand an upload that may hold
   hashes; each can be checked again or deleted.
4. **Replacement folders are typed per library** in the report and
   sent as §12's `library_paths`; "Check again" re-runs the dry run
   with them before apply.
5. **Sidecar status counts the existing ledgers** (`item_sidecars`,
   `item_nfos`, `sidecar_nfos`, `sidecar_artwork`) against available
   movies, series and episodes; the switch stays on Server → Library
   behind XFER5's confirm.
6. **Backup settings: a saved row wins whole.** A saved blank folder is
   off even if the environment sets one; "Use the environment default"
   deletes the row. The environment never turns hashes on.
7. **A folder that doesn't exist or can't be written is saved anyway**,
   with the nightly job's own warning shown on the page — an unmounted
   backup disk shouldn't block setting it up.
8. **Nightly hashes need both the queued payload and the setting at run
   time**, so turning them off always wins.
9. **A nightly Download reads from the folder its job wrote to**, not
   the folder set now (replaces §15's reading 7 detail).
10. **Backup setting changes are audited** as `export.backup_settings`
    Activity rows under "Exports".
11. **`backup_settings` is not in the bundle** (§6: non-secret settings
    need a decision before they join it).
12. **The audio proposal (§9, D36) is still open.** Nothing here
    carries audiobook progress or chapters.
