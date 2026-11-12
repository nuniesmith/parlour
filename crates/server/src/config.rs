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
    /// A3: first-admin bootstrap (`PARLOUR_ADMIN_USER` /
    /// `PARLOUR_ADMIN_PASSWORD`). When BOTH are set and no users
    /// exist yet, that account is created as admin, once; later
    /// boots ignore them.
    pub admin_user: Option<String>,
    pub admin_password: Option<String>,
    /// Set the Secure flag on the session cookie
    /// (`PARLOUR_COOKIE_SECURE`) — on behind the HTTPS proxy.
    pub cookie_secure: bool,
    /// TMDB API key (Milestone 1.4): `PARLOUR_TMDB_API_KEY`, or
    /// `TMDB_API_KEY` (the name Jordan's .env already uses).
    pub tmdb_api_key: Option<String>,
    /// TheTVDB API key (MD5): `PARLOUR_TVDB_API_KEY` / `TVDB_API_KEY`.
    pub tvdb_api_key: Option<String>,
    /// Provider base-URL overrides (tests point these at a stub).
    pub tmdb_base: Option<String>,
    pub tvdb_base: Option<String>,
    /// Local artwork cache directory (`PARLOUR_ARTWORK_DIR`).
    pub artwork_dir: PathBuf,
    pub emby_url: Option<String>,
    pub emby_api_key: Option<String>,
    pub cache_dir: PathBuf,
    pub stream_secret: Option<String>,
    /// UI2: display name on the Server Dashboard
    /// (`PARLOUR_SERVER_NAME`, default "Parlour").
    pub server_name: String,
    /// UI2: the addresses this server is reached by, shown (never
    /// guessed) on the Dashboard: `PARLOUR_URL_LAN`,
    /// `PARLOUR_URL_TAILNET`, `PARLOUR_URL_PUBLIC`.
    pub url_lan: Option<String>,
    pub url_tailnet: Option<String>,
    pub url_public: Option<String>,
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

        let admin_user = vars
            .get("PARLOUR_ADMIN_USER")
            .filter(|s| !s.trim().is_empty())
            .cloned();
        let admin_password = vars
            .get("PARLOUR_ADMIN_PASSWORD")
            .filter(|s| !s.trim().is_empty())
            .cloned();
        let cookie_secure = vars
            .get("PARLOUR_COOKIE_SECURE")
            .map(|s| matches!(s.as_str(), "true" | "1" | "TRUE"))
            .unwrap_or(false);

        let tmdb_api_key = vars
            .get("PARLOUR_TMDB_API_KEY")
            .or_else(|| vars.get("TMDB_API_KEY"))
            .filter(|s| !s.trim().is_empty())
            .cloned();
        let tvdb_api_key = vars
            .get("PARLOUR_TVDB_API_KEY")
            .or_else(|| vars.get("TVDB_API_KEY"))
            .filter(|s| !s.trim().is_empty())
            .cloned();
        let tmdb_base = vars
            .get("PARLOUR_TMDB_BASE")
            .filter(|s| !s.trim().is_empty())
            .cloned();
        let tvdb_base = vars
            .get("PARLOUR_TVDB_BASE")
            .filter(|s| !s.trim().is_empty())
            .cloned();
        let emby_url = vars
            .get("PARLOUR_EMBY_URL")
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .or_else(|| {
                vars.get("EMBY_URL")
                    .filter(|s| !s.trim().is_empty())
                    .cloned()
            });
        let emby_api_key = vars
            .get("PARLOUR_EMBY_API_KEY")
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .or_else(|| {
                vars.get("EMBY_API_KEY")
                    .filter(|s| !s.trim().is_empty())
                    .cloned()
            });
        let cache_dir = vars
            .get("PARLOUR_CACHE_DIR")
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cache"));
        let stream_secret = vars
            .get("PARLOUR_STREAM_SECRET")
            .filter(|s| !s.trim().is_empty())
            .cloned();
        let artwork_dir = vars
            .get("PARLOUR_ARTWORK_DIR")
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("artwork"));

        let server_name = vars
            .get("PARLOUR_SERVER_NAME")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Parlour".to_string());
        let url_lan = access_url(vars, "PARLOUR_URL_LAN")?;
        let url_tailnet = access_url(vars, "PARLOUR_URL_TAILNET")?;
        let url_public = access_url(vars, "PARLOUR_URL_PUBLIC")?;

        Ok(Self {
            bind_addr,
            database_url,
            log_level,
            web_dir,
            admin_user,
            admin_password,
            cookie_secure,
            tmdb_api_key,
            tvdb_api_key,
            tmdb_base,
            tvdb_base,
            artwork_dir,
            emby_url,
            emby_api_key,
            cache_dir,
            stream_secret,
            server_name,
            url_lan,
            url_tailnet,
            url_public,
        })
    }
}

/// An optional access URL for the Dashboard. Unset or blank is
/// `None`; anything set must be an http(s) URL, because the UI
/// renders it as a link.
fn access_url(
    vars: &HashMap<String, String>,
    name: &'static str,
) -> Result<Option<String>, ConfigError> {
    let Some(raw) = vars.get(name).map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let rest = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"));
    match rest {
        Some(host) if !host.is_empty() && !raw.contains(char::is_whitespace) => {
            Ok(Some(raw.to_string()))
        }
        _ => Err(ConfigError::InvalidVar {
            name,
            value: raw.to_string(),
            reason: "expected a full URL starting with http:// or https://",
        }),
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
        assert_eq!(cfg.admin_user, None);
        assert!(!cfg.cookie_secure);
        assert_eq!(cfg.server_name, "Parlour");
        assert_eq!(cfg.url_lan, None);
        assert_eq!(cfg.url_tailnet, None);
        assert_eq!(cfg.url_public, None);
    }

    #[test]
    fn server_name_and_access_urls_are_read() {
        let cfg = Config::from_vars(&vars(&[
            ("PARLOUR_DATABASE_URL", "postgres://u:p@localhost/parlour"),
            ("PARLOUR_SERVER_NAME", "  oryx  "),
            ("PARLOUR_URL_LAN", "http://192.168.1.20:8585"),
            ("PARLOUR_URL_TAILNET", "https://parlour.example.ts.net"),
            ("PARLOUR_URL_PUBLIC", "   "),
        ]))
        .unwrap();
        assert_eq!(cfg.server_name, "oryx");
        assert_eq!(cfg.url_lan.as_deref(), Some("http://192.168.1.20:8585"));
        assert_eq!(
            cfg.url_tailnet.as_deref(),
            Some("https://parlour.example.ts.net")
        );
        assert_eq!(cfg.url_public, None, "blank means not set");
    }

    #[test]
    fn access_url_without_a_scheme_is_a_clear_error() {
        for bad in ["parlour.example.com", "https://", "ftp://x", "http://a b"] {
            let err = Config::from_vars(&vars(&[
                ("PARLOUR_DATABASE_URL", "postgres://u:p@localhost/parlour"),
                ("PARLOUR_URL_PUBLIC", bad),
            ]))
            .unwrap_err();
            assert!(
                err.to_string().contains("PARLOUR_URL_PUBLIC"),
                "{bad}: {err}"
            );
        }
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
