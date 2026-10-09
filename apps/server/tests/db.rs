//! Database-gated integration tests (STEP-001 §18: database
//! connectivity/migration).
//!
//! Requires `KOOPERATIF_TEST_DATABASE_URL` pointing at a disposable
//! database (scripts/db-verify.sh and the CI database job provision one).
//! Without the variable these tests skip with an explicit notice —
//! plain `cargo test` stays deterministic and dependency-free.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use std::sync::Arc;

use kooperatif_server::auth::AuthRuntime;
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::AuthConfig;
use kooperatif_server::db;
use kooperatif_server::http::{cors_layer, router, AppState};

/// Required DB-gated test gate (PILOT-FIX-001 / F5): a missing
/// KOOPERATIF_TEST_DATABASE_URL is an explicit FAILURE, never a
/// silent skip — CI and release gates must prove these tests ran.
/// Unit-only execution stays unaffected: this function is only
/// reached by database-gated setup paths.
fn test_database_url() -> Option<String> {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated              integration tests cannot silently pass; point it at a              disposable PostgreSQL database"
        )
    });
    let trimmed = value.trim().to_string();
    assert!(
        !trimmed.is_empty(),
        "KOOPERATIF_TEST_DATABASE_URL is empty — required DB-gated tests need a database"
    );
    Some(trimmed)
}

#[tokio::test]
async fn migrations_apply_to_a_clean_database_and_readiness_reports_ready() {
    // PILOT-FIX-001 / F5: test_database_url() fails loudly when unset.
    let url = test_database_url().expect("test database url");

    let pool = db::connect(&url).await.expect("database connectivity");

    db::run_migrations(&pool).await.expect("migrations apply");
    // Re-running must be a no-op (deterministic, idempotent mechanism).
    db::run_migrations(&pool)
        .await
        .expect("migrations re-run is a no-op");

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("migration ledger readable");
    assert!(
        applied >= 1,
        "at least one migration must be recorded, got {applied}"
    );

    let app = router(
        AppState {
            db: Some(pool),
            auth: Arc::new(AuthRuntime::new(
                AuthConfig {
                    allowed_origins: vec!["http://localhost:5173".to_string()],
                    cookie_secure: false,
                    session_absolute_ttl_secs: 3600,
                    session_idle_ttl_secs: 600,
                    argon2_m_cost: kooperatif_server::config::ARGON2_M_COST_FLOOR,
                    argon2_t_cost: 1,
                    argon2_p_cost: 1,
                    login_window_secs: 60,
                    login_username_max_attempts: 5,
                    login_ip_max_attempts: 100,
                    forwarded_ip: false,
                },
                Arc::new(MutableClock::new(time::OffsetDateTime::now_utc())),
            )),
        },
        cors_layer(&[]),
    );
    let response = app
        .oneshot(
            Request::get("/ready")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("infallible");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readable body");
    let readiness: Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(
        readiness,
        serde_json::json!({ "status": "ready", "checks": { "database": "ok" } })
    );
}
