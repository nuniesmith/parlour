use std::net::SocketAddr;

use parlour_server::config::Config;
use parlour_server::http::AppState;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(2);
        }
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level)),
        )
        .init();

    // Compose starts the app and the DB together; wait for Postgres
    // instead of crashing on the first connection attempt.
    let pool = {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match parlour_db::connect(&config.database_url).await {
                Ok(pool) => break pool,
                Err(e) if attempt < 15 => {
                    tracing::warn!(attempt, error = %e, "database not ready, retrying");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "could not connect to the database");
                    std::process::exit(1);
                }
            }
        }
    };

    if let Err(e) = parlour_db::migrate(&pool).await {
        tracing::error!(error = %e, "database migration failed");
        std::process::exit(1);
    }

    // A3: one-time first-admin bootstrap from the environment.
    if let Err(e) = parlour_server::auth::bootstrap_admin(&pool, &config).await {
        tracing::error!(error = %e, "admin bootstrap failed");
        std::process::exit(1);
    }

    // C6/C7: background scan worker, filesystem watcher, and the
    // nightly full-scan safety net. Clone the pool before it moves
    // into the app state.
    let metadata = parlour_server::metadata::MetadataSettings::from_config(&config);
    parlour_server::catalog::spawn_background(pool.clone(), metadata.clone());

    let mut state = AppState::new(pool, config.web_dir.clone(), config.cookie_secure);
    state.metadata = metadata;
    let app = parlour_server::http::router(state);
    let addr = config.bind_addr;
    tracing::info!(%addr, version = env!("CARGO_PKG_VERSION"), "parlour listening");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, %addr, "could not bind");
            std::process::exit(1);
        }
    };

    if let Err(e) = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    {
        tracing::error!(error = %e, "server error");
        std::process::exit(1);
    }
}
