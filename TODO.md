# Parlour — TODO / working plan

The living checklist. `docs/ROADMAP.md` says *what order* things
happen; this file says *exactly what to build*, part by part.
Check boxes off in the same PR that does the work. Sizes are
T-shirt estimates (S = hours, M = a day or two, L = most of a week)
for one developer with AI assistance, not promises.

**Current position:** Phase 0 (skeleton) ✅ done 2026-11-10.
Everything below Phase 1 is unchecked.

---

## Decisions (D1–D7 — all decided 2026-11-10)

- [x] **D1 — Ports. DECIDED 2026-11-10:** Parlour uses its own
  ports, not Emby/Jellyfin's — **8585 HTTP** (the one listener;
  web UI + API + streams) and **8586 HTTPS** (reserved for a
  direct-TLS listener if the reverse proxy doesn't terminate it).
  Nothing else on sullivan uses them (checked against its service
  list: 7878/8080/8686/8989/9696/32400/8096).
- [x] **D2 — Public TLS. DECIDED:** a reverse proxy on sullivan
  (Caddy) terminates HTTPS and forwards to Parlour on
  localhost:8585. Cert renewal stays out of the Rust code; the
  8586 direct-TLS reservation stays unused unless this changes.
- [x] **D3 — Names. DECIDED:** `parlour.7gram.xyz` is the
  tailnet/LAN name (7gram names only resolve/work on the
  tailnet). The public route stays IP/DDNS-based straight to
  the sullivan proxy, exactly how the truck reaches Emby today.
- [x] **D4 — Host. DECIDED:** sullivan is the production host —
  the library, Emby, and the *arr stack already live there.
- [x] **D5 — TMDB API key. DECIDED (procedure):** Jordan creates
  a free TMDB account key when Milestone 1.4 starts and puts it
  in the server `.env` — never in the repo. Nothing needed
  before then.
- [x] **D6 — Versions. DECIDED:** bump the declared versions
  before Phase 1 code lands (research note 2026-11-10: axum is
  at 0.8.9, sqlx has a 0.9 line out, and the workspace's sqlx
  0.8 declaration is still unused by any code). Done as task
  F2 in Milestone 1.1.
- [x] **D7 — Sessions. DECIDED:** hand-rolled DB-backed sessions
  (cookie = random token, only its hash stored in the DB). The
  invite flow is custom anyway; one less dependency to trust.

---

## The parts, and when each gets worked on

| Part (crate / folder) | Job | Phase |
|---|---|---|
| `crates/server` | Binary: config, startup, HTTP API, embeds web UI | 1 |
| `crates/core` | Shared types: ids, client profiles, playback plans | 1 (grows) |
| `crates/db` | sqlx queries + migration runner | 1 |
| `crates/catalog` | Scanner, ffprobe ingest, matching, Emby import | 1 |
| `crates/metadata` | TMDB first, TVDB second | 1 |
| `crates/playback` | Decision engine, sessions, streaming, watch state | 1–2 |
| `crates/subtitles` | Index, extract, WebVTT, forced logic, burn-in | 1–2 |
| `crates/jobs` | Postgres job queue + workers | 1 |
| `crates/transcode` | FFmpeg plans, NVENC/QSV/VAAPI/software backends | 2 |
| `crates/integrations` | *arr, Prowlarr, qBittorrent, requests | 4 |
| `crates/discord` | Optional in-binary Discord bot | 4 |
| `web/` | SvelteKit UI — the first client | 1 |
| `clients/android` | Kotlin/Media3 phone + Fire TV | 3 |
| `clients/ios` | Swift/AVPlayer | 6 |
| `migrations/` | Schema, timestamped + append-only | 1 (grows) |

---

## Phase 1 — Direct-play MVP

Goal: **replace Emby at home for movies + TV** — log in, browse,
press play, resume where you left off, subtitles on.

### Milestone 1.1 — Foundations (`server`, `db`, `core`)
- [ ] F1 (M) Config loading: env + `.env`, typed `Config` struct,
  fail-fast with a clear message on bad config
- [ ] F2 (S) Bump workspace deps to current (axum 0.8.9, sqlx 0.9 —
  see D6); remember axum 0.8 path syntax is `/{id}`, not `/:id`
- [ ] F3 (M) Tracing/logging setup (`tracing` + env filter),
  request-id per request, no secrets in logs — ever
- [ ] F4 (M) DB pool in `db` crate + `sqlx::migrate!()` at startup;
  `/healthz` endpoint that actually checks the DB (a green check
  that lies is worse than none)
- [ ] F5 (M) First real migration: `users`, `profiles`, `sessions`,
  `libraries`, `library_access` tables
- [ ] F6 (S) Error type → HTTP mapping (JSON errors, no stack
  traces to clients); unknown routes return JSON 404, not HTML
- [ ] F7 (M) Embed the built web UI in the binary (rust-embed) with
  SPA fallback; dev mode serves from disk instead
- [ ] F8 (S) `docker compose up` runs the whole thing locally,
  healthcheck green, empty home page loads
- **Done when:** a fresh clone + `.env` boots, migrates, and serves
  the placeholder UI with a real DB behind `/healthz`.

### Milestone 1.2 — Users, profiles, invites (`server`, `db`)
- [ ] A1 (M) Password hashing with `argon2`; login endpoint with
  rate limiting (per-IP + per-account backoff)
- [ ] A2 (M) DB-backed sessions per D7: HttpOnly + SameSite cookie,
  token hash at rest, logout/revoke, session list per user
- [ ] A3 (S) First-admin bootstrap: `PARLOUR_ADMIN_USER/PASSWORD`
  env used once if no users exist, then ignored
- [ ] A4 (M) Invite-only sign-up: admin creates an invite link,
  invitee sets name + password; no open registration route exists
- [ ] A5 (M) Profiles under a user: CRUD, optional PIN, avatar
  colour/initial; per-profile defaults (audio lang, subtitle
  lang/mode, remote bitrate cap)
- [ ] A6 (M) Library access + rating limits enforced in SQL on
  every item query (someone else's library = 404, never 403)
- [ ] A7 (S) Admin-only route guard; admin surfaces never render
  for normal users
- **Done when:** Jordan + Kayla + one invited friend can each log
  in with separate profiles, and the friend cannot see a private
  library even by guessing URLs.

### Milestone 1.3 — Catalog + scanner (`catalog`, `jobs`, `db`)
- [ ] C1 (M) Second migration: `media_items`, `media_files`,
  `media_streams`, `subtitle_tracks`, `people`, `item_people`,
  `images` (paths/hashes only — image bytes stay on disk)
- [ ] C2 (M) Library CRUD API (admin): name, kind, root paths
- [ ] C3 (L) Folder walker: *arr/TRaSH naming conventions, video
  extensions allowlist, skip samples/extras by rule; fingerprint
  (path, size, mtime) skip for unchanged files
- [ ] C4 (M) ffprobe runner: parse container, duration, codecs,
  resolution/HDR flags, audio + subtitle streams into
  `media_streams` / `subtitle_tracks`
- [ ] C5 (M) TV structure: series → seasons → episodes from
  `SxxExx` naming; movies standalone; multi-file items supported
- [ ] C6 (M) Scan as a **job** (`jobs` crate): manual trigger,
  per-library, progress + errors visible in admin; idempotent
  re-runs (stable content ids)
- [ ] C7 (M) Filesystem watcher (`notify`) → debounce → targeted
  rescan job; plus a nightly full-scan job as the safety net
- [ ] C8 (S) Missing-file handling: vanished files mark the item
  unavailable, never delete watch state
- **Done when:** pointing Parlour at the real movies + TV folders
  produces a correct, browsable catalog, and re-scanning changes
  nothing (idempotent).

### Milestone 1.4 — Metadata (`metadata`, `catalog`)
- [ ] MD1 (M) TMDB client (API key from env/DB settings): search,
  movie + TV details, artwork URLs, provider ids stored on items
- [ ] MD2 (M) Matcher: TMDB/TVDB id in folder/file name wins;
  else normalised title + year; low-confidence matches queue for
  manual fix, never silently guess
- [ ] MD3 (S) Artwork download job: poster/backdrop/logo cached
  locally at two sizes; items without art get a generated tile
- [ ] MD4 (S) Manual fix-match UI/API: search TMDB, pick the right
  one, re-fetch that item only
- [ ] MD5 (S) TheTVDB client for episode ordering/metadata where
  TMDB is thin (second provider behind the same trait)
- **Done when:** ≥95% of the real library matches correctly with
  no hand-holding, and every miss is fixable from the UI.

### Milestone 1.5 — Web UI (`web/`)
- [ ] W1 (M) API client + auth screens: login, profile picker
  (+PIN), invite acceptance page
- [ ] W2 (M) Home: Continue Watching row, Recently Added,
  libraries row (data from 1.6 endpoints; shell can land first)
- [ ] W3 (M) Library browse: poster grid, sort/filter (year,
  genre, unwatched), alphabet jump for big libraries
- [ ] W4 (M) Item pages: movie detail; series → season → episode
  lists; cast row; "more like this" can wait
- [ ] W5 (L) Player page: native `<video>` + HLS fallback, subtitle
  track picker (shows forced/default/SDH), audio track picker,
  resume prompt, keyboard shortcuts, progress reporting (1.6)
- [ ] W6 (M) Search: Postgres FTS over titles/people to start;
  search-as-you-type with debounce
- [ ] W7 (M) Settings: my profile/preferences; admin section —
  users + invites, libraries + scan buttons + job status
- [ ] W8 (S) Empty/error states everywhere (no library yet, scan
  running, server unreachable) — a blank page is a bug
- **Done when:** Kayla can use it on the TV-browser/laptop without
  asking how anything works.

### Milestone 1.6 — Playback: direct play + remux (`playback`, `subtitles`)
- [ ] P1 (M) Playback-info endpoint: given item + client profile +
  network kind, return the plan from `core`'s decision types
  (Phase 1 only ever answers DirectPlay or Remux, with reasons)
- [ ] P2 (M) File streaming with HTTP Range (206, seeking works),
  correct content types, stream tokens (short-lived, signed) so
  media URLs aren't open to the world
- [ ] P3 (M) Remux path: `ffmpeg -c copy` to HLS/fMP4 for
  containers the web player rejects (MKV → fMP4), segments cleaned
  up with the session
- [ ] P4 (M) Playback reporting, same shape the big servers use:
  start / progress heartbeat (~10 s) / stopped → `watch_state`
  (position, played at ≥90%, play count, last played)
- [ ] P5 (S) Mark played/unplayed + favourite endpoints + UI
- [ ] P6 (M) Subtitles v1: sidecar + embedded **text** tracks
  extracted to WebVTT and served as tracks; forced-track
  auto-pick per profile language rules
- [ ] P7 (S) Continue Watching + Next Up queries (resume items,
  next episode after a finished one)
- [ ] P8 (S) Concurrent-session registry (who is watching what —
  admin view; needed later for transcode caps)
- **Done when:** a full movie and a full episode play start to
  finish in the browser with subtitles, survive a seek, and resume
  at the right second on a second device.

### Milestone 1.7 — Emby cutover (`catalog`)
Research baked in (2026-11-10): Emby serves its own OpenAPI at
`/emby/openapi.json`; auth is an API key via `X-Emby-Token`; user
state lives on items' `UserData` (`Played`,
`PlaybackPositionTicks`, `PlayCount`, `LastPlayedDate`,
`IsFavorite`); **match by `ProviderIds` (TMDB/TVDB), never by
title**; note Emby keys watch state by provider id, so duplicates
of one film share state — import by title, not by file.
- [ ] E1 (M) Read-only Emby client in `catalog`: list users,
  page items with `ProviderIds` + `UserData`
- [ ] E2 (M) Importer job: map Emby user → Parlour profile (manual
  mapping screen), apply played/position/favourites by provider
  id; dry-run mode prints the counts first
- [ ] E3 (S) Import report: matched / unmatched / ambiguous counts;
  unmatched items listed so nothing is silently lost
- [ ] E4 (S) Cutover checklist run: Parlour on 8585 beside Emby for
  a week of real use → fix the top pains → D1 port swap
- **Done when:** Continue Watching looks the same in Parlour as it
  did in Emby, for both Jordan and Kayla.

### Phase 1 definition of done (the whole thing)
A week of real household use with Emby off: movies + TV, two
users + profiles, subtitles on, resume working, zero "we had to
go back to Emby for X" moments.

---

## Phase 2 — Transcoding (`transcode`, `playback`, `subtitles`)
- [ ] T1 (M) Backend probing at startup: real 1 s test encodes for
  NVENC / QSV / VAAPI / software; results cached, shown in admin
  with FFmpeg version + driver
- [ ] T2 (L) Decision engine v2: full plan matrix (client × codec ×
  network × per-user remote cap × subtitle choice) — pure
  function, table-driven tests for every row
- [ ] T3 (L) Transcode sessions: keyframe-aligned HLS/fMP4 (6 s),
  seek = restart FFmpeg at the keyframe, throttle to a buffer
  ahead, scratch on SSD/tmpfs with reaping
- [ ] T4 (M) HDR→SDR tone mapping order: libplacebo → vendor
  filters → CPU last resort; logged per session
- [ ] T5 (M) Subtitle burn-in path for picture subs on clients that
  can't render them (web first) — and the UI says that's *why*
  it's transcoding
- [ ] T6 (M) Caps: max concurrent transcodes (GPU-bound), per-user
  remote bitrate cap enforced in the plan; queue/wait behaviour
- [ ] T7 (S) Truck test: Fire TV + browser from the truck on the
  public route, bandwidth-limited stream starts < 5 s
- [ ] T8 (S) Admin playback dashboard: live sessions, backend in
  use, FFmpeg command per session (Jellyfin's good habit)
- [ ] T9 (M) Public-listener hardening (with D2/D3): HTTPS only,
  auth rate limits already in A1, no admin routes without auth,
  security headers; tailnet/LAN unchanged

## Phase 3 — Android + Fire TV (`clients/android`)
- [ ] N1 (M) Kotlin/Media3 skeleton: server list with multi-address
  entries tried in order LAN → Tailscale → public; one login
- [ ] N2 (M) TV UI (Leanback-style rows) + phone UI from one
  codebase; D-pad-first navigation, big type, fast focus
- [ ] N3 (M) Player: Media3 with native PGS/picture-subtitle
  rendering, audio/subtitle pickers, resume + reporting (P4 shape)
- [ ] N4 (S) Sideload install on every home Fire Stick; living-room
  soak test vs the Emby app it replaces
- [ ] N5 (M) Store prep (only when publishing — see DECISIONS):
  Amazon developer account, reviewer demo server with
  public-domain media + test account, privacy policy page, store
  assets, name-clearance check finished first
- [ ] N6 (S) Optional later: offline downloads in the **phone**
  build only (truck use); TV build stays streaming-only

## Phase 4 — Requests + integrations (`integrations`, `discord`)
- [ ] I1 (M) *arr clients: Sonarr/Radarr/Lidarr (`/api/v3`,
  `X-Api-Key`), Prowlarr (`/api/v1`), qBittorrent WebUI —
  credentials encrypted in DB, admin-only, connection-test buttons
- [ ] I2 (L) Requests in Parlour: search (TMDB) → already-in-library
  check → request → admin approval or per-user quota → route to
  the right *arr app → status sync job → notify the requester
- [ ] I3 (M) Books flow: no *arr app exists (Readarr retired) —
  Prowlarr search → qBittorrent → library folder, tracked as a
  request like everything else
- [ ] I4 (M) Discord bot in-binary (feature flag): `/request`,
  `/library`, `/downloads`, `/queue` speaking to the same request
  service — retires discordarr + shelfmark when it covers them
- [ ] I5 (S) Decommission checklist: Emby off, discordarr/shelfmark
  retired only after a clean month on Parlour requests

## Phase 5 — Compatibility APIs (only if still wanted)
- [ ] X1 (L) Jellyfin surface over the same catalog:
  `AuthenticateByName`, `Items`, `PlaybackInfo`, Sessions
  Playing/Progress/Stopped, UserData writes — enough for mature
  third-party clients; a view, never a second catalog
- [ ] X2 (L) Audiobookshelf surface: libraries/items/progress in
  ABS shape so ABS apps work before Parlour grows native
  audiobook support
- [ ] X3 (S) Decide with real usage: keep, freeze, or delete —
  compat that nobody uses gets deleted, not maintained

## Phase 6 — iOS (`clients/ios`)
- [ ] S1 (M) Swift/AVPlayer app: same server entries + login,
  browse, play with reporting; subtitles via HLS/WebVTT
- [ ] S2 (S) TestFlight → App Store prep (same demo-server +
  privacy-policy assets as N5, Apple flavour)

---

## Cross-cutting (every phase)
- [ ] Tests: decision-engine tables (T2), matcher fixtures from
  the *real* library's folder names, API tests per milestone;
  CI keeps fmt + clippy `-D warnings` + tests mandatory
- [ ] CI: add `cargo audit` + a Docker image build job once the
  Dockerfile matters (Phase 1.1 F8)
- [ ] Backups: Postgres dump joins sullivan's existing backup run
  the day Parlour holds real watch state (Phase 1.2) — and the
  host backup script gets updated in the same change
- [ ] Observability: `/healthz` in Uptime Kuma on freddy from the
  first sullivan deploy
- [ ] Security pass before ANY public listener: A1/A6/T9 are the
  checklist; no new public port without it
- [ ] Docs: every merged feature updates `docs/` + this file in
  the same PR; DECISIONS.md gets an entry for every judgment call

## Research spikes (timeboxed, do before the milestone that needs them)
- [ ] R1 (2 h) sqlx 0.9 vs 0.8 breaking changes → feeds F2/D6
- [ ] R2 (2 h) Real-library naming census: dump the actual movie/TV
  folder names, count how many carry TMDB/TVDB ids → sizes MD2
- [ ] R3 (1 h) Pull one full Emby `UserData` export from Jordan's
  running Emby → fixture for E1/E2 tests
- [ ] R4 (2 h, Phase 2) libplacebo tonemap flags on the RTX card in
  sullivan + NVENC session limits on consumer drivers
- [ ] R5 (1 h, Phase 3) Media3 on Fire OS: which Fire OS versions
  on the household sticks, ExoPlayer subtitle support matrix
