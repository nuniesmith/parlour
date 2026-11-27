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
**2026-11-23:** owner added **Phases 10–12** — native audiobooks
(first slice AB1–AB5 built 2026-11-23), podcasts (plan only),
and the audio-client sync contract (API design only). Spec:
`docs/AUDIOBOOKS-PODCASTS.md`; ten open questions are
**D26–D35** in `docs/DECISIONS.md`, all the owner's to answer.
**2026-11-24:** owner decided **D25** (export bundle scope —
unblocks the XFER chain) and **D26–D35**: no ABS-compat bridge,
Parlour's own web + mobile clients (D26); no ABS import (D27, AB9
cancelled); otherwise mirror Audiobookshelf where it has an
equivalent, and defer what it doesn't until the owner asks
(D28–D35). Audio UI spec from his Audiobooth / Overcast
screenshots: `docs/UI-AUDIO-REFERENCE.md`.

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
- [ ] X2 (L) **CANCELLED 2026-11-24 by D26 — never built** (no
  ABS-compat bridge; Parlour's own clients instead). Original:
  Audiobookshelf surface: libraries/items/progress in
  ABS shape so ABS apps work before Parlour grows native
  audiobook support. **2026-11-23:** native audiobooks are now
  Phase 10; whether X2 is built at all (bridge, permanent, or
  never) is the open **D26**. Nothing ABS-shaped exists today.
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
- [x] UI6 (L) **Admin IA re-org**: the one tabbed `/admin`
  page becomes the sidebar Server tree: Dashboard, General,
  Users (users + invites + access), Library (libraries,
  paths, scans, Emby import), Metadata (review queue),
  Sessions (UI3), Jobs (run-now where supported, error
  detail), Import/Export (XFER8 lands here). All deep-
  linkable under `/admin/...`; old tab URLs redirect.
- [x] UI7 (M) **Preferences IA re-org**: Settings splits into
  Display, Home Screen (rail visibility/order), Playback
  (SEG5/SEG6 prefs live here), Subtitles, Profile, Account.
- [x] UI8 (S) **Home parity bits**: library tile row +
  per-library Latest rails alongside the existing
  Continue Watching / Next Up / Recently Added rails.
- [x] UI9 (S) Search affordances: `/` keyboard shortcut
  focuses search from anywhere; recent searches per profile.
- [x] UI10 (M) **Account self-service**: change-username
  endpoint (case-insensitive unique, current password
  required, existing sessions survive) + Account page UI;
  admins can rename users too.
- [x] UI11 (S) **Visual QA pass**: screenshot checklist
  against `docs/WEBUI-EMBY-NOTES.md` §1 (shell, dashboard,
  session card, rails) at desktop + phone widths; fix
  spacing/anatomy drift; empty states everywhere.
- **Done when:** an Emby user can sit down and find
  everything where Emby taught them to look — dashboard,
  sessions with explained stream decisions, users, libraries,
  jobs — without a tour; Parlour branding throughout.

## Phase 8 — Import/export & sidecar metadata (`server`, `catalog`, `jobs`, `db`, D14)

- [x] XFER1 (M) **Bundle format spec**: versioned
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
  **D25 decided 2026-11-24 — unblocked:** the bundle also
  carries each profile's **recent searches** (next to its
  prefs) and the **Activity feed** (keyed by username /
  stable_key, not ids); hashes may sit in the plain,
  unencrypted zip (the admin confirm + warning stays). Audio
  tables weren't in D25: propose `book_progress` under
  `user_data` in the spec and get the owner's yes.
  *Done 2026-11-24, per D14/D25:* spec `docs/IMPORT-EXPORT.md`;
  format in `crates/server/src/bundle/` — the six documents'
  types, `BundleWriter` / `BundleReader` (plain zip, documents
  deflated, optional `artwork/<sha256>.<ext>` stored and
  hash-checked) and `Bundle::problems()` (unique keys, the
  database's CHECKs, every reference between documents
  resolved; writer and reader refuse an inconsistent bundle).
  Strict: unknown fields are errors, and the format version is
  checked before anything else is parsed. Hashes are all or
  nothing: absent unless confirmed, PIN hashes in `users.json`
  by profile name, `has_pin` always. Activity `detail` travels
  verbatim (sign-in IPs and old ids included). `coverage.rs`
  places every column of the 26 tables — 13 carried — and the
  ignored `bundle_schema_flow` checks it, and the mirrored
  CHECK sets, against a freshly migrated database. Fixtures:
  `crates/server/tests/fixtures/bundle-v1/`. **Still open:**
  the audio proposal (`book_progress` under `user_data.json`
  as `books[]`, `book_chapters` not carried) awaits the
  owner's yes — format v1 carries neither until then; eleven
  readings of D14/D25 are listed for veto in the spec's §10.
