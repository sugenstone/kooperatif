-- M1-P2-C5-3 — global UNIQUE swap + cooperative ownership enforcement.
--
-- C5-1 added 42 cooperative-scoped unique indexes next to the legacy
-- global uniques; C5-2 pointed every idempotency ON CONFLICT arbiter at
-- the scoped indexes. This migration completes the swap:
--
--   * drops the 42 legacy global unique indexes/constraints that now have
--     a cooperative-scoped replacement (21 numbering + 20 idempotency +
--     financial_categories_name_key);
--   * keeps the 19 structural global uniques untouched (FK-pair,
--     movement-binding and partial keys — every key column is already a
--     globally-unique PK, scoping would be redundant);
--   * enforces cooperative_id NOT NULL on all 37 business tables via
--     CHECK (NOT VALID) -> VALIDATE -> SET NOT NULL -> drop the
--     redundant CHECK, so the catalog-visible guarantee is a plain
--     NOT NULL column while validation never blocks writers with an
--     AccessExclusive scan.
--
-- Hard blockers (fail-closed, whole migration rolls back):
--   * any of the 42 scoped replacement indexes missing/invalid;
--   * any NULL cooperative_id row in any of the 37 business tables —
--     ownership is never silently assigned here;
--   * post-checks: all 42 legacy globals gone, all 19 structural globals
--     present, all 37 columns attnotnull.
--
-- Numbers remain GENERATED ALWAYS AS IDENTITY — independent per-
-- cooperative document numbering is still an explicit M1-P3 requirement
-- and is NOT implemented here.
--
-- No NULL-lane compatibility indexes are created: ownership is now
-- mandatory, so the NULL lane is closed by construction.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '300s';

-- ---------------------------------------------------------------
-- NULL-lane seed retirement: migration 0010's ten global
-- financial_categories seeds (`cooperative_id IS NULL AND created_by
-- IS NULL`) cannot satisfy the incoming NOT NULL and cannot be owned —
-- no cooperative may exist yet on a fresh migrate. They are therefore
-- DELETED here and re-inserted by bootstrap_initial_cooperative bound
-- to the bootstrap cooperative. The predicate is exactly the historical
-- claim set (`created_by IS NULL` = system-seeded only); user-created
-- categories always carry created_by and are NEVER touched. On an
-- already-bootstrapped database the seeds are owned, so this deletes
-- zero rows.
-- ---------------------------------------------------------------
DELETE FROM financial_categories
 WHERE cooperative_id IS NULL
   AND created_by IS NULL;

-- ---------------------------------------------------------------
-- Pre-flight guard: scoped replacements in place + zero NULL ownership.
-- ---------------------------------------------------------------
DO $$
DECLARE
    missing text;
    nulls   bigint;
BEGIN
    -- 1) all 42 cooperative-scoped replacement indexes exist, unique, valid
    SELECT string_agg(ix.indexrelid::regclass::text, ', ')
      INTO missing
      FROM (VALUES
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
      ) AS need(name)
      LEFT JOIN pg_index ix
        ON ix.indexrelid = to_regclass('public.' || need.name)
       AND ix.indisunique AND ix.indisvalid
      WHERE ix.indexrelid IS NULL;
    IF missing IS NOT NULL THEN
        RAISE EXCEPTION 'C5-3 pre-flight: missing/invalid scoped unique indexes: %', missing;
    END IF;

    -- 2) zero NULL cooperative_id across all 37 business tables — hard blocker
    FOREACH missing IN ARRAY ARRAY[
        'persons','families','shareholders','shareholder_family_memberships',
        'shares','share_events','share_ownerships',
        'periods','assessment_rules','assessments','assessment_share_sources',
        'payments','payment_allocations',
        'financial_accounts','account_movements','account_transfers',
        'shareholder_credits','credit_applications',
        'financial_categories','income_entries','expense_entries',
        'share_returns','share_return_entitlements','share_return_settlements',
        'investments','investment_fundings','investment_valuations',
        'investment_incomes','investment_disposals','investment_disposal_proceeds',
        'social_aid_funds','social_aid_donations','social_aid_disbursements',
        'governance_bodies','governance_memberships','governance_decisions',
        'governance_votes'
    ] LOOP
        EXECUTE format('SELECT count(*) FROM %I WHERE cooperative_id IS NULL', missing)
           INTO nulls;
        IF nulls > 0 THEN
            RAISE EXCEPTION 'C5-3 hard blocker: % NULL cooperative_id row(s) in % — refusing to silently assign ownership', nulls, missing;
        END IF;
    END LOOP;
