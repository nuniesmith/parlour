# Parlour — TODO / working plan

The living checklist. `docs/ROADMAP.md` says *what order* things
happen; this file says *exactly what to build*, part by part.
Check boxes off in the same PR that does the work. Sizes are
T-shirt estimates (S = hours, M = a day or two, L = most of a week)
for one developer with AI assistance, not promises.

**Current position:** Phase 0 ✅ and Phase 1 **code** ✅ done
2026-11-10 (M1.1–M1.7 merged, PRs #1–#7). The only Phase 1 item
left is E4, the operational cutover week. Phase 2 was expanded
2026-11-11 with the ABR / segment-cache plan (D8–D9, T10–T18),
Phase 4 with recommendations (D10, REC1–REC5) and a detailed
*arr request lifecycle (D11), and the client phases re-sequenced
per D12 (web is the ABR test vehicle; Android phone next; TV
deferred pending the OS check R7; iOS after that).
**2026-11-11 (late):** a test instance runs on oryx behind
https://parlour.7gram.xyz (temporary princess proxy; production
still moves to sullivan per D4). Owner added three new tracks,
now Phases 7–9 below: **Web UI parity with Emby's setup**
(D13, spec: `docs/WEBUI-EMBY-NOTES.md`), **import/export of
everything + sidecar metadata stored with the media files**
(D14), and **segments — intro/credits detection, skip, and
autoplay next episode** (D15). These are the owner's priority
tracks after the Phase 1 operational finish; the phase numbers
are labels, not a queue order — the owner picks. An in-app
change-password PR (A8) is in flight separately.

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
- [x] **D8 — Adaptive streaming, no manual bitrate picker.
  DECIDED 2026-11-11:** remote playback is Netflix-style ABR —
  the server offers an HLS bitrate ladder, the *client* measures
  throughput + buffer and switches rungs per segment, starting
  low and ramping up. No bitrate menu anywhere; the only user
  control is the per-profile remote cap that already exists.
  HLS-only for v1 (iOS AVPlayer has no DASH; a DASH manifest
  over the same CMAF segments is a later add-on, not a
  re-encode). Research: 2026-11-11 ABR report §1.
