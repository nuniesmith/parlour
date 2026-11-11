//! Parlour server.
//!
//! Startup order: load config (fail fast on a bad one) -> init
//! logging -> connect to Postgres (with retries, so `docker
//! compose up` races are not fatal) -> run migrations -> serve.
//! The router itself lives in `http.rs`; compatibility routers
//! (Jellyfin / Audiobookshelf) arrive in later phases as views
//! over the same catalog — they own no data.

mod config;
mod error;
mod http;

use std::process::ExitCode;
use std::time::Duration;

use config::Config;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("parlour: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load()?;

    let filter = tracing_subscriber::EnvFilter::try_new(&config.log_level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting parlour");

    let pool = connect_with_retry(&config.database_url).await?;
    parlour_db::migrate(&pool).await?;

    let state = http::AppState {
        pool,
        web_dir: config.web_dir.clone(),
    };
    let app = http::router(state);

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// The database may still be starting when the server does (the
/// compose healthcheck normally prevents this, but a bare binary
/// run should not be flaky either). 15 tries, 1 s apart, then fail
/// with the last error so the operator sees the real cause.
async fn connect_with_retry(database_url: &str) -> Result<parlour_db::PgPool, parlour_db::DbError> {
    let mut last_err = None;
    for attempt in 1..=15 {
        match parlour_db::connect(database_url).await {
            Ok(pool) => return Ok(pool),
            Err(e) => {
                tracing::warn!(attempt, "database not ready yet: {e}");
                last_err = Some(e);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    Err(last_err.expect("at least one attempt ran"))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
