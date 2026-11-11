# Parlour

**A personal media server — what Emby and Jellyfin do, but better, in Rust, and built to grow.**

Parlour organises your own movies, shows, and (later) audiobooks and other
libraries, and streams them to the screens in your house and to you on the
road. One Rust server holds the library, the users, and the transcoding.
Thin clients — web first, then Android / Fire TV, then iOS — just connect,
browse, and play.

> Parlour provides no content. You point it at media you own, on your own
> server.

## Why another media server?

- **Subtitles first.** Text, embedded, forced, and picture subtitles are a
  core subsystem, not an afterthought.
- **Every GPU, explained.** NVIDIA NVENC first, with Intel Quick Sync,
  AMD VAAPI/AMF, and software fallback — probed at startup, and every
  playback tells you *why* it direct-played, remuxed, or transcoded.
- **Direct play by default.** Transcoding only when the client, the
  codec, or the connection actually needs it — mainly remote viewing.
- **Finds your server wherever you are.** Clients try, in order:
  LAN → Tailscale → public address. At home, at a friend's, or on a
  Fire TV in a truck, it just connects.
- **Yours.** No paywall for hardware transcoding, no vendor account,
  no telemetry. Your library, your users, your rules.
- **Requests built in.** Friends and family who connect to your server
  can request media in Parlour; it routes to Sonarr / Radarr / Lidarr
  and Prowlarr / qBittorrent — no separate bot to run.
- **Built to grow.** Movies and TV first. The catalog is library-type
  agnostic, so audiobooks, ebooks, music, and podcasts slot in without
  a rewrite, and new clients only need the one API.

## Status

Design phase. Repo created 2026-11-10. Nothing here runs yet.
The working checklist — phases, milestones, and open decisions —
lives in `TODO.md`.

Roadmap, in order:

1. **Web UI first** — server, catalog, scanning, direct play / remux,
   watch state, multi-user accounts and profiles.
2. **Transcoding** — decision engine, all hardware backends, HDR
   tone mapping, subtitle burn-in where a client truly needs it.
3. **Android app** — one Kotlin app for phone, Android TV, and Fire TV
   (the Fire TV build is streaming-only). Sideloadable, and planned
   for the Amazon Appstore so friends and family can install it easily.
4. **Compatibility APIs** — Jellyfin and Audiobookshelf surfaces, if
   still wanted once the native clients exist.
5. **iOS app** — Swift, last.

## Stack

| Layer | Choice |
|---|---|
| Server | Rust, Axum (Tokio / Tower), single binary |
| Database | PostgreSQL, sqlx, timestamped append-only SQL migrations |
| Web UI | SvelteKit 2 / Svelte 5, static SPA, embedded in the server binary |
| Transcoding | FFmpeg / ffprobe as subprocesses — no transcoding engine to build |
| Jobs | PostgreSQL-backed job table (`FOR UPDATE SKIP LOCKED`) |
| Clients | Kotlin (Android / Fire TV), Swift (iOS) — later phases |

## Design ground rules

- **Clean-room.** Parlour is an original implementation inspired by
  how Emby, Jellyfin, and Silo behave. It is not a port of any of them.
- **Modular monolith.** One binary, clear internal crates (catalog,
  playback, transcode, subtitles, integrations). No microservices,
  no plugin system, no clustering — until a real need shows up.
- **Invite-only.** Accounts are created by the server owner. There is
  no public sign-up.
- **Compat is a view, not a second catalog.** Any compatibility API
  translates to the same catalog and playback services.

See `docs/ARCHITECTURE.md` for the code layout and the subsystem designs.

## Licence

MIT — see `LICENSE`.
