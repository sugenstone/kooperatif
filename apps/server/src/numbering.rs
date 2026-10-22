//! M1-P3 — cooperative-local document numbering allocator.
//!
//! Thin, typed surface over `allocate_document_number` /
//! `reserve_document_number` (migration 0026). Every call must run on
//! the transaction that performs the business write — the counter row
//! is locked inside that transaction, so allocation commits or rolls
//! back with the document and can never produce duplicates or gaps.
//!
//! `families.sequence_number` is deliberately NOT a series: it is
//! operator-supplied data enforced only by
//! `families_coop_sequence_number_uq` (docs/M1-P3 design, §5).

use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// The 20 automatically generated numbering series — compile-time
/// whitelist mirroring `document_number_series`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberingSeries {
    AccountTransferNumber,
    ExpenseNumber,
    GovernanceBodyNumber,
    GovernanceDecisionNumber,
    IncomeEntryNumber,
    InvestmentDisposalNumber,
    InvestmentFundingNumber,
    InvestmentIncomeNumber,
    InvestmentValuationNumber,
    InvestmentNumber,
    PaymentNumber,
    PeriodNumber,
    ShareReturnEntitlementNumber,
    ShareReturnSettlementNumber,
    ShareReturnNumber,
    ShareholderCreditNumber,
    ShareNumber,
    SocialAidDisbursementNumber,
    SocialAidDonationNumber,
    SocialAidFundNumber,
}

impl NumberingSeries {
    /// The `document_number_series.series_key` value.
    pub fn as_key(self) -> &'static str {
        match self {
            Self::AccountTransferNumber => "account_transfers.transfer_number",
            Self::ExpenseNumber => "expense_entries.expense_number",
            Self::GovernanceBodyNumber => "governance_bodies.body_number",
            Self::GovernanceDecisionNumber => "governance_decisions.decision_number",
            Self::IncomeEntryNumber => "income_entries.income_number",
            Self::InvestmentDisposalNumber => "investment_disposals.disposal_number",
            Self::InvestmentFundingNumber => "investment_fundings.funding_number",
            Self::InvestmentIncomeNumber => "investment_incomes.income_number",
            Self::InvestmentValuationNumber => "investment_valuations.valuation_number",
            Self::InvestmentNumber => "investments.investment_number",
            Self::PaymentNumber => "payments.payment_number",
            Self::PeriodNumber => "periods.period_number",
            Self::ShareReturnEntitlementNumber => "share_return_entitlements.entitlement_number",
            Self::ShareReturnSettlementNumber => "share_return_settlements.settlement_number",
            Self::ShareReturnNumber => "share_returns.return_number",
            Self::ShareholderCreditNumber => "shareholder_credits.credit_number",
            Self::ShareNumber => "shares.share_number",
            Self::SocialAidDisbursementNumber => "social_aid_disbursements.disbursement_number",
            Self::SocialAidDonationNumber => "social_aid_donations.donation_number",
            Self::SocialAidFundNumber => "social_aid_funds.fund_number",
        }
    }
}

/// Allocate the next document number for `series` inside
/// `cooperative_id`. The returned counter row stays locked until the
/// caller's transaction ends — callers MUST run this on the same
/// transaction as the document insert.
pub async fn allocate<'e, E>(
    executor: E,
    cooperative_id: Uuid,
    series: NumberingSeries,
) -> Result<i64, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query_scalar::<_, i64>("SELECT allocate_document_number($1, $2)")
        .bind(cooperative_id)
        .bind(series.as_key())
        .fetch_one(executor)
        .await
}

/// Reserve `value`: raise the (cooperative, series) floor to
/// `value + 1` so the allocator never reissues a number that arrived
/// via import, manual entry or administrative correction. Returns the
/// resulting floor. Lowering a counter is not possible.
pub async fn reserve<'e, E>(
    executor: E,
    cooperative_id: Uuid,
    series: NumberingSeries,
    value: i64,
) -> Result<i64, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query_scalar::<_, i64>("SELECT reserve_document_number($1, $2, $3)")
        .bind(cooperative_id)
        .bind(series.as_key())
        .bind(value)
        .fetch_one(executor)
        .await
}