END $$;

-- ---------------------------------------------------------------
-- Swap part 1: drop the 23 legacy global PLAIN unique indexes
-- (21 numbering + credit_applications partial + category name).
-- ---------------------------------------------------------------
DROP INDEX account_transfers_transfer_number_key;
DROP INDEX expense_entries_expense_number_key;
DROP INDEX families_sequence_number_key;
DROP INDEX governance_bodies_body_number_key;
DROP INDEX governance_decisions_decision_number_key;
DROP INDEX income_entries_income_number_key;
DROP INDEX investment_disposals_disposal_number_key;
DROP INDEX investment_fundings_funding_number_key;
DROP INDEX investment_incomes_income_number_key;
DROP INDEX investment_valuations_valuation_number_key;
DROP INDEX investments_investment_number_key;
DROP INDEX payments_payment_number_key;
DROP INDEX periods_period_number_key;
DROP INDEX share_return_entitlements_number_key;
DROP INDEX share_return_settlements_number_key;
DROP INDEX share_returns_return_number_key;
DROP INDEX shareholder_credits_credit_number_key;
DROP INDEX shares_share_number_key;
DROP INDEX social_aid_disbursements_disbursement_number_key;
DROP INDEX social_aid_donations_donation_number_key;
DROP INDEX social_aid_funds_fund_number_key;
DROP INDEX credit_applications_idempotency_key_key;
DROP INDEX financial_categories_name_key;

-- ---------------------------------------------------------------
-- Swap part 2: drop the 19 legacy global idempotency UNIQUE constraints.
-- ---------------------------------------------------------------
ALTER TABLE account_transfers        DROP CONSTRAINT account_transfers_idempotency_key_key;
ALTER TABLE expense_entries          DROP CONSTRAINT expense_entries_idempotency_key_key;
ALTER TABLE governance_bodies        DROP CONSTRAINT governance_bodies_idempotency_key_key;
ALTER TABLE governance_decisions     DROP CONSTRAINT governance_decisions_idempotency_key_key;
ALTER TABLE governance_memberships   DROP CONSTRAINT governance_memberships_idempotency_key_key;
ALTER TABLE governance_votes         DROP CONSTRAINT governance_votes_idempotency_key_key;
ALTER TABLE income_entries           DROP CONSTRAINT income_entries_idempotency_key_key;
ALTER TABLE investment_disposals     DROP CONSTRAINT investment_disposals_idempotency_key_key;
ALTER TABLE investment_fundings      DROP CONSTRAINT investment_fundings_idempotency_key_key;
ALTER TABLE investment_incomes       DROP CONSTRAINT investment_incomes_idempotency_key_key;
ALTER TABLE investment_valuations    DROP CONSTRAINT investment_valuations_idempotency_key_key;
ALTER TABLE investments              DROP CONSTRAINT investments_idempotency_key_key;
ALTER TABLE payments                 DROP CONSTRAINT payments_idempotency_key_key;
ALTER TABLE share_return_settlements DROP CONSTRAINT share_return_settlements_idempotency_key_key;
ALTER TABLE share_returns            DROP CONSTRAINT share_returns_idempotency_key_key;
ALTER TABLE shareholder_credits      DROP CONSTRAINT shareholder_credits_idempotency_key_key;
ALTER TABLE social_aid_disbursements DROP CONSTRAINT social_aid_disbursements_idempotency_key_key;
ALTER TABLE social_aid_donations     DROP CONSTRAINT social_aid_donations_idempotency_key_key;
ALTER TABLE social_aid_funds         DROP CONSTRAINT social_aid_funds_idempotency_key_key;

