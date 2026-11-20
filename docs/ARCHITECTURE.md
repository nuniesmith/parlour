# Parlour — Architecture (draft, 2026-11-10)

Mission, in the owner's words: *what Emby and/or Jellyfin are doing, but
better, with Rust, and something I can grow out of / grow with.*

Every decision below was made with the owner on 2026-11-10. This draft
collects them in one place so the repo starts from them, not from a
generic media-server template.

## 1. Shape

One Rust binary + PostgreSQL. The SvelteKit web UI is built as a static
SPA and embedded in the binary (`rust-embed`). No Node at runtime, no
Redis, no S3 requirement, no plugin runtime, no multi-node clustering.

Deployment target today: the owner's media host (sullivan), replacing
Emby. A Docker Compose of `parlour + postgres` is the install.

## 2. Users — multi-user from day one

Friends and family connect to the owner's server; they do not run
their own. So accounts are core, not polish:

- `users` — one login per person. Invite-only creation, sign-in rate
  limiting, sessions that can be revoked.
- `profiles` — household members under a user (e.g. kids), with
  optional PINs, their own watch state, library access, rating limits.
- `watch_state` is per profile + item: position, played, favourites.
- Per-user defaults: audio language, subtitle language/mode, remote
  bitrate cap.
- Roles: `admin` (owner) vs normal. Admin surfaces (settings, *arr
  keys, users, jobs) are never exposed to normal users, and never on
  the public listener's unauthenticated surface.

## 3. Remote access — three routes, one server entry

Clients store several addresses for a server and try in order:

1. LAN address
2. Tailscale address (MagicDNS name / 100.x) — the primary remote
   path; most friends/family already have Tailscale. Server owners
   should share only the Parlour host (Tailscale machine sharing /
   ACLs), not the whole tailnet.
3. Public address (DDNS/IP + port) — fallback where Tailscale is not
   practical, e.g. a Fire TV in a truck. This replaces the owner's
   current Emby pattern (public IP, port 8096 open for Emby) — but
   on Parlour's own ports, decided 2026-11-10: **8585 HTTP** (the
   one listener) and **8586 HTTPS** (reserved).

Rules for the public route: HTTPS (Parlour-served TLS or reverse
proxy), rate-limited auth, short-lived stream tokens, admin routes
not reachable without auth. Tailnet/LAN traffic is already encrypted
(WireGuard) / local.

## 4. Playback — direct play first

For each (file, client, network, subtitle choice):

1. **Direct play** — serve the file with HTTP Range. The default.
2. **Remux** — codecs fine, container not: `ffmpeg -c copy` to
   HLS/fMP4 or MP4.
3. **Transcode** — codec unsupported, remote bitrate cap, HDR→SDR,
   or subtitle burn-in required.

The decision is a pure function in `core` (no DB, no FFmpeg), fully
unit-testable, and it returns a *reason* that the UI shows:
`remote bandwidth`, `unsupported codec`, `HDR tonemap`,
`subtitle burn-in`.

Inputs: ffprobe data cached at scan time; an explicit per-client
capability profile (web, android-phone, android-tv/fire-tv, ios —
not User-Agent guessing); session context (LAN / tailnet / public,
per-user remote cap).

## 5. Transcoding — all vendors, probed

FFmpeg/ffprobe run as subprocesses. Do not build a transcoding
engine; do not bind libav*.

```rust
trait HwBackend {
    fn name(&self) -> &str;   // nvenc | qsv | vaapi | amf | videotoolbox | software
    fn probe(&self, ffmpeg: &Path) -> bool; // runs a real 1 s test encode
    fn build_args(&self, plan: &TranscodePlan) -> Vec<String>;
}
```

- Default preference: NVENC → QSV → VAAPI → software. Owner's main
  GPU is NVIDIA; the design must still work on Intel/AMD/CPU-only
  hosts. A present driver does not count as working — probe results
  are cached and re-probed when FFmpeg/driver changes.
- Output: HLS with fMP4 segments, keyframe-aligned to segment length
  (6 s), seek by restarting FFmpeg at the keyframe before the target.
