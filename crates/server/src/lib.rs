//! Parlour server library: configuration, HTTP routing, and the
//! auth/profile/invite/library API. `main.rs` is a thin wrapper so
//! integration tests can drive the real router.

pub mod auth;
pub mod config;
pub mod error;
pub mod http;
pub mod invites;
pub mod libraries;
pub mod limiter;
pub mod profiles;