-- ---------------------------------------------------------------
-- Ownership enforcement: validated CHECK -> SET NOT NULL -> drop CHECK.
-- The NOT VALID + VALIDATE pair scans under ShareUpdateExclusive
-- (writers keep working); SET NOT NULL then reuses the validated
-- constraint and skips its own AccessExclusive scan. Lock timeout
-- (10s) bounds every individual lock wait.
-- ---------------------------------------------------------------
ALTER TABLE persons                        ADD CONSTRAINT persons_nn_coop                        CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE families                       ADD CONSTRAINT families_nn_coop                       CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE shareholders                   ADD CONSTRAINT shareholders_nn_coop                   CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE shareholder_family_memberships ADD CONSTRAINT shareholder_family_memberships_nn_coop CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE shares                         ADD CONSTRAINT shares_nn_coop                         CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE share_events                   ADD CONSTRAINT share_events_nn_coop                   CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE share_ownerships               ADD CONSTRAINT share_ownerships_nn_coop               CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE periods                        ADD CONSTRAINT periods_nn_coop                        CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE assessment_rules               ADD CONSTRAINT assessment_rules_nn_coop               CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE assessments                    ADD CONSTRAINT assessments_nn_coop                    CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE assessment_share_sources       ADD CONSTRAINT assessment_share_sources_nn_coop       CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE payments                       ADD CONSTRAINT payments_nn_coop                       CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE payment_allocations            ADD CONSTRAINT payment_allocations_nn_coop            CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE financial_accounts             ADD CONSTRAINT financial_accounts_nn_coop             CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE account_movements              ADD CONSTRAINT account_movements_nn_coop              CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE account_transfers              ADD CONSTRAINT account_transfers_nn_coop              CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE shareholder_credits            ADD CONSTRAINT shareholder_credits_nn_coop            CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE credit_applications            ADD CONSTRAINT credit_applications_nn_coop            CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE financial_categories           ADD CONSTRAINT financial_categories_nn_coop           CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE income_entries                 ADD CONSTRAINT income_entries_nn_coop                 CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE expense_entries                ADD CONSTRAINT expense_entries_nn_coop                CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE share_returns                  ADD CONSTRAINT share_returns_nn_coop                  CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE share_return_entitlements      ADD CONSTRAINT share_return_entitlements_nn_coop      CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE share_return_settlements       ADD CONSTRAINT share_return_settlements_nn_coop       CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE investments                    ADD CONSTRAINT investments_nn_coop                    CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE investment_fundings            ADD CONSTRAINT investment_fundings_nn_coop            CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE investment_valuations          ADD CONSTRAINT investment_valuations_nn_coop          CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE investment_incomes             ADD CONSTRAINT investment_incomes_nn_coop             CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE investment_disposals           ADD CONSTRAINT investment_disposals_nn_coop           CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE investment_disposal_proceeds   ADD CONSTRAINT investment_disposal_proceeds_nn_coop   CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE social_aid_funds               ADD CONSTRAINT social_aid_funds_nn_coop               CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE social_aid_donations           ADD CONSTRAINT social_aid_donations_nn_coop           CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE social_aid_disbursements       ADD CONSTRAINT social_aid_disbursements_nn_coop       CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE governance_bodies              ADD CONSTRAINT governance_bodies_nn_coop              CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE governance_memberships         ADD CONSTRAINT governance_memberships_nn_coop         CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE governance_decisions           ADD CONSTRAINT governance_decisions_nn_coop           CHECK (cooperative_id IS NOT NULL) NOT VALID;
ALTER TABLE governance_votes               ADD CONSTRAINT governance_votes_nn_coop               CHECK (cooperative_id IS NOT NULL) NOT VALID;

