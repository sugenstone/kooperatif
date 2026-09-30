//! Durable security/audit events (STEP-002 §25).
//!
//! Boundary (ADR-011): these rows are AUDIT records — durable business
//! history. Tracing output is observability and may be lost. Never
//! stored here: passwords, raw session tokens, cookie values, CSRF
//! secrets, password hashes (docs/19, docs/22).

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub enum SecurityEventType {
    LoginSucceeded,
    LoginFailed,
    Logout,
    SessionRevoked,
    SessionsRevokedAllOthers,
    UserCreated,
    RoleCreated,
    RoleUpdated,
    RoleEnabled,
    RoleDisabled,
    RolePermissionsChanged,
    RoleAssigned,
    RoleUnassigned,
    BootstrapRoleGranted,
    LockoutPrevented,
    PersonCreated,
    PersonUpdated,
    FamilyCreated,
    ShareholderCreated,
    ShareholderUpdated,
    ShareholderStatusChanged,
    ShareholderFamilyChanged,
    ShareCreated,
    ShareTransferred,
    ShareSold,
    ShareStatusChanged,
    PeriodCreated,
    PeriodUpdated,
    PeriodDeleted,
    PeriodClosed,
    AssessmentsGenerated,
    PaymentPosted,
    PaymentAllocationsAdded,
    PaymentReversed,
    PaymentAllocationReversed,
    FinancialAccountCreated,
    FinancialAccountUpdated,
    FinancialAccountStatusChanged,
    AccountTransferPosted,
    AccountTransferReversed,
    ShareholderCreditAssigned,
    ShareholderCreditReversed,
    CreditApplied,
    CreditApplicationReversed,
}

impl SecurityEventType {
    fn as_str(self) -> &'static str {
        match self {
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::Logout => "logout",
            Self::SessionRevoked => "session_revoked",
            Self::SessionsRevokedAllOthers => "sessions_revoked_all_others",
            Self::UserCreated => "user_created",
            Self::RoleCreated => "role_created",
            Self::RoleUpdated => "role_updated",
            Self::RoleEnabled => "role_enabled",
            Self::RoleDisabled => "role_disabled",
            Self::RolePermissionsChanged => "role_permissions_changed",
            Self::RoleAssigned => "role_assigned",
            Self::RoleUnassigned => "role_unassigned",
            Self::BootstrapRoleGranted => "bootstrap_role_granted",
            Self::LockoutPrevented => "lockout_prevented",
            Self::PersonCreated => "person_created",
            Self::PersonUpdated => "person_updated",
            Self::FamilyCreated => "family_created",
            Self::ShareholderCreated => "shareholder_created",
            Self::ShareholderUpdated => "shareholder_updated",
            Self::ShareholderStatusChanged => "shareholder_status_changed",
            Self::ShareholderFamilyChanged => "shareholder_family_changed",
            Self::ShareCreated => "share_created",
            Self::ShareTransferred => "share_transferred",
            Self::ShareSold => "share_sold",
            Self::ShareStatusChanged => "share_status_changed",
            Self::PeriodCreated => "period_created",
            Self::PeriodUpdated => "period_updated",
            Self::PeriodDeleted => "period_deleted",
            Self::PeriodClosed => "period_closed",
            Self::AssessmentsGenerated => "assessments_generated",
            Self::PaymentPosted => "payment_posted",
            Self::PaymentAllocationsAdded => "payment_allocations_added",
            Self::PaymentReversed => "payment_reversed",
            Self::PaymentAllocationReversed => "payment_allocation_reversed",
            Self::FinancialAccountCreated => "financial_account_created",
            Self::FinancialAccountUpdated => "financial_account_updated",
            Self::FinancialAccountStatusChanged => "financial_account_status_changed",
            Self::AccountTransferPosted => "account_transfer_posted",
            Self::AccountTransferReversed => "account_transfer_reversed",
            Self::ShareholderCreditAssigned => "shareholder_credit_assigned",
            Self::ShareholderCreditReversed => "shareholder_credit_reversed",
            Self::CreditApplied => "credit_applied",
            Self::CreditApplicationReversed => "credit_application_reversed",
        }
    }
}

/// Record one durable security event. Failures are logged and swallowed
/// by design: an audit-write failure must not flip the security outcome
/// of an operation that already happened (and must never 500 a login).
pub async fn record(
    pool: &PgPool,
    event_type: SecurityEventType,
    user_id: Option<Uuid>,
    session_id: Option<Uuid>,
    metadata: serde_json::Value,
) {
    let result = sqlx::query(
        "INSERT INTO security_events (event_type, user_id, session_id, metadata) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(event_type.as_str())
    .bind(user_id)
    .bind(session_id)
    .bind(metadata)
    .execute(pool)
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, event = event_type.as_str(), "security event write failed");
    }
}

/// Convenience: failed-login reason categories (safe for storage).
pub fn login_failed_metadata(normalized_username: &str, reason: &str) -> serde_json::Value {
    json!({ "username": normalized_username, "reason": reason })
}