- [x] XFER2 (L) **Export**: admin endpoint + background job
  producing the bundle; optional artwork inclusion (files
  under `artwork/` in the zip, off by default with a size
  warning); download from the Jobs UI.
  *Done 2026-11-24, per D14/D25, format v1 as it stands (no
  audio tables — the §9 proposal is still open):*
  `crates/server/src/export/`. `POST /api/v1/admin/export`
  (`include_hashes` = the explicit confirm, `include_artwork`;
  both off by default) queues an `export_bundle` job. The job
  reads everything in one `REPEATABLE READ` snapshot, writes in
  the spec's fixed order, and keeps the zip in
  `<cache>/exports/`. There is also an estimate route for the
  size warning, plus download and delete routes. Jobs page:
  "Export the server" (artwork opt-in with its size, no hash
  option — XFER8's page owns that warning), and Download / Delete
  on the finished row. Exports land in the Activity feed. Four
  readings for veto are in `docs/IMPORT-EXPORT.md` §11; the first
  is that a stale duplicate image row is dropped rather than
  failing the export.
- [x] XFER3 (L) **Import**: upload → parse → **dry-run
  report** (creates / updates / conflicts, matched by
  username, library name, item stable_key/provider ids) →
  apply as a job. Idempotent; import never deletes anything
  by default; conflicting usernames are reported, not merged.
  *Done 2026-11-25, per D14/D25/D37/D38, format v1 as it
  stands (no audio tables — the §9 proposal is still open):*
  `crates/server/src/import/`. `POST /api/v1/admin/import`
  streams the zip to `<cache>/imports/` (4 GiB cap; each
  document 512 MiB unpacked, now checked by `BundleReader`),
  opens it with `BundleReader` and answers with a dry-run
  report; `…/{id}/dry-run` re-runs it with the admin's
  replacement library folders; `…/{id}/apply` queues an
  `import_bundle` job (own worker, like the export's);
  `DELETE …/{id}` drops an upload. Dry run and apply are one
  walk in one transaction (rolled back / committed), in the
  spec's insert order, so the report is what the apply does.
  Never deletes; rows with a clock change only when the
  bundle's copy is newer, rows without one are only added;
  re-import changes nothing, Activity included. Users match by
  username + `created_at` (same account) or conflict, except
  D38: a pristine bootstrap admin is adopted by the bundle's
  admin. D37: hash-less users (and PIN'd profiles) are created
  with a locked hash; new `PUT /api/v1/admin/users/{id}/password`
  plus "Set password" / "No password" on Server → Users. Items
  match on key, then a unique provider id; new items arrive
  unavailable until a scan. Artwork is extracted only by the
  reader's names, hash-checked, into the artwork cache. Eleven
  readings for veto are in `docs/IMPORT-EXPORT.md` §12.
- [x] XFER4 (L) **Sidecar metadata with the media files**:
  per-file `<name>.parlour.json` (series: folder-level
  `series.parlour.json` + per-episode files), versioned,
  holding item metadata (titles, year, provider ids, genres,
  overview, rating, cast, segments). Scanner reads sidecars
  **before** provider lookup and treats them as
  authoritative for those fields; a metadata job rewrites
  sidecars when matched metadata changes. Atomic writes
  (temp + rename).
  *Done 2026-11-25, per D14 (audio proposal §9 / D36 still open —
  sidecars carry no per-user state either way):* spec
  `docs/IMPORT-EXPORT.md` §13, format `parlour-sidecar` v1 (strict:
  version first, unknown fields are errors; present = authoritative,
  absent = from the provider). Code `crates/catalog/src/sidecar/`.
  `<video stem>.parlour.json` per movie/episode file,
  `series.parlour.json` in the series folder. The scan applies new
  or changed sidecars (fingerprinted in the new `item_sidecars`) and
  re-applies them when a video is re-read; the matcher's new rule 0
  fetches the sidecar's provider id, and every automatic match puts
  the sidecar's fields back over the provider's. An admin's
  fix-match wins over the sidecar (rewrites it, or marks it
  overridden until the file changes). The `fetch_metadata` job (and
  manual apply) rewrites matched items' sidecars only where
  `libraries.write_sidecars` is on — a new column, **default off,
  with no API or UI yet** (XFER5 turns it on); byte-identical files
  are left alone, writes are temp + rename. Segments: manual rows
  only, tied to the video's byte size. Neither the column nor
  `item_sidecars` is in the bundle (format v1 unchanged). Twelve
  readings for veto are in §13.9.
- [x] XFER5 (M) **Sidecar write opt-in**: per-library
  `write_sidecars` flag (default off — media mounts are
  read-only today); when on, XFER4 writes + optionally
  copies poster/backdrop as `poster.jpg` / `backdrop.jpg`
  next to the media. The flag + mount change is an explicit
  ops step, documented in the task PR.
  *Done 2026-11-26, per D14 (audio proposal §9 / D36 still open —
  untouched):* spec `docs/IMPORT-EXPORT.md` §13.10–§13.12. The flag
  changes only through `PUT /api/v1/admin/libraries/{id}/write-sidecars`
  (admins; `movies` / `shows` libraries; logged in Activity under
  "Library settings"). Creating or editing a library with the field
  is refused, and an import never sets it. The admin library view
  reports each root folder's writability (`access(2)`, nothing
  written). **Server → Library** has a per-library card with the
  state, the folders, and a confirmed "Let Parlour write here…", plus
  a library-wide **Download artwork** action (new
  `POST …/libraries/{id}/artwork`). Nothing is written until the flag
  is on **and** the folder is mounted writable. The mount step (a
  writable bind mount for just that library's folder, the container
  user, recreate) is in §13.11. A read-only mount fails cleanly: the
  job lists each path and still succeeds, and a warning notice stays
  up until the folder can be written or the flag is off. The artwork
  step (`download_item_artwork`) copies a matched movie's or series'
  cached JPEG poster and backdrop as `poster.jpg` / `backdrop.jpg`
  into folders that belong to that item alone (never a library root
  or a shared folder). Writes are atomic, identical files are left
  alone, and a file Parlour didn't write is never replaced (new
  `sidecar_artwork` table, LeftOut of the bundle). Parlour's own
  copies refresh when the cache changes and are never deleted. Ten
  readings for veto are in §13.12.
- [x] XFER6 (S) **NFO interop**: scanner also reads
  Kodi/Emby-style `.nfo` (ids + title) as an input source;
  XFER4 can optionally write `.nfo` alongside the JSON.
  *Done 2026-11-25, per D14 (audio proposal §9 / D36 still open —
  untouched; an NFO holds no per-user state):* spec
  `docs/IMPORT-EXPORT.md` §13.13–§13.14. Code
  `crates/catalog/src/sidecar/nfo.rs` (streaming `quick-xml` reader,
  deterministic writer) and `nfo_sync.rs`. **Read:** `<stem>.nfo`
  beside each movie/episode video, `movie.nfo` only in a folder that
  belongs to one movie, `tvshow.nfo` in each series folder;
  `season.nfo` ignored. Only the root's direct `<title>` and ids
  (`<uniqueid type=tmdb|tvdb|imdb>`, `<tmdbid>`, `<tvdbid>`,
  `<imdbid>`) are read; `<season>` / `<episode>` are checks;
  year, plot and the rest are deliberately not read. The scanner's
  NFO pass runs after the sidecar pass (fingerprinted in the new
  `item_nfos`, `nfos_applied` in the scan report). A bad NFO is listed
  on the scan job and the scan carries on. **Precedence:** fix-match >
  sidecar > provider > NFO > file name; the matcher's new rule 0b
  fetches the NFO's id (after the sidecar's rule 0, before the file
  name's rule 1). The sidecar beats the NFO field by field without a
  report, and a fix-match overrides the NFO until its file changes.
  **Write:** under the same `write_sidecars` opt-in (no switch of its
  own), same scope, atomic, identical bytes left alone. The written
  NFO holds only title, year (or season / episode) and ids. Each
  write is recorded by SHA-256 in the new `sidecar_nfos`, so an NFO
  another program wrote is never replaced (listed as
  `nfos_left_alone`). Parlour's own NFO isn't re-read as new input.
  Both tables are LeftOut of the bundle (format v1 unchanged).
  Eleven readings for veto are in §13.14.
- [x] XFER7 (M) **Automatic backups**: nightly export job to
  a configurable folder with retention (ties into the
  Cross-cutting backups item); restore = XFER3.
  *Done 2026-11-25, per D14/D25, format v1 as it stands (no audio
  tables — the §9 proposal / D36 is still open):* spec
  `docs/IMPORT-EXPORT.md` §15; code `crates/server/src/backup.rs`
  and `run_backup_job` in `export/mod.rs`. The existing nightly tick
  (after 07:30 UTC) queues one `export_bundle` job a day with
  `trigger: nightly`; the export worker runs the same export as a
  manual one into `PARLOUR_BACKUP_DIR` (absolute; **unset = no
  backups**) as `parlour-backup-YYYYMMDD-HHMMSS.zip` (UTC), written
  as `.part` and renamed. Then retention keeps the newest
  `PARLOUR_BACKUP_KEEP` (1–365, default 7) by name, deleting only
  files named exactly like a nightly bundle (and stale `.part`s),
  only after a successful write. A missing folder (never created:
  an unmounted disk), a non-folder or an unwritable one fails the
  job with the path; the next night retries (one try per UTC day).
  Surfaced on the Jobs page ("Nightly backup": Download, no Delete,
  "Removed by retention"), as a "Nightly backup failed" Dashboard
  notice, in the Activity feed, and the folder joins the D16
  free-space check. Server → General shows the setting. **No hashes
  and no artwork** (D14's confirm is per export): a restored user is
  locked until an admin sets a password (D37). **Not a complete
  host-move story on its own:** no logins, no audiobook progress
  (D36) — the Postgres dump in the Cross-cutting backups item covers
  those and stays production work. Seven readings for veto are in
  §15.
- [x] XFER8 (M) **Admin UI**: Server → Import/Export page —
  export (options + download), import (upload, dry-run
  report render, apply), per-library sidecar status.
  *Done 2026-11-25, per D14/D25/D37/D38 and the owner's new **D39**
  (backups are set up in the web UI), format v1 as it stands (no audio
  tables — the §9 proposal / D36 is still open):* spec
  `docs/IMPORT-EXPORT.md` §16. `/admin/import-export` has four parts.
  **Export:** artwork (with its size estimate) and password/PIN hashes
  behind the §6 warning plus a confirm; the server's bundles with
  Download / Delete. **Import:** upload → the dry run rendered from
  XFER3's report (`web/src/lib/ImportReport.svelte`), with D38's
  take-over and D37's no-password line in a "Before you apply" box,
  accounts, libraries (with replacement folders and "Check again"),
  every tally, conflicts and notes → Apply (confirmed) → the job's
  report under Recent imports; waiting uploads are listed (new
  `GET /api/v1/admin/import`). **Nightly backups (D39):** folder, keep
  and hashes, stored in the new `backup_settings` table
  (`GET`/`PUT`/`DELETE /api/v1/admin/backup`); the env vars are the
  bootstrap default until a save; a saved blank folder is off; read
  fresh, no restart; hashes off by default and only with the warning;
  every change audited. Nothing is enabled on any host. **Sidecar
  status:** new `GET /api/v1/admin/sidecars` per library — XFER5's
  opt-in, folder writability and notice, and the counts the ledgers
  already keep; the switch stays on Server → Library. Jobs page: the
  export form moved here (finished rows keep Download / Delete);
  General shows backups read-only and links here. New `kind` filter on
  `GET /api/v1/admin/jobs`. Twelve readings for veto are in §16.7.
  **Phase 8 "Done when": passed end to end for the bundle half** —
  `tests/import_export_page_flow.rs` exports a seeded server from the
  page's endpoints, imports it into a freshly created database through
  upload → dry run → apply, and the new server exports the same users,
  profiles and watch history (old passwords work). The sidecar half
  (copying a movie folder carries its metadata) was shown by XFER4's
  `sidecar_flow` and still passes; both on a throwaway Postgres, not
  the live instance or a real host move.
- **Done when:** the whole server state survives a host
  move — bundle out on oryx, bundle in on a fresh install,
  same users/profiles/watch history — and copying a movie
  folder to a new drive carries its Parlour metadata with it.

## Phase 9 — Segments: intro/credits detection, skip, autoplay next (`catalog`, `playback`, `web`, D15)

- [x] SEG1 (M) **`media_segments`**: migration + types —
  (item_id, kind intro/credits/recap, start_ms, end_ms,
  source chapters/fingerprint/manual, confidence);
  playback-info returns the file's segments; manual source
  always wins over auto.
  *Done 2026-11-15:* one row per (item, kind, source) so a
  detector re-run never touches a manual row; playback-info
  `segments` = at most one per kind, manual first, else
  highest confidence (tie → chapters before fingerprint).
- [x] SEG2 (M) **Cheap detection pass**: `DetectSegments`
  job — ffprobe chapters named intro/opening/credits, plus
  black-frame + silence heuristics at head/tail. Runs on
  scan for series episodes; fills SEG1 with source=chapters
  or a low-confidence heuristic row.
  *Done 2026-11-15:* a scan queues `detect_segments` when an
  episode's playback file is new or changed (per-file
  `segments_detected_at`, cleared by the scanner); own
  worker. Chapters at 1.0; otherwise black + silence per
  D17/D18, stored as source `heuristic` at 0.5; tie-breaks
  per D19. The 0.6 floor in `effective_segments` keeps
  every guess away from the player; manual rows are exempt.
- [x] SEG3 (L) **Audio-fingerprint detection**: fingerprint
  each episode's audio (ffmpeg chromaprint) once, cache by
  file hash; cross-correlate episodes within a season —
  the long common segment inside the first ~25% is the
  intro; a common tail segment is the credits. Confidence
  from match length/quality; below threshold → no segment,
  never a guess. This is the Jellyfin intro-skipper method;
  implement from the algorithm description, no borrowed code.
  *Done 2026-11-15, per D20* (confidence = quality ×
  agreement; length is a gate): `parlour_catalog::fingerprint`,
  run by `detect_segments` after SEG2's pass, season by
  season. Head (first 25 %, ≤ 10 min) and tail (last 600 s)
  prints cached in `audio_fingerprints` by SHA-256 of size +
  first/last 4 MiB; every pair of a season's episodes tried
  at every alignment; each episode keeps what most of the
  season shares, median edges; stored as source
  `fingerprint` ≥ 0.6 only where chapters left the kind
  open. Migration resets `segments_detected_at` once so
  existing seasons get matched. Evidence is synthetic
  only — no real episodes on oryx yet.
- [x] SEG4 (M) **Segment editor**: on the item admin view —
  list segments, add/edit times/delete, "preview from here"
  seek in the player; saves as source=manual.
  *Done 2026-11-19, per D21:* an admin-only Segments section
  on `/item/[id]` for anything with a playable file, movies
  included, backed by `/api/v1/admin/items/{id}/segments`.
  Lists every stored row with what it means for playback;
  the admin's times save as the kind's `manual` row (1.0),
  checked only for 0 ≤ start < end ≤ the file's length.
  Deleting a detected row stores a manual "no segment of
  this kind here" marker (a manual row with no times,
  migration `20261119000000`) that `effective_segments`
  honours over every detector and every re-run; deleting a
  manual row — times or marker — hands the kind back to
  detection. "Preview" opens `/watch/[id]?preview=<ms>`:
  the player starts there and sends no playback reports.