ALTER TABLE persons                        VALIDATE CONSTRAINT persons_nn_coop;
ALTER TABLE families                       VALIDATE CONSTRAINT families_nn_coop;
ALTER TABLE shareholders                   VALIDATE CONSTRAINT shareholders_nn_coop;
ALTER TABLE shareholder_family_memberships VALIDATE CONSTRAINT shareholder_family_memberships_nn_coop;
ALTER TABLE shares                         VALIDATE CONSTRAINT shares_nn_coop;
ALTER TABLE share_events                   VALIDATE CONSTRAINT share_events_nn_coop;
ALTER TABLE share_ownerships               VALIDATE CONSTRAINT share_ownerships_nn_coop;
ALTER TABLE periods                        VALIDATE CONSTRAINT periods_nn_coop;
ALTER TABLE assessment_rules               VALIDATE CONSTRAINT assessment_rules_nn_coop;
ALTER TABLE assessments                    VALIDATE CONSTRAINT assessments_nn_coop;
ALTER TABLE assessment_share_sources       VALIDATE CONSTRAINT assessment_share_sources_nn_coop;
ALTER TABLE payments                       VALIDATE CONSTRAINT payments_nn_coop;
ALTER TABLE payment_allocations            VALIDATE CONSTRAINT payment_allocations_nn_coop;
ALTER TABLE financial_accounts             VALIDATE CONSTRAINT financial_accounts_nn_coop;
ALTER TABLE account_movements              VALIDATE CONSTRAINT account_movements_nn_coop;
ALTER TABLE account_transfers              VALIDATE CONSTRAINT account_transfers_nn_coop;
ALTER TABLE shareholder_credits            VALIDATE CONSTRAINT shareholder_credits_nn_coop;
ALTER TABLE credit_applications            VALIDATE CONSTRAINT credit_applications_nn_coop;
ALTER TABLE financial_categories           VALIDATE CONSTRAINT financial_categories_nn_coop;
ALTER TABLE income_entries                 VALIDATE CONSTRAINT income_entries_nn_coop;
ALTER TABLE expense_entries                VALIDATE CONSTRAINT expense_entries_nn_coop;
ALTER TABLE share_returns                  VALIDATE CONSTRAINT share_returns_nn_coop;
ALTER TABLE share_return_entitlements      VALIDATE CONSTRAINT share_return_entitlements_nn_coop;
ALTER TABLE share_return_settlements       VALIDATE CONSTRAINT share_return_settlements_nn_coop;
ALTER TABLE investments                    VALIDATE CONSTRAINT investments_nn_coop;
ALTER TABLE investment_fundings            VALIDATE CONSTRAINT investment_fundings_nn_coop;
ALTER TABLE investment_valuations          VALIDATE CONSTRAINT investment_valuations_nn_coop;
ALTER TABLE investment_incomes             VALIDATE CONSTRAINT investment_incomes_nn_coop;
ALTER TABLE investment_disposals           VALIDATE CONSTRAINT investment_disposals_nn_coop;
ALTER TABLE investment_disposal_proceeds   VALIDATE CONSTRAINT investment_disposal_proceeds_nn_coop;
ALTER TABLE social_aid_funds               VALIDATE CONSTRAINT social_aid_funds_nn_coop;
ALTER TABLE social_aid_donations           VALIDATE CONSTRAINT social_aid_donations_nn_coop;
ALTER TABLE social_aid_disbursements       VALIDATE CONSTRAINT social_aid_disbursements_nn_coop;
ALTER TABLE governance_bodies              VALIDATE CONSTRAINT governance_bodies_nn_coop;
ALTER TABLE governance_memberships         VALIDATE CONSTRAINT governance_memberships_nn_coop;
ALTER TABLE governance_decisions           VALIDATE CONSTRAINT governance_decisions_nn_coop;
ALTER TABLE governance_votes               VALIDATE CONSTRAINT governance_votes_nn_coop;

ALTER TABLE persons                        ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE families                       ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE shareholders                   ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE shareholder_family_memberships ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE shares                         ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE share_events                   ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE share_ownerships               ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE periods                        ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE assessment_rules               ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE assessments                    ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE assessment_share_sources       ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE payments                       ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE payment_allocations            ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE financial_accounts             ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE account_movements              ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE account_transfers              ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE shareholder_credits            ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE credit_applications            ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE financial_categories           ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE income_entries                 ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE expense_entries                ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE share_returns                  ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE share_return_entitlements      ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE share_return_settlements       ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE investments                    ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE investment_fundings            ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE investment_valuations          ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE investment_incomes             ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE investment_disposals           ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE investment_disposal_proceeds   ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE social_aid_funds               ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE social_aid_donations           ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE social_aid_disbursements       ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE governance_bodies              ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE governance_memberships         ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE governance_decisions           ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE governance_votes               ALTER COLUMN cooperative_id SET NOT NULL;

