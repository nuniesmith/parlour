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
- Licence: undecided. Decide before any public binary / store
  release. (Cargo metadata carries a proprietary placeholder so
  nothing is accidentally published under a wrong licence.)

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