- [x] SEG5 (M) **Skip in the player**: Skip Intro / Skip
  Credits button while the playhead is inside a segment
  (web first); per-profile mode in Preferences → Playback
  (UI7): button / auto-skip / off.
  *Done 2026-11-19, per D22:* playback-info carries the
  active profile's `segment_skip`, read fresh every time.
  The web player (`lib/skip.ts`) offers Skip Recap / Intro /
  Credits while the playhead is inside one; in auto mode it
  skips each segment once — the first time playback reaches
  it, starting inside it included — says "Skipped the
  intro", and offers the button on the way back in. A skip
  lands on the segment's end; one ending within 1 s of the
  file's end ends playback there, with the usual stopped
  report (so the item counts as played). A preview never
  skips by itself and always offers the button. F and a new
  button put the whole player area in full screen;
  Chromium's own full-screen button is hidden (in Chrome
  151 `controlslist` alone only greys it out), while Firefox
  and Safari keep theirs.
- [x] SEG6 (M) **Autoplay next episode**: playback-info
  gains `next_episode` (reuse the 1.6 next-up query);
  at credits-start or file end, a countdown card (10 s,
  poster + title, Play now / Cancel); on fire: stopped
  report for the old episode + start for the new one, so
  Continue Watching and affinity data stay correct.
  Per-profile toggle (default on), Preferences → Playback.
  *Done 2026-11-19, per D23:* playback-info carries
  `next_episode` — the next episode in series order
  (season, then episode, into later seasons), played or
  not, skipping any with no available file; not the
  next-up query, whose unplayed-only filter would stop a
  rewatch — shaped like a Next Up row, plus the profile's
  `autoplay_next`, read fresh every time. The web player
  (`lib/autoplay.ts`, `lib/NextEpisodeCard.svelte`) puts
  the card up when the credits begin, or at the end of
  the file, and counts 10 s while the video plays or has
  ended, waiting while it is paused; the card stands in
  for Skip Credits and the automatic skip. Play now, or
  the count running out, rolls into the next episode in
  the same player, so full screen stays and the URL is
  replaced without a new history entry: the old
  episode's stopped report (at the end of the file when
  the credits run there, so it counts as played; else at
  the playhead), then the next one's start report from
  its resume point — a played one from 0 — with no
  resume prompt. Cancel ends autoplay for that viewing
  and gives the credits the Skip button whatever the
  mode. A preview gets no card.
