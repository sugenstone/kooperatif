//! HTTP integration tests against the real router (no network, no DB).
//!
//! Proves STEP-001 §18: backend health, readiness semantics, error
//! contract, request-ID correlation — plus STEP-002's dependency
//! semantics for auth endpoints without a database.

use std::sync::Arc;

use axum::body::Body;
use axum::http::header::HeaderName;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use kooperatif_server::auth::AuthRuntime;
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::http::{cors_layer, router, AppState};

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

fn app_without_database() -> axum::Router {
    let auth = AuthRuntime::new(
        AuthConfig {
            allowed_origins: vec!["http://localhost:5173".to_string()],
            cookie_secure: false,
            session_absolute_ttl_secs: 3600,
            session_idle_ttl_secs: 600,
            argon2_m_cost: ARGON2_M_COST_FLOOR, // fastest legal parameters
            argon2_t_cost: 1,
            argon2_p_cost: 1,
            login_window_secs: 60,
            login_username_max_attempts: 5,
            login_ip_max_attempts: 100,
            forwarded_ip: false,
        },
        Arc::new(MutableClock::new(time::OffsetDateTime::now_utc())),
    );
    router(
        AppState {
            db: None,
            auth: Arc::new(auth),
        },
        cors_layer(&[]),
    )
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

#[tokio::test]
async fn auth_endpoints_fail_closed_when_the_database_is_missing() {
    // Without the database nothing can be authenticated: every auth
    // request answers 503 with the stable dependency code — the
    // extractor fails closed before cookie parsing.
    let (status, body, _) = get_json(app_without_database(), "/api/auth/me").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "dependency_unavailable");

    // A well-formed login against a missing database answers 503 with
    // the stable dependency code, never a raw connection error.
    // (ConnectInfo mirrors the production serve wiring.)
    let mut request = Request::post("/api/auth/login")
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "username": "kullanici", "password": "parola-parola" }).to_string(),
        ))
        .expect("valid request");
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo::<std::net::SocketAddr>(
            "127.0.0.1:0".parse().expect("static addr"),
        ));
    let response = app_without_database()
        .oneshot(request)
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readable body");
    let json: Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(json["error"]["code"], "dependency_unavailable");
}

#[tokio::test]
async fn every_response_carries_baseline_security_headers() {
    // STEP-017 §27: hardening headers on API responses, including errors.
    let app = app_without_database();
    for path in ["/health", "/api/auth/me", "/nonexistent"] {
        let response = app
            .clone()
            .oneshot(
                Request::get(path)
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("infallible");
        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff",
            "{path} missing nosniff"
        );
        assert_eq!(
            response.headers().get("x-frame-options").unwrap(),
            "DENY",
            "{path} missing frame denial"
        );
        assert_eq!(
            response.headers().get("referrer-policy").unwrap(),
            "no-referrer",
            "{path} missing referrer policy"
        );
    }
}
