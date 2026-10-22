//! Durable security/audit events (STEP-002 §25).
//!
//! Boundary (ADR-011): these rows are AUDIT records — durable business
//! history. Tracing output is observability and may be lost. Never
//! stored here: passwords, raw session tokens, cookie values, CSRF
//! secrets, password hashes (docs/19, docs/22).

use crate::http::error::ApiError;
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
    UserDisabled,
    UserEnabled,
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
    ShareholderDefaultAccountChanged,
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
    IncomePosted,
    IncomeReversed,
    ExpensePosted,
    ExpenseReversed,
    FinancialCategoryCreated,
    FinancialCategoryUpdated,
    FinancialCategoryStatusChanged,
    ShareReturnRequested,
    ShareReturnCancelled,
    ShareReturnFinalized,
    ShareReturnEntitlementRecognized,
    ShareReturnEntitlementDetermined,
    ShareReturnEntitlementCancelled,
    ShareReturnSettlementPosted,
    ShareReturnSettlementReversed,
    InvestmentCreated,
    InvestmentCancelled,
    InvestmentFundingPosted,
    InvestmentFundingReversed,
    InvestmentValuationRecorded,
    InvestmentValuationCancelled,
    InvestmentIncomePosted,
    InvestmentIncomeReversed,
    InvestmentDisposed,
    SocialAidFundCreated,
    SocialAidFundClosed,
    SocialAidFundCancelled,
    SocialAidDonationPosted,
    SocialAidDonationReversed,
    SocialAidDisbursementPosted,
    SocialAidDisbursementReversed,
    GovernanceBodyCreated,
    GovernanceBodyClosed,
    GovernanceMembershipStarted,
    GovernanceMembershipEnded,
    GovernanceDecisionCreated,
    GovernanceDecisionUpdated,
    GovernanceDecisionOpened,
    GovernanceDecisionCancelled,
    GovernanceVoteRecorded,
    GovernanceDecisionFinalized,
    CooperativeCreated,
    CooperativeBootstrapped,
    CooperativeMembershipGranted,
    CooperativeContextSwitched,
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
            Self::UserDisabled => "user_disabled",
            Self::UserEnabled => "user_enabled",
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
            Self::ShareholderDefaultAccountChanged => "shareholder_default_account_changed",
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
            Self::IncomePosted => "income_posted",
            Self::IncomeReversed => "income_reversed",
            Self::ExpensePosted => "expense_posted",
            Self::ExpenseReversed => "expense_reversed",
            Self::FinancialCategoryCreated => "financial_category_created",
            Self::FinancialCategoryUpdated => "financial_category_updated",
            Self::FinancialCategoryStatusChanged => "financial_category_status_changed",
            Self::ShareReturnRequested => "share_return_requested",
            Self::ShareReturnCancelled => "share_return_cancelled",
            Self::ShareReturnFinalized => "share_return_finalized",
            Self::ShareReturnEntitlementRecognized => "share_return_entitlement_recognized",
            Self::ShareReturnEntitlementDetermined => "share_return_entitlement_determined",
            Self::ShareReturnEntitlementCancelled => "share_return_entitlement_cancelled",
            Self::ShareReturnSettlementPosted => "share_return_settlement_posted",
            Self::ShareReturnSettlementReversed => "share_return_settlement_reversed",
            Self::InvestmentCreated => "investment_created",
            Self::InvestmentCancelled => "investment_cancelled",
            Self::InvestmentFundingPosted => "investment_funding_posted",
            Self::InvestmentFundingReversed => "investment_funding_reversed",
            Self::InvestmentValuationRecorded => "investment_valuation_recorded",
            Self::InvestmentValuationCancelled => "investment_valuation_cancelled",
            Self::InvestmentIncomePosted => "investment_income_posted",
            Self::InvestmentIncomeReversed => "investment_income_reversed",
            Self::InvestmentDisposed => "investment_disposed",
            Self::SocialAidFundCreated => "social_aid_fund_created",
            Self::SocialAidFundClosed => "social_aid_fund_closed",
            Self::SocialAidFundCancelled => "social_aid_fund_cancelled",
            Self::SocialAidDonationPosted => "social_aid_donation_posted",
            Self::SocialAidDonationReversed => "social_aid_donation_reversed",
            Self::SocialAidDisbursementPosted => "social_aid_disbursement_posted",
            Self::SocialAidDisbursementReversed => "social_aid_disbursement_reversed",
            Self::GovernanceBodyCreated => "governance_body_created",
            Self::GovernanceBodyClosed => "governance_body_closed",
            Self::GovernanceMembershipStarted => "governance_membership_started",
            Self::GovernanceMembershipEnded => "governance_membership_ended",
            Self::GovernanceDecisionCreated => "governance_decision_created",
            Self::GovernanceDecisionUpdated => "governance_decision_updated",
            Self::GovernanceDecisionOpened => "governance_decision_opened",
            Self::GovernanceDecisionCancelled => "governance_decision_cancelled",
            Self::GovernanceVoteRecorded => "governance_vote_recorded",
            Self::GovernanceDecisionFinalized => "governance_decision_finalized",
            Self::CooperativeCreated => "cooperative_created",
            Self::CooperativeBootstrapped => "cooperative_bootstrapped",
            Self::CooperativeMembershipGranted => "cooperative_membership_granted",
            Self::CooperativeContextSwitched => "cooperative_context_switched",
        }
    }
}

