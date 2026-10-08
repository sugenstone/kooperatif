//! Real-time change-signal delivery (STEP-016).
//!
//! Canonical lifecycle (ADR-005, docs/27):
//!
//! ```text
//! Command → Authorization → DB Transaction → COMMIT
//!         → pg_notify (transactional) → RealtimeHub (transient)
//!         → authorized WebSocket client → canonical Read API refetch
//! ```
//!
//! A WebSocket event is an invalidation signal only. It is never the
//! transaction, never a financial record, never an audit record, never
//! an authorization decision. Clients always re-read the canonical
//! REST/reporting APIs; no payload row data, no derived totals and no
//! personal/financial values are ever broadcast (docs/22 minimization).
//!
//! Delivery guarantees, documented honestly:
//! * Commit boundary is guaranteed by PostgreSQL — `pg_notify` fires
//!   only after COMMIT and is discarded on ROLLBACK.
//! * Delivery to connected sockets is at-least-once best-effort per
//!   signal; after any loss/reconnect the client performs canonical
//!   snapshot resync (no durable replay claimed).
//! * LISTEN/NOTIFY is cluster-wide across any backend instances that
//!   share this database (ADR-012 single node today, multi-instance
//!   safe without additional infrastructure).

pub mod routes;

use std::collections::HashSet;
use std::sync::OnceLock;

use tokio::sync::broadcast;

/// PostgreSQL LISTEN/NOTIFY channel carrying transactional domain tags.
pub const NOTIFY_CHANNEL: &str = "kooperatif_domain_changed";

/// Logical domain tags emitted by the `kooperatif_notify_domain()`
/// trigger function (migration 0016). Each maps to exactly one read
/// permission from `authz::catalog`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    Shareholders,
    Families,
    Shares,
    Periods,
    Assessments,
    Payments,
    Credits,
    Accounts,
    IncomeExpense,
    ShareReturns,
    Investments,
    SocialAid,
    Governance,
}

impl Domain {
    /// Parses the trigger payload tag. Unknown tags are ignored —
    /// a malformed NOTIFY must not crash the listener.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "shareholders" => Self::Shareholders,
            "families" => Self::Families,
            "shares" => Self::Shares,
            "periods" => Self::Periods,
            "assessments" => Self::Assessments,
            "payments" => Self::Payments,
            "credits" => Self::Credits,
            "accounts" => Self::Accounts,
            "income_expense" => Self::IncomeExpense,
            "share_returns" => Self::ShareReturns,
            "investments" => Self::Investments,
            "social_aid" => Self::SocialAid,
            "governance" => Self::Governance,
            _ => return None,
        })
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::Shareholders => "shareholders",
            Self::Families => "families",
            Self::Shares => "shares",
            Self::Periods => "periods",
            Self::Assessments => "assessments",
            Self::Payments => "payments",
            Self::Credits => "credits",
            Self::Accounts => "accounts",
            Self::IncomeExpense => "income_expense",
            Self::ShareReturns => "share_returns",
            Self::Investments => "investments",
            Self::SocialAid => "social_aid",
            Self::Governance => "governance",
        }
    }

    /// Read permission required to receive invalidation for this
    /// domain — identical to the read permission of its canonical
    /// APIs, so visibility cannot exceed what REST already allows.
    pub fn read_permission(self) -> &'static str {
        use crate::auth::authz::catalog as c;
        match self {
            Self::Shareholders => c::SHAREHOLDERS_READ,
            Self::Families => c::FAMILIES_READ,
            Self::Shares => c::SHARES_READ,
            Self::Periods => c::PERIODS_READ,
            Self::Assessments => c::ASSESSMENTS_READ,
            Self::Payments => c::PAYMENTS_READ,
            Self::Credits => c::CREDITS_READ,
            Self::Accounts => c::FINANCIAL_ACCOUNTS_READ,
            Self::IncomeExpense => c::INCOME_EXPENSE_READ,
            Self::ShareReturns => c::SHARE_RETURNS_READ,
            Self::Investments => c::INVESTMENTS_READ,
            Self::SocialAid => c::SOCIAL_AID_READ,
            Self::Governance => c::GOVERNANCE_READ,
        }
    }
}

