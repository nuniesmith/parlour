# Parlour — roadmap

## Phase 0 — Skeleton (this setup)
Repo organised: docs, Rust workspace with crate stubs, web app
placeholder, client folders, Docker, CI. Everything compiles;
nothing plays yet.

## Phase 1 — Direct-play MVP
Server + config + Postgres schema; users/profiles/invite-only
auth; libraries and scanner; ffprobe ingest incl. subtitle index;
web UI (browse, search, item pages); direct play and remux with
Range/HLS; per-profile watch state; Emby watched-state importer.

## Phase 2 — Transcoding
Playback decision engine (with reasons in the UI); hardware
backends NVENC/QSV/VAAPI/software with startup probing; HDR->SDR
tone mapping; subtitle extraction/WebVTT and burn-in path;
per-user remote bitrate caps; concurrent-transcode limits.

## Phase 3 — Android + Fire TV
Kotlin/Media3 app: phone + TV from one codebase; multi-address
server entries (LAN -> Tailscale -> public); native picture
subtitle rendering. Sideload first, Amazon Appstore after.

## Phase 4 — Requests and integrations
Sonarr/Radarr/Lidarr, Prowlarr, qBittorrent; in-Parlour requests
with approval/quotas and status; optional in-binary Discord bot.
Retires the separate request pipeline when it covers it.

## Phase 5 — Compatibility APIs (only if still wanted)
Jellyfin API surface, then Audiobookshelf surface, as translation
layers over the same catalog.

## Phase 6 — iOS
Swift/AVPlayer app. Deliberately last.

## Growth after that
New library types (audiobooks, ebooks, music, podcasts) as
catalog + metadata additions; new hardware as a `HwBackend`;
new clients against `/api/v1`. See `ARCHITECTURE.md` §11.