ALTER TABLE persons                        DROP CONSTRAINT persons_nn_coop;
ALTER TABLE families                       DROP CONSTRAINT families_nn_coop;
ALTER TABLE shareholders                   DROP CONSTRAINT shareholders_nn_coop;
ALTER TABLE shareholder_family_memberships DROP CONSTRAINT shareholder_family_memberships_nn_coop;
ALTER TABLE shares                         DROP CONSTRAINT shares_nn_coop;
ALTER TABLE share_events                   DROP CONSTRAINT share_events_nn_coop;
ALTER TABLE share_ownerships               DROP CONSTRAINT share_ownerships_nn_coop;
ALTER TABLE periods                        DROP CONSTRAINT periods_nn_coop;
ALTER TABLE assessment_rules               DROP CONSTRAINT assessment_rules_nn_coop;
ALTER TABLE assessments                    DROP CONSTRAINT assessments_nn_coop;
ALTER TABLE assessment_share_sources       DROP CONSTRAINT assessment_share_sources_nn_coop;
ALTER TABLE payments                       DROP CONSTRAINT payments_nn_coop;
ALTER TABLE payment_allocations            DROP CONSTRAINT payment_allocations_nn_coop;
ALTER TABLE financial_accounts             DROP CONSTRAINT financial_accounts_nn_coop;
ALTER TABLE account_movements              DROP CONSTRAINT account_movements_nn_coop;
ALTER TABLE account_transfers              DROP CONSTRAINT account_transfers_nn_coop;
ALTER TABLE shareholder_credits            DROP CONSTRAINT shareholder_credits_nn_coop;
ALTER TABLE credit_applications            DROP CONSTRAINT credit_applications_nn_coop;
ALTER TABLE financial_categories           DROP CONSTRAINT financial_categories_nn_coop;
ALTER TABLE income_entries                 DROP CONSTRAINT income_entries_nn_coop;
ALTER TABLE expense_entries                DROP CONSTRAINT expense_entries_nn_coop;
ALTER TABLE share_returns                  DROP CONSTRAINT share_returns_nn_coop;
ALTER TABLE share_return_entitlements      DROP CONSTRAINT share_return_entitlements_nn_coop;
ALTER TABLE share_return_settlements       DROP CONSTRAINT share_return_settlements_nn_coop;
ALTER TABLE investments                    DROP CONSTRAINT investments_nn_coop;
ALTER TABLE investment_fundings            DROP CONSTRAINT investment_fundings_nn_coop;
ALTER TABLE investment_valuations          DROP CONSTRAINT investment_valuations_nn_coop;
ALTER TABLE investment_incomes             DROP CONSTRAINT investment_incomes_nn_coop;
ALTER TABLE investment_disposals           DROP CONSTRAINT investment_disposals_nn_coop;
ALTER TABLE investment_disposal_proceeds   DROP CONSTRAINT investment_disposal_proceeds_nn_coop;
ALTER TABLE social_aid_funds               DROP CONSTRAINT social_aid_funds_nn_coop;
ALTER TABLE social_aid_donations           DROP CONSTRAINT social_aid_donations_nn_coop;
ALTER TABLE social_aid_disbursements       DROP CONSTRAINT social_aid_disbursements_nn_coop;
ALTER TABLE governance_bodies              DROP CONSTRAINT governance_bodies_nn_coop;
ALTER TABLE governance_memberships         DROP CONSTRAINT governance_memberships_nn_coop;
ALTER TABLE governance_decisions           DROP CONSTRAINT governance_decisions_nn_coop;
ALTER TABLE governance_votes               DROP CONSTRAINT governance_votes_nn_coop;

-- ---------------------------------------------------------------
-- Post-checks: swap is exact, structure preserved, ownership enforced.
-- ---------------------------------------------------------------
DO $$
DECLARE
    bad text;
    n   bigint;
