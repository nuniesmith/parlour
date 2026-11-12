# Parlour — decisions log

The calls made before and during the repo setup, newest last.
Changing one means editing this file in the same PR that changes
the code.

## 2026-11-10 — Name and repo

- Name: **Parlour** (Canadian spelling). Repo:
  `nuniesmith/parlour`, public.
- A proper trademark / domain / app-store clearance check is still
  owed before brand assets or store listings. Store display name
  will need a qualifier ("Parlour — Personal Media Server").

## 2026-11-10 — What Parlour is

- Mission (owner's words): what Emby and/or Jellyfin are doing, but
  better, with Rust, and something to grow with.
- Clean-room original implementation inspired by Emby / Jellyfin /
  Silo behaviour. **Not a port** — porting Silo (AGPL) would make
  Parlour an AGPL derivative, and "Silo" is trademarked.
- Replaces the owner's current Emby install. Fire Sticks are the
  primary TV client today and stay first-class.
- Licence: **MIT** (decided 2026-11-10, after the scaffold first
  carried a proprietary placeholder). `LICENSE` is in the repo
  root and the Cargo metadata says `MIT`.

## 2026-11-10 — Ports

- Parlour uses **its own ports**: **8585** for HTTP (the single
  listener: web UI, API, streaming) and **8586** reserved for
  HTTPS if Parlour ever serves TLS directly instead of behind a
  reverse proxy. Deliberately NOT Emby/Jellyfin's 8096 or Silo's
  8090. Nothing else on the media host uses 8585/8586.

## 2026-11-10 — Deployment, naming, sessions (D2–D7, defaults accepted)

- **D2 Public TLS:** a reverse proxy on sullivan (Caddy)
  terminates HTTPS and forwards to Parlour on localhost:8585.
  Parlour does not serve TLS itself; port 8586 stays reserved.
- **D3 Names:** `parlour.7gram.xyz` is the tailnet/LAN name
  (7gram names are tailnet-only in practice). The public route
  is IP/DDNS straight to the sullivan proxy — the same pattern
  the truck uses to reach Emby today.
- **D4 Host:** sullivan is the production host.
- **D5 TMDB key:** created by Jordan when the metadata
  milestone (1.4) starts; lives in the server `.env`, never
  in the repo.
- **D6 Versions:** workspace deps bumped to current (axum
  0.8.9, sqlx 0.9) as task F2, before Phase 1 code uses them.
- **D7 Sessions:** hand-rolled, DB-backed; cookie carries a
  random token, the DB stores only its hash.

## 2026-11-10 — Stack

- Rust, Axum + sqlx on PostgreSQL, single binary.
- SvelteKit 2 / Svelte 5 web UI, built static and embedded in the
  binary. Web UI ships **first**, then the Android app, then iOS.
- FFmpeg/ffprobe as subprocesses. No transcoding engine, no libav
  bindings.
- Jobs in Postgres. No Redis, no plugin system, no clustering
  until a real need shows up.

## 2026-11-10 — Playback and subtitles

- Direct play by default; remux next; transcode only on need
  (client codec, remote bandwidth, HDR tone mapping, subtitle
  burn-in) — and the UI always shows which and why.
- Transcoding backs every vendor: NVIDIA NVENC primary, Intel
  Quick Sync, AMD VAAPI/AMF, software fallback. "Works" means a
  real test encode passed, not just a driver present.
- Subtitles are first-class: index all streams at scan time,
  forced-track logic, picture subtitles rendered natively where
  the client can, burned in only where it cannot.

## 2026-11-10 — Users and remote access

- Friends/family **connect to the owner's server**; they do not
  run their own. Multi-user (accounts, profiles, per-user watch
  state, library access, rating limits) is core, Phase 0/1.
  Invite-only sign-up. Easy self-host install is secondary.
- A server entry in a client holds several addresses, tried in
  order: **LAN -> Tailscale -> public**. Tailscale is the primary
  remote path for most people; the public path exists for cases
  like a Fire TV in the truck (today's Emby pattern: public IP,
  port 8096). The public listener gets HTTPS, rate-limited auth,
  and no unauthenticated admin surface.

## 2026-11-10 — Clients and publishing

- Clients are thin: connect, browse, play. All catalog and
  transcoding live in the server.
- One Kotlin app (Media3) for Android phone + Android TV / Fire
  TV. The TV build is streaming-only; offline downloads, if any,
  belong to the phone build. No Google Play Services dependencies.
- iOS (Swift/AVPlayer) comes last.
- The Android/Fire TV app will probably be **published to the
  Amazon Appstore** (Canada) to make installs easy for friends.
  That means: a separate reviewer demo server with public-domain
  sample media and a test account, a privacy policy, store assets,
  and a cleared name — all before submission.
- Vega OS Fire TV sticks don't run Android apps; supporting them
  is a separate, later decision.

## 2026-11-11 — ABR, caching, recommendations, requests, client order

From the ABR/recommendations/clients research of 2026-11-11
(full report kept with the project's research notes).

- **D8 Adaptive streaming:** Netflix-style ABR instead of a
  manual bitrate picker. HLS ladder (5 H.264 rungs, 360p–1080p,
  shared AAC track), 6 s segments / 2 s GOP, CMAF/fMP4, one
  multi-output FFmpeg per session, ~120 s transcode-ahead.
  HLS-only for v1; DASH would be a manifest over the same
  segments. Client players measure and switch; the server
  only offers rungs. GPU session budgets are a design input
  (GeForce NVENC: 12 sessions/system), with admission control
  instead of failed FFmpeg spawns.
- **D9 No Redis:** segments are cached on filesystem/SSD with
  a Postgres cache index (LRU, pinning for partially-watched
  items). Redis/Memcached never hold segment bytes. Jobs and
  sessions stay in Postgres. Revisit only for multi-node.
- **D10 Recommendations:** content-based + TMDB edges,
  SQL-scored per profile. No collaborative filtering / ML —
  it cannot work with a handful of users.
- **D11 Requests:** Seerr-shaped lifecycle native to Parlour;
  Sonarr/Radarr called directly (`/api/v3`); Prowlarr never
  called for requests (indexer sync only, health at most);
  Lidarr is `/api/v1`.
- **D12 Client order:** web first (it is the ABR test vehicle,
  including on the TV's browser), Android phone next, TV
  native deferred, iOS after Android. **TV OS correction:**
  the owner assumed his new built-in "Ember" TV runs Vega OS;
  Amazon's own device table lists the Ember Artline (2026) as
  Fire OS 8 (Android 11) — Vega OS 2.0 is sticks-only so far.
  The TV's About screen (build model / OS version) is the
  deciding check (TODO R7). If Fire OS: TV app = Android TV
  flavour, sideloadable. Vega/Kepler support stays a parked,
  separate app track, started only if a Vega device is owned.
