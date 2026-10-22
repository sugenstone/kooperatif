-- M1-P4-R3 — financial history & grant-surface hardening (closure-audit F4).
--
-- The audit proved that the runtime principal held blanket
-- DELETE + UPDATE over EVERY cooperative-owned business/financial
-- table, while the application code only ever DELETEs from
-- `periods`/`assessment_rules` (draft cleanup) and UPDATEs a narrow,
-- per-table column set. Reproduced on a real sealed kooperatif_app
-- connection (v53 baseline):
--
--   * `UPDATE payments SET amount = 5`          -> UPDATE 1
--   * `UPDATE account_movements SET amount=1, direction='outflow'`
--                                               -> UPDATE 1
--   * `DELETE FROM payments`, `DELETE FROM account_movements`
--                                               -> DELETE 1 each
--
-- i.e. a plain member's sealed context could destructively erase and
-- rewrite posted financial history — "a posted financial transaction
-- is never deleted and historical amounts/ownership/attribution are
-- never rewritten" was enforced only by the application layer.
--
-- Remediation invariant:
--
--   * DELETE survives only where the code legitimately deletes:
--     `periods` + `assessment_rules`, and even there a RESTRICTIVE
--     policy confines deletion to DRAFT periods (and rules whose
--     parent period is still draft) — posting makes the rows
--     physically non-deletable for the runtime role.
--   * UPDATE is reduced to exactly the columns each repository
--     writes. Historical amount / account / ownership / payer /
--     attribution columns leave the runtime surface entirely.
--   * On the reversal/closure-terminal tables the ONLY legitimate
--     UPDATE is the single forward lifecycle transition; a
--     RESTRICTIVE FOR UPDATE policy pins (old status -> new status)
--     and freezes terminal rows, so forged status/metadata writes
--     that are not the documented transition are rejected by the
--     database itself. A companion RESTRICTIVE FOR INSERT policy
--     blocks inserting rows already carrying a terminal status.
--   * Tables the code never UPDATEs at all lose UPDATE entirely
--     (append-only for the runtime role): assessments,
--     assessment_share_sources, investment_disposals,
--     investment_disposal_proceeds, share_events, families.
--   * RLS scope policies, composite FKs and the existing
--     reversal-attribution CHECKs are untouched — this migration only
--     removes privilege and adds restrictive predicates.
--
-- Column grants below mirror, verbatim, the SET lists in
-- apps/server/src (payments/repo.rs, financial_accounts/repo.rs,
-- income_expense/repo.rs, credits/repo.rs, investments/repo.rs,
-- share_returns/repo.rs, shares/repo.rs, social_aid/repo.rs,
-- periods/repo.rs, governance/repo.rs, parties/repo.rs,
-- parties/routes.rs).

BEGIN;
SET LOCAL statement_timeout = '30s';

-- ==================================================================
-- 1. DELETE: keep only the two legitimately deletable tables.
-- ==================================================================
REVOKE DELETE ON
    account_movements, account_transfers, assessments,
    assessment_share_sources, credit_applications, expense_entries,
    families, financial_accounts, financial_categories,
    governance_bodies, governance_decisions, governance_memberships,
    governance_votes, income_entries, investment_disposal_proceeds,
    investment_disposals, investment_fundings, investment_incomes,
    investment_valuations, investments, payment_allocations, payments,
    persons, share_events, share_ownerships, share_return_entitlements,
    share_return_settlements, share_returns, shareholder_credits,
    shareholder_family_memberships, shareholders, shares,
    social_aid_disbursements, social_aid_donations, social_aid_funds
FROM kooperatif_app;

-- ==================================================================
-- 2. UPDATE: full revoke on the never-updated tables (append-only).
-- ==================================================================
REVOKE UPDATE ON
    assessments, assessment_share_sources, investment_disposals,
    investment_disposal_proceeds, share_events, families,
    governance_votes
FROM kooperatif_app;

