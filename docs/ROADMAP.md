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

## Phase 2 — Transcoding + adaptive streaming
Playback decision engine (with reasons in the UI); hardware
backends NVENC/QSV/VAAPI/software with startup probing; HDR->SDR
tone mapping; subtitle extraction/WebVTT and burn-in path;
per-user remote bitrate caps; concurrent-transcode limits.
Plus (2026-11-11): Netflix-style ABR — a 5-rung H.264 HLS
ladder, one multi-output FFmpeg per session, a ~120 s
transcode-ahead window, and a filesystem segment cache
(no Redis — see DECISIONS D9). The web player is the ABR
test vehicle.

## Phase 3 — Android (phone first, TV flavour later)
Kotlin/Media3 app: phone first; the TV build is a flavour of
the same app, gated on verifying the TV's OS (Amazon lists
the owner's Ember TV as Fire OS 8, not Vega). Multi-address
server entries (LAN -> Tailscale -> public); native picture
subtitle rendering. Sideload first, Amazon Appstore after.
A Vega/Kepler TV app is a separate parked track.

## Phase 4 — Recommendations, requests and integrations
Content-based recommendations + TMDB discovery (no
collaborative filtering at household scale); Sonarr/Radarr,
qBittorrent; in-Parlour requests with approval/quotas and
status (Prowlarr is never called for requests — it feeds
indexers to the *arr apps); optional in-binary Discord bot.
Retires the separate request pipeline when it covers it.

## Phase 5 — Compatibility APIs (only if still wanted)
Jellyfin API surface, then Audiobookshelf surface, as translation
layers over the same catalog.

## Phase 6 — iOS
Swift/AVPlayer app (HLS-only, which the Phase 2 ladder
already serves). Deliberately last.

## Phase 7 — Web UI parity (Emby-style)
Sidebar shell and grouped IA; server Dashboard with info
card, notices and Alerts; Now Playing session cards with
explained stream decisions; Activity feed; admin as a
section tree instead of tabs; per-user preferences split by
topic; account self-service incl. username change.
Spec: `docs/WEBUI-EMBY-NOTES.md`. Presentation-led; does not
wait on Phase 2.

## Phase 8 — Import/export & sidecar metadata
Versioned export bundle for the whole server (users/logins,
profiles, libraries, catalog metadata, watch history,
segments) with dry-run, idempotent import; per-item
`.parlour.json` sidecars stored with the media files so
metadata travels with them; nightly export backups.

## Phase 9 — Segments, skip & autoplay
Per-episode intro/credits records; detection from chapters,
heuristics, then audio-fingerprint correlation across a
season; Skip Intro/Credits in the player (per-profile
button/auto/off); next-episode autoplay behind a countdown
card on the existing playback reporting.

**Owner's priority note (2026-11-11):** Phases 7–9 are the
tracks the owner wants worked next, alongside the Phase 1
operational cutover; their numbers are labels, not a queue
order relative to Phases 2–6.

## Growth after that
New library types (audiobooks, ebooks, music, podcasts) as
catalog + metadata additions; new hardware as a `HwBackend`;
new clients against `/api/v1`. See `ARCHITECTURE.md` §11.
