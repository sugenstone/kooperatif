-- M1-P3-B1 — cooperative-local numbering foundation.
--
-- Establishes the allocator infrastructure for independent
-- per-cooperative document numbering (implementation/M1-P3-
-- NUMBERING-DESIGN.md):
--
--   * document_number_series — registry of the 20 automatically
--     generated series (families.sequence_number is deliberately
--     absent: it is operator-supplied data);
--   * cooperative_number_counters — one row per
--     (cooperative_id, series_key), the serialized counter state;
--   * allocate_document_number(coop, series) — transactional
--     allocator. The INSERT...ON CONFLICT DO UPDATE serializes
--     concurrent writers on the counter row inside the CALLER's
--     transaction, so allocation commits or rolls back together with
--     the business write — gapless, duplicate-free, and each
--     cooperative independently starts at 1;
--   * reserve_document_number(coop, series, n) — raises the counter
--     floor to n+1 for imports/administrative corrections so a
--     manually supplied or imported number can never be reissued.
--
-- This migration deliberately changes NO business table: all 20
-- IDENTITY columns and their sequences stay live until each series'
-- own adoption batch. Nothing is renumbered.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '60s';

CREATE TABLE document_number_series (
    series_key text PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO document_number_series (series_key) VALUES
    ('account_transfers.transfer_number'),
    ('expense_entries.expense_number'),
    ('governance_bodies.body_number'),
    ('governance_decisions.decision_number'),
    ('income_entries.income_number'),
    ('investment_disposals.disposal_number'),
    ('investment_fundings.funding_number'),
    ('investment_incomes.income_number'),
    ('investment_valuations.valuation_number'),
    ('investments.investment_number'),
    ('payments.payment_number'),
    ('periods.period_number'),
    ('share_return_entitlements.entitlement_number'),
    ('share_return_settlements.settlement_number'),
    ('share_returns.return_number'),
    ('shareholder_credits.credit_number'),
    ('shares.share_number'),
    ('social_aid_disbursements.disbursement_number'),
    ('social_aid_donations.donation_number'),
    ('social_aid_funds.fund_number');

CREATE TABLE cooperative_number_counters (
    cooperative_id uuid NOT NULL REFERENCES cooperatives (id),
    series_key     text NOT NULL REFERENCES document_number_series (series_key),
    next_value     bigint NOT NULL CHECK (next_value >= 1),
    PRIMARY KEY (cooperative_id, series_key)
);

-- Allocate the next number for (cooperative, series) inside the
-- caller's transaction. First allocation in a fresh (coop, series)
-- returns 1; the ON CONFLICT loser blocks on the winner's row lock
-- until commit/rollback, so concurrent allocations can never produce
-- duplicates and a rollback never leaves a consumed number behind.
CREATE OR REPLACE FUNCTION allocate_document_number(
    p_cooperative_id uuid,
    p_series_key     text
) RETURNS bigint
LANGUAGE sql
AS $$
    INSERT INTO cooperative_number_counters
        (cooperative_id, series_key, next_value)
    VALUES (p_cooperative_id, p_series_key, 2)
    ON CONFLICT (cooperative_id, series_key) DO UPDATE
        SET next_value = cooperative_number_counters.next_value + 1
    RETURNING next_value - 1;
$$;

-- Reserve n: raise the counter floor to n+1 so the allocator never
-- reissues a number that arrived via import, manual entry or an
-- administrative correction. Returns the resulting next_value.
CREATE OR REPLACE FUNCTION reserve_document_number(
    p_cooperative_id uuid,
    p_series_key     text,
    p_value          bigint
) RETURNS bigint
LANGUAGE sql
AS $$
    INSERT INTO cooperative_number_counters
        (cooperative_id, series_key, next_value)
    VALUES (p_cooperative_id, p_series_key, p_value + 1)
    ON CONFLICT (cooperative_id, series_key) DO UPDATE
        SET next_value = GREATEST(
            cooperative_number_counters.next_value,
            EXCLUDED.next_value)
    RETURNING next_value;
$$;

-- Fail-closed post-checks: the registry holds exactly the 20
-- automatic series and both functions exist.
DO $$
DECLARE
    series_count integer;
    fn_count integer;
BEGIN
    SELECT count(*) INTO series_count FROM document_number_series;
    IF series_count <> 20 THEN
        RAISE EXCEPTION 'document_number_series must hold exactly 20 series, found %', series_count;
    END IF;
    SELECT count(*) INTO fn_count FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
     WHERE n.nspname = 'public'
       AND p.proname IN ('allocate_document_number', 'reserve_document_number');
    IF fn_count <> 2 THEN
        RAISE EXCEPTION 'numbering functions missing (found %)', fn_count;
    END IF;
END $$;