-- ==================================================================
-- 3. UPDATE: narrow to the exact columns each repository writes.
-- ==================================================================
-- Reversal/cancellation/closure terminals — lifecycle columns only.
REVOKE UPDATE ON account_movements FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason)
    ON account_movements TO kooperatif_app;

REVOKE UPDATE ON account_transfers FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON account_transfers TO kooperatif_app;

REVOKE UPDATE ON credit_applications FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason)
    ON credit_applications TO kooperatif_app;

REVOKE UPDATE ON expense_entries FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON expense_entries TO kooperatif_app;

REVOKE UPDATE ON income_entries FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON income_entries TO kooperatif_app;

REVOKE UPDATE ON investment_fundings FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON investment_fundings TO kooperatif_app;

REVOKE UPDATE ON investment_incomes FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON investment_incomes TO kooperatif_app;

REVOKE UPDATE ON investment_valuations FROM kooperatif_app;
GRANT UPDATE (status, cancelled_at, cancelled_by, cancellation_reason,
              updated_at)
    ON investment_valuations TO kooperatif_app;

REVOKE UPDATE ON investments FROM kooperatif_app;
GRANT UPDATE (status, cancelled_at, cancelled_by, cancellation_reason,
              disposed_at, disposed_by, updated_at)
    ON investments TO kooperatif_app;

REVOKE UPDATE ON payment_allocations FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason)
    ON payment_allocations TO kooperatif_app;

REVOKE UPDATE ON payments FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON payments TO kooperatif_app;

REVOKE UPDATE ON share_return_settlements FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON share_return_settlements TO kooperatif_app;

REVOKE UPDATE ON share_returns FROM kooperatif_app;
GRANT UPDATE (status, cancelled_at, cancelled_by, cancellation_reason,
              finalized_at, finalized_by, updated_at)
    ON share_returns TO kooperatif_app;

REVOKE UPDATE ON shareholder_credits FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON shareholder_credits TO kooperatif_app;

REVOKE UPDATE ON social_aid_disbursements FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON social_aid_disbursements TO kooperatif_app;

REVOKE UPDATE ON social_aid_donations FROM kooperatif_app;
GRANT UPDATE (status, reversed_at, reversed_by, reversal_reason,
              updated_at)
    ON social_aid_donations TO kooperatif_app;

REVOKE UPDATE ON social_aid_funds FROM kooperatif_app;
GRANT UPDATE (status, closed_at, closed_by, cancelled_at, cancelled_by,
              cancellation_reason, updated_at)
    ON social_aid_funds TO kooperatif_app;

REVOKE UPDATE ON governance_bodies FROM kooperatif_app;
GRANT UPDATE (status, closed_at, closed_by, updated_at)
    ON governance_bodies TO kooperatif_app;

-- Mutable operational configuration / draft lifecycle rows.
REVOKE UPDATE ON assessment_rules FROM kooperatif_app;
GRANT UPDATE (rule_type, base_amount, assessment_effective_date,
              updated_at)
    ON assessment_rules TO kooperatif_app;

REVOKE UPDATE ON financial_accounts FROM kooperatif_app;
GRANT UPDATE (name, description, bank_name, iban, status, updated_at)
    ON financial_accounts TO kooperatif_app;

REVOKE UPDATE ON financial_categories FROM kooperatif_app;
GRANT UPDATE (name, description, status, updated_at)
    ON financial_categories TO kooperatif_app;

REVOKE UPDATE ON governance_decisions FROM kooperatif_app;
GRANT UPDATE (title, decision_text, decision_on, effective_on, status,
              opened_at, opened_by, cancelled_at, cancelled_by,
              cancellation_reason, finalized_at, finalized_by,
              eligible_count, approve_count, reject_count,
              abstain_count, updated_at)
    ON governance_decisions TO kooperatif_app;

REVOKE UPDATE ON governance_memberships FROM kooperatif_app;
GRANT UPDATE (ended_at, ended_by, end_reason, updated_at)
    ON governance_memberships TO kooperatif_app;