- [x] SEG7 (S) **Still-watching prompt** (stretch): after
  3 episodes autoplayed back-to-back, the countdown card
  asks once; resets on any interaction.
  *Done 2026-11-20, per D24:* the web player counts the
  rolls its countdown fired (`lib/stillWatching.ts`). After
  three in a row with no interaction — the episode the
  viewer started plus three autoplayed ones — the card at
  the end of the fourth asks "Still watching?" instead of
  counting down: Keep watching plays the next episode now,
  Stop does what Cancel does. It waits as long as it takes
  while the episode plays out to its end and the usual
  stopped report, and nothing else plays. Any key press,
  click, tap or wheel turn on the page, or a play, pause,
  seek or volume change the player didn't make itself,
  starts the count over and turns an asking card back into
  the 10 s countdown. Play now, Keep watching and Stop count
  as interactions too, so their rolls don't count. The
  player's own actions — the roll's pause and play, the
  automatic skips, the resume seek and hls.js's own seeks —
  don't count, and neither does moving the pointer. The
  count lives only in the open player page: a roll keeps
  it, while a reload or another item starts it over.
  Nothing is stored on the server, there is no new
  preference, and a preview never asks.
- **Done when:** a season with a shared intro gets Skip
  Intro on every episode with no manual work, episodes roll
  into each other hands-free, and turning it off per profile
  actually turns all of it off.

---

## Phase 10 — Native audiobooks (`catalog`, `server`, `web`, D26–D30 decided 2026-11-24)

Spec: `docs/AUDIOBOOKS-PODCASTS.md`. Audiobooks become a
first-class library in the shared catalog (ARCHITECTURE §7/§10),
not an Audiobookshelf-compat story. AB1–AB5 are the first PR;
the rest wait on the owner's decisions.

- [x] AB1 (M) **Schema**: item kind `audiobook`;
  `media_files.track_index`; `book_chapters` (book-global
  times); `book_progress` per profile + book (position,
  duration, finished, finished_at, last_listened_at). The
  progress table and scope are provisional until **D29**.
  *Done 2026-11-23:* migration `20261123000000_audiobooks.sql`.
- [x] AB2 (M) **Scanner for audiobooks libraries**: audio
  allowlist; one book per folder holding audio, disc folders
  (`CD 1`, `Disc 2`, `Part 3`; *AB11: not `Part`*) folded into their parent, loose
  root files one book each; track order disc tag → disc folder → track tag →
  natural file name; ffprobe tags → title / authors /
  narrators / year / overview / genres; chapters from the files
  or one per track; `cover.jpg`/`folder.jpg` as the poster.
  Idempotent, missing-marking as for video (C8).
  *Done 2026-11-23:* `parlour_catalog::audiobooks`; `scan_library`
  hands audiobooks libraries to it. A book with any new, changed or
  missing file is re-probed and rebuilt whole; unchanged books are
  skipped unprobed. Covers are `images` rows (variant `file`)
  pointing into the media folder, served by the existing artwork
  endpoint.
- [x] AB3 (M) **Native API**: `GET /api/v1/audiobooks` (list,
  progress inline), `GET /api/v1/audiobooks/{id}` (tracks with
  offsets + tokenised stream URLs, chapters),
  `GET …/tracks/{index}/stream` with HTTP Range.
  *Done 2026-11-23:* `crates/server/src/audiobooks.rs`. Stream URLs
  reuse `/api/v1/stream/{file_id}` with its 6 h HMAC token.
- [x] AB4 (S) **Progress API**: `GET|PUT|DELETE
  /api/v1/audiobooks/{id}/progress`, bulk
  `GET /api/v1/audiobooks/progress?since=`. Book-global
  position, clamped to the book's length; finished only when
  a client says so; last write wins (D29, D33 open).
  *Done 2026-11-23:* per profile, own table, explicit finished, last
  write wins. All four are provisional until D29/D33.
  *D29/D33 decided 2026-11-24 (mirror ABS): per profile and
  own table stay; auto-finish, the hide flag and client
  timestamps are AB11.*
