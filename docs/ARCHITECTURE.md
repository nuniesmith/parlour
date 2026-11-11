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
   current Emby pattern (public IP, port 8096 open).

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