REVOKE UPDATE ON periods FROM kooperatif_app;
GRANT UPDATE (name, collection_start_date, due_date, status,
              updated_at)
    ON periods TO kooperatif_app;

REVOKE UPDATE ON persons FROM kooperatif_app;
GRANT UPDATE (first_name, last_name, search_name, updated_at)
    ON persons TO kooperatif_app;

REVOKE UPDATE ON share_ownerships FROM kooperatif_app;
GRANT UPDATE (ended_at) ON share_ownerships TO kooperatif_app;

REVOKE UPDATE ON share_return_entitlements FROM kooperatif_app;
GRANT UPDATE (amount, due_date, policy_reference, description,
              determined_at, status, cancelled_at, cancelled_by,
              cancellation_reason, updated_at)
    ON share_return_entitlements TO kooperatif_app;

REVOKE UPDATE ON shareholder_family_memberships FROM kooperatif_app;
GRANT UPDATE (ended_at, reason)
    ON shareholder_family_memberships TO kooperatif_app;

REVOKE UPDATE ON shareholders FROM kooperatif_app;
GRANT UPDATE (guardian_person_id, default_collection_account_id,
              status, updated_at)
    ON shareholders TO kooperatif_app;

REVOKE UPDATE ON shares FROM kooperatif_app;
GRANT UPDATE (status, updated_at) ON shares TO kooperatif_app;

-- ==================================================================
-- 4. RESTRICTIVE FOR UPDATE transition guards.
--
-- On these tables the ONLY UPDATE the runtime ever performs is the
-- forward terminal transition written by the reversal / cancellation
-- / closure flows (the paired-attribution CHECKs already force
-- reversed_at/reversed_by/reversal_reason to be filled). Pinning
-- old->new means:
--   * terminal rows are frozen — nothing can un-reverse, reopen or
--     otherwise touch them;
--   * non-terminal rows cannot be mutated in place at all (no column
--     the runtime can write changes unless the write performs exactly
--     the documented transition).
-- ==================================================================

-- posted/active -> reversed (sole transition per table)
CREATE POLICY account_movements_app_terminal_guard ON account_movements
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'active') WITH CHECK (status = 'reversed');
CREATE POLICY account_transfers_app_terminal_guard ON account_transfers
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');
CREATE POLICY credit_applications_app_terminal_guard ON credit_applications
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'active') WITH CHECK (status = 'reversed');
CREATE POLICY expense_entries_app_terminal_guard ON expense_entries
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');
CREATE POLICY income_entries_app_terminal_guard ON income_entries
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');
CREATE POLICY investment_fundings_app_terminal_guard ON investment_fundings
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');
CREATE POLICY investment_incomes_app_terminal_guard ON investment_incomes
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');
CREATE POLICY payment_allocations_app_terminal_guard ON payment_allocations
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'active') WITH CHECK (status = 'reversed');
CREATE POLICY payments_app_terminal_guard ON payments
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');
CREATE POLICY share_return_settlements_app_terminal_guard
    ON share_return_settlements
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');
CREATE POLICY shareholder_credits_app_terminal_guard ON shareholder_credits
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'active') WITH CHECK (status = 'reversed');
CREATE POLICY social_aid_disbursements_app_terminal_guard
    ON social_aid_disbursements
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');
CREATE POLICY social_aid_donations_app_terminal_guard ON social_aid_donations
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'posted') WITH CHECK (status = 'reversed');

-- cancellation / closure / finalization automata
CREATE POLICY investment_valuations_app_terminal_guard
    ON investment_valuations
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'recorded') WITH CHECK (status = 'cancelled');
CREATE POLICY investments_app_terminal_guard ON investments
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'active')
    WITH CHECK (status IN ('cancelled', 'disposed'));
CREATE POLICY share_returns_app_terminal_guard ON share_returns
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'pending')
    WITH CHECK (status IN ('finalized', 'cancelled'));
CREATE POLICY social_aid_funds_app_terminal_guard ON social_aid_funds
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'active')
    WITH CHECK (status IN ('closed', 'cancelled'));