- [x] **D9 — No Redis. DECIDED 2026-11-11:** video segments are
  never cached in Redis/Memcached (wrong tool: large immutable
  files; Memcached's default item limit is 1 MB). Segments live
  in a filesystem/SSD cache (T13); jobs and sessions stay in
  Postgres. Revisit Redis only if Parlour ever goes multi-node
  or a measured Postgres bottleneck appears. Research §2.
- [x] **D10 — Recommendations are content-based. DECIDED
  2026-11-11:** no collaborative filtering and no ML in v1
  (2–10 users make CF meaningless). V1 is watch-history affinity
  + catalog content (genres/people/keywords) + TMDB's own
  recommendations/similar edges, scored in SQL, per profile,
  every personalised row labelled with its seed. Research §3.
- [x] **D11 — Requests are native, *arr are downstream. DECIDED
  2026-11-11:** the request lifecycle (Seerr-shaped) lives in
  Parlour with Parlour auth/profiles/quotas. Parlour calls
  Sonarr/Radarr directly; it **never calls Prowlarr to place or
  track a request** (Prowlarr has no library/queue — it only
  syncs indexers into the *arr apps; read-only health at most).
  Lidarr is `/api/v1` + MusicBrainz, not `/api/v3`. Research §4.
- [x] **D12 — Client sequence. DECIDED 2026-11-11:** web client
  is the primary ABR test vehicle (including on the TV's
  browser); Android **phone** next; TV native deferred behind
  the OS check (R7) — Amazon's device table lists the owner's
  new Ember Artline TV as Fire OS 8 (Android 11), *not* Vega
  OS, so the TV build is probably the cheap Android TV flavour
  after all; Vega/Kepler is a separate parked track that starts
  only if a Vega device is actually owned. iOS after Android.
  Research §5. **R7 confirmed 2026-11-11: the set runs
  Fire OS 8.1.8.5.**
- [x] **D13 — Web UI follows Emby's information architecture.
  DECIDED 2026-11-11:** sidebar shell with grouped sections
  (web settings / per-user preferences / server admin tree),
  a server Dashboard with info card, Now Playing session
  cards, Activity and Alerts — the layout the owner already
  knows. Original Parlour implementation and branding
  (clean-room rule applies to UI too: patterns yes, Emby
  code/assets/wording no). Spec: `docs/WEBUI-EMBY-NOTES.md`.
  Explicit non-goals: Premiere/paywall, Live TV, DLNA,
  Camera Upload, Cinema Intros, plugin store.
- [x] **D14 — Import/export everything, plus sidecar metadata.
  DECIDED 2026-11-11:** one integrated system: a versioned
  export bundle (users/logins, profiles, library definitions
  + access, catalog metadata, watch history, segments) that
  imports back idempotently with a dry-run report; AND
  per-item metadata sidecars stored **with the media files**,
  so the metadata travels when files move. Item metadata
  lives in sidecars; per-user state (watch history, logins)
  lives in the bundle only. Media stays read-only unless a
  library opts into sidecar writing.
- [x] **D15 — Segments + autoplay. DECIDED 2026-11-11:**
  intros/credits are detected automatically (chapters first,
  then audio-fingerprint matching across a season's episodes,
  Jellyfin-intro-skipper style), stored per episode, and the
  player offers Skip (button or auto, per profile). Next
  episode autoplays behind a countdown card, per-profile
  toggle, riding the existing playback reporting.
- [x] **D16 — Notices: disk check scope + restart notice.
  DECIDED 2026-11-12:** the disk-space check covers only the
  folders Parlour writes to (cache, artwork) — a nearly full
  media drive is normal and raises nothing — warning below
  5 GB free, error below 1 GB. The "restart to apply" notice
  is supported by the model and the Dashboard banner but has
  no trigger until a setting can change at runtime (config is
  environment-only today).

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
| `clients/android` | Kotlin/Media3 phone first; TV flavour later (D12) | 3 |
| `clients/ios` | Swift/AVPlayer | 6 |
| — (parked) | Vega/Kepler TV app (React Native, `.vpkg`) — only if a Vega device is owned | — |
| `migrations/` | Schema, timestamped + append-only | 1 (grows) |

---

## Phase 1 — Direct-play MVP

Goal: **replace Emby at home for movies + TV** — log in, browse,
press play, resume where you left off, subtitles on.

### Milestone 1.1 — Foundations (`server`, `db`, `core`)
- [x] F1 (M) Config loading: env + `.env`, typed `Config` struct,
  fail-fast with a clear message on bad config
- [x] F2 (S) Bump workspace deps to current (axum 0.8.9, sqlx 0.9 —
  see D6); remember axum 0.8 path syntax is `/{id}`, not `/:id`
- [x] F3 (M) Tracing/logging setup (`tracing` + env filter),
  request-id per request, no secrets in logs — ever
- [x] F4 (M) DB pool in `db` crate + `sqlx::migrate!()` at startup;
  `/healthz` endpoint that actually checks the DB (a green check
  that lies is worse than none)
- [x] F5 (M) First real migration: `users`, `profiles`, `sessions`,
  `libraries`, `library_access` tables
- [x] F6 (S) Error type → HTTP mapping (JSON errors, no stack
  traces to clients); unknown routes return JSON 404, not HTML
- [x] F7 (M) Embed the built web UI in the binary (rust-embed) with
  SPA fallback; dev mode serves from disk instead
- [x] F8 (S) `docker compose up` runs the whole thing locally,
  healthcheck green, empty home page loads
- **Done when:** a fresh clone + `.env` boots, migrates, and serves
  the placeholder UI with a real DB behind `/healthz`.

### Milestone 1.2 — Users, profiles, invites (`server`, `db`)
- [x] A1 (M) Password hashing with `argon2`; login endpoint with
  rate limiting (per-IP + per-account backoff)
- [x] A2 (M) DB-backed sessions per D7: HttpOnly + SameSite cookie,
  token hash at rest, logout/revoke, session list per user
- [x] A3 (S) First-admin bootstrap: `PARLOUR_ADMIN_USER/PASSWORD`
  env used once if no users exist, then ignored
- [x] A4 (M) Invite-only sign-up: admin creates an invite link,
  invitee sets name + password; no open registration route exists
- [x] A5 (M) Profiles under a user: CRUD, optional PIN, avatar
  colour/initial; per-profile defaults (audio lang, subtitle
  lang/mode, remote bitrate cap)
- [x] A6 (M) Library access + rating limits enforced in SQL on
  every item query (someone else's library = 404, never 403)
- [x] A7 (S) Admin-only route guard; admin surfaces never render
  for normal users
- [x] A8 (S) In-app change password: `POST /api/v1/auth/change-password`
  + settings form; changing revokes the user's other sessions
- **Done when:** Jordan + Kayla + one invited friend can each log
  in with separate profiles, and the friend cannot see a private
  library even by guessing URLs.

### Milestone 1.3 — Catalog + scanner (`catalog`, `jobs`, `db`)
- [x] C1 (M) Second migration: `media_items`, `media_files`,
  `media_streams`, `subtitle_tracks`, `people`, `item_people`,
  `images` (paths/hashes only — image bytes stay on disk)
- [x] C2 (M) Library CRUD API (admin): name, kind, root paths
- [x] C3 (L) Folder walker: *arr/TRaSH naming conventions, video
  extensions allowlist, skip samples/extras by rule; fingerprint
  (path, size, mtime) skip for unchanged files
- [x] C4 (M) ffprobe runner: parse container, duration, codecs,
  resolution/HDR flags, audio + subtitle streams into
  `media_streams` / `subtitle_tracks`
- [x] C5 (M) TV structure: series → seasons → episodes from
  `SxxExx` naming; movies standalone; multi-file items supported
- [x] C6 (M) Scan as a **job** (`jobs` crate): manual trigger,
  per-library, progress + errors visible in admin; idempotent
  re-runs (stable content ids)
- [x] C7 (M) Filesystem watcher (`notify`) → debounce → targeted
  rescan job; plus a nightly full-scan job as the safety net
- [x] C8 (S) Missing-file handling: vanished files mark the item
  unavailable, never delete watch state
- **Done when:** pointing Parlour at the real movies + TV folders
  produces a correct, browsable catalog, and re-scanning changes
  nothing (idempotent).

### Milestone 1.4 — Metadata (`metadata`, `catalog`)
- [x] MD1 (M) TMDB client (API key from env/DB settings): search,
  movie + TV details, artwork URLs, provider ids stored on items
- [x] MD2 (M) Matcher: TMDB/TVDB id in folder/file name wins;
  else normalised title + year; low-confidence matches queue for
  manual fix, never silently guess
- [x] MD3 (S) Artwork download job: poster/backdrop/logo cached
  locally at two sizes; items without art get a generated tile
- [x] MD4 (S) Manual fix-match UI/API: search TMDB, pick the right
  one, re-fetch that item only
- [x] MD5 (S) TheTVDB client for episode ordering/metadata where
  TMDB is thin (second provider behind the same trait)
- **Done when:** ≥95% of the real library matches correctly with
  no hand-holding, and every miss is fixable from the UI.

### Milestone 1.5 — Web UI (`web/`)
- [x] W1 (M) API client + auth screens: login, profile picker
  (+PIN), invite acceptance page
- [x] W2 (M) Home: Continue Watching row, Recently Added,
  libraries row (data from 1.6 endpoints; shell can land first)
- [x] W3 (M) Library browse: poster grid, sort/filter (year,
  genre, unwatched), alphabet jump for big libraries
- [x] W4 (M) Item pages: movie detail; series → season → episode
  lists; cast row; "more like this" can wait
- [x] W5 (L) Player page: native `<video>` + HLS fallback, subtitle
  track picker (shows forced/default/SDH), audio track picker,
  resume prompt, keyboard shortcuts, progress reporting (1.6)
- [x] W6 (M) Search: Postgres FTS over titles/people to start;
  search-as-you-type with debounce
- [x] W7 (M) Settings: my profile/preferences; admin section —
  users + invites, libraries + scan buttons + job status
- [x] W8 (S) Empty/error states everywhere (no library yet, scan
  running, server unreachable) — a blank page is a bug
- **Done when:** Kayla can use it on the TV-browser/laptop without
  asking how anything works.

### Milestone 1.6 — Playback: direct play + remux (`playback`, `subtitles`)
- [x] P1 (M) Playback-info endpoint: given item + client profile +
  network kind, return the plan from `core`'s decision types
  (Phase 1 only ever answers DirectPlay or Remux, with reasons)
- [x] P2 (M) File streaming with HTTP Range (206, seeking works),
  correct content types, stream tokens (short-lived, signed) so
  media URLs aren't open to the world
- [x] P3 (M) Remux path: `ffmpeg -c copy` to HLS/fMP4 for
  containers the web player rejects (MKV → fMP4), segments cleaned
  up with the session
- [x] P4 (M) Playback reporting, same shape the big servers use:
  start / progress heartbeat (~10 s) / stopped → `watch_state`
  (position, played at ≥90%, play count, last played)
- [x] P5 (S) Mark played/unplayed + favourite endpoints + UI
- [x] P6 (M) Subtitles v1: sidecar + embedded **text** tracks
  extracted to WebVTT and served as tracks; forced-track
  auto-pick per profile language rules
- [x] P7 (S) Continue Watching + Next Up queries (resume items,
  next episode after a finished one)
- [x] P8 (S) Concurrent-session registry (who is watching what —
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
- [x] E1 (M) Read-only Emby client in `catalog`: list users,
  page items with `ProviderIds` + `UserData`
- [x] E2 (M) Importer job: map Emby user → Parlour profile (manual
  mapping screen), apply played/position/favourites by provider
  id; dry-run mode prints the counts first
- [x] E3 (S) Import report: matched / unmatched / ambiguous counts;
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
- [ ] T3 (L) Transcode sessions: **one FFmpeg process per
  session with multiple outputs** (decode once, `split` to the
  ladder rungs, `-var_stream_map` + master playlist), never one
  process per rung; keyframe-aligned HLS/fMP4 — **6 s segments,
  2 s GOP** (`-g`/`-keyint_min` = fps×2, `-sc_threshold 0`,
  segment must start on an IDR at identical timestamps on every
  rung); seek = restart FFmpeg at the keyframe
- [ ] T4 (M) HDR→SDR tone mapping order: libplacebo → vendor
  filters → CPU last resort; logged per session
- [ ] T5 (M) Subtitle burn-in path for picture subs on clients that
  can't render them (web first) — and the UI says that's *why*
  it's transcoding
- [ ] T6 (M) Caps: max concurrent transcodes (GPU-bound), per-user
  remote bitrate cap enforced in the plan; queue/wait behaviour
- [ ] T7 (S) Truck test: **web client** from the truck on the
  public route (the D12 test vehicle), bandwidth-limited stream
  starts < 5 s, ramps up without a manual quality choice
- [ ] T8 (S) Admin playback dashboard: live sessions, backend in
  use, FFmpeg command per session (Jellyfin's good habit)
- [ ] T9 (M) Public-listener hardening (with D2/D3): HTTPS only,
  auth rate limits already in A1, no admin routes without auth,
  security headers; tailnet/LAN unchanged

### Milestone 2.1 — ABR ladder + segment cache (D8/D9, research §1–§2)
- [ ] T10 (S) Ladder config in `core`: data-driven rendition
  table, H.264 v1 — 360p 0.4–0.8 M, 480p 1.2–1.5 M, 540p 2 M
  (default start), 720p 3–4.5 M, 1080p 6–8 M, **one shared AAC
  stereo track (128k) across all rungs**; prune rungs above the
  source's resolution/bitrate, never upscale; 2160p only as
  DirectPlay/pre-transcode. Depends on: nothing. Blocks T11.
- [ ] T11 (M) GPU session budget + admission control: probe the
  backend's real budget (GeForce NVENC cap is **12 sessions per
  system** — a 5-rung ladder × 3 viewers = 15 and *fails*;
  QSV/VAAPI have no session cap, only throughput). Count
  sessions as Σ rungs per active session; when a new session
  would exceed budget − headroom, degrade it (fewer rungs,
  Remux, or DirectPlay) and say so in the UI — never spawn
  FFmpeg until it errors. Depends on: T1, T10.
- [ ] T12 (M) JIT ahead-window: transcode only **~120 s ahead**
  of the playhead (20 × 6 s segments), pause/kill-and-resume at
  a segment offset when the window is full, reap idle sessions.
  The server window is deliberately separate from the client's
  buffer target. Depends on: T3, T13.
- [ ] T13 (L) Filesystem segment cache (**the** cache — D9, no
  Redis): layout `cache/<file_id>/<rendition>/seg-*.m4s` +
  playlists; keyed by (file, source mtime/hash,
  rendition-params hash incl. encoder-settings version);
  atomic writes (temp → rename, never serve partial output);
  Postgres cache index (size, last-accessed) with an LRU
  sweeper under a configurable size cap, TTL for never-finished
  JIT cache, **pinning for partially-watched items**;
  single-flight lock per (file, rendition) so two viewers never
  spawn duplicate ladders; cache-aware start — resume encoding
  at the first missing segment, not from 0. Depends on: T3.
  Blocks T12, T14.
- [ ] T14 (M) Pre-transcode jobs: Postgres-queued background
  full-ladder encodes for popular / recently-added /
  partially-watched items, off-peak; output lands in the T13
  cache so playback is then pure file serving. Depends on:
  T11, T13.
- [ ] T15 (M) Web player ABR profile — **the primary test
  vehicle (D12)**: hls.js `maxBufferLength 90`,
  `maxMaxBufferLength 120`, `maxBufferSize` **250 MB** (the
  60 MB default silently caps a 90 s buffer at high rungs),
  `backBufferLength 30`, EWMA VoD defaults (fast 3.0 / slow
  9.0 — do not copy blog values), `abrEwmaDefaultEstimate`
  ~2 Mbps on Wi-Fi / ~730 kbps on cellular, fragment retry 6,
  fatal-error recovery (`NETWORK_ERROR → startLoad`,
  `MEDIA_ERROR → recoverMediaError`); QoE logging (startup ms,
  rebuffers, rung switches, buffer health) posted with the P4
  stopped report. Pin the hls.js version and re-check defaults
  against *its* docs (R9). Depends on: T10 (ladder to play).
- [ ] T16 (M) Web on the TV browser: spatial/D-pad navigation +
  visible focus in the SvelteKit UI, 10-foot layout/large type,
  small `backBufferLength` + byte caps for TV-class memory;
  test on the actual Ember TV's built-in browser and record
  whether MSE/hls.js are viable there — that result, not a
  guess, gates the native TV timeline. Depends on: T15.
- [ ] T17 (S) HEVC as an *additional* ladder for clients that
  report hardware HEVC (Apple/TV) — never the only ladder
  (browser support is hardware-fragmented; an HEVC-only web
  ladder black-screens some clients). AV1 is an offline
  pre-transcode experiment only — live AV1 JIT is oversold.
  Parked behind T10–T15 working.
- **Done when (2.1):** on a throttled connection the web player
  starts at a low rung in ~1 s, ramps to the best rung the
  connection sustains, drops a rung instead of rebuffering when
  the connection dips, and a resumed title plays from cache
  with zero GPU use.

## Phase 3 — Android phone, then TV flavour (`clients/android`, D12)
- [ ] N1 (M) Kotlin/Media3 skeleton: server list with multi-address
  entries tried in order LAN → Tailscale → public, failover at
  playback start / on fatal network error (restart at position
  on the new origin), one login; client sends a capability
  profile at session start (codecs, max resolution) so the
  server can pick ladder/DirectPlay
- [ ] N2 (M) **Phone UI first** (Compose): browse, search, detail,
  profile picker; TV UI is N8, a flavour of this app, not a
  second app
- [ ] N3 (M) Player: Media3 with native PGS/picture-subtitle
  rendering, audio/subtitle pickers, resume + reporting (P4 shape)
- [ ] N7 (M) Media3 ABR profile (research §1.5):
  `DefaultLoadControl` min 50 s / max 120 s buffer,
  bufferForPlayback 2.5 s / after-rebuffer 5 s (large *target*,
  fast *start* — don't conflate them),
  `prioritizeTimeOverSizeThresholds(true)` or an explicit
  ~200 MB byte target so the byte cap doesn't override the time
  target; `DefaultTrackSelector` caps (max video size,
  data-saver bitrate cap on cellular); keep the default
  AdaptiveTrackSelection/bandwidth meter — tune parameters,
  don't fork the ABR. Depends on: T10.
- [ ] N4 (S) Sideload install on the owner's phone + living-room /
  road soak test vs the Emby app it replaces (adb install; no
  store needed for household use)
- [ ] N8 (L) TV flavour — **R7 confirmed Fire OS 8.1.8.5 (2026-11-11)**:
  `LEANBACK_LAUNCHER` flavour, 320×180 banner, Compose-for-TV
  rows, D-pad-first navigation with visible focus (the focus
  work, not playback, is the bulk of this), byte-capped buffers
  for 1–2 GB RAM TV hardware; sideload onto the owner's set via
  adb/Downloader. Depends on: N1–N3, N7, R7.
- [ ] N9 (—) **PARKED — Vega/Kepler TV app.** Separate project
  (React Native + Vega SDK, `.vpkg`, W3C MSE player with
  Shaka/hls.js, dev-install via Vega CLI — no adb, no APKs,
  cleartext-LAN needs explicit config). Starts only if a Vega
  device is actually owned (D12). Do not estimate it as a port.
- [ ] N5 (M) Store prep (only when publishing — see DECISIONS):
  Amazon developer account, reviewer demo server with
  public-domain media + test account, privacy policy page, store
  assets, name-clearance check finished first
- [ ] N6 (S) Optional later: offline downloads in the **phone**
  build only (truck use); TV build stays streaming-only

## Phase 4 — Recommendations + requests + integrations (`integrations`, `discord`)

### Recommendations (D10, research §3)
- [ ] REC1 (M) TMDB edges cache: per catalog item, fetch +
  persist `/recommendations` and `/similar` (movie + TV
  endpoints; recommendations weighted above similar), plus
  trending (day/week) snapshots; weekly refresh job. Never call
  TMDB per page render. Depends on: MD1.
- [ ] REC2 (M) Affinity profiles: per `profile_id`, aggregate
  genre/person/keyword affinity from watch events — completed
  (≥85%) **+3**, favourite **+3**, rewatch **+2**, partial
  (20–85%) **+1**, abandoned (<20% after ≥5 min) **−2**,
  dismissed/not-interested **−5** — with recency decay
  `0.5^(age_days/180)`; recomputed nightly + on watch events.
  Profiles strictly separate. Depends on: P4 reporting being
  accurate (completion % is an input, not just UX).
- [ ] REC3 (L) Scoring v1 in SQL: candidate score = 3.0·genre
  affinity + 2.0·people affinity + 1.0·keyword affinity +
  2.5·TMDB-edge signal (capped) + 0.5·quality prior
  (vote average × log vote count) + 0.5·in-library boost;
  exclude watched/dismissed; materialised per-profile scores
  refreshed by job; row APIs: Because-you-watched,
  More-like-this, Top-picks, Trending. Depends on: REC1, REC2.
- [ ] REC4 (M) Recs UI rows in the web UI: every personalised
  row names its seed ("Because you watched X"); item pages show
  shared genre/people chips; **kids profiles get a hard
  certification filter inside the query**, not UI hiding;
  cold-start profiles fall back to onboarding genre picks +
  trending/top-rated — never fake personalisation. Depends
  on: REC3.
- [ ] REC5 (S) Discover page: TMDB trending/popular + rec-driven
  not-in-library titles, clearly global (not personalised),
  each with a Request button into I2. Depends on: REC1, I2.

### Requests + *arr (D11, research §4)
- [ ] I1 (M) *arr clients: Sonarr + Radarr (`/api/v3`,
  `X-Api-Key` header; lookup by `tvdb:`/`tmdb:` term, root
  folders + quality profiles discovered for the admin mapping
  UI, add + search commands), Lidarr (`/api/v1`, MusicBrainz —
  later), qBittorrent WebUI — credentials encrypted in DB,
  admin-only, **Test connection hits `GET /system/status`** and
  verifies the instance's own API shape before anything else.
  Prowlarr: read-only health only (D11).
- [ ] I2 (L) Requests in Parlour (Seerr-shaped lifecycle):
  search (TMDB) → already-in-library check → request (movies
  whole; **TV partial-season** — selected seasons only) →
  states pending / approved / declined / processing /
  available / failed / no-releases → admin approval or
  per-user auto-approve + **quotas** (per-user movie/TV-season
  limits per window) → dispatch: existence pre-check first
  (`GET /movie?tmdbId=`, `GET /series?tvdbId=`), add with
  `monitored` + `searchForMissingEpisodes`/`searchForMovie`,
  **an add that returns 400/422 "already added" is idempotent
  success, not an error**; `hasFile` = already available, never
  dispatch; no-releases is a real state with scheduled retry,
  not an eternal "processing"; stuck queue items surface their
  reason with a blocklist-and-research action
  (`DELETE /queue/{id}?removeFromClient=true&blocklist=true`
  + new search). Sanitise Radarr tags to `[a-z0-9-]`.
- [ ] I6 (M) *arr webhook receiver: authenticated endpoint for
  Sonarr/Radarr Connect webhooks — On Import + On Upgrade flip
  the request to available, trigger a targeted scan, and
  notify the requester; On Grab / Download Failure update
  status. **Plus startup reconciliation**: poll queue/history
  for active requests on boot (webhooks get missed while
  Parlour is down). Depends on: I2.
- [ ] I7 (S) Request status poller: interval poll of
  `/queue` + `/history` for active requests between webhooks;
  surfaces `trackedDownloadStatus` warnings in the UI.
  Depends on: I2.
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

## Phase 6 — iOS (`clients/ios`, D12 — after Android)
- [ ] S1 (M) Swift/AVPlayer app: same server entries + login,
  browse, play with reporting; subtitles via HLS/WebVTT.
  HLS-only platform — the T10 ladder serves it unchanged.
- [ ] S3 (S) AVPlayer caps (the *only* ABR control Apple
  exposes — the algorithm itself is opaque, don't try to
  replace it): `preferredPeakBitRate` per network kind
  (+ `preferredPeakBitRateForExpensiveNetworks` ~1.5 Mbps),
  `preferredForwardBufferDuration` ~60 s if testing shows the
  default under-buffers on cellular,
  `automaticallyWaitsToMinimizeStalling = true` for VoD.
- [ ] S2 (S) TestFlight → App Store prep (same demo-server +
  privacy-policy assets as N5, Apple flavour). TestFlight
  facts: 100 internal / 10,000 external testers, **builds
  expire after 90 days** — put a refresh reminder on the
  calendar, don't discover it on the road.
- [ ] S4 (M) Optional later: offline downloads via
  `AVAssetDownloadTask` — clean for Parlour's **unencrypted**
  HLS (no FairPlay complexity — don't import it); downloads
  are sandbox assets, rendition chosen from the T10 ladder.
  Phone only, like N6.

---

## Phase 7 — Web UI: Emby-style shell, dashboard & sessions (`web/`, `server`, D13)

Spec: `docs/WEBUI-EMBY-NOTES.md` — read it first; it has the
screenshot analysis, the current-UI inventory, and the gap
table this phase closes. Presentation + light server support;
does NOT depend on Phase 2 transcoding.

- [x] UI1 (L) **Sidebar shell**: left sidebar with collapsible
  sections (Libraries · Web Settings · `<profile>`
  Preferences · Server, admins only), active-item pill,
  search box in the sidebar, top bar with back/title +
  account actions; drawer under 1024 px; keep Parlour
  tokens + focus states; usable in the TV's browser.
- [x] UI2 (M) **Server Dashboard page** (`/admin` landing):
  server name, version (`/api/v1/version`), listener port,
  DB status, the LAN / tailnet / public URLs from server
  config with copy buttons, job-queue snapshot, restart /
  power actions where the server honestly supports them.
- [x] UI3 (L) **Now Playing session cards**: extend the P8
  session payload server-side (playback plan per stream:
  source → mode + reason; client app/device, user+profile,
  source IP/protocol, position/duration) and render Emby-
  anatomy cards (poster, S/E line, progress bar, stream
  block, controls: stop session; pause = revoke + stop in
  v1, noted as such). Poll ~5 s; quiet empty state.
- [x] UI4 (M) **Activity feed**: `activity` table + writes at
  login, playback start/stop, scan/import completion, user/
  invite admin actions; Dashboard feed + full page; 90-day
  retention job.
- [x] UI5 (M) **Notices / Alerts**: server notices model fed
  by failed jobs, metadata-review backlog, disk-space check,
  config-changed-restart flag; Dashboard banner + right-rail
  Alerts list with icons + timestamps; dismiss per admin.
- [ ] UI6 (L) **Admin IA re-org**: the one tabbed `/admin`
  page becomes the sidebar Server tree: Dashboard, General,
  Users (users + invites + access), Library (libraries,
  paths, scans, Emby import), Metadata (review queue),
  Sessions (UI3), Jobs (run-now where supported, error
  detail), Import/Export (XFER8 lands here). All deep-
  linkable under `/admin/...`; old tab URLs redirect.
- [ ] UI7 (M) **Preferences IA re-org**: Settings splits into
  Display, Home Screen (rail visibility/order), Playback
  (SEG5/SEG6 prefs live here), Subtitles, Profile, Account.
- [ ] UI8 (S) **Home parity bits**: library tile row +
  per-library Latest rails alongside the existing
  Continue Watching / Next Up / Recently Added rails.
- [ ] UI9 (S) Search affordances: `/` keyboard shortcut
  focuses search from anywhere; recent searches per profile.
- [ ] UI10 (M) **Account self-service**: change-username
  endpoint (case-insensitive unique, current password
  required, existing sessions survive) + Account page UI;
  admins can rename users too.
- [ ] UI11 (S) **Visual QA pass**: screenshot checklist
  against `docs/WEBUI-EMBY-NOTES.md` §1 (shell, dashboard,
  session card, rails) at desktop + phone widths; fix
  spacing/anatomy drift; empty states everywhere.
- **Done when:** an Emby user can sit down and find
  everything where Emby taught them to look — dashboard,
  sessions with explained stream decisions, users, libraries,
  jobs — without a tour; Parlour branding throughout.

## Phase 8 — Import/export & sidecar metadata (`server`, `catalog`, `jobs`, `db`, D14)

- [ ] XFER1 (M) **Bundle format spec**: versioned
  `parlour-export v1` (zip): `manifest.json` (format +
  server versions, export time), `users.json` (username,
  is_admin, password + PIN **hashes** — hashes leave the
  server only behind an explicit admin confirm with a
  warning), `profiles.json` (+ prefs), `libraries.json`
  (definitions, paths, access grants by username),
  `catalog.json` (items keyed by library + stable_key, with
  provider ids, genres, people, ratings, segments),
  `user_data.json` (per username+profile: positions,
  played, favourites, play counts, last played). Settings
  secrets (API keys, stream secret) are **never** exported.
  Written up in `docs/IMPORT-EXPORT.md` as part of this task.
- [ ] XFER2 (L) **Export**: admin endpoint + background job
  producing the bundle; optional artwork inclusion (files
  under `artwork/` in the zip, off by default with a size
  warning); download from the Jobs UI.
- [ ] XFER3 (L) **Import**: upload → parse → **dry-run
  report** (creates / updates / conflicts, matched by
  username, library name, item stable_key/provider ids) →
  apply as a job. Idempotent; import never deletes anything
  by default; conflicting usernames are reported, not merged.
- [ ] XFER4 (L) **Sidecar metadata with the media files**:
  per-file `<name>.parlour.json` (series: folder-level
  `series.parlour.json` + per-episode files), versioned,
  holding item metadata (titles, year, provider ids, genres,
  overview, rating, cast, segments). Scanner reads sidecars
  **before** provider lookup and treats them as
  authoritative for those fields; a metadata job rewrites
  sidecars when matched metadata changes. Atomic writes
  (temp + rename).
- [ ] XFER5 (M) **Sidecar write opt-in**: per-library
  `write_sidecars` flag (default off — media mounts are
  read-only today); when on, XFER4 writes + optionally
  copies poster/backdrop as `poster.jpg` / `backdrop.jpg`
  next to the media. The flag + mount change is an explicit
  ops step, documented in the task PR.
- [ ] XFER6 (S) **NFO interop**: scanner also reads
  Kodi/Emby-style `.nfo` (ids + title) as an input source;
  XFER4 can optionally write `.nfo` alongside the JSON.
- [ ] XFER7 (M) **Automatic backups**: nightly export job to
  a configurable folder with retention (ties into the
  Cross-cutting backups item); restore = XFER3.
- [ ] XFER8 (M) **Admin UI**: Server → Import/Export page —
  export (options + download), import (upload, dry-run
  report render, apply), per-library sidecar status.
- **Done when:** the whole server state survives a host
  move — bundle out on oryx, bundle in on a fresh install,
  same users/profiles/watch history — and copying a movie
  folder to a new drive carries its Parlour metadata with it.

## Phase 9 — Segments: intro/credits detection, skip, autoplay next (`catalog`, `playback`, `web`, D15)

- [ ] SEG1 (M) **`media_segments`**: migration + types —
  (item_id, kind intro/credits/recap, start_ms, end_ms,
  source chapters/fingerprint/manual, confidence);
  playback-info returns the file's segments; manual source
  always wins over auto.
- [ ] SEG2 (M) **Cheap detection pass**: `DetectSegments`
  job — ffprobe chapters named intro/opening/credits, plus
  black-frame + silence heuristics at head/tail. Runs on
  scan for series episodes; fills SEG1 with source=chapters
  or a low-confidence heuristic row.
- [ ] SEG3 (L) **Audio-fingerprint detection**: fingerprint
  each episode's audio (ffmpeg chromaprint) once, cache by
  file hash; cross-correlate episodes within a season —
  the long common segment inside the first ~25% is the
  intro; a common tail segment is the credits. Confidence
  from match length/quality; below threshold → no segment,
  never a guess. This is the Jellyfin intro-skipper method;
  implement from the algorithm description, no borrowed code.
- [ ] SEG4 (M) **Segment editor**: on the item admin view —
  list segments, add/edit times/delete, "preview from here"
  seek in the player; saves as source=manual.
- [ ] SEG5 (M) **Skip in the player**: Skip Intro / Skip
  Credits button while the playhead is inside a segment
  (web first); per-profile mode in Preferences → Playback
  (UI7): button / auto-skip / off.
- [ ] SEG6 (M) **Autoplay next episode**: playback-info
  gains `next_episode` (reuse the 1.6 next-up query);
  at credits-start or file end, a countdown card (10 s,
  poster + title, Play now / Cancel); on fire: stopped
  report for the old episode + start for the new one, so
  Continue Watching and affinity data stay correct.
  Per-profile toggle (default on), Preferences → Playback.
- [ ] SEG7 (S) **Still-watching prompt** (stretch): after
  3 episodes autoplayed back-to-back, the countdown card
  asks once; resets on any interaction.
- **Done when:** a season with a shared intro gets Skip
  Intro on every episode with no manual work, episodes roll
  into each other hands-free, and turning it off per profile
  actually turns all of it off.

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
- [ ] R6 (1 h, before T11) **Sullivan GPU inventory**: which GPU
  is actually in sullivan (the NVENC session math assumes
  GeForce; an Intel-only box makes QSV *throughput* the limit)
  + a real concurrent-encode benchmark on that hardware
- [x] R7 (5 min, owner, gates N8) **TV OS check — DONE
  2026-11-11: Fire OS 8.1.8.5** (owner read it off the set).
  Not Vega OS: the TV runs Android apps and sideloads APKs.
  N8 (Android TV flavour) is the TV path; N9 (Vega) stays
  parked unless a Vega device is ever owned
- [ ] R8 (2 h, before T16) TV built-in browser viability: does
  the Ember TV's browser run MSE + hls.js at usable speed?
- [ ] R9 (1 h, before T15/N7) Pin hls.js + Media3 versions in
  the lockfiles and re-check every ABR default in T15/N7
  against the *bundled* versions' docs (defaults drift)
