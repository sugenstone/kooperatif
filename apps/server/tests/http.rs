//! HTTP integration tests against the real router (no network, no DB).
//!
//! Proves STEP-001 §18: backend health, readiness semantics, error
//! contract and the request-ID correlation foundation.

use axum::body::Body;
use axum::http::header::HeaderName;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use kooperatif_server::http::{cors_layer, router, AppState};

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

fn app_without_database() -> axum::Router {
    router(AppState { db: None }, cors_layer(&[]))
}

async fn get_json(app: axum::Router, path: &str) -> (StatusCode, Value, Option<String>) {
    let response = app
        .oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("infallible");
    let status = response.status();
    let request_id = response
        .headers()
        .get(X_REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readable body");
    let json = serde_json::from_slice(&body).expect("JSON body");
    (status, json, request_id)
}

#[tokio::test]
async fn health_answers_ok_without_any_dependency() {
    let (status, body, _) = get_json(app_without_database(), "/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "status": "ok" }));
}

#[tokio::test]
async fn readiness_reports_database_unconfigured_without_a_pool() {
    let (status, body, _) = get_json(app_without_database(), "/ready").await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        serde_json::json!({ "status": "not_ready", "checks": { "database": "unconfigured" } })
    );
}

#[tokio::test]
async fn unknown_routes_answer_the_stable_not_found_contract() {
    let (status, body, _) = get_json(app_without_database(), "/no/such/route").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        serde_json::json!({ "error": { "code": "not_found" } })
    );
}

#[tokio::test]
async fn every_response_carries_a_request_id_for_correlation() {
    let (_, _, request_id) = get_json(app_without_database(), "/health").await;

    let request_id = request_id.expect("x-request-id header present");
    assert!(!request_id.is_empty(), "request id must not be empty");
}