- [x] AB5 (M) **Minimal web**: an audiobooks library lists
  books; `/book/[id]` plays track after track in `<audio>`, with
  chapters, resume, and progress writes; `audiobook` items
  elsewhere link there.
  *Done 2026-11-23:* `web/src/lib/BookShelf.svelte`,
  `web/src/routes/book/[id]`. Checked in headless Chromium against
  a private instance.
- [x] AB6 (M) **Metadata providers + embedded covers** —
  after **D30**. *D30 (2026-11-24): mirror ABS — per-library
  precedence list, manual match (Google Books, Open Library;
  Audnexus after a terms check), embedded covers when no image
  file.*
  *Done 2026-11-26:* `libraries.metadata_precedence` (default
  folder → tags → nfo → txt → opf → abs_json → sidecar, later
  wins; edited per library on Server → Library); tags from the
  first track only; folder authors split ABS's way; readers for
  ABS-style `.nfo`, `desc.txt` / `reader.txt`, `.opf`, ABS's
  `metadata.json` and `book.parlour.json` (`book_meta.rs`).
  Manual match per book (`/book/[id]`, admin; Google Books or
  Open Library; the admin picks overwrite or fill gaps) and a
  library-wide quick match (Google Books, fills gaps only, ABS's
  confirmed default). The Google key is optional and travels
  only in a header. Audnexus is left out: no published terms
  permit it. Embedded covers go into the artwork cache when the
  folder has no image. A match is kept server-side
  (`book_sidecars`) and written beside the book only under
  `write_sidecars` (XFER5). Spec: AUDIOBOOKS-PODCASTS.md
  "Metadata sources, matching and covers (AB6)" (12 readings for
  veto); IMPORT-EXPORT.md §13.15. **D40** (does a match travel in
  the bundle?) and D36 are open.
- [x] AB7 (M) **Series + author browsing** — after **D28**.
  *D28 (2026-11-24): mirror ABS — `series` records + a book ↔
  series join with a sequence; series from the
  `Author/Series/Title` folder.*
  *Done 2026-11-26:* `series` (one per library + name, case-blind)
  and `book_series` (sequence on the link; a book can sit in several
  series), migration `20261130000000_series.sql`. A book's series
  are part of AB6's merge: the `folder` source gives the series
  folder (no sequence until AB11's title-folder parsing), the `tags`
  source ABS's series tags (`series`/`series-part` and their
  fallbacks, `grouping` as `Name #n; …`); a later source replaces
  the list whole; links are rebuilt with the book and empty series
  removed after each scan, so re-scans duplicate nothing. Authors
  are the existing `item_people` rows (no migration). API
  `GET /api/v1/series[/{id}]`, `GET /api/v1/authors[/{id}]`; books
  gain `series` + `author_refs`. Web: Books / Series / Authors views
  on an audiobooks library, `/series/[id]` (sequence order),
  `/author/[id]`, linked from `/book/[id]`. Follow-up item 7
  decided: narrators stay `people` rows. Both new tables stay out of
  the bundle (rebuilt by a scan; format v1 unchanged). Spec:
  AUDIOBOOKS-PODCASTS.md "Series and authors (AB7)" (12 readings for
  veto). D36 and D40 are still open.
- [x] AB11 (M) **Align AB1/AB2/AB4 with ABS** (D28/D29): the
  numbered "Follow-up from D28–D35" list in
  `docs/AUDIOBOOKS-PODCASTS.md` — disc folders without `Part`,
  title-folder parsing, nearest-folder author, author
  splitting (folder names done in AB6), auto-finish under 10 s
  remaining, the
  hide-from-Continue flag, client timestamps.
  *Done 2026-11-26 (follow-up items 1, 3, 4, 8, 9, 10; item 5 needed
  nothing more):* disc folders are only `CD` / `Disc` / `Disk` +
  optional whitespace + 1–3 digits, so `Part 1` (and `Disc-1`) is a
  book of its own — its identity moves; the library on oryx holds no
  audiobooks, so the owner's real library wasn't checked. The `folder`
  source parses the title folder (ASIN `[…]`, narrator `{…}`, the
  series sequence when there is a series folder, a leading year, and a
  subtitle behind a per-library "parse subtitles" setting, off) and
  takes the nearest author folder; a folder series now has its
  sequence. Merge rules `ab11.1` (one re-merge). Per-library
  `finish_remaining_seconds` (10) / `finish_percent` (off) on
  `GET|PUT /api/v1/admin/libraries/{id}/audiobook-settings`, edited on
  Server → Library. Progress writes follow ABS: auto-finish at the
  threshold, a finished book moved back is unfinished, un-finishing by
  hand resets to 0, `hidden` cleared by the next move, the client's
  `client_updated_at` stored (last arrival still wins; `updated_at`
  stays the sync cursor). Migration
  `20261201000000_ab11_align_abs.sql`; the new columns stay out of the
  bundle (`book_progress` stays D36's open proposal). Spec:
  AUDIOBOOKS-PODCASTS.md "Aligning with Audiobookshelf (AB11)" (12
  readings for veto).
- [x] AB8 (S) **Listening in Now Playing / Activity, Home
  "Continue listening"** — after **D29** / **D35**. *D29
  (2026-11-24): ABS's shelves (Continue Listening with a
  per-item hide, Continue Series, Listen Again, …); listens are
  sessions, no `play_count` bump.*
  *Done 2026-11-26 (follow-up item 13):* a listen is a session in the
  new `listening_sessions` (per profile; migration
  `20261202000000_ab8_listening_sessions.sql`). The web player opens,
  beats and closes it through the existing
  `POST /api/v1/playback/{start,progress,stopped}`, which branch on an
  audiobook: no `user_data` row, no `play_count`; listening time is the
  player's measure capped by the clock; a new start on the same book
  replaces an open session, an hour's silence closes one (15-minute
  tick), and a report on a closed one opens a fresh one. Open sessions
  sit in the live registry, so **Now Playing** shows them and its Stop
  works (every track's token revoked); closed ones write a
  `playback.listened` **Activity** line. Per-profile
  `GET /api/v1/listening/sessions` and `…/stats?tz=` (Preferences →
  Listening), admin `GET /api/v1/admin/listening/sessions` (under Now
  Playing on Server → Sessions). `GET /api/v1/audiobooks/home` gives
  ABS's seven shelves (Continue Listening, Continue Series, Recently
  Added, Recent Series, Discover, Listen Again, Newest Authors, as ABS's
  queries have them); they are Home Screen rails on Home (Continue
  Listening can't be hidden) and the shelves of an audiobooks library's
  new default **Home** view. A Continue Listening card has the hide
  control and Mark as finished; a hidden book's page offers "Show it
  there again". `listening_sessions` is *Proposed* in the bundle (D36
  still open; format v1 unchanged). Spec: AUDIOBOOKS-PODCASTS.md
  "Listening sessions and Home shelves (AB8)" (13 readings for veto).
