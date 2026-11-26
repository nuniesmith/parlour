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
layers over the same catalog. (2026-11-23: native audiobooks are
now Phase 10, so the Audiobookshelf surface is no longer the
audiobook plan; whether it is built at all is the open D26.
2026-11-24: D26 — it is never built; Phase 5 is the Jellyfin
surface only.)

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

## Phase 10 — Native audiobooks
Audiobooks as a first-class library in Parlour's own catalog
instead of leaning on Audiobookshelf-compat: a scanner for audio
folders (books, ordered tracks, chapters, tag metadata and folder
covers), per-profile listening progress (position, finished,
last listened), a native `/api/v1/audiobooks` API with Range
streaming and progress sync, and a book page with a player in
the web UI. Metadata providers, series, the ABS importer and
Continue-listening follow the owner's calls D26–D30.
(2026-11-24: decided — mirror Audiobookshelf's catalog,
progress and metadata behaviour; no ABS importer.)
Spec: `docs/AUDIOBOOKS-PODCASTS.md`.

## Phase 11 — Podcasts
The server manages RSS feeds in its own interface, fetches new
episodes on the Postgres job queue, and stores or links episode
audio per the owner's retention call. Clients (iOS, Fire Stick,
tablet, web) sync subscriptions and progress from the server and
never fetch feeds themselves. Ownership, download and retention
policy are open (D31, D32). (2026-11-24: decided — mirror
Audiobookshelf: podcasts are an admin-managed library, the server
downloads episodes, per-show retention.)

## Phase 12 — Audio clients: sync contract
API design for the native audio clients: the iOS audiobook
experience modelled on the Audiobookshelf client and the podcast
experience on Overcast (behaviour only, clean-room). Device tokens
for native apps, one cursor-based sync endpoint, listening
sessions, and offline download manifests if offline is wanted
(D33–D35). The client code itself belongs to Phases 3 and 6; app
shape and order is D34. (2026-11-24: Parlour's own web + mobile
apps, offline downloads in, sync what Audiobookshelf syncs; UI
spec `docs/UI-AUDIO-REFERENCE.md`.)

**Owner's note (2026-11-23):** the owner added Phases 10–12 and
asked for Phase 10's first slice (AB1–AB5) to be built along with
the plan. Podcasts and the client contract are plan only for now.
(2026-11-26: Phase 11 has started — POD1's schema and POD2's feed
checks are built.)

## Growth after that
New library types (ebooks, music) as catalog + metadata
additions; new hardware as a `HwBackend`;
new clients against `/api/v1`. See `ARCHITECTURE.md` §11.
