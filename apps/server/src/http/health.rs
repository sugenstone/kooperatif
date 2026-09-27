//! Health and readiness endpoints (infrastructure subset of ADR-011).
//!
//! Distinct semantics:
//! - `GET /health` — "the process is alive". Never touches PostgreSQL
//!   and answers 200 whenever the router can respond at all.
//! - `GET /ready` — "this instance can serve traffic". Answers 503 with
//!   a per-dependency report when a required dependency (currently
//!   PostgreSQL) is unconfigured or unreachable.
//!
//! Bodies match `@kooperatif/contracts` (`packages/contracts/src/health.ts`).

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use sqlx::PgPool;

use crate::http::AppState;

/// Probe timeout for the readiness dependency check.
const READINESS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Serialize)]
pub struct ReadinessChecks {
    pub database: &'static str,
}

#[derive(Serialize)]
pub struct ReadinessResponse {
    pub status: &'static str,
    pub checks: ReadinessChecks,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn ready(State(state): State<AppState>) -> (StatusCode, Json<ReadinessResponse>) {
    match database_status(state.db.as_ref()).await {
        DatabaseStatus::Ok => (
            StatusCode::OK,
            Json(ReadinessResponse {
                status: "ready",
                checks: ReadinessChecks { database: "ok" },
            }),
        ),
        status => {
            let database = match status {
                DatabaseStatus::Ok => unreachable!("handled above"),
                DatabaseStatus::Unconfigured => "unconfigured",
                DatabaseStatus::Unavailable => "unavailable",
            };
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ReadinessResponse {
                    status: "not_ready",
                    checks: ReadinessChecks { database },
                }),
            )
        }
    }
}

enum DatabaseStatus {
    Ok,
    Unconfigured,
    Unavailable,
}

async fn database_status(pool: Option<&PgPool>) -> DatabaseStatus {
    let Some(pool) = pool else {
        return DatabaseStatus::Unconfigured;
    };
    let probe = sqlx::query("SELECT 1").execute(pool);
    match tokio::time::timeout(READINESS_PROBE_TIMEOUT, probe).await {
        Ok(Ok(_)) => DatabaseStatus::Ok,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "readiness database probe failed");
            DatabaseStatus::Unavailable
        }
        Err(_) => {
            tracing::warn!("readiness database probe timed out");
            DatabaseStatus::Unavailable
        }
    }
}
