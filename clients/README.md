# Parlour clients

Thin clients only: connect to a Parlour server, browse, play.
All catalog, users, and transcoding live in the Rust server.

A server entry holds several addresses, tried in order:
**LAN -> Tailscale -> public** (see `docs/ARCHITECTURE.md` §3).

## android/ — Phase 3
One Kotlin + Media3 codebase for Android phone, Android TV, and
Fire TV. TV build is streaming-only. No Google Play Services
dependencies (Fire OS has none).

## ios/ — Phase 6
Swift + AVPlayer. Deliberately last.

Folders are intentionally not scaffolded further until their
phase starts.
