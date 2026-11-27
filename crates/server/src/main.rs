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
    let emby = parlour_catalog::EmbySettings {
        base_url: config.emby_url.clone(),
        api_key: config.emby_api_key.clone(),
    };
    // XFER7: nightly backups — the nightly tick queues them, the
    // export worker writes them. D39: the setting lives in the web UI
    // (Server → Import/Export); PARLOUR_BACKUP_DIR / _KEEP are only the
    // default until an admin saves one, so this is read fresh each
    // time rather than fixed here.
    let backup = parlour_server::backup::BackupSettings::from_config(&config);
    match parlour_server::backup::effective(&pool, backup.as_ref()).await {
        Ok(Some(b)) => {
            tracing::info!(folder = %b.dir.display(), keep = b.keep, "nightly backups on")
        }
        Ok(None) => tracing::info!("nightly backups off (no backup folder is set)"),
        Err(e) => tracing::warn!(error = %e, "could not read the backup setting"),
    }
    parlour_server::catalog::spawn_background(
        pool.clone(),
        metadata.clone(),
        emby.clone(),
        backup.clone(),
    );
    // XFER2: export bundles have a worker of their own, so a long
    // export with artwork never holds up scans or metadata.
    let export_dir = parlour_server::export::export_dir(&config.cache_dir);
    parlour_server::export::spawn_worker(pool.clone(), export_dir.clone(), backup.clone());
    // XFER3: so do imports, applied from uploads under the cache.
    let import_dir = parlour_server::import::import_dir(&config.cache_dir);
    parlour_server::import::spawn_worker(
        pool.clone(),
        parlour_server::import::ImportDirs {
            import_dir: import_dir.clone(),
            artwork_dir: metadata.artwork_dir.clone(),
        },
    );
    // POD2: podcast feed checks on their schedule (D32), with a
    // worker of their own so a slow feed never holds up a scan.
    // POD6: the same checks keep each show's artwork cached.
    parlour_server::podcasts::spawn(pool.clone(), metadata.artwork_dir.clone());
    // UI5: keep the Dashboard's notices in line with reality.
    parlour_server::notices::spawn(
        pool.clone(),
        parlour_server::notices::NoticeSettings::from_config(&config),
    );

    let mut state = AppState::new(pool, config.web_dir.clone(), config.cookie_secure);
    state.metadata = metadata;
    state.emby = emby;
    state.server = parlour_server::dashboard::ServerSettings::from_config(&config);
    state.export_dir = export_dir;
    state.import_dir = import_dir;
    state.backup = backup;
    if let Some(base) = config.apple_search_base.clone() {
        state.apple_search_base = base;
    }
    std::sync::Arc::get_mut(&mut state.playback)
        .expect("fresh playback state")
        .configure(config.cache_dir.clone(), config.stream_secret.clone());
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
