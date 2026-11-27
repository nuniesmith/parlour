//! Parlour server library: configuration, HTTP routing, and the
//! auth/profile/invite/library API. `main.rs` is a thin wrapper so
//! integration tests can drive the real router.

pub mod activity;
pub mod audiobooks;
pub mod auth;
pub mod backup;
pub mod book_match;
pub mod book_settings;
pub mod bundle;
pub mod catalog;
pub mod config;
pub mod dashboard;
pub mod discover;
pub mod emby_admin;
pub mod error;
pub mod export;
pub mod http;
pub mod import;
pub mod invites;
pub mod libraries;
pub mod limiter;
pub mod listening;
pub mod metadata;
pub mod notices;
pub mod playback;
pub mod podcast_admin;
pub mod podcasts;
pub mod profiles;
pub mod segments;
pub mod series;
pub mod shelves;
pub mod sidecars;