CREATE POLICY governance_bodies_app_terminal_guard ON governance_bodies
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (status = 'active') WITH CHECK (status = 'closed');

-- ==================================================================
-- 5. RESTRICTIVE FOR INSERT: terminal rows cannot be born dead.
-- A runtime INSERT carrying status 'reversed'/'cancelled'/... would
-- mint pre-killed history without an audited transition.
-- ==================================================================
CREATE POLICY account_movements_app_insert_live ON account_movements
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY account_transfers_app_insert_live ON account_transfers
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY credit_applications_app_insert_live ON credit_applications
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY expense_entries_app_insert_live ON expense_entries
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY income_entries_app_insert_live ON income_entries
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY investment_fundings_app_insert_live ON investment_fundings
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY investment_incomes_app_insert_live ON investment_incomes
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY investment_valuations_app_insert_live
    ON investment_valuations
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status = 'recorded');
CREATE POLICY investments_app_insert_live ON investments
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status = 'active');
CREATE POLICY payment_allocations_app_insert_live ON payment_allocations
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY payments_app_insert_live ON payments
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY share_return_settlements_app_insert_live
    ON share_return_settlements
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY share_returns_app_insert_live ON share_returns
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status = 'pending');
CREATE POLICY shareholder_credits_app_insert_live ON shareholder_credits
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY social_aid_disbursements_app_insert_live
    ON social_aid_disbursements
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY social_aid_donations_app_insert_live ON social_aid_donations
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status <> 'reversed');
CREATE POLICY social_aid_funds_app_insert_live ON social_aid_funds
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status = 'active');
CREATE POLICY governance_bodies_app_insert_live ON governance_bodies
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (status = 'active');

-- ==================================================================
-- 6. Draft-confined deletion: the only runtime DELETEs left.
-- `periods` rows are deletable only while draft (the repository
-- checks this; now the database does too), and `assessment_rules`
-- rows only while their parent period is still draft — a rule feeding
-- an open/closed period can no longer disappear under the runtime.
-- Rule edits are likewise confined to draft periods.
-- ==================================================================
CREATE POLICY periods_app_delete_draft_only ON periods
    AS RESTRICTIVE FOR DELETE TO kooperatif_app
    USING (status = 'draft');

-- Per-command policies: SELECT stays governed by the permissive
-- scope policy alone (rules of open/closed periods remain readable);
-- INSERT/UPDATE/DELETE are confined to draft-parent rows.
CREATE POLICY assessment_rules_app_insert_draft_parent
    ON assessment_rules
    AS RESTRICTIVE FOR INSERT TO kooperatif_app
    WITH CHECK (EXISTS (
        SELECT 1 FROM periods p
        WHERE p.id = assessment_rules.period_id
          AND p.status = 'draft'
          AND p.cooperative_id IS NOT DISTINCT FROM
              assessment_rules.cooperative_id));
CREATE POLICY assessment_rules_app_update_draft_parent
    ON assessment_rules
    AS RESTRICTIVE FOR UPDATE TO kooperatif_app
    USING (EXISTS (
        SELECT 1 FROM periods p
        WHERE p.id = assessment_rules.period_id
          AND p.status = 'draft'
          AND p.cooperative_id IS NOT DISTINCT FROM
              assessment_rules.cooperative_id))
    WITH CHECK (EXISTS (
        SELECT 1 FROM periods p
        WHERE p.id = assessment_rules.period_id
          AND p.status = 'draft'
          AND p.cooperative_id IS NOT DISTINCT FROM
              assessment_rules.cooperative_id));
CREATE POLICY assessment_rules_app_delete_draft_parent
    ON assessment_rules
    AS RESTRICTIVE FOR DELETE TO kooperatif_app
    USING (EXISTS (
        SELECT 1 FROM periods p
        WHERE p.id = assessment_rules.period_id
          AND p.status = 'draft'
          AND p.cooperative_id IS NOT DISTINCT FROM
              assessment_rules.cooperative_id));

COMMIT;
