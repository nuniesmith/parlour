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
- **Now Playing (UI3)**: session cards with §1.4's anatomy
  fed by the existing registry, extended server-side to
  carry the playback plan (container/codecs → mode +
  reason per stream) and client/device/IP fields; controls:
  pause (client-agnostic: revoke stream + mark session
  stopped is the honest v1 — real client pause needs client
  support, note it), stop session, and message-to-screen
  later if clients grow it. Poll ~5 s; empty state is quiet.
- **Activity (UI4)**: an `activity` table written at login,
  playback start/stop, scan/import completion, invite/user
  admin actions; Dashboard feed + a full page under Server →
  Logs & Activity. Retention job (90 days).
- **Admin IA (UI6)**: tabs become sidebar pages: Dashboard,
  General (server settings), Users (users + invites +
  profile access), Library (libraries, paths, access,
  scans), Metadata (review queue, manual match), Sessions
  (UI3), Jobs (was Scheduled Tasks: list, run-now where a
  kind supports it, errors), Import/Export (Phase 8's UI8
  lands here), Emby Import (under Library). URLs stay
  `/admin/...` deep-linkable.
- **Preferences IA (UI7)**: Settings splits into Display,
  Home Screen (rail toggles/order), Playback (autoplay-next
  + skip behaviour — Phase 9 SEG5/SEG6 read these), 
  Subtitles, Profile, Account (password — PR #10 — and
  username, UI10).
- **Home (UI8)**: add library tile row + per-library
  "Latest" rails; keep existing rails and labels.
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
