//! Parlour server library: configuration, HTTP routing, and the
//! auth/profile/invite/library API. `main.rs` is a thin wrapper so
//! integration tests can drive the real router.

pub mod auth;
pub mod catalog;
pub mod config;
pub mod dashboard;
pub mod discover;
pub mod emby_admin;
pub mod error;
pub mod http;
pub mod invites;
pub mod libraries;
pub mod limiter;
pub mod metadata;
pub mod playback;
pub mod profiles;