/// Every permission a client may subscribe to: any domain `*.read` plus
/// `reports.read` (the aggregate-reporting scope receives signals for
/// all domains, matching what `/api/reports/*` already exposes).
pub fn known_scopes() -> HashSet<&'static str> {
    use crate::auth::authz::catalog as c;
    [
        c::SHAREHOLDERS_READ,
        c::FAMILIES_READ,
        c::SHARES_READ,
        c::PERIODS_READ,
        c::ASSESSMENTS_READ,
        c::PAYMENTS_READ,
        c::CREDITS_READ,
        c::FINANCIAL_ACCOUNTS_READ,
        c::INCOME_EXPENSE_READ,
        c::SHARE_RETURNS_READ,
        c::INVESTMENTS_READ,
        c::SOCIAL_AID_READ,
        c::GOVERNANCE_READ,
        c::REPORTS_READ,
    ]
    .into_iter()
    .collect()
}

/// Transient signal inside the fan-out hub.
#[derive(Debug, Clone)]
pub enum HubSignal {
    /// A transaction touching this domain committed.
    Changed(Domain),
    /// The listener lost its DB link (or a client's queue lagged) —
    /// subscribers must re-read canonical state; nothing is assumed.
    Resync,
}

/// Bounded in-process broadcast between the NOTIFY listener task and
/// connected sockets. Capacity is deliberately small: a slow client
/// lags, receives `Resync`, and reloads canonical state — the server
/// never grows memory for slow consumers.
pub struct RealtimeHub {
    tx: broadcast::Sender<HubSignal>,
}

const HUB_CAPACITY: usize = 256;

impl RealtimeHub {
    pub fn publish(&self, signal: HubSignal) {
        // No connected sockets → SendError; that is fine, the next
        // subscriber resyncs from canonical state anyway.
        let _ = self.tx.send(signal);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HubSignal> {
        self.tx.subscribe()
    }
}

/// Process-wide hub. The hub carries no state and no durable
/// responsibility, so a lazily shared instance is safe for both the
/// server and tests.
pub fn hub() -> &'static RealtimeHub {
    static HUB: OnceLock<RealtimeHub> = OnceLock::new();
    HUB.get_or_init(|| RealtimeHub {
        tx: broadcast::channel(HUB_CAPACITY).0,
    })
}

/// LISTEN loop with reconnect. Holds its own dedicated PG connection
/// (NOT a pool checkout) so the listener cannot starve request traffic.
/// On reconnect publishes `Resync`: the gap between disconnect and
/// re-LISTEN may have contained commits that were never notified.
pub async fn listen_loop(database_url: String) -> ! {
    let mut backoff_ms: u64 = 200;
    let mut ever_connected = false;
    loop {
        match sqlx::postgres::PgListener::connect(&database_url).await {
            Ok(mut listener) => {
                if listener.listen(NOTIFY_CHANNEL).await.is_ok() {
                    if ever_connected {
                        // We were listening before; commits during the
                        // gap produced no notification here.
                        hub().publish(HubSignal::Resync);
                    }
                    ever_connected = true;
                    backoff_ms = 200;
                    dispatch(&mut listener).await;
                    // dispatch() returns only on connection loss.
                    tracing::warn!("realtime listener lost database connection; reconnecting");
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "realtime listener connect failed; retrying");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms * 2).min(10_000);
    }
}

async fn dispatch(listener: &mut sqlx::postgres::PgListener) {
    loop {
        match listener.recv().await {
            Ok(notification) => {
                if let Some(domain) = Domain::from_tag(notification.payload()) {
                    hub().publish(HubSignal::Changed(domain));
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "realtime listener recv failed");
                return;
            }
        }
    }
}
