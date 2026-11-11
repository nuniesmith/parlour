//! Server configuration (task F1).
//!
//! Everything comes from environment variables (a local `.env` is
//! loaded first if present). Parsing is separated from the process
//! environment (`Config::from_vars`) so it is unit-testable, and a
//! bad or missing value fails startup with a message that says
//! exactly which variable is wrong — no silent defaults for the
//! things that matter, above all the database URL.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Address the HTTP server binds (`PARLOUR_BIND` + `PARLOUR_PORT`).
    pub bind_addr: SocketAddr,
    /// Postgres connection string (`PARLOUR_DATABASE_URL`, required).
    pub database_url: String,
    /// tracing filter, e.g. "info" or "parlour=debug" (`PARLOUR_LOG_LEVEL`).
    pub log_level: String,
    /// Serve the web UI from this directory instead of the embedded
    /// copy (`PARLOUR_WEB_DIR`, development only).
    pub web_dir: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "missing required environment variable {0} (set it in the environment or in a .env file)"
    )]
    MissingVar(&'static str),
    #[error("invalid value for {name}: {value:?} — {reason}")]
    InvalidVar {
        name: &'static str,
        value: String,
        reason: &'static str,
    },
}

impl Config {
    /// Load configuration from the process environment, reading a
    /// `.env` file first when one exists.
    pub fn load() -> Result<Self, ConfigError> {
        let _ = dotenvy::dotenv();
        Self::from_vars(&std::env::vars().collect())
    }

    pub fn from_vars(vars: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let database_url = vars
            .get(parlour_db::DATABASE_URL_ENV)
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .ok_or(ConfigError::MissingVar(parlour_db::DATABASE_URL_ENV))?;

        let host = vars
            .get("PARLOUR_BIND")
            .cloned()
            .unwrap_or_else(|| "0.0.0.0".to_string());
        let port_raw = vars
            .get("PARLOUR_PORT")
            .cloned()
            .unwrap_or_else(|| "8585".to_string());
        let port: u16 = port_raw.parse().map_err(|_| ConfigError::InvalidVar {
            name: "PARLOUR_PORT",
            value: port_raw.clone(),
            reason: "expected a port number between 0 and 65535",
        })?;
        let bind_addr: SocketAddr =
            format!("{host}:{port}")
                .parse()
                .map_err(|_| ConfigError::InvalidVar {
                    name: "PARLOUR_BIND",
                    value: host.clone(),
                    reason: "expected an IP address or hostname to bind",
                })?;

        let log_level = vars
            .get("PARLOUR_LOG_LEVEL")
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "info".to_string());

        let web_dir = vars
            .get("PARLOUR_WEB_DIR")
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        Ok(Self {
            bind_addr,
            database_url,
            log_level,
            web_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_apply_when_only_database_url_set() {
        let cfg = Config::from_vars(&vars(&[(
            "PARLOUR_DATABASE_URL",
            "postgres://u:p@localhost/parlour",
        )]))
        .unwrap();
        assert_eq!(cfg.bind_addr.to_string(), "0.0.0.0:8585");
        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.web_dir, None);
    }

    #[test]
    fn missing_database_url_is_a_clear_error() {
        let err = Config::from_vars(&vars(&[])).unwrap_err();
        assert!(err.to_string().contains("PARLOUR_DATABASE_URL"), "{err}");
    }

    #[test]
    fn bad_port_is_a_clear_error() {
        let err = Config::from_vars(&vars(&[
            ("PARLOUR_DATABASE_URL", "postgres://u:p@localhost/parlour"),
            ("PARLOUR_PORT", "eighty-five"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("PARLOUR_PORT"), "{err}");
    }
}
