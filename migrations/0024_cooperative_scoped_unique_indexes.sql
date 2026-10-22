-- M1-P2-C5-1 — cooperative-scoped UNIQUE foundation (dual enforcement).
--
-- Adds 42 new cooperative-scoped unique indexes alongside the existing
-- global uniques, which are ALL PRESERVED in this migration:
--   * 21 numbering  -> UNIQUE (cooperative_id, <number>)
--   * 20 idempotency -> UNIQUE (cooperative_id, idempotency_key)
--                        WHERE idempotency_key IS NOT NULL
--   * 1 category name -> UNIQUE (cooperative_id, category_type,
--                        lower(btrim(name)))
--
-- The 19 other structural uniques (FK-pair, movement-binding and partial
-- keys) intentionally stay global-only: every key column is already a
-- globally-unique PK, so scoping would be redundant.
--
-- Numbers remain GENERATED ALWAYS AS IDENTITY — this is the transitional
-- "Option A" strategy only: global sequences keep allocating, so the
-- scoped indexes can never be violated while the globals exist and the
-- migration is unconditionally rollback-safe. Independent per-coop
-- numbering is an explicit M1-P3 requirement, NOT implemented here.
--
-- Legacy NULL-lane note: rows with cooperative_id IS NULL are distinct
-- under the composite indexes (NULL never conflicts). The NULL lane's
-- global uniqueness continues to be enforced by the preserved global
-- unique indexes until C5-3; dedicated NULL-lane partial indexes are
-- created there when the globals drop.
--
-- ON CONFLICT application logic is intentionally unchanged in C5-1 —
-- the arbiter switch is C5-2.

-- ---------------------------------------------------------------
-- Numbering (21)
-- ---------------------------------------------------------------
CREATE UNIQUE INDEX account_transfers_coop_transfer_number_uq
    ON account_transfers (cooperative_id, transfer_number);
CREATE UNIQUE INDEX expense_entries_coop_expense_number_uq
    ON expense_entries (cooperative_id, expense_number);
CREATE UNIQUE INDEX families_coop_sequence_number_uq
    ON families (cooperative_id, sequence_number);
CREATE UNIQUE INDEX governance_bodies_coop_body_number_uq
    ON governance_bodies (cooperative_id, body_number);
CREATE UNIQUE INDEX governance_decisions_coop_decision_number_uq
    ON governance_decisions (cooperative_id, decision_number);
CREATE UNIQUE INDEX income_entries_coop_income_number_uq
    ON income_entries (cooperative_id, income_number);
CREATE UNIQUE INDEX investment_disposals_coop_disposal_number_uq
    ON investment_disposals (cooperative_id, disposal_number);
CREATE UNIQUE INDEX investment_fundings_coop_funding_number_uq
    ON investment_fundings (cooperative_id, funding_number);
CREATE UNIQUE INDEX investment_incomes_coop_income_number_uq
    ON investment_incomes (cooperative_id, income_number);
CREATE UNIQUE INDEX investment_valuations_coop_valuation_number_uq
    ON investment_valuations (cooperative_id, valuation_number);
CREATE UNIQUE INDEX investments_coop_investment_number_uq
    ON investments (cooperative_id, investment_number);
CREATE UNIQUE INDEX payments_coop_payment_number_uq
    ON payments (cooperative_id, payment_number);
CREATE UNIQUE INDEX periods_coop_period_number_uq
    ON periods (cooperative_id, period_number);
CREATE UNIQUE INDEX share_return_entitlements_coop_entitlement_number_uq
    ON share_return_entitlements (cooperative_id, entitlement_number);
CREATE UNIQUE INDEX share_return_settlements_coop_settlement_number_uq
    ON share_return_settlements (cooperative_id, settlement_number);
CREATE UNIQUE INDEX share_returns_coop_return_number_uq
    ON share_returns (cooperative_id, return_number);
CREATE UNIQUE INDEX shareholder_credits_coop_credit_number_uq
    ON shareholder_credits (cooperative_id, credit_number);
CREATE UNIQUE INDEX shares_coop_share_number_uq
    ON shares (cooperative_id, share_number);
CREATE UNIQUE INDEX social_aid_disbursements_coop_disbursement_number_uq
    ON social_aid_disbursements (cooperative_id, disbursement_number);
CREATE UNIQUE INDEX social_aid_donations_coop_donation_number_uq
    ON social_aid_donations (cooperative_id, donation_number);
