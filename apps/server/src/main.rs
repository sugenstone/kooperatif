//! Kooperatif API server binary.
//!
//! Subcommands:
//! - `serve`        — run the HTTP API (default)
//! - `migrate`      — apply pending database migrations, then exit
//! - `create-user`  — controlled bootstrap of the first internal User
//!
//! The future Rust worker process (ADR-009/ADR-012) will be added as a
//! sibling binary sharing the `kooperatif_server` library.

use std::process::ExitCode;
use std::sync::Arc;

use kooperatif_server::auth::{audit, identity, users, AuthRuntime};
use kooperatif_server::clock::SystemClock;
use kooperatif_server::config::Config;
use kooperatif_server::db;
use kooperatif_server::http;
use kooperatif_server::observability;

#[tokio::main]
async fn main() -> ExitCode {
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "serve".to_string());

    if !matches!(command.as_str(), "serve" | "migrate" | "create-user") {
        eprintln!("usage: kooperatif-server [serve|migrate|create-user]");
        return ExitCode::FAILURE;
    }

    if command == "create-user" {
        // Bootstrap runs before any observability/log setup: nothing the
        // operator types may ever reach a log sink (STEP-002 §7/§8).
        return run_create_user(std::env::args().skip(2)).await;
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

    let auth = Arc::new(AuthRuntime::new(config.auth.clone(), Arc::new(SystemClock)));

    let app = http::router(
        http::AppState { db: database, auth },
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

    // ConnectInfo supplies the peer socket address used (only) for
    // login rate limiting keyed by client IP.
    match axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
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

/// Controlled first-User bootstrap (STEP-002 §7).
///
///   kooperatif-server create-user --username ali --display-name "Ali Yılmaz" [--password-stdin]
///
/// - Explicit operator action; no public /register exists anywhere.
/// - Password is read from a hidden interactive prompt (twice) or, with
///   `--password-stdin`, from stdin for scripted dev/test environments.
///   `--password-stdin` limitation (documented): the caller is responsible
///   for the stdin content — e.g. shell history or a heredoc file could
///   retain it. It is never printed, never logged, never echoed.
/// - Duplicate usernames are rejected cleanly (database constraint).
/// - No default/hard-coded credential is ever seeded.
async fn run_create_user(args: impl Iterator<Item = String>) -> ExitCode {
    let mut username_arg: Option<String> = None;
    let mut display_name_arg: Option<String> = None;
    let mut password_stdin = false;

    let mut iter = args.peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--username" => username_arg = iter.next(),
            "--display-name" => display_name_arg = iter.next(),
            "--password-stdin" => password_stdin = true,
            other => {
                eprintln!("bilinmeyen argüman: {other}");
                eprintln!(
                    "kullanım: kooperatif-server create-user --username AD --display-name \"AD SOYAD\" [--password-stdin]"
                );
                return ExitCode::FAILURE;
            }
        }
    }

    let Some(raw_username) = username_arg else {
        eprintln!("--username gereklidir");
        return ExitCode::FAILURE;
    };
    let Some(display_name) = display_name_arg else {
        eprintln!("--display-name gereklidir");
        return ExitCode::FAILURE;
    };
    if display_name.trim().is_empty() {
        eprintln!("--display-name boş olamaz");
        return ExitCode::FAILURE;
    }

    let username = match identity::normalize_and_validate_username(&raw_username) {
        Ok(username) => username,
        Err(error) => {
            // Turkish operator-facing message; nothing secret involved.
            match error {
                identity::UsernameError::InvalidLength => eprintln!(
                    "Kullanıcı adı en az {} en fazla {} karakter olmalıdır.",
                    identity::USERNAME_MIN_LEN,
                    identity::USERNAME_MAX_LEN
                ),
                identity::UsernameError::InvalidCharacters => eprintln!(
                    "Kullanıcı adı yalnızca harf, rakam, nokta, alt çizgi ve tire içerebilir."
                ),
            }
            return ExitCode::FAILURE;
        }
    };

    // Password handling: hidden prompt (confirmed twice) or stdin.
    let password = if password_stdin {
        let mut buffer = String::new();
        use std::io::Read;
        if std::io::stdin().read_to_string(&mut buffer).is_err() {
            eprintln!("parola stdin'den okunamadı");
            return ExitCode::FAILURE;
        }
        // Strip exactly one trailing newline from piped input.
        let trimmed = buffer.strip_suffix('\n').unwrap_or(&buffer);
        let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
        trimmed.to_string()
    } else {
        let first = match rpassword::prompt_password("Parola: ") {
            Ok(value) => value,
            Err(_) => {
                eprintln!("parola okunamadı");
                return ExitCode::FAILURE;
            }
        };
        let second = match rpassword::prompt_password("Parola (tekrar): ") {
            Ok(value) => value,
            Err(_) => {
                eprintln!("parola okunamadı");
                return ExitCode::FAILURE;
            }
        };
        if first != second {
            eprintln!("Parolalar eşleşmiyor.");
            return ExitCode::FAILURE;
        }
        first
    };

    if let Err(error) = identity::validate_password_policy(&password) {
        match error {
            identity::PasswordPolicyError::TooShort => eprintln!(
                "Parola en az {} karakter olmalıdır.",
                identity::PASSWORD_MIN_LEN
            ),
            identity::PasswordPolicyError::TooLong => eprintln!(
                "Parola en fazla {} karakter olabilir (kısaltılmaz).",
                identity::PASSWORD_MAX_LEN
            ),
        }
        return ExitCode::FAILURE;
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(url) = config.database_url.as_deref() else {
        eprintln!("create-user requires KOOPERATIF_DATABASE_URL");
        return ExitCode::FAILURE;
    };

    let pool = match db::connect(url).await {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("veritabanına bağlanılamadı: {error}");
            return ExitCode::FAILURE;
        }
    };

    // Hash with the configured (floor-enforced) Argon2id parameters.
    let runtime = AuthRuntime::new(config.auth.clone(), Arc::new(SystemClock));
    let password_hash = match identity::hash_password(&runtime.argon2, &password) {
        Ok(hash) => hash,
        Err(error) => {
            eprintln!("parola işlenemedi: {error}");
            return ExitCode::FAILURE;
        }
    };
    drop(password);

    match users::create_user(&pool, &username, display_name.trim(), &password_hash).await {
        Ok(user) => {
            audit::record(
                &pool,
                audit::SecurityEventType::UserCreated,
                Some(user.id),
                None,
                serde_json::json!({ "username": username, "source": "cli" }),
            )
            .await;
            eprintln!("Kullanıcı oluşturuldu: {}", user.username);
            ExitCode::SUCCESS
        }
        Err(users::CreateUserError::DuplicateUsername) => {
            eprintln!("'{username}' kullanıcı adı zaten kullanılıyor.");
            ExitCode::FAILURE
        }
        Err(users::CreateUserError::Database(error)) => {
            eprintln!("kullanıcı oluşturulamadı: {error}");
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
