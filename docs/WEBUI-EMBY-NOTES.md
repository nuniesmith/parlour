# Parlour Web UI — Emby reference notes & target spec

Written 2026-11-11 from the owner's screenshots of his Emby
server (sullivan, Emby 4.10.2.0, Dashboard page) and an
inventory of Parlour's current web UI. Owner's direction:
**set up Parlour's web UI the way Emby's is set up** — same
information architecture and page anatomy, original Parlour
implementation. This doc is the spec for TODO Phase 7 (UI1–UI11).

Clean-room rule, same as the server: IA, layout patterns and
feature *ideas* are fine to adopt; Emby code, assets, icons,
wording and branding are not. Parlour keeps its own name,
cobalt armchair icon, and design tokens.

---

## 1. What the screenshots show (Emby Dashboard)

### 1.1 Global layout — three regions
- **Left sidebar** (dark, ~250 px): product logo top-left,
  a **search box** under it, then the navigation tree in
  collapsible sections (§1.2). The sidebar is present on
  admin pages; the active item is a filled blue pill.
- **Top bar**: back arrow + page title ("Dashboard") on the
  left; utility icons right (cast, user/account, settings).
- **Content**: two columns — the main column (server card,
  Now Playing, Activity) and a **right rail** (Alerts).
- Everything is dense, list-and-card based, dark theme,
  small type, blue accent (#00a4dc-ish). No hero art on
  admin pages; posters appear only as small thumbnails.

### 1.2 Sidebar tree (the IA to mirror)
- **Home**
- **Emby Web Settings** (client settings for the web app
  itself): General · Playback · Keyboard and Remote
- **<user> Preferences**: Display · Home Screen · Playback ·
  Subtitles · Profile · Notifications
- **Emby Server** (admin): Dashboard · General · Users ·
  Emby Premiere · Library · Live TV · Network · Transcoding ·
  Database · Conversions · Scheduled Tasks · Logs ·
  Cinema Intros
- **Devices**: Devices · Downloads · Camera Upload · DLNA
- **Advanced**: Plugins · Api Keys · Metadata Manager ·
  Backup & Restore · Disk Space · Open Subtitles

Takeaways: (a) *personal* settings and *server* settings are
separate sidebar sections, not one settings page; (b) server
admin is a tree of single-purpose pages, not one tabbed page;
(c) user preferences are grouped by topic (Display / Home
Screen / Playback / Subtitles), each its own page.

### 1.3 Dashboard page anatomy
- **Server header**: server name ("sullivan"), overflow menu
  (…), a restart/power icon, a plan badge. Below it, a
  **notice banner** when the server needs attention:
  "Please restart the server to finish applying updates."
- **Server info card**: version (linked), a green-check
  status line ("Emby Server is up to date"), "Running on
  http port 8096", **Home (LAN) access: <url>** and
  **Remote (WAN) access: <url>** as links, and a
  "Connection Help" button.
- **Alerts (right rail)**: reverse-chronological notices with
  an icon and timestamp — "Backup Completed on sullivan",
  "Xml TV 1.2.6.0 Installed on sullivan", plugin installs.
  This is the server's *ambient awareness* surface.
- **Now Playing**: one card per active session (detail §1.4).
- **Activity**: a feed below Now Playing (logins, playback,
  admin events) — the audit trail version of Alerts.

### 1.4 Now Playing session card (the important one)
For a session playing *Castle Rock* S2:E8 "Dirty":
- Poster thumbnail, series title, `S2:E8 - Dirty`,
  **position / duration** (`20:18 / 35:36`) and a progress
  bar (elapsed vs buffered portions shown differently).
- Client line: app + version ("AndroidTv 2.5.03a"), device
  name ("Kayla's 2nd Fire TV"), **source IP + protocol**
  ("72.143.217.176 http/1.1").
- **Stream decision block**, three rows, each showing
  *source → what the server is doing*:
  - Stream: `MKV (3 mbps) → HLS (3 mbps)`,
    "Converting to compatible video profile"
  - Video: `1080p HEVC → Transcode (HEVC 3 mbps)`
  - Audio: `English EAC3 5.1 (Default) → Direct Play`
- Username under the block; transport controls at the
  bottom (pause, stop, subtitles/message).

This card is exactly Parlour's "explained decisions"
philosophy, already built server-side in 1.6 (playback
plans carry a reason per decision; the session registry
exists behind `GET /api/v1/admin/sessions`). Parlour's card
should show the same anatomy with *its* reasons
(DirectPlay/Direct, Remux/ContainerUnsupported, …).

---

## 2. Parlour's current web UI (inventory, 2026-11-11)

- **Shell**: a *top bar*, not a sidebar — brand, Home,
  one link per library, Search, Admin (admins only),
  profile chip, Switch (profiles). Responsive by wrapping.
- **Pages**: `/` home rails (Continue Watching, Next Up,
  Recently Added) · `/library/[id]` grid + filters ·
  `/item/[id]` detail (cast, files/streams/subtitles
  inspector, similar, mark played/favourite) · `/watch/[id]`
  player (resume prompt, HLS remux, subtitle tracks,
  keyboard shortcuts, plan + warnings shown) · `/search`
  (FTS + people) · `/profiles` who's-watching + management ·
  `/settings` one page (profile prefs, sessions list/revoke,
  and — PR #10 — change password) · `/admin` **one page with
  tabs** (Users & invites, Libraries & access, Jobs,
  Metadata review, Emby import) · `/invite/[token]`,
  `/login`.
- **What Parlour has that the screenshots' IA needs**:
  sessions endpoint + registry (P8), job list, playback
  plans with reasons, user_data/play counts. Most of the
  *data* for an Emby-style dashboard already exists; it's
  the *presentation and IA* that differ.
- **Gaps vs §1**: no sidebar; no Dashboard page (server
  info/URLs/version/notices); Now Playing exists only as an
  admin sessions list, without the card anatomy, stream
  block, or controls; no Activity feed (nothing records
  logins/admin actions as events); no Alerts/notices model;
  admin is tabs on one page instead of a section tree;
  personal settings are one page instead of topical pages;
  no username change; home has no per-library "Latest" rails
  or library tile row.

## 3. Target spec (maps to TODO Phase 7)

- **Shell (UI1)**: left sidebar ≥1024 px wide screens,
  collapsible sections mirroring §1.2's grouping (Web
  settings / <profile> Preferences / Server for admins),
  top bar collapses to brand + menu + profile under that;
  a drawer on small screens. Keep Parlour tokens; the
  current top-bar library links become a "Libraries"
  sidebar section. Focus states and text sizes must stay
  TV-browser usable (this shell is also the Ember TV
  browser experience until the native TV app).
  *Shipped (UI1):* `web/src/lib/Sidebar.svelte` + the root
  layout. Section collapse state is per-browser
  (`localStorage`). Until UI6/UI7 split the pages, the
  Server entries deep-link to `/admin?tab=…` and the
  Preferences entries to `/settings#…`; the sidebar search
  box submits to `/search?q=…`. Web Settings holds one
  page so far, `/web-settings` (keyboard shortcut
  reference) — what else belongs there is the owner's call.
- **Dashboard (UI2 + UI5)**: `/admin` landing becomes the
  Dashboard: server name + version (from `/api/v1/version`),
  listener port, the three URLs Parlour is reached by
  (LAN / tailnet / public, from server config — display +
  copy button, never guess), DB status, job-queue snapshot,
  and the notices banner + Alerts rail (UI5: a `notices`
  feed sourced from failed jobs, metadata-review backlog,
  disk-space checks, and "restart to apply" flags after a
  config change).
  *Shipped (UI2):* `GET /api/v1/admin/server` (admin-only)
  feeds `web/src/lib/ServerDashboard.svelte`, the default
  view of `/admin`. Name and URLs come from
  `PARLOUR_SERVER_NAME` / `PARLOUR_URL_LAN` /
  `PARLOUR_URL_TAILNET` / `PARLOUR_URL_PUBLIC` (see
  `.env.example`); an unset URL shows "Not set". Restart /
  shut down are **not** offered: the compose file sets no
  restart policy, so the server cannot honestly promise to
  come back — the payload's `actions` flags are both false
  and the page points at the host instead. Notices banner +
  Alerts rail are UI5.
  *Shipped (UI5):* `notices` + `notice_dismissals` tables
  (migration `20261112010000_notices.sql`).
  `crates/server/src/notices.rs` **reconciles** once a
  minute rather than being pushed to: failed jobs from the
  last 7 days, the metadata-review backlog (one rolling
  notice), and free space for the cache and artwork folders
  (D16) each raise a notice; a condition that clears
  resolves its notice. `GET /api/v1/admin/notices` lists
  what this admin has not dismissed;
  `POST /api/v1/admin/notices/{id}/dismiss` hides one for
  that admin only, until it resolves or its severity
  changes. The Dashboard shows errors and a pending restart
  in a banner and everything in the Alerts list — a right
  rail from 1280 px, under the server card below that.
- **Now Playing (UI3)**: session cards with §1.4's anatomy
  fed by the existing registry, extended server-side to
  carry the playback plan (container/codecs → mode +
  reason per stream) and client/device/IP fields; controls:
  pause (client-agnostic: revoke stream + mark session
  stopped is the honest v1 — real client pause needs client
  support, note it), stop session, and message-to-screen
  later if clients grow it. Poll ~5 s; empty state is quiet.
  *Shipped (UI3):* `GET /api/v1/admin/sessions` now carries,
  per session, the plan playback-info actually handed out
  (action + reason, delivery, and source → mode for the
  video and audio stream), the client's self-description +
  User-Agent, the socket peer IP / protocol (plus the
  proxy's unverified `X-Forwarded-For` first hop, shown
  separately), position/duration, and series/season/episode
  for the S:E line. `POST /api/v1/admin/sessions/{id}/stop`
  revokes the stream tokens issued to that profile for that
  file and marks the session stopped; the player's next
  heartbeat gets 409 and says why. There is no remote pause
  — the card has one Stop control and says so. Cards live
  in `web/src/lib/NowPlaying.svelte` on the Dashboard
  (UI6 gives Sessions its own page). Session state is
  in-memory, as in P8: a server restart forgets live
  sessions and revocations.
- **Activity (UI4)**: an `activity` table written at login,
  playback start/stop, scan/import completion, invite/user
  admin actions; Dashboard feed + a full page under Server →
  Logs & Activity. Retention job (90 days).
  *Shipped (UI4):* `activity` table (migration
  `20261112000000_activity.sql`) written by
  `crates/server/src/activity.rs::record` — a failed write is
  logged and never fails the request or job that caused it.
  Kinds: `auth.login`, `auth.password_changed`,
  `playback.start`, `playback.stop`,
  `playback.stopped_by_admin`, `scan.completed`,
  `scan.failed`, `import.completed`, `import.failed`,
  `invite.created`, `invite.revoked`, `user.joined`,
  `user.renamed` (UI10). Failed
  sign-ins are not recorded (the login limiter covers them).
  `GET /api/v1/admin/activity` pages newest-first
  (`limit`, `before`, `category`). The Dashboard shows the
  latest few; the full page is Server → Activity
  (`/admin?tab=activity`). Retention is a nightly
  `purge_activity` job, visible in Jobs with a deleted count.
- **Admin IA (UI6)**: tabs become sidebar pages: Dashboard,
  General (server settings), Users (users + invites +
  profile access), Library (libraries, paths, access,
  scans), Metadata (review queue, manual match), Sessions
  (UI3), Jobs (was Scheduled Tasks: list, run-now where a
  kind supports it, errors), Import/Export (Phase 8's UI8
  lands here), Emby Import (under Library). URLs stay
  `/admin/...` deep-linkable.
  *Shipped (UI6):* one route per page under
  `web/src/routes/admin/` — `/admin` (Dashboard),
  `/admin/general`, `/admin/users`, `/admin/library`,
  `/admin/library/emby-import`, `/admin/metadata`,
  `/admin/sessions`, `/admin/jobs`, `/admin/activity`,
  `/admin/import-export` — sharing `admin/+layout.svelte`
  (table/form styles, and a "Page not found" for non-admins;
  the API remains the real gate). Old `/admin?tab=…` links
  redirect. General is read-only: it shows each setting with
  the environment variable that sets it. Library access
  grants moved to Users. Jobs gained per-job error and result
  detail and "run now" for the two kinds that can be started
  by hand (per-library scan and metadata match). Import /
  Export is an honest placeholder until Phase 8.
- **Preferences IA (UI7)**: Settings splits into Display,
  Home Screen (rail toggles/order), Playback (autoplay-next
  + skip behaviour — Phase 9 SEG5/SEG6 read these), 
  Subtitles, Profile, Account (password — PR #10 — and
  username, UI10).
  *Shipped (UI7):* one route per page under
  `web/src/routes/settings/` — `/settings/display`,
  `/settings/home`, `/settings/playback`,
  `/settings/subtitles`, `/settings/profile`,
  `/settings/account` — sharing `settings/+layout.svelte`
  (form styles; every page but Account asks for a profile
  first). Bare `/settings` opens Display; the old anchors
  redirect (`#playback` → Playback, `#password` /
  `#sessions` → Account). Everything except Account is
  stored **on the profile** (so it follows the profile to
  every device and rides the D14 export), edited through
  `PATCH /api/v1/profiles/{id}` and returned on every
  profile DTO (including `/auth/me`'s `active_profile`).
  Migration `20261115000000_profile_preferences.sql` adds:
  `home_rail_order` + `home_rails_hidden` (the DTO merges
  them into `home_rails: [{id, visible}]`, always every
  known rail — rails the profile has never ordered go last,
  visible, so UI8's new rails appear without a migration;
  unknown or repeated ids in a PATCH are a 400; the known
  ids live in `profiles::HOME_RAILS`), `segment_skip`
  (`button` | `auto` | `off`, default `button` — Phase 9's
  "Skip Intro with no manual work" needs the button on by
  default) and `autoplay_next` (default on, D15). Home
  renders its rails in the profile's order and skips hidden
  ones. Playback holds audio language, the remote bitrate
  cap, and the two Phase 9 choices. Since SEG5 the player
  acts on the skip choice (playback-info carries it as
  `segment_skip`; the fieldset now says intros, recaps and
  credits, per D22), and since SEG6 on autoplay-next too
  (playback-info carries it as `autoplay_next`, with the
  `next_episode`; the hint now describes the countdown card,
  per D23). Since SEG7 a second hint says the card asks
  "Still watching?" after three autoplays in a row with
  nobody touching the player (D24); that prompt has no
  setting of its own.
  Subtitles holds mode + language. Profile edits the active
  profile's name, colour and PIN (adding/deleting profiles
  stays on Who's watching). Account holds change password,
  signed-in sessions, switch profile and sign out (and,
  since UI10, change username). Display
  has no options yet — Parlour has no per-profile display
  settings; the page exists so the tree matches §1, and
  what belongs there is the owner's call. PATCH also gained
  `clear_audio_lang` / `clear_subtitle_lang` (like
  `clear_bitrate_cap`): before, blanking a language in the
  form silently kept the old one.
- **Account self-service (UI10)**: change-username endpoint
  (case-insensitive unique, current password required,
  existing sessions survive) + Account page UI; admins can
  rename users too.
  *Shipped (UI10):* no migration — `users_username_lower_idx`
  (unique on `lower(username)`, since Milestone 1.2) already
  enforces case-insensitive uniqueness. One helper,
  `auth::rename_user`, backs two routes:
  `POST /api/v1/auth/change-username`
  `{current_password, new_username}` → 200 with the user
  (`{id, username, is_admin}`), and admin-only
  `PATCH /api/v1/admin/users/{id}` `{username}` → 200 with the
  admin user row (404 for unknown ids and for non-admins, per
  A7). Rules, both routes: surrounding whitespace is trimmed;
  the name must pass sign-up validation (3–32 of letters,
  digits, `_ . -`) → else 400; a name another account holds
  in any case → 409; the exact current name → 400; changing
  only the case of your own name (`jordan` → `Jordan`) is
  allowed. Self-service checks the password **first** (401
  before any 400/409), so the route can't probe which names
  exist; the admin route needs no password, like every other
  admin action. Nothing else changes: sessions are keyed by
  user id and read the name per request, so every session
  stays signed in and sees the new name (no revocation —
  unlike a password change, a rename is not a credential
  reset); the password is untouched; **profiles keep their
  names** (the default profile was named after the username
  at sign-up and is not renamed with it). Each rename writes
  a `user.renamed` activity row (old + new name in detail;
  the actor is the name at the time). UI: Preferences →
  Account gains a Change username form above Change password
  (new name + current password; the shell's name refreshes
  on success); Server → Users gains a per-row Rename (inline
  field, Save / Cancel). Integration test:
  `crates/server/tests/username_flow.rs`.
- **Home (UI8)**: add library tile row + per-library
  "Latest" rails; keep existing rails and labels.
  *Shipped (UI8):* `GET /api/v1/home` gained `libraries`
  (one tile per library the active profile can see, by name
  as in the sidebar: id, name, kind, top-level item count,
  and `art_item_id` — the newest top-level item with a cached
  backdrop, or null for a plain tile) and `latest` (one rail
  per non-empty visible library, same order, up to 16
  top-level items each). Latest orders by *most recent
  arrival*: a series counts as new when any of its seasons or
  episodes was added, so a returning show surfaces without a
  new series row; Recently Added keeps its old ordering.
  Played items are not filtered out of Latest. Two rail ids
  join `profiles::HOME_RAILS`: `libraries` (default first)
  and `latest` (default last) — the per-library rails are
  one id, shown or hidden and moved as a group, so the id set
  doesn't depend on which libraries exist. Per UI7, a profile
  that already saved an order gets both at the end, visible,
  until it moves them in Preferences → Home Screen. The tile
  row (`web/src/lib/LibraryTiles.svelte`) replaces Home's old
  "Browse <library> →" links; each Latest rail's heading
  links to its library.
- **Search (UI9)**: `/` focuses search from anywhere; recent
  searches per profile.
  *Shipped (UI9):* the root layout answers `/` on every shell
  page (not on login / Who's watching / invite): on `/search`
  it focuses that page's field, otherwise the sidebar search
  box, opening the drawer first under 1024 px. It does nothing
  while focus is in a field (so `/` still types), with
  Ctrl / Cmd / Alt held, or mid-IME composition, and it
  suppresses the browser's own `/` quick-find. Web Settings →
  Keyboard lists it. Recent searches are stored **on the
  server per profile** like the UI7 preferences (table
  `recent_searches`, migration
  `20261115010000_recent_searches.sql`; deleted with the
  profile): `GET /api/v1/search/recent` (newest first),
  `POST {query}` (returns the updated list), `DELETE` (clear
  all) and `DELETE /api/v1/search/recent/{id}`, all scoped to
  the active profile — another profile's entry is a 404. A
  search is recorded only when committed — Enter in the
  sidebar box or on the Search page, or opening a result — not
  on each debounced keystroke. Queries are trimmed, 2–200
  characters, de-duplicated case-insensitively (searching
  again moves it to the top with the latest spelling), and
  only the newest 10 are kept. The Search page lists them
  while its field is empty (click to rerun, × to forget one,
  Clear all); the sidebar box offers them as native
  suggestions (`<datalist>`). Whether the D14 bundle carries
  them is XFER1's call; they are profile state, so the
  natural home is next to the profile's preferences.
  *D25 (2026-11-24): they are in the bundle, next to the
  profile's preferences — `profiles.json` → `recent_searches`,
  newest first (`docs/IMPORT-EXPORT.md` §5.3).*
- **Visual QA (UI11)**: screenshot checklist against §1 at
  desktop + phone widths; fix spacing/anatomy drift; empty
  states everywhere.
  *Shipped (UI11):* checked in headless Chromium at
  1440×900 and 390×844 on a private server with its own
  throwaway database (an empty server, then two libraries
  and two live sessions), every shell page, with three
  automatic checks per page: nothing blank, no sideways
  overflow, and the gap above the first content. The
  checklist and what it found:
  - *Shell (§1.1–1.2)*: sidebar sections, active pill,
    search box, drawer under 1024 px — as specified. Pages
    started 0–26 px below the top bar depending on the
    page; the root layout now gives the first block no top
    margin, so every page starts at the same place.
  - *Dashboard (§1.3)*: the server card's label / value
    grid had collapsed into run-on lines since UI6 — the
    admin layout styles `.row` globally for form rows and
    that won over the card's own `.row`. Renamed (the
    Alerts list's `.actions` had the same clash). Recent
    jobs show readable kinds ("Library scan", not
    `scan_library`), and every admin timestamp — Dashboard,
    Jobs, Activity, Alerts, Account sessions — uses one
    format (`formatDateTime` in `lib/types.ts`), which also
    stopped the phone job list wrapping its times.
  - *Session card (§1.4)*: anatomy as specified. The Stream
    row printed ffprobe's format family (`MATROSKA`, `MOV`);
    it now says `MKV`, and `MP4/MOV` for the `mov` family,
    which covers both (the server stores only the family).
  - *Rails*: an episode in Continue Watching or Next Up
    showed only its own title ("Episode 2") on a letter
    tile. `GET /api/v1/home` rows in those two rails now
    carry `series_title` and `poster_item_id` (the same rule
    as the Now Playing card: the series' poster when it has
    one, else the item's own, else null), and the tile reads
    show name, then `S1:E2 · Episode 2`, on the show's
    poster. Other rows are unchanged.
  - *Empty states*: Home said nothing at all when every rail
    with content was hidden — it now says so and links to
    Preferences → Home Screen; an admin on an empty server
    gets an Add a library button. Server → Library and
    Users no longer show bare table headers for no
    libraries / no invites, and Library access says to add a
    library first when there is none.
- **Explicit non-goals** (in Emby's tree, not wanted):
  Emby Premiere/paywall surfaces, Live TV, DLNA, Camera
  Upload, Cinema Intros, plugin store. If the owner ever
  wants one, it's a new decision, not part of Phase 7.

## 4. Sequencing note

Phase 7 is presentation + a little server support (notices,
activity, richer session payloads). It does not depend on
Phase 2 transcoding — the Now Playing card renders Phase 1
plans today and grows transcode rows when T-tasks land.
Owner priority (2026-11-11): this is the next web track;
Fable runs it from TODO Phase 7 with this doc as spec.