/// MANDATORY lane (S2B, migration 0048): the event is written INSIDE
/// the caller's sealed request transaction — it commits atomically with
/// the protected operation and rolls back with it. Actor, session and
/// cooperative attribution are derived from the verified context seal
/// in the database (`app_ctx_audit`); the caller supplies only the
/// event type and metadata. A write failure fails the operation —
/// callers must propagate with `?`, never swallow.
pub async fn record_tx(
    conn: &mut sqlx::PgConnection,
    event_type: SecurityEventType,
    metadata: serde_json::Value,
) -> Result<(), ApiError> {
    sqlx::query("SELECT public.app_ctx_audit($1, $2)")
        .bind(event_type.as_str())
        .bind(metadata)
        .execute(&mut *conn)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, event = event_type.as_str(), "mandatory security event write failed");
            ApiError::Internal
        })?;
    Ok(())
}

/// Session-bound platform event (S2B best-effort): the definer derives
/// the actor and session from the presented token hash and forces
/// cooperative attribution to NULL. Used where an audit write must not
/// block the action it describes (login success). Failures are loud
/// (`tracing::error`) but never change the response.
pub async fn record_platform_session(
    pool: &PgPool,
    session_token_hash: &[u8],
    event_type: SecurityEventType,
    metadata: serde_json::Value,
    now: time::OffsetDateTime,
) {
    let result = sqlx::query("SELECT public.app_session_audit_platform($1, $2, $3, $4)")
        .bind(session_token_hash)
        .bind(event_type.as_str())
        .bind(metadata)
        .bind(now)
        .execute(pool)
        .await;
    if let Err(error) = result {
        tracing::error!(error = %error, event = event_type.as_str(), "session-bound platform event write failed");
    }
}

/// Session-bound cooperative event (S2B best-effort): for denial events
/// whose protected transaction was already rolled back
/// (lockout_prevented). The definer re-verifies the session and an
/// active membership in the claimed cooperative — cross-cooperative
/// attribution is rejected server-side. Failures are loud but cannot
/// and must not change the denial outcome.
pub async fn record_scoped_session(
    pool: &PgPool,
    session_token_hash: &[u8],
    event_type: SecurityEventType,
    cooperative_id: Option<Uuid>,
    metadata: serde_json::Value,
    now: time::OffsetDateTime,
) {
    let result = sqlx::query("SELECT public.app_session_audit_scoped($1, $2, $3, $4, $5)")
        .bind(session_token_hash)
        .bind(cooperative_id)
        .bind(event_type.as_str())
        .bind(metadata)
        .bind(now)
        .execute(pool)
        .await;
    if let Err(error) = result {
        tracing::error!(error = %error, event = event_type.as_str(), "session-bound scoped event write failed");
    }
}

/// Unauthenticated login-failure lane (S2B best-effort): the definer
/// hardcodes event_type `login_failed` and NULL attribution — the
/// caller can only supply the metadata payload. Authentication must
/// never fail because an audit write did.
pub async fn record_login_failed(
    pool: &PgPool,
    metadata: serde_json::Value,
    now: time::OffsetDateTime,
) {
    let result = sqlx::query("SELECT public.app_audit_login_failed($1, $2)")
        .bind(metadata)
        .bind(now)
        .execute(pool)
        .await;
    if let Err(error) = result {
        tracing::error!(error = %error, "login_failed event write failed");
    }
}

/// Record one durable security event. Failures are logged and swallowed
/// by design: an audit-write failure must not flip the security outcome
/// of an operation that already happened (and must never 500 a login).
///
/// OWNER-LANE ONLY since S2B: this direct INSERT is retained for the
/// privileged administration/CLI pool (main.rs). Runtime-pool callers
/// go through `record_tx` / `record_*_session` / `record_login_failed` —
/// `kooperatif_app` no longer holds INSERT on `security_events`.
pub async fn record(
    pool: &PgPool,
    event_type: SecurityEventType,
    user_id: Option<Uuid>,
    session_id: Option<Uuid>,
    metadata: serde_json::Value,
) {
    record_scoped(pool, event_type, None, user_id, session_id, metadata).await;
}

/// Tenant-attributed variant (M1-P0): `cooperative_id` is Some(_) for
/// cooperative-scope events, NULL for platform-scope events (login,
/// user lifecycle). From P1/P2 every coop-bound call site is migrated
/// to this variant.
pub async fn record_scoped(
    pool: &PgPool,
    event_type: SecurityEventType,
    cooperative_id: Option<Uuid>,
    user_id: Option<Uuid>,
    session_id: Option<Uuid>,
    metadata: serde_json::Value,
) {
    let result = sqlx::query(
        "INSERT INTO security_events \
             (event_type, cooperative_id, user_id, session_id, metadata) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(event_type.as_str())
    .bind(cooperative_id)
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
