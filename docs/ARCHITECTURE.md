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