BEGIN
    -- 1) none of the 42 legacy global uniques may survive
    SELECT count(*) INTO n
      FROM pg_index ix
     WHERE ix.indexrelid::regclass::text IN (
        'account_transfers_transfer_number_key','expense_entries_expense_number_key','families_sequence_number_key',
        'governance_bodies_body_number_key','governance_decisions_decision_number_key','income_entries_income_number_key',
        'investment_disposals_disposal_number_key','investment_fundings_funding_number_key','investment_incomes_income_number_key',
        'investment_valuations_valuation_number_key','investments_investment_number_key','payments_payment_number_key',
        'periods_period_number_key','share_return_entitlements_number_key','share_return_settlements_number_key',
        'share_returns_return_number_key','shareholder_credits_credit_number_key','shares_share_number_key',
        'social_aid_disbursements_disbursement_number_key','social_aid_donations_donation_number_key','social_aid_funds_fund_number_key',
        'account_transfers_idempotency_key_key','credit_applications_idempotency_key_key','expense_entries_idempotency_key_key',
        'governance_bodies_idempotency_key_key','governance_decisions_idempotency_key_key','governance_memberships_idempotency_key_key',
        'governance_votes_idempotency_key_key','income_entries_idempotency_key_key','investment_disposals_idempotency_key_key',
        'investment_fundings_idempotency_key_key','investment_incomes_idempotency_key_key','investment_valuations_idempotency_key_key',
        'investments_idempotency_key_key','payments_idempotency_key_key','share_return_settlements_idempotency_key_key',
        'share_returns_idempotency_key_key','shareholder_credits_idempotency_key_key','social_aid_disbursements_idempotency_key_key',
        'social_aid_donations_idempotency_key_key','social_aid_funds_idempotency_key_key','financial_categories_name_key');
    IF n <> 0 THEN
        RAISE EXCEPTION 'C5-3 post-check: % legacy global unique index(es) still exist', n;
    END IF;

    -- 2) all 19 structural global uniques must still be present
    SELECT string_agg(need.name, ', ') INTO bad
      FROM (VALUES
        ('account_movements_source_leg_key'),
        ('assessment_rules_period_id_key'),
        ('assessments_period_shareholder_key'),
        ('expense_entries_movement_key'),
        ('financial_categories_type_key'),
        ('governance_memberships_active_key'),
        ('governance_votes_one_per_member_key'),
        ('income_entries_movement_key'),
        ('investment_disposal_proceeds_movement_key'),
        ('investment_disposals_one_per_investment'),
        ('investment_fundings_movement_key'),
        ('investment_incomes_movement_key'),
        ('payment_allocations_payment_assessment_key'),
        ('share_return_entitlements_one_per_right'),
        ('share_return_settlements_movement_key'),
        ('share_returns_one_pending_per_share'),
        ('shareholders_person_id_key'),
        ('social_aid_disbursements_movement_key'),
        ('social_aid_donations_movement_key')
      ) AS need(name)
      WHERE NOT EXISTS (
        SELECT 1 FROM pg_index ix
         WHERE ix.indexrelid = to_regclass('public.' || need.name)
           AND ix.indisunique);
    IF bad IS NOT NULL THEN
        RAISE EXCEPTION 'C5-3 post-check: structural global uniques missing: %', bad;
    END IF;

    -- 3) cooperative_id NOT NULL on all 37 business tables
    SELECT string_agg(t.relname, ', ') INTO bad
      FROM pg_class t
      JOIN pg_attribute a ON a.attrelid = t.oid
     WHERE t.relkind = 'r'
       AND t.relname IN (
        'persons','families','shareholders','shareholder_family_memberships',
        'shares','share_events','share_ownerships',
        'periods','assessment_rules','assessments','assessment_share_sources',
        'payments','payment_allocations',
        'financial_accounts','account_movements','account_transfers',
        'shareholder_credits','credit_applications',
        'financial_categories','income_entries','expense_entries',
        'share_returns','share_return_entitlements','share_return_settlements',
        'investments','investment_fundings','investment_valuations',
        'investment_incomes','investment_disposals','investment_disposal_proceeds',
        'social_aid_funds','social_aid_donations','social_aid_disbursements',
        'governance_bodies','governance_memberships','governance_decisions',
        'governance_votes')
       AND a.attname = 'cooperative_id'
       AND NOT a.attnotnull;
    IF bad IS NOT NULL THEN
        RAISE EXCEPTION 'C5-3 post-check: cooperative_id still nullable on: %', bad;
    END IF;
END $$;
