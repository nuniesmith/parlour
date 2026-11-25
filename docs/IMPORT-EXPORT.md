# Parlour — the export bundle (`parlour-export` v1)

Written 2026-11-24 for Phase 8, task XFER1. This is the format spec
for the export bundle that D14 asks for: one file that carries the
server's users, profiles, libraries, catalog and per-profile state
from one install to another. It defines the zip layout, the six JSON
documents and their fields, and the rules a valid bundle follows.

**Status.** The format exists in code (`crates/server/src/bundle/`)
with fixtures and tests, but nothing produces or applies a bundle
yet: export is XFER2, import is XFER3, the admin page is XFER8 and
nightly backups are XFER7. The per-item sidecars stored with the
media files (XFER4–XFER6) are a separate format, specified when
XFER4 is built.

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

At 2026-11-24 the schema has 26 tables, 13 of them carried:

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

  Before XFER2 ships, that amends v1 in place (§3). Podcast-episode
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

## 11. Notes for XFER2 (export)

- **What to read:** each `Value` column in `coverage.rs`. Resolve
  each `Reference` column to the names above. Read everything in one
  `REPEATABLE READ` transaction, so the references agree with each
  other.
- **Writing:** fill the types in `bundle/mod.rs`, then call
  `Manifest::new(...)`. Next, `BundleWriter::new(writer, &bundle)`
  checks the bundle and writes the documents. Then call
  `add_artwork(name, reader)` for each named file, and `finish()`.
- **The hash confirm** is an explicit per-export flag, and it is off
  by default.
- **Write in a fixed order,** so two exports of an unchanged server
  are identical:
  - users by username;
  - profiles by username, then `created_at`;
  - libraries by name;
  - items by library, then `stable_key`;
  - people by `sort_order`, then name; segments by kind, then
    source; images by kind, then variant;
  - watch state by profile, then item;
  - recent searches newest first;
  - Activity oldest first.

  Readers don't rely on order.
- **Artwork:** only cached images (a real `path` and a `hash`) can
  have a `file`. The name is `artwork_file_name(hash, extension of
  the cached path)`.

## 12. Notes for XFER3 (import) — problems known now

Two problems were flagged before this spec and belong to XFER3. They
are noted here, not solved:

1. **A user imported without a hash can't sign in.**
   `users.password_hash` is NOT NULL, and there is no admin password
   reset today. Importing a bundle made without the confirm leaves
   every imported login without a usable password.
2. **A fresh install's first admin clashes with the bundle.**
   `PARLOUR_ADMIN_USER` creates a user (plus a default profile) on
   first start. If the bundle holds the same username, XFER3's rule
   that "conflicting usernames are reported, not merged" turns away
   the owner's own account.

Other things XFER3 has to handle:

- **Creating users:** don't go through `create_user`, which also adds
  a default profile named after the user. The bundle already carries
  every profile, the default one included.
- **Library paths** are the old host's. The dry run should show them,
  and the import should let them change.
- **Items match** on (`library`, `stable_key`), falling back to
  provider ids (TODO). Keys only match when the folders under each
  library root are laid out the same way: movie keys include the
  folder, audiobook keys the relative path.
- **Segment times are positions in a file,** so they hold only if
  the new host's files are the same encodes.
- **Activity entries have no key.** A re-import must not duplicate
  them, so treat an entry as present when every field matches.
- **Imported `needs_review` items** come with no candidates (§8).
- **Artwork:** extract only the names `BundleReader` lists, and copy
  through `copy_artwork`, which checks each file's hash.
- **Uploads:** put an upper bound on their size. The reader parses
  each document fully in memory.
- **Insert order:**
  - users;
  - then profiles;
  - then libraries, paths and access;
  - then items, parents first;
  - then people, segments and images;
  - then watch state, recent searches and Activity.

  Never delete anything by default (TODO).

## 13. In the code

- **`crates/server/src/bundle/`:**
  - `mod.rs` — the types;
  - `archive.rs` — `BundleWriter` and `BundleReader`;
  - `check.rs` — `Bundle::problems()`;
  - `coverage.rs` — the schema map and the CHECK constraints mirrored.
- **Fixtures:** `crates/server/tests/fixtures/bundle-v1/`.
- **Unit tests** in `bundle/tests.rs` (run in CI):
  - each fixture round-trips and is byte for byte what the writer
    writes;
  - whole bundles go through the zip and back, with and without
    hashes, with artwork;
  - every refusal the reader and writer make;
  - the value sets match the server's.
- **Flow test** `tests/bundle_schema_flow.rs`: ignored, needs a
  throwaway Postgres, run locally.
