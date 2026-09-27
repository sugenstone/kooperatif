//! Structured tracing initialization (infrastructure subset of ADR-011).
//!
//! Machine-readable JSON output in `test`/`production`, human-readable
//! output in `development`. Never log secrets, session tokens or
//! sensitive payloads (ADR-011, docs/22-SECURITY.md).

use crate::config::AppEnv;
use tracing_subscriber::EnvFilter;

const DEFAULT_FILTER: &str = "kooperatif_server=info,tower_http=info";

/// Initialize the global tracing subscriber. Logs to stdout.
pub fn init(env: AppEnv) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    match env {
        AppEnv::Development => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_writer(std::io::stdout)
                .init();
        }
        AppEnv::Test | AppEnv::Production => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(std::io::stdout)
                .init();
        }
    }
}
