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

use kooperatif_server::db;
use kooperatif_server::http::{cors_layer, router, AppState};

fn test_database_url() -> Option<String> {
    std::env::var("KOOPERATIF_TEST_DATABASE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[tokio::test]
async fn migrations_apply_to_a_clean_database_and_readiness_reports_ready() {
    let Some(url) = test_database_url() else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };

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

    let app = router(AppState { db: Some(pool) }, cors_layer(&[]));
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
