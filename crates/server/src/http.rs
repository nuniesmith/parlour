//! HTTP surface: `/healthz`, `/api/v1/*`, and the web UI.
//!
//! - `/healthz` runs a real database query (task F4) — 200 only
//!   when Postgres actually answers, 503 otherwise.
//! - Unknown `/api/...` paths get a JSON 404 (task F6), never the
//!   SPA or an HTML error page.
//! - Everything else serves the web UI: embedded in the binary via
//!   rust-embed (task F7), or from `PARLOUR_WEB_DIR` on disk in
//!   development. Unknown non-API paths fall back to `index.html`
//!   so the SPA can route client-side.

use std::path::PathBuf;

use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde_json::json;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct AppState {
    pub pool: parlour_db::PgPool,
    pub web_dir: Option<PathBuf>,
    /// A1 login rate limiting (per-IP + per-account backoff).
    pub limiter: std::sync::Arc<crate::limiter::LoginLimiter>,
    /// Set the Secure flag on session cookies (production, behind
    /// the HTTPS reverse proxy).
    pub cookie_secure: bool,
    /// Milestone 1.4 provider settings (keys, artwork cache).
    /// Defaults to unconfigured; `main` fills it from `Config`.
    pub metadata: crate::metadata::MetadataSettings,
    /// Milestone 1.6 playback state (stream tokens, sessions,
    /// remux cache). `main` configures dir/secret from Config.
    pub playback: std::sync::Arc<crate::playback::PlaybackState>,
    /// Milestone 1.7 Emby connection (optional).
    pub emby: parlour_catalog::EmbySettings,
    /// UI2 Dashboard facts (name, port, access URLs). Defaults to
    /// unconfigured; `main` fills it from `Config`.
    pub server: crate::dashboard::ServerSettings,
}

impl AppState {
    pub fn new(pool: parlour_db::PgPool, web_dir: Option<PathBuf>, cookie_secure: bool) -> Self {
        Self {
            pool,
            web_dir,
            limiter: std::sync::Arc::new(crate::limiter::LoginLimiter::new()),
            cookie_secure,
            metadata: crate::metadata::MetadataSettings::default(),
            playback: std::sync::Arc::new(crate::playback::PlaybackState::new()),
            emby: parlour_catalog::EmbySettings::default(),
            server: crate::dashboard::ServerSettings::default(),
        }
    }
}

/// The web UI as embedded in the binary. The Docker build copies
/// the SvelteKit `web/build` output over this folder before
/// compiling; the checked-in copy is a placeholder page so the
/// workspace builds and runs before the UI build exists.
#[derive(RustEmbed)]
#[folder = "assets/"]
struct WebAssets;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", axum::routing::get(healthz))
        .route("/api/v1/version", axum::routing::get(version))
        .merge(crate::auth::routes())
        .merge(crate::profiles::routes())
        .merge(crate::invites::routes())
        .merge(crate::libraries::routes())
        .merge(crate::catalog::routes())
        .merge(crate::metadata::routes())
        .merge(crate::discover::routes())
        .merge(crate::playback::routes())
        .merge(crate::emby_admin::routes())
        .merge(crate::dashboard::routes())
        .merge(crate::activity::routes())
        .fallback(fallback)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Liveness + database truth. 200 `{status:"ok"}` only when a real
/// `SELECT 1` succeeds; otherwise 503. Safe to point Uptime Kuma at.
async fn healthz(State(state): State<AppState>) -> Response {
    let version = env!("CARGO_PKG_VERSION");
    if parlour_db::ping(&state.pool).await {
        (
            StatusCode::OK,
            Json(json!({ "status": "ok", "db": "ok", "version": version })),
        )
            .into_response()
    } else {
        tracing::warn!("health check: database did not answer");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "error", "db": "unreachable", "version": version })),
        )
            .into_response()
    }
}

async fn version() -> Json<serde_json::Value> {
    Json(json!({ "name": "parlour", "version": env!("CARGO_PKG_VERSION") }))
}

/// Everything that is not a named route: JSON 404 under `/api/`,
/// the web UI (with SPA fallback) everywhere else.
async fn fallback(State(state): State<AppState>, uri: Uri) -> AppResult<Response> {
    let path = uri.path();
    if path.starts_with("/api/") {
        return Err(AppError::NotFound(format!("no such route: {path}")));
    }
    Ok(serve_web(&state, path).await)
}

async fn serve_web(state: &AppState, path: &str) -> Response {
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };

    if let Some(bytes) = read_web_file(state, rel).await {
        return file_response(rel, bytes);
    }
    // SPA fallback: client-side routes have no file on disk.
    if let Some(bytes) = read_web_file(state, "index.html").await {
        return file_response("index.html", bytes);
    }
    AppError::NotFound("web UI is not available".to_string()).into_response()
}

async fn read_web_file(state: &AppState, rel: &str) -> Option<Vec<u8>> {
    if let Some(dir) = &state.web_dir {
        // Development override: read from disk. `rel` comes from the
        // request path; refuse anything that escapes the directory.
        if rel.contains("..") {
            return None;
        }
        return tokio::fs::read(dir.join(rel)).await.ok();
    }
    WebAssets::get(rel).map(|f| f.data.into_owned())
}

fn file_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime.as_ref())],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_router() -> Router {
        // Lazy pool pointed at a port nothing listens on: the
        // health check must honestly report the DB as down.
        let pool = parlour_db::connect_lazy("postgres://parlour:parlour@127.0.0.1:1/parlour")
            .expect("lazy pool builds without connecting");
        router(AppState::new(pool, None, false))
    }

    async fn get(app: Router, path: &str) -> (StatusCode, String) {
        let res = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn version_endpoint_answers() {
        let (status, body) = get(test_router(), "/api/v1/version").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"parlour\""), "{body}");
    }

    #[tokio::test]
    async fn healthz_reports_down_database_honestly() {
        let (status, body) = get(test_router(), "/healthz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("\"unreachable\""), "{body}");
    }

    #[tokio::test]
    async fn unknown_api_route_is_json_404() {
        let (status, body) = get(test_router(), "/api/v1/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("\"not_found\""), "{body}");
    }

    #[tokio::test]
    async fn root_serves_the_web_ui() {
        let (status, body) = get(test_router(), "/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Parlour"), "{body}");
    }

    #[tokio::test]
    async fn protected_routes_reject_anonymous_requests() {
        // No cookie: the auth extractor answers 401 before any DB
        // work, so these hold even against the dead test pool.
        for path in [
            "/api/v1/auth/me",
            "/api/v1/profiles",
            "/api/v1/auth/sessions",
            "/api/v1/libraries",
            "/api/v1/admin/server",
            "/api/v1/admin/activity",
        ] {
            let (status, _) = get(test_router(), path).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        }
        let app = test_router();
        let res = app
            .oneshot(
                Request::post("/api/v1/admin/invites")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn spa_fallback_serves_index_for_client_routes() {
        let (status, body) = get(test_router(), "/library/movies").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Parlour"), "{body}");
    }
}
