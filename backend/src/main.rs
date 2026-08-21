// Use the library part of the `backend` crate instead of a local module.
use backend::web_server::AppState;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use backend::config::AppConfig;
use backend::db::DbPoolOptions;

use actix_web::HttpServer;
use tokio::signal;

use std::env;
use std::net::IpAddr;

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("signal received, starting graceful shutdown");
}

#[actix_web::main]
async fn main() {
    // --- Setup ---
    // 1. Initialize structured logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::filter::LevelFilter::INFO) // This sets the minimum level to INFO
        .init();

    let config = AppConfig::from_env().expect("Failed to load configuration");

    let database_url =
        env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");

    let db_pool = DbPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .unwrap();

    tracing::info!("Running database migrations...");

    #[cfg(feature = "db-sqlite")]
    sqlx::migrate!("./migrations/sqlite")
        .run(&db_pool)
        .await
        .unwrap();

    #[cfg(feature = "db-postgres")]
    sqlx::migrate!("./migrations/postgres")
        .run(&db_pool)
        .await
        .unwrap();

    tracing::info!("Migrations complete.");

    let app_state = AppState {
        db_pool,
        app_config: config.clone(),
    };

    // --- Run Server ---
    // 3. Start the web server and pass it the state
    tracing::info!("Initializing server...");
    let governor_conf = backend::web_server::create_governor_config(&config);
    let server_state = app_state.clone();

    let ip_addr: IpAddr = config
        .web
        .addr
        .parse()
        .expect("Invalid IP address in config");

    let addr = SocketAddr::new(ip_addr, config.web.port);
    tracing::info!("Serving frontend and API at http://{}", addr);

    let server = HttpServer::new(move || {
        backend::web_server::create_app(server_state.clone(), governor_conf.clone())
    })
    // The shutdown is driven by `shutdown_signal` below instead of the built-in handlers
    .disable_signals()
    .bind(addr)
    .unwrap()
    .run();

    let server_handle = server.handle();
    tokio::spawn(async move {
        shutdown_signal().await;
        // `true` asks for a graceful shutdown of the workers
        server_handle.stop(true).await;
    });

    server.await.unwrap();

    // This code runs after the server has stopped accepting new connections
    tracing::info!("Server shut down gracefully. Closing database connections.");
    app_state.db_pool.close().await;
    tracing::info!("Database pool closed.");
}
