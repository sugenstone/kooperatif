//! Kooperatif API server binary.
//!
//! Subcommands:
//! - `serve`   — run the HTTP API (default)
//! - `migrate` — apply pending database migrations, then exit
//!
//! The future Rust worker process (ADR-009/ADR-012) will be added as a
//! sibling binary sharing the `kooperatif_server` library.

use std::process::ExitCode;

use kooperatif_server::config::Config;
use kooperatif_server::db;
use kooperatif_server::http;
use kooperatif_server::observability;

#[tokio::main]
async fn main() -> ExitCode {
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "serve".to_string());

    if !matches!(command.as_str(), "serve" | "migrate") {
        eprintln!("usage: kooperatif-server [serve|migrate]");
        return ExitCode::FAILURE;
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            // Configuration errors go to stderr directly: the tracing
            // setup depends on the environment we just failed to parse.
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    observability::init(config.env);

    match command.as_str() {
        "serve" => run_server(config).await,
        "migrate" => run_migrate(&config).await,
        _ => unreachable!("command validated above"),
    }
}

async fn run_server(config: Config) -> ExitCode {
    tracing::info!(environment = %config.env, "starting kooperatif api server");

    // A failed database connection degrades readiness instead of killing
    // the process: /health stays 200 (process alive) while /ready reports
    // the dependency as unavailable (ADR-011 semantics).
    let database = match &config.database_url {
        Some(url) => match db::connect(url).await {
            Ok(pool) => {
                tracing::info!("database pool connected");
                Some(pool)
            }
            Err(error) => {
                tracing::error!(error = %error, "database connection failed; starting degraded");
                None
            }
        },
        None => {
            tracing::warn!("KOOPERATIF_DATABASE_URL not set; starting without a database");
            None
        }
    };

    let app = http::router(
        http::AppState { db: database },
        http::cors_layer(&config.cors_origins),
    );

    let listener = match tokio::net::TcpListener::bind((config.host.as_str(), config.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(error = %error, host = %config.host, port = config.port, "failed to bind");
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(host = %config.host, port = config.port, "listening");

    match axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        Ok(()) => {
            tracing::info!("server stopped gracefully");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(error = %error, "server failed");
            ExitCode::FAILURE
        }
    }
}

async fn run_migrate(config: &Config) -> ExitCode {
    let Some(url) = config.database_url.as_deref() else {
        eprintln!("migrate requires KOOPERATIF_DATABASE_URL");
        return ExitCode::FAILURE;
    };

    let pool = match db::connect(url).await {
        Ok(pool) => pool,
        Err(error) => {
            tracing::error!(error = %error, "database connection failed");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = db::run_migrations(&pool).await {
        tracing::error!(error = %error, "migration failed");
        return ExitCode::FAILURE;
    }

    tracing::info!("migrations applied");
    ExitCode::SUCCESS
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        signal(SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("shutdown signal received (ctrl+c)"),
        () = terminate => tracing::info!("shutdown signal received (sigterm)"),
    }
}
