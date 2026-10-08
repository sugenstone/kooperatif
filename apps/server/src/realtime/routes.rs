//! `/api/realtime` — authenticated WebSocket endpoint (STEP-016).
//!
//! Security posture (docs/22, §8–§10):
//! * Handshake uses the SAME session cookie pipeline as every API —
//!   `CurrentAuth` runs full classify/touch before upgrade.
//! * Origin is enforced unconditionally here (a WS upgrade is GET, so
//!   the CSRF layer's safe-method exemption does not apply).
//! * No token is ever accepted in the URL or query string.
//! * Subscriptions are permission-scoped server-side; permissions are
//!   RE-READ from PostgreSQL for every delivered signal, and session
//!   validity is re-checked per signal and per heartbeat tick — a
//!   revoked/expired session or lost permission terminates delivery
//!   (close 1008), it never rides a stale socket.

use std::collections::HashSet;

use axum::{
    extract::{ws::Message, ws::WebSocketUpgrade, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::auth::authz;
use crate::auth::cookies;
use crate::auth::extractor::CurrentAuth;
use crate::auth::session::{self, SessionRejection};
use crate::auth::token;
use crate::http::{error::ApiError, AppState};
use crate::realtime::{hub, known_scopes, HubSignal};

/// Client → server subscription request. `scopes` are permission keys
/// (`payments.read`, `reports.read`, …); the server intersects the
/// request with the caller's effective permissions.
#[derive(Debug, Deserialize)]
struct SubscribeRequest {
    scopes: Vec<String>,
}

/// Server → client envelopes. Invalidation signals only — never row
/// data, never amounts, never person identifiers.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ServerEvent {
    /// A transaction touching `domain` committed; re-read canonical APIs.
    #[serde(rename_all = "camelCase")]
    DataChanged {
        domain: &'static str,
        occurred_at: String,
    },
    /// Delivery is known-incomplete (listener reconnect, client lag):
    /// perform a full canonical snapshot resync.
    #[serde(rename_all = "camelCase")]
    Resync {
        reason: &'static str,
        occurred_at: String,
    },
    /// Subscription acknowledged; `granted` may be narrower than asked.
    #[serde(rename_all = "camelCase")]
    Subscribed {
        granted: Vec<String>,
        occurred_at: String,
    },
}

const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(30);
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn realtime_router() -> Router<AppState> {
    Router::new().route(
        "/api/realtime",
        get(websocket_upgrade).route_layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        )),
    )
}

async fn websocket_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    auth: CurrentAuth,
) -> Result<Response, ApiError> {
    if state.db.is_none() {
        return Err(ApiError::DependencyUnavailable);
    }
    // Unconditional Origin allowlist — WS upgrades are GET and thus
    // exempt from the CSRF origin check (docs/22 cross-origin safety).
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let allowed = state
        .auth
        .config
        .allowed_origins
        .iter()
        .any(|o| Some(o.as_str()) == origin);
    if !allowed {
        return Err(ApiError::PermissionDenied);
    }
    let raw_token =
        cookies::extract_session_token(&headers).ok_or(ApiError::AuthenticationRequired)?;
    let token_hash = token::hash_token(&raw_token);
    Ok(ws
        .on_upgrade(move |socket| run_connection(state, socket, auth.user_id, token_hash))
        .into_response())
}

/// Re-checks session validity AND fresh effective permissions — one
/// round-trip pair per delivered signal. Returns `None` when the
/// session is no longer valid (revoked/expired/disabled).
async fn revalidate(
    pool: &PgPool,
    token_hash: [u8; 32],
    user_id: Uuid,
    now: OffsetDateTime,
) -> Option<HashSet<String>> {
    match session::classify_session(pool, token_hash, now).await {
        Ok(Ok(_)) => {}
        Ok(Err(
            SessionRejection::Unknown
            | SessionRejection::Revoked
            | SessionRejection::Expired
            | SessionRejection::IdleExpired
            | SessionRejection::UserDisabled,
        )) => return None,
        Err(error) => {
            // Fail closed on lookup errors: delivery halts, client
            // resyncs on reconnect — never silently continue.
            tracing::error!(error = %error, "realtime session revalidation failed");
            return None;
        }
    }
    match authz::effective_permissions(pool, user_id).await {
        Ok(perms) => Some(perms),
        Err(error) => {
            tracing::error!(error = %error, "realtime permission lookup failed");
            None
        }
    }
}