- [ ] AB9 (M) **CANCELLED 2026-11-24 by D27 — not to be
  built.** Original: importer from the owner's Audiobookshelf
  (progress + finished) — only if **D27** says yes.
- [x] AB10 (S) **Transcode fallback** for book codecs a
  client profile can't play (AAC), using CL5's profiles.
  *Done 2026-11-26:* audio profiles in `parlour_playback`
  (`AudioProfile`, `decide_audio`): `web` (browsers: MP3, AAC in MP4,
  16/8-bit WAV), `android` for phones and Fire TV (ExoPlayer's platform
  decoders: adds FLAC and Ogg Opus/Vorbis/FLAC) and `ios`/`tvos`
  (AVPlayer: adds ALAC and FLAC), as container + codec pairs, named by
  the client, never sniffed. `GET /api/v1/audiobooks/{id}?client=`
  gives each track a `plan`, and a track the profile can't play gets
  `GET /api/v1/stream/{file}/aac.m4a`: AAC in MP4 (ADTS AAC copied,
  everything else converted), made once per track by the runner the
  video remux now shares, kept and stamped with its source, index first
  so Range seeks it, 503 + `Retry-After` until ready, same stream token
  (an admin stop revokes it). Without `client` the answer is AB3's. The
  book detail starts the resume track's copy; the web book page names
  `web`, waits for a copy with a visible note (seeks kept), asks for the
  next track's ahead, and says why a track is converted; Now Playing
  shows a book's plan. Sessions, progress writes, auto-finish, the hide
  flag and the shelves are unchanged. No migration; bundle untouched.
  First play of a converted track waits ~40 s per hour of audio (oryx,
  ffmpeg 5.1). Spec: AUDIOBOOKS-PODCASTS.md "Transcode fallback and
  audio profiles (AB10)" (13 readings for veto).
- **Done when:** the owner's real audiobook folders scan into
  Parlour with sensible titles, authors and chapters; a book
  plays in the web UI; and a position written by one client is
  where another client resumes.

## Phase 11 — Podcasts (`integrations`/`catalog`, `jobs`, `server`, `web`; D31–D32 decided 2026-11-24)

**2026-11-24:** D31/D32 mirror Audiobookshelf's podcast model —
podcasts are a library reached by the normal access grants (no
per-profile subscriptions), admin-only adding, server downloads
episodes (an undownloaded one streams by redirect to its enclosure —
D32 as amended 2026-11-27), per-show auto-download (off) /
hourly check / keep N (0 = all) / 3 new per check. Details:
`docs/AUDIOBOOKS-PODCASTS.md` → Phase 11. The lines below keep
their original wording; read them through that.

POD1 (the schema), POD2 (feed checks), POD3 (downloads and
retention), POD4 (managing feeds), POD5 (the listener API), POD6
(the web pages and player) and POD7 (directory search) are built;
POD8 is plan only. Spec: `docs/AUDIOBOOKS-PODCASTS.md`.

- [x] POD1 (M) **Schema**: `podcasts` (feed URL, metadata,
  ETag/Last-Modified, refresh state), `podcast_episodes`
  (guid, enclosure, published, duration, local file),
  subscriptions shaped by **D31**, episode progress in the
  same shape as `book_progress`.
  *Done 2026-11-26:* migration `20261203000000_pod1_podcasts.sql`.
  `podcasts` is one row per feed in a podcasts library. It holds:
  - the feed URL, unique per library;
  - the show as the feed describes it, plus a cached artwork path;
  - `folder_path`, the show's folder inside the library's writable
    store (D32; no store created, no mount changed);
  - the ETag / Last-Modified validators;
  - the refresh state (last checked, last error, failures in a row);
  - D32's per-show settings as columns, with ABS's defaults:
    auto-download off, `0 * * * *`, keep 0 = all, 3 new per check.

  As in ABS, the schedule runs only while auto-download is on, and
  that flag is what the 24-failure rule turns off.
  `podcast_episodes` is one row per feed item with an enclosure. It
  holds:
  - the guid, unique per show, with the enclosure URL standing in when
    there is none;
  - the enclosure (URL, type, length), published time and the feed's
    duration;
  - season, episode and type, and the Podcasting 2.0 chapter and
    transcript links;
  - the local copy's path, size, probed length, container and codec,
    all NULL until POD3 downloads it.

  D31: there is no subscriptions table. A profile reaches shows only
  through its `library_access` grant on the podcasts library. D29:
  episode progress shares `book_progress`:
  - a nullable `episode_id` beside `item_id`, exactly one set;
  - each unique per profile (the primary key became `UNIQUE
    (profile_id, item_id)`, so book queries are unchanged);
  - AB11's write rules apply as they are.

  There is no API and no UI (POD5/POD6). Both new tables are left out
  of the bundle, which stays at format v1. Episode progress follows
  `book_progress`'s open proposal (D36). Flow:
  `podcast_schema_flow.rs`. Spec: AUDIOBOOKS-PODCASTS.md "Schema
  (POD1)" (12 readings for veto).