- Throttle: transcode a buffer ahead, not the whole film. Cap
  concurrent transcodes (GPU and tonemap capacity are the limit).
- HDR→SDR: libplacebo preferred, then vendor tonemap filters, CPU
  zscale+tonemap only as a deliberate last resort.
- Log the full FFmpeg command for every session (Jellyfin's habit).
- Scratch space on SSD/tmpfs, reaped aggressively.

## 6. Subtitles — a first-class crate

The owner uses subtitles on most playback.

- On scan, index every subtitle stream: codec, language, forced,
  default, SDH. Extract embedded text subtitles; generate WebVTT
  copies for the web player.
- Text (SRT/ASS/external): served as sidecar — no transcode.
- Picture (PGS/VobSub/DVB): clients that can render them (the native
  Android app via Media3) get them as-is; the web player cannot, so
  selecting one there means burn-in — which forces a transcode and is
  surfaced as the transcode reason.
- Forced-track logic: foreign-dialogue track when audio is in the
  user's language; full track otherwise.
- OpenSubtitles download is a background job, never in the play path.

## 7. Catalog and scanning

- Libraries are typed (`movies`, `shows`, later `audiobooks`,
  `ebooks`, `music`, `podcasts`) but share one catalog:
  `media_items` (deterministic content ID, so rescans are idempotent)
  + `media_files` (one item : many files) +
  `media_streams` (ffprobe output, incl. subtitle streams).
- Segments (Phase 9, D15): `media_segments` holds intro / credits /
  recap spans per item — one row per (item, kind, source), sources
  `chapters`, `fingerprint`, `manual`, `heuristic`, each with a 0–1
  confidence. Detectors write only their own source's rows; the read
  path (`parlour_playback::effective_segments`) hands the player at
  most one segment per kind: nothing for a kind an admin marked as
  having none (SEG4, D21); otherwise manual always wins, whatever
  its stored confidence; otherwise the highest-confidence automatic
  row at or above the 0.6 floor (D17), ties going to the earlier
  detector (chapters, fingerprint, heuristic). `playback-info`
  returns the result as `segments`.
- Segment detection, the cheap pass (SEG2, D17–D19;
  `parlour_catalog::segments`): a finished scan queues
  `detect_segments` for its library when an episode's playback file
  is new or changed — `media_files.segments_detected_at` stays NULL
  until the job has looked at that exact file, and the scanner
  clears it on every re-probe. The job has a worker of its own, so a
  long first pass never delays scans. Per file: chapters titled with
  the whole word intro / opening / credits → source `chapters` at
  1.0 (D19: a title naming both, "Opening Credits", is the intro;
  of several chapters naming one kind, the earliest wins); for a
  kind the chapters left open, ffmpeg `blackdetect` +
  `silencedetect` over the first 300 s (intro) and the last 600 s
  (credits), where a break is a black run and a silence run at most
  2 s apart and spans follow D18 (D19: of several qualifying
  credits spans, the one nearest the end wins) → source
  `heuristic` at 0.5: stored for the SEG4 editor and SEG3, never
  served. Each run replaces its own rows for intro and credits (a
  changed file loses the old file's spans); manual and fingerprint
  rows are never touched. A tool that fails on one file is listed on
  the job and not retried until the file changes; ffmpeg/ffprobe
  that cannot start at all fails the job and leaves the files queued.
- Segment detection, the fingerprint pass (SEG3, D20;
  `parlour_catalog::fingerprint`): the same job then re-matches, as
  a whole, every season with a new or changed episode — the job
  works season by season, and a season's files are marked as looked
  at only together with its fingerprint rows, so a job that stops
  part-way leaves them queued. Each usable episode (an audio stream
  and a known length) is fingerprinted with ffmpeg's chromaprint
  muxer (`-ac 1 -ar 11025 -f chromaprint -fp_format raw`: one 32-bit
  point per 0.124 s) over two windows — the head, the first 25 %
  capped at 10 minutes, for the intro; the tail, the last 600 s, for
  the credits. Prints are cached in `audio_fingerprints`, keyed by
  the SHA-256 of the file's size plus its first and last 4 MiB;
  `media_files.file_hash` links a file to its print and is cleared
  by the scanner on re-probe; prints no file row references are
  deleted at the end of each job. Matching tries every alignment of
  every pair of episodes: points match within 6 of 32 bits, a shared
  stretch jumps gaps up to 1 s, needs 80 % of its points matching
  and — the silence guard — half of them distinct values (silence
  and steady tones print one repeated value). The same stretch at
  neighbouring alignments counts once. Intro candidates are 15–120 s
  long; credits candidates at least 15 s, ending within the last
  120 s. Each episode keeps the stretch of its own audio the most
  other episodes share (candidates agree when they overlap by half
  the shorter one; ties: the earliest intro, the credits nearest the
  end, after D18/D19), with median edges, at confidence = quality ×
  (1 − 0.5ⁿ) for n sharing episodes; below 0.6 nothing is stored,
  and seasons with fewer than 3 usable episodes are not matched — a
  re-match then clears the season's fingerprint rows. Rows are
  written as source `fingerprint` only for kinds the chapters left
  open; manual, chapter and heuristic rows are never touched.
  Measured on synthetic episodes, edges land within 1.4 s of the
  truth, nearly always early; release-build matching costs ~26 ms
  per pair of 600 s windows (a 24-episode season: ~7 s per window).
- Segment editor (SEG4, D21; `crates/server/src/segments.rs`,
  `web/src/lib/SegmentEditor.svelte`): an admin-only Segments
  section on the item page for any item with a playable file —
  movies included, though detection only covers episodes.
  `GET /api/v1/admin/items/{id}/segments` lists every stored row
  with what it means for playback now (used; overridden by the
  admin's times; hidden by a marker; below the floor; outranked)
  and the playback file the times are in — the first available
  file by relative path, then path, as detection and playback-info
  pick it. `PUT …/segments/{kind}` `{start_ms, end_ms}` sets the
  kind's `manual` row (confidence 1.0), checked only for
  0 ≤ start < end ≤ the file's length (a file of unknown length
  can't take times); D17/D18's lengths and positions don't apply
  and kinds may overlap. `DELETE …/segments/{kind}/{source}`
  deletes one listed row: a manual row — times or marker — is
  removed, and detection decides that kind again; a detected row
  is kept (detection owns it and would write it back) and the
  kind's manual "no segment here" marker is stored instead,
  unless the admin's own times already override that kind (409).
  A marker is a manual row with NULL times (migration
  `20261119000000_segment_markers.sql`: only manual rows may lack
  times, and then both); since one row per (item, kind, source)
  exists, a kind holds the admin's times or the admin's "none",
  never both. It beats every automatic source in
  `effective_segments`, including rows a later detection run
  writes. playback-info and the editor read through one path
  (`segments::effective_for`), so "used by the player" in the
  editor is what the player gets. Edits write no activity row,
  like the MD4 manual match. "Preview from here" links to
  `/watch/[id]?preview=<ms>`: the player opens at that time with
  no resume prompt and sends no start / progress / stopped
  report — so no position, played mark, play count, Now Playing
  session or activity line. The mode only suppresses the
  viewer's own reports, which a client could always skip; the
  stream is authorised exactly as for normal playback.
- Skip in the player (SEG5, D22; `web/src/lib/skip.ts`, the
  player in `web/src/routes/watch/[id]/+page.svelte`):
  playback-info also returns the active profile's
  `segment_skip` (`button` | `auto` | `off`, Preferences →
  Playback), read on every call like the subtitle pick, so a
  change made on another device applies to the next playback.
  The segment at the playhead is the one with start ≤ t < end
  (kinds may overlap: then the one that started first). At the
  media's end, as the browser measures it, there is none, even
  when a stored end lies past it: ffprobe's file length, which
  bounds manual times and "to the end" spans, can run longer
  than what the browser plays — an MKV whose subtitles outlast
  the picture is remuxed without them — and a Skip Credits
  button must not linger once playback has ended.
  `button` offers Skip Recap / Skip Intro / Skip Credits while
  the playhead is inside a segment, paused or playing; `off`
  offers nothing; `auto` skips a segment the first time
  playback reaches it — playback starting inside it included;
  a paused playhead waits for play — shows "Skipped the
  intro" for 4 s, and never skips that segment by itself again
  in that playback: back inside, it plays and the button is
  offered. Nothing is offered or skipped before the opening
  seek (resume or preview) has happened. A skip lands on the
  segment's end. A segment ending within 1 s of the media's
  end runs to the end of the file (its times are on ffprobe's
  clock, the media's length on the browser's), so the skip
  ends playback there and the player sends the stopped report
  itself — Chrome fires `ended` after a seek to the end only
  while playing — once, whatever `ended` does afterwards; the
  item is then played under the usual ≥ 90 % rule. A preview
  never skips by itself and always offers the button. Full
  screen is the player area, not the `<video>`, so the button
  stays in view: F and a new corner button toggle it (the
  button fades with the browser's controls: 3 s after playback
  starts or the pointer last moved over the player).
  Where the player area can go full screen, Chromium's own
  full-screen button is turned off with
  `controlslist="nofullscreen"` — which in Chrome 151 only
  greys it out, and also turns off double-click full screen —
  and hidden with `::-webkit-media-controls-fullscreen-button`.
  Firefox and Safari keep theirs, which shows the bare video
  without the Skip button.
- Autoplay next episode (SEG6, D23; `web/src/lib/autoplay.ts`,
  `web/src/lib/NextEpisodeCard.svelte`, the player, and
  `discover::next_episode`): playback-info also returns
  `next_episode` and the active profile's `autoplay_next`
  (Preferences → Playback, default on), read on every call
  like `segment_skip`. `next_episode` is the episode after
  this one in series order — (season, episode) above the
  current one, so specials (season 0) lead into season 1 and
  never follow it — played or not, skipping episodes with no
  available file (playback-info would refuse them), within
  the same series and libraries the profile can reach; null
  for a movie, the last episode, and episodes without both
  numbers. It is shaped like a Home Next Up row (series
  title, poster pick) and sent whatever `autoplay_next`
  says. The 1.6 Next Up query is not used: its
  unplayed-only filter would stop a rewatch from rolling.
  In the player, with autoplay on, a next episode, and not
  a preview, the countdown card is up while the playhead is
  in the credits — from their start to their end, or on to
  the end of the file when they run there (1 s slack, as for
  skips) — and once the media is at its end (ended, or a
  paused seek to the end, which Chrome doesn't mark ended).
  It counts 10 s while the video plays or has ended and
  waits while it is paused; leaving that stretch takes it
  down, and coming back starts the count again. The card
  shows the series poster (a Next Up tile's pick, else a
  letter tile), the show, `S1:E2 · Title`, "Starts in N s"
  and a draining bar, with Play now and Cancel; on phones
  (up to 600 px wide) it keeps to two rows — the episode,
  then the count beside the buttons — so it fits a 16:9
  player above the browser's controls. While
  autoplay is on, the credits get neither the Skip button
  nor the automatic skip — the card's Play now does the
  skipping. Cancel ends autoplay for that playback: no card
  again, and the credits get the Skip button whatever the
  mode. Play now, or the count running out, rolls: the old
  episode's stopped report goes first, at the end of the
  file when the playhead is in credits that run there (so
  the ≥ 90 % rule marks it played, as a Skip Credits would)
  and otherwise at the playhead; then the next episode's
  playback-info is fetched and its stream loads into the
  same `<video>` — so full screen stays — from its resume
  point with no prompt (played episodes resume at 0), and
  its start report goes out once it plays from there. The
  URL moves with `goto(…, { replaceState: true })`; the
  root layout keys pages on their path, so the player tells
  it (`web/src/lib/viewKey.svelte.ts`) which paths it has
  carried its page to, and those keep its key until the
  page goes away. Back, Forward and reload then land on the
  episode the URL names, and no history entry is added.
  Answers to reports about an episode the player has rolled
  on from are ignored, and a report that can't reach the
  server never holds a roll up. If the next episode's
  details can't be fetched, the old one stays, paused, and
  an overlay says why; if its stream doesn't start (a remux
  that never becomes ready), the player is left empty under
  the new episode with the same overlay.
- Scanner: `notify` watcher for local disks + *arr webhooks/history
  polling for targeted rescans + a periodic full scan. Skip unchanged
  files by (path, size, mtime) fingerprint before probing.
- Filenames follow the *arr / TRaSH conventions the owner already
  uses; TMDB/TVDB IDs in folder names are a primary match signal.
- Metadata behind traits: TMDB, TheTVDB first; MusicBrainz,
  Audnexus, OpenLibrary when those library types land.
- Emby migration: a one-off importer for watched state / progress /
  favourites per user, so the household does not lose Continue
  Watching at cutover.

## 8. Integrations — built in, server-side only

- Sonarr / Radarr / Lidarr (`/api/v3`, `X-Api-Key`), Prowlarr
  (`/api/v1`), qBittorrent WebUI API (cookie auth, read-mostly).
- Requests live in Parlour: search → library check → owner approval /
  quota → route to the right *arr app → status reconciliation →
  notify. This replaces the separate Discord bot + book pipeline
  over time; a Discord bot (poise on serenity) can run in the same
  binary, feature-flagged, speaking to the same request service.
- Books have no *arr app (Readarr is retired): keep the manual
  Prowlarr → qBittorrent → library flow inside the requests UI.
- Integration credentials are stored encrypted in the DB, set by an
  admin, never shown to normal users and never sent to clients.

## 9. Clients

| Client | Tech | When | Notes |
|---|---|---|---|
| Web | SvelteKit SPA, embedded | Phase 1 | The reference UI; HLS.js / native HLS |
| Android phone + TV / Fire TV | Kotlin, Media3 | Phase 3 | One codebase; phone build may add offline downloads, **TV build is streaming-only**; no Google Play Services dependencies (Fire OS has none) — no FCM, no Cast as a hard dependency |
| iOS / tvOS | Swift, AVPlayer | Phase 5 | Last, deliberately |

Distribution: sideloading always works on Fire OS devices. The
Android/Fire TV app is planned for the **Amazon Appstore** (Canada
first) so friends/family can install it easily. Store reviewers get
a **separate demo server** with public-domain sample media and a
test account — never access to the owner's real server or library.

Newer Vega OS Fire TV sticks do not run Android apps and do not
allow sideloading; supporting them is a separate, later decision.

## 10. Code layout

```
parlour/
  Cargo.toml                  # workspace
  crates/
    server/                   # binary: config, startup, backend probing, routers
      src/
        routes/               # native /api/v1
        compat_jellyfin/      # later, a view over the same catalog
        compat_abs/           # later
    core/                     # types only: ids, ClientProfile, PlaybackPlan, errors
    db/                       # sqlx queries + embedded migrations
    catalog/                  # scanner, ffprobe ingest, matching, Emby importer
    playback/                 # decision engine, sessions, HLS, watch state
    transcode/                # ffmpeg builder, backends/{nvenc,qsv,vaapi,software}
    subtitles/                # extract, convert, forced logic, download jobs
    metadata/                 # provider traits + implementations
    integrations/             # arr/, prowlarr/, qbittorrent/, requests/
    jobs/                     # Postgres job queue + workers
    discord/                  # optional, feature = "discord"
  web/                        # SvelteKit 2 / Svelte 5, adapter-static
  migrations/                 # timestamped SQL, append-only, never renumber
  clients/
    android/                  # added in Phase 3, not scaffolded before
    ios/                      # added in Phase 5
  docker/
    docker-compose.yml        # parlour + postgres; /dev/dri and NVIDIA runtime optional
  docs/
```

Rules: `core` has no I/O. Compat layers never own data. Every new
library type is a `catalog` + `metadata` addition, not a new server.

## 11. Growth — what "grow out" means in practice

- New library types: add a kind + a metadata provider + UI section.
- New hardware: add one `HwBackend`.
- New clients: implement `/api/v1` + a capability profile.
- New integrations: add a module under `integrations/`.
- None of those should touch playback decisions, watch state, or
  the job queue. If one does, the boundary is wrong — fix the
  boundary, not the feature.

Explicitly not planned: plugin runtime, multi-node transcoding,
Live TV / DVR, public sign-up, hosting other people's libraries.