CREATE UNIQUE INDEX social_aid_funds_coop_fund_number_uq
    ON social_aid_funds (cooperative_id, fund_number);

-- ---------------------------------------------------------------
-- Idempotency (20) — NULL keys never conflict; partial for symmetry
-- with credit_applications' existing partial index and to keep the
-- indexes lean.
-- ---------------------------------------------------------------
CREATE UNIQUE INDEX account_transfers_coop_idempotency_uq
    ON account_transfers (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX credit_applications_coop_idempotency_uq
    ON credit_applications (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX expense_entries_coop_idempotency_uq
    ON expense_entries (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX governance_bodies_coop_idempotency_uq
    ON governance_bodies (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX governance_decisions_coop_idempotency_uq
    ON governance_decisions (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX governance_memberships_coop_idempotency_uq
    ON governance_memberships (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX governance_votes_coop_idempotency_uq
    ON governance_votes (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX income_entries_coop_idempotency_uq
    ON income_entries (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX investment_disposals_coop_idempotency_uq
    ON investment_disposals (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX investment_fundings_coop_idempotency_uq
    ON investment_fundings (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX investment_incomes_coop_idempotency_uq
    ON investment_incomes (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX investment_valuations_coop_idempotency_uq
    ON investment_valuations (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX investments_coop_idempotency_uq
    ON investments (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX payments_coop_idempotency_uq
    ON payments (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX share_return_settlements_coop_idempotency_uq
    ON share_return_settlements (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX share_returns_coop_idempotency_uq
    ON share_returns (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX shareholder_credits_coop_idempotency_uq
    ON shareholder_credits (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX social_aid_disbursements_coop_idempotency_uq
    ON social_aid_disbursements (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX social_aid_donations_coop_idempotency_uq
    ON social_aid_donations (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX social_aid_funds_coop_idempotency_uq
    ON social_aid_funds (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

-- ---------------------------------------------------------------
-- Category name (1) — the one structural key whose value is domain
-- text: different cooperatives legitimately share category names.
-- ---------------------------------------------------------------
CREATE UNIQUE INDEX financial_categories_coop_name_uq
    ON financial_categories (cooperative_id, category_type, lower(btrim(name)));

-- ---------------------------------------------------------------
-- In-transaction verification: exactly the 42 intended new indexes
-- exist, and all 61 pre-existing non-PK global unique candidates are
-- still present. Any deviation aborts and rolls the migration back.
-- ---------------------------------------------------------------
DO $$
DECLARE
    missing_new text;
    missing_old text;
BEGIN
    WITH expected(n) AS (
        VALUES
        ('account_transfers_coop_transfer_number_uq'),
        ('expense_entries_coop_expense_number_uq'),
        ('families_coop_sequence_number_uq'),
        ('governance_bodies_coop_body_number_uq'),
        ('governance_decisions_coop_decision_number_uq'),
        ('income_entries_coop_income_number_uq'),
        ('investment_disposals_coop_disposal_number_uq'),
        ('investment_fundings_coop_funding_number_uq'),
        ('investment_incomes_coop_income_number_uq'),
        ('investment_valuations_coop_valuation_number_uq'),
        ('investments_coop_investment_number_uq'),
        ('payments_coop_payment_number_uq'),
        ('periods_coop_period_number_uq'),
        ('share_return_entitlements_coop_entitlement_number_uq'),
        ('share_return_settlements_coop_settlement_number_uq'),
        ('share_returns_coop_return_number_uq'),
        ('shareholder_credits_coop_credit_number_uq'),
        ('shares_coop_share_number_uq'),
        ('social_aid_disbursements_coop_disbursement_number_uq'),
        ('social_aid_donations_coop_donation_number_uq'),
        ('social_aid_funds_coop_fund_number_uq'),
        ('account_transfers_coop_idempotency_uq'),
        ('credit_applications_coop_idempotency_uq'),
        ('expense_entries_coop_idempotency_uq'),
        ('governance_bodies_coop_idempotency_uq'),
        ('governance_decisions_coop_idempotency_uq'),
        ('governance_memberships_coop_idempotency_uq'),
        ('governance_votes_coop_idempotency_uq'),
        ('income_entries_coop_idempotency_uq'),
        ('investment_disposals_coop_idempotency_uq'),
        ('investment_fundings_coop_idempotency_uq'),
        ('investment_incomes_coop_idempotency_uq'),
        ('investment_valuations_coop_idempotency_uq'),
        ('investments_coop_idempotency_uq'),
        ('payments_coop_idempotency_uq'),
        ('share_return_settlements_coop_idempotency_uq'),
        ('share_returns_coop_idempotency_uq'),
        ('shareholder_credits_coop_idempotency_uq'),
        ('social_aid_disbursements_coop_idempotency_uq'),
        ('social_aid_donations_coop_idempotency_uq'),
        ('social_aid_funds_coop_idempotency_uq'),
        ('financial_categories_coop_name_uq')
    )
    SELECT string_agg(e.n, ', ') INTO missing_new
    FROM expected e
    WHERE NOT EXISTS (
        SELECT 1 FROM pg_class i JOIN pg_index ix ON ix.indexrelid = i.oid
        WHERE i.relname = e.n AND ix.indisunique
    );
    IF missing_new IS NOT NULL THEN
        RAISE EXCEPTION 'C5-1: scoped unique indexes missing: %', missing_new;
    END IF;

    WITH expected(n) AS (
        VALUES
        ('account_movements_source_leg_key'),
        ('account_transfers_idempotency_key_key'),
        ('account_transfers_transfer_number_key'),
        ('assessment_rules_period_id_key'),
        ('assessments_period_shareholder_key'),
        ('credit_applications_idempotency_key_key'),
        ('expense_entries_expense_number_key'),
        ('expense_entries_idempotency_key_key'),
        ('expense_entries_movement_key'),
        ('families_sequence_number_key'),
        ('financial_categories_name_key'),
        ('financial_categories_type_key'),
        ('governance_bodies_body_number_key'),
        ('governance_bodies_idempotency_key_key'),
        ('governance_decisions_decision_number_key'),
        ('governance_decisions_idempotency_key_key'),
        ('governance_memberships_active_key'),
        ('governance_memberships_idempotency_key_key'),
        ('governance_votes_idempotency_key_key'),
        ('governance_votes_one_per_member_key'),
        ('income_entries_idempotency_key_key'),
        ('income_entries_income_number_key'),
        ('income_entries_movement_key'),
        ('investment_disposal_proceeds_movement_key'),
        ('investment_disposals_disposal_number_key'),
        ('investment_disposals_idempotency_key_key'),
        ('investment_disposals_one_per_investment'),
        ('investment_fundings_funding_number_key'),
        ('investment_fundings_idempotency_key_key'),
        ('investment_fundings_movement_key'),
        ('investment_incomes_idempotency_key_key'),
        ('investment_incomes_income_number_key'),
        ('investment_incomes_movement_key'),
        ('investment_valuations_idempotency_key_key'),
        ('investment_valuations_valuation_number_key'),
        ('investments_idempotency_key_key'),
        ('investments_investment_number_key'),
        ('payment_allocations_payment_assessment_key'),
        ('payments_idempotency_key_key'),
        ('payments_payment_number_key'),
        ('periods_period_number_key'),
        ('share_return_entitlements_number_key'),
        ('share_return_entitlements_one_per_right'),
        ('share_return_settlements_idempotency_key_key'),
        ('share_return_settlements_movement_key'),
        ('share_return_settlements_number_key'),
        ('share_returns_idempotency_key_key'),
        ('share_returns_one_pending_per_share'),
        ('share_returns_return_number_key'),
        ('shareholder_credits_credit_number_key'),
        ('shareholder_credits_idempotency_key_key'),
        ('shareholders_person_id_key'),
        ('shares_share_number_key'),
        ('social_aid_disbursements_disbursement_number_key'),
        ('social_aid_disbursements_idempotency_key_key'),
        ('social_aid_disbursements_movement_key'),
        ('social_aid_donations_donation_number_key'),
        ('social_aid_donations_idempotency_key_key'),
        ('social_aid_donations_movement_key'),
        ('social_aid_funds_fund_number_key'),
        ('social_aid_funds_idempotency_key_key')
    )
    SELECT string_agg(e.n, ', ') INTO missing_old
    FROM expected e
    WHERE NOT EXISTS (
        SELECT 1 FROM pg_class i JOIN pg_index ix ON ix.indexrelid = i.oid
        WHERE i.relname = e.n AND ix.indisunique
    );
    IF missing_old IS NOT NULL THEN
        RAISE EXCEPTION 'C5-1: pre-existing global unique indexes missing: %', missing_old;
    END IF;
END $$;
