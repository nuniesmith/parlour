//! Parlour integrations (placeholder).
//!
//! Server-side only. Credentials live encrypted in the database,
//! are set by an admin, and are never sent to clients.
//!
//! Planned:
//! - Sonarr / Radarr / Lidarr (`/api/v3`, `X-Api-Key`)
//! - Prowlarr (`/api/v1`)
//! - qBittorrent WebUI API (cookie auth, read-mostly)
//! - The **request flow** lives in Parlour: search -> library check
//!   -> approval/quota -> route to the right *arr app -> status
//!   reconciliation -> notify. This replaces the separate Discord
//!   bot + book-request pipeline over time.
//! - Books have no *arr app (Readarr is retired): keep the manual
//!   Prowlarr -> qBittorrent -> library flow inside the request UI.

/// An external service Parlour can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationKind {
    Sonarr,
    Radarr,
    Lidarr,
    Prowlarr,
    QBittorrent,
}
