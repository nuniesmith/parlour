//! A1: login rate limiting — per-IP and per-account backoff.
//!
//! In-memory on purpose: it only needs to slow down password
//! guessing on this one process, and a restart clearing the
//! counters is acceptable (the counters are not a security
//! boundary, the password check is). After `FREE_FAILURES`
//! consecutive failures the key locks for 60s, doubling per
//! further failure, capped at an hour. A success clears the key.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const FREE_FAILURES: u32 = 4;
const BASE_LOCK: Duration = Duration::from_secs(60);
const MAX_LOCK: Duration = Duration::from_secs(3600);

#[derive(Default)]
struct Attempt {
    failures: u32,
    locked_until: Option<Instant>,
}

#[derive(Default)]
pub struct LoginLimiter {
    attempts: Mutex<HashMap<String, Attempt>>,
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Some(remaining) when the key is currently locked.
    pub fn locked_for(&self, key: &str, now: Instant) -> Option<Duration> {
        let map = self.attempts.lock().expect("limiter lock");
        let attempt = map.get(key)?;
        match attempt.locked_until {
            Some(until) if until > now => Some(until - now),
            _ => None,
        }
    }

    pub fn record_failure(&self, key: &str, now: Instant) {
        let mut map = self.attempts.lock().expect("limiter lock");
        let attempt = map.entry(key.to_string()).or_default();
        attempt.failures += 1;
        if attempt.failures > FREE_FAILURES {
            let over = attempt.failures - FREE_FAILURES - 1;
            let factor = 2u32.saturating_pow(over.min(6));
            let lock = (BASE_LOCK * factor).min(MAX_LOCK);
            attempt.locked_until = Some(now + lock);
        }
    }

    pub fn record_success(&self, key: &str) {
        let mut map = self.attempts.lock().expect("limiter lock");
        map.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_failures_are_free_then_lockout_doubles() {
        let limiter = LoginLimiter::new();
        let t0 = Instant::now();
        for _ in 0..FREE_FAILURES {
            limiter.record_failure("k", t0);
            assert!(limiter.locked_for("k", t0).is_none());
        }
        limiter.record_failure("k", t0);
        let lock = limiter
            .locked_for("k", t0)
            .expect("locked after 5 failures");
        assert_eq!(lock, BASE_LOCK);
        // Still locked part-way through; free after it elapses.
        assert!(limiter
            .locked_for("k", t0 + Duration::from_secs(30))
            .is_some());
        assert!(limiter
            .locked_for("k", t0 + Duration::from_secs(61))
            .is_none());
    }

    #[test]
    fn success_clears_the_counter() {
        let limiter = LoginLimiter::new();
        let t0 = Instant::now();
        for _ in 0..6 {
            limiter.record_failure("k", t0);
        }
        assert!(limiter.locked_for("k", t0).is_some());
        limiter.record_success("k");
        assert!(limiter.locked_for("k", t0).is_none());
    }
}