async fn run_connection(
    state: AppState,
    socket: axum::extract::ws::WebSocket,
    user_id: Uuid,
    token_hash: [u8; 32],
) {
    let Some(pool) = state.db.clone() else { return };
    let (mut sink, mut stream) = socket.split();
    // Subscribe BEFORE the client fetches its snapshot — closes the
    // subscribe/snapshot race: commits after this point are signaled.
    let mut hub_rx = hub().subscribe();
    let mut granted: HashSet<&'static str> = HashSet::new();
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            inbound = stream.next() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    let Ok(request) = serde_json::from_str::<SubscribeRequest>(&text) else { continue };
                    let known = known_scopes();
                    let Ok(effective) = authz::effective_permissions(&pool, user_id).await else {
                        close(&mut sink, 1011, "internal").await;
                        return;
                    };
                    granted = request
                        .scopes
                        .iter()
                        .filter_map(|s| known.get(s.as_str()).copied())
                        .filter(|s| effective.contains(*s))
                        .collect();
                    let event = ServerEvent::Subscribed {
                        granted: granted.iter().map(|s| s.to_string()).collect(),
                        occurred_at: now_rfc3339(),
                    };
                    if !send_json(&mut sink, &event).await { return; }
                }
                Some(Ok(Message::Close(_))) | None => return,
                Some(Ok(_)) => {}   // ping/pong/binary — ignored
                Some(Err(_)) => return,
            },
            signal = hub_rx.recv() => match signal {
                Ok(HubSignal::Changed(domain)) => {
                    let now = state.auth.clock.now();
                    // Fresh session + fresh permission for THIS delivery.
                    let Some(effective) = revalidate(&pool, token_hash, user_id, now).await else {
                        close(&mut sink, 1008, "session").await;
                        return;
                    };
                    let permitted = granted.contains(domain.read_permission())
                        || granted.contains(authz::catalog::REPORTS_READ);
                    // Authorization is rechecked against live
                    // permissions, not the set cached at subscribe time.
                    let authorized = permitted
                        && (effective.contains(domain.read_permission())
                            || effective.contains(authz::catalog::REPORTS_READ));
                    if authorized {
                        let event = ServerEvent::DataChanged {
                            domain: domain.tag(),
                            occurred_at: now_rfc3339(),
                        };
                        if !send_json(&mut sink, &event).await { return; }
                    }
                }
                Ok(HubSignal::Resync) => {
                    let event = ServerEvent::Resync { reason: "listener", occurred_at: now_rfc3339() };
                    if !send_json(&mut sink, &event).await { return; }
                }
                Err(RecvError::Lagged(_)) => {
                    // Bounded queue overflowed — tell the client to
                    // resync instead of pretending it is current.
                    let event = ServerEvent::Resync { reason: "lagged", occurred_at: now_rfc3339() };
                    if !send_json(&mut sink, &event).await { return; }
                }
                Err(RecvError::Closed) => return,
            },
            _ = heartbeat.tick() => {
                // Session revocation/expiry/disable must terminate a
                // live socket even while no domain events flow.
                if revalidate(&pool, token_hash, user_id, state.auth.clock.now()).await.is_none() {
                    close(&mut sink, 1008, "session").await;
                    return;
                }
                if tokio::time::timeout(SEND_TIMEOUT, sink.send(Message::Ping(Vec::new().into())))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

/// Bounded send: a wedged/slow client cannot stall this connection
/// forever; exceeding SEND_TIMEOUT drops it (client then resyncs).
async fn send_json(
    sink: &mut futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>,
    event: &ServerEvent,
) -> bool {
    let Ok(text) = serde_json::to_string(event) else {
        return true;
    };
    tokio::time::timeout(SEND_TIMEOUT, sink.send(Message::Text(text.into())))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

async fn close(
    sink: &mut futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>,
    code: u16,
    reason: &'static str,
) {
    let _ = tokio::time::timeout(
        SEND_TIMEOUT,
        sink.send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.into(),
        }))),
    )
    .await;
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