- [x] POD2 (M) **Feed fetch + parse**: RSS 2.0 + iTunes
  namespace (+ Podcasting 2.0 chapter/transcript links), a pure
  fixture-tested parser, conditional GET, a `refresh_podcasts`
  job on the Postgres queue at **D32**'s cadence, and a
  Dashboard notice for feeds failing repeatedly.
  *Done 2026-11-26:* no migration (every column is POD1's).
  - **Parser.** `parlour_catalog::podcast_feed` is pure and
    fixture-tested: an episodic feed, a serial feed, odd items and
    an HTML page, in `crates/catalog/tests/fixtures/podcasts/`.
  - **Check.** `parlour_catalog::podcasts` does the conditional GET
    (ETag / Last-Modified; a 304 only stamps the check), stores the
    show and a row per feed episode (matched by guid, then enclosure
    URL; nothing downloaded), and counts failures. The 24th failure
    in a row turns `auto_download` off; a success clears the count.
  - **Schedule.** A once-a-minute tick queues one `refresh_podcasts`
    job when a show with `auto_download` on is due at its five-field
    cron schedule (`parlour_jobs::cron`, UTC). The job has its own
    worker, and a job naming one show (`podcast_id`) checks it
    whatever its schedule says.
  - **Notice.** A Dashboard notice per show from the 3rd failure in
    a row, with the count; an error from the 24th; cleared after a
    success.

  No API, no UI, no download. The bundle is unchanged (format v1,
  D36 open). Flow: `podcast_refresh_flow.rs`. Spec:
  AUDIOBOOKS-PODCASTS.md "Feed fetch and parse (POD2)" (13 readings
  for veto).
- [x] POD3 (M) **Episode download + retention** per **D32**,
  into a writable podcast store (never a media mount).
  *Done 2026-11-26:* no migration (every column is POD1's).
  - **Store.** The podcasts library's folder is the store; Parlour
    never creates it. A store overlapping another library's folder is
    refused. A show's folder is named from its title the first time
    something is downloaded into it, unique in the store. On oryx the
    store is a `parlour-podcasts` volume at `/podcasts` in
    `docker-compose.yml`. Sullivan's disk is still to be decided
    (D32), before POD3 runs in production.
  - **Download.** It rides on the `refresh_podcasts` run, after a
    successful check of a show with `auto_download` on. New episodes
    are as in ABS (published after the newest download, else the
    previous check), newest first, at most
    `max_new_episodes_per_check`. Each goes to a temporary file, is
    probed with ffprobe (`probe_audio`) and is renamed into place;
    only then are the `file_*` columns and `downloaded_at` filled.
    A failure leaves the row undownloaded and no file, and is listed
    on the job by show and episode title, never by URL.
  - **By hand.** A job `{"episode_id": …}` downloads one episode
    whatever the show's settings (POD4's hook).
  - **Retention.** The oldest dated download over
    `max_episodes_to_keep` loses its file, one per check. The row
    stays, "not downloaded", with its progress (reading 1).
  - **Scans.** Podcasts libraries are never scanned, so the store's
    files aren't catalogued as videos.

  No API, no UI. The job is now labelled "Podcast check and download".
  The bundle is unchanged (format v1, D36 open). Flow:
  `podcast_download_flow.rs`. Spec: AUDIOBOOKS-PODCASTS.md "Episode
  download and retention (POD3)" (12 readings for veto).
- [x] POD4 (M) **Manage feeds in the UI**: add by URL, OPML
  import/export, refresh now, per-feed settings; who may add
  is **D31**.
  *Done 2026-11-26:* no migration (every column is POD1's).
  - **Who.** Admins only (D31): every route is under
    `/api/v1/admin/` and is a 404 for anyone else. There are no
    subscriptions.
  - **Add by URL.** The feed is fetched and read in the request. An
    address that isn't an http(s) feed is refused with a plain error
    that never repeats the address (a paid feed's token), and nothing
    is stored. A duplicate in the library is a 409. A feed that reads
    is stored together with its first check, so the show is never
    empty. Nothing is downloaded, and the folder is still named at the
    first download (POD3).
  - **OPML.** Import adds each feed the same way, four at a time. It
    skips feeds already in the library and repeats in the file, and
    reports added / skipped / not added by name, never by address.
    Export downloads the library's shows as OPML 2.0, which imports
    back (round trip).
  - **Refresh now / Download.** They queue POD2's `{"podcast_id"}`
    and POD3's `{"episode_id"}` jobs, and a second click returns the
    waiting job. An episode that already has a file is a 409.
  - **Settings.** D32's four settings, with an unreadable or
    never-firing cron schedule refused at input (POD2 reading 4).
  - **Pages.** Server → Podcasts lists a library's shows, with the add
    form, OPML import and export. A show's page has its check state,
    settings, details and episode list with "Download". The
    Dashboard's failing-feed notice links to the show's page.

  Not here: playback, the listener pages and API (POD5/POD6), removing
  a show, and the podcasts library's finish rule (POD5). The bundle is
  unchanged (format v1, D36 open). Flow: `podcast_manage_flow.rs`.
  Spec: AUDIOBOOKS-PODCASTS.md "Managing feeds (POD4)" (13 readings
  for veto).
- [x] POD5 (M) **Native API**: podcasts, episodes, episode
  stream (Range; or redirect to the enclosure if not
  downloaded), progress get/put/delete, OPML.
  *Done 2026-11-27:* built to **D41** (decided 2026-11-27: a
  per-profile explicit flag, on by default) and **D32 as amended**
  (2026-11-27: an undownloaded episode streams by redirect).
  Migration `20261204000000_pod5_explicit_episode_sessions.sql`.
  - **Routes.** `GET /api/v1/podcasts` (the reachable shows, with
    counts and this profile's finished / in-progress) and
    `/api/v1/podcasts/{id}` (the episode list, serial oldest first,
    episodic newest first). `GET /api/v1/podcast-episodes/{id}`, its
    `/stream`, `/aac.m4a` and `GET|PUT|DELETE …/progress`.
  - **Who.** A profile reaches shows only through its grant on the
    podcasts library (D31). With its `allow_explicit` off (D41), an
    explicit show and everything of it is a 404. The grant and the
    flag are re-checked on every stream request, token or not.
  - **Stream.** A downloaded episode streams with Range, and AB10's
    plan and AAC fallback are widened to it. An undownloaded one is a
    `302` to its enclosure (`no-store`, `no-referrer`). That is the
    only place the enclosure address leaves the server, never in an
    answer, error, log, job, notice or Activity line.
  - **Progress.** AB11's `apply_write`, unchanged, upserted on
    `(profile_id, episode_id)`. The length is the download's, else
    the feed's. Undownloaded episodes take every write, mark-finished
    included.
  - **Finish rule.** A podcasts twin of AB11's settings,
    `GET|PUT /api/v1/admin/libraries/{id}/podcast-settings`, edited on
    Server → Podcasts.
  - **Sessions.** `listening_sessions.episode_id` (exactly one target):
    the playback reports take `episode_id`, with AB8's rules. Episode
    sessions show on Now Playing (under the show, admin stop revokes
    the token), in the history and stats totals, and as an Activity
    line naming "Show · Episode".
  - **D41 in the UI.** Server → Users → Profiles has an "Explicit
    podcasts" checkbox per profile (`GET /api/v1/admin/profiles`,
    `PUT …/{id}/explicit`); only admins can change it.
  - **OPML.** `GET /api/v1/podcasts/opml` is export only and admins
    only: it carries feed addresses (POD4 reading 6). Import stays
    POD4's.

  Not here: the listener pages and player (POD6), a bulk episode
  progress sync (CL2), artwork. `library_access.max_rating` is still
  enforced nowhere (a known gap outside POD5). The bundle is unchanged
  (format v1): `profiles.allow_explicit` is left out until format v2
  (D40), and `listening_sessions` stays *Proposed* (D36 open). Flow:
  `podcast_api_flow.rs`. Spec: AUDIOBOOKS-PODCASTS.md "Native API
  (POD5)" (14 readings for veto).
- [x] POD6 (M) **Web**: podcast pages + the AB5 player.
  *Done 2026-11-27:* no migration (`podcasts.image_path` is POD1's).
  - **Pages.**
    - **Shows list.** A podcasts library's page lists the shows the
      profile can reach. Each card has artwork, author and this
      profile's counts, and there is a filter and Current / All.
    - **Show page** (`/podcast/{id}`). It has the description, shown
      safely, and the episode list in POD5's order. Each row has its
      state (new dot, progress ring, played check), date and length,
      or time left. An undownloaded episode is marked "plays from the
      publisher". Rows have Play, Mark as played / unplayed, and
      Download for admins.
    - **Episode page** (`/episode/{id}`). It holds AB5's player for an
      episode: back 15 s / forward 30 s, an episode-wide scrubber with
      the current chapter, next chapter, the chapter list and the show
      notes. It plays the download (Range), its AAC copy, or the
      enclosure redirect (D32 as amended). Progress goes through
      POD5's routes, and sessions through AB8's reports with
      `episode_id`.
    - **D41.** With the flag off, a profile sees no explicit show:
      not in the list, the filter, the Home tile count or the stats,
      and its pages say "Not found".
  - **Artwork.** The server fetches it into the artwork cache after
    each check and after an add, and serves it from
    `GET /api/v1/podcasts/{id}/artwork` (grant and D41 checked). It is
    never hot-linked. Only JPEG, PNG, WebP or GIF up to 20 MB is kept,
    a new address replaces the copy, and a failure names the show on
    the job, never the address.
  - **Chapters.** `GET /api/v1/podcast-episodes/{id}/chapters` reads
    the download's own chapter marks, else the feed's Podcasting 2.0
    list, server-side. Only start and title are sent, and an
    unreadable list is reported without its address.
  - **Elsewhere.**
    - The stats gain a per-show list.
    - Now Playing, the history and the admin session list link
      episodes to their page, with the show's artwork.
    - A podcasts library's Home tile counts its reachable shows.
  - **Checked.** In headless Chromium against a private instance of
    this branch with stub feeds: 68 checks, all passed. The live test
    instance predates POD1.

  Not here: the persistent player bar, the podcast Home cards,
  speed, sleep timer and keys (device settings, D35), global search
  of shows. The bundle is unchanged (format v1, D36 open). Flow:
  `podcast_pages_flow.rs`. Spec: AUDIOBOOKS-PODCASTS.md "Web pages and
  player (POD6)" (15 readings for veto).
- [x] POD7 (S) **Directory search** (Apple / Podcast Index) —
  only if **D31** wants it. *D31: yes, Apple's search API.*
  *Done 2026-11-27:* built to **D31** (Apple's search API, no key,
  admins only). No migration: `podcasts.itunes_id` is POD1's, written
  here for the first time.
  - **Search.** `POST /api/v1/admin/libraries/{id}/podcasts/search`
    `{"query", "country"?}` asks Apple's iTunes Search API
    (`parlour_catalog::podcast_directory`): 25 results, 15 s, 2 MB, store
    country US by default. Only the server asks Apple. The query is in
    the body, never in a URL or the log. A feed address typed as a query
    is refused before anything is sent, because it may hold a token.
  - **Results** carry title, author, the feed address (admins only),
    Apple's artwork (only `*.mzstatic.com`, shown with no referrer),
    genres, episode count, explicit and latest date. A result with no
    feed address says it can't be added.
  - **Already added.** A result is marked "already added" when its show
    is in this library, by Apple's id first, else by its exact feed
    address. It then links to the show's Server → Podcasts page.
  - **Add** is POD4's add with `itunes_id`. The feed is read in the
    request and stored with its first check, and the artwork is cached
    afterwards (POD6). A duplicate address or Apple id is a 409, and a
    refused feed is a 400 that stores nothing.
  - **Apple failing** is a 502 (`AppError::Upstream`) that names the
    reason, never the query.
  - **The page.** The search sits on Server → Podcasts, under the shows
    table and above "Add a show by its address". Errors show per result.
  - **Checked.** In headless Chromium against a private instance of
    this branch and a stub Apple host on the loopback: 49 checks, all
    passed. The live test instance predates POD1.

  The bundle is unchanged (format v1, D36 open). Flow:
  `podcast_search_flow.rs`. Spec: AUDIOBOOKS-PODCASTS.md "Directory
  search (POD7)" (13 readings for veto).
- [ ] POD8 (S) **Up Next queue / playlists synced** — only if
  **D35** says server-synced. *D35: playlists sync (per
  profile per library); the Up Next queue stays on the device.*
- **Done when:** a feed added in the web UI gets new episodes on
  its own, and an episode started on one client resumes on
  another.

## Phase 12 — Audio clients: the sync contract (`server`; API design, D33–D35 decided 2026-11-24)

**2026-11-24:** clients are Parlour's own web UI and mobile apps
(D26); offline downloads are in (D33, so CL4 goes ahead); sync
covers progress, bookmarks, sessions + stats, playlists and
collections, not the queue or speed (D35). UI spec:
`docs/UI-AUDIO-REFERENCE.md`. App-vs-video shape, iOS order, Fire
Stick audio and tablet layout: no ABS equivalent, deferred.

The server side of the future iOS (audiobooks like the
Audiobookshelf client, podcasts like Overcast — behaviour only,
clean-room), Fire Stick and tablet apps. Client code stays in
Phases 3 and 6; app shape and order is **D34**.

- [ ] CL1 (M) **Device tokens for native apps**:
  `Authorization: Bearer`, long-lived, stored hashed (D7),
  listed and revocable under Settings → Devices.
- [ ] CL2 (M) **One sync endpoint**: `GET /api/v1/sync?cursor=`
  → changed book/episode progress, subscriptions, new
  episodes, tombstones, and the next cursor.
- [ ] CL3 (S) **Listening sessions**: start/progress/stop for
  audio into the P8 registry (Now Playing, Activity);
  listening-time stats only if **D35** wants them.
  *AB8 (2026-11-26) built the server side and the web player's use of
  it; CL3 is what a native client needs on top (its token, offline
  sessions uploaded later, the CL6 contract).*
- [ ] CL4 (M) **Offline**: download manifests + replay of
  offline progress with client timestamps under **D33**'s
  conflict rule — only if **D33** says clients download.
- [ ] CL5 (S) **Audio capability profiles** (AVPlayer,
  ExoPlayer on Fire OS, browsers) → direct play vs AAC
  fallback.
  *AB10 (2026-11-26) built the profiles and the fallback on the server,
  used by the web player; CL5 is what a native client needs on top: it
  names its profile (`?client=`), the profile contents are confirmed on
  real devices (AB10 readings 2–3), and the CL6 contract.*
- [ ] CL6 (S) **`docs/API-AUDIO.md`**: the contract with
  examples, written when CL2 lands.
- **Done when:** a native client can sign in once, pull
  everything it needs in one sync call after a week offline,
  and push its progress back without clobbering newer progress
  from another device.

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
