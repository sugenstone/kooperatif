-- M1-P4 — special security tables, STEP S1: cooperative_number_counters.
--
-- The audit (implementation/M1-P4-SPECIAL-SECURITY-TABLES-AUDIT.md)
-- showed this table is already fully behind the P3C SECURITY DEFINER
-- allocators: kooperatif_app holds ZERO direct grants; all numbering
-- flows through public.allocate_document_number /
-- public.reserve_document_number (owner = kooperatif, sealed-context
-- gate inside).
--
-- ENABLE + FORCE here is defense-in-depth: it closes the residual
-- path where a future mistaken GRANT would expose the counters. There
-- is deliberately NO kooperatif_app policy — under FORCE RLS the
-- absence of a matching policy is a full deny for the runtime role,
-- which is exactly the intended surface. The sole policy is the owner
-- lane so owner-lane operations (migrations, allocator bodies,
-- administrative inspection and test fixtures) keep working — the
-- allocator function bodies execute as their owner (kooperatif), so
-- they are covered by the same lane.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

ALTER TABLE public.cooperative_number_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.cooperative_number_counters FORCE ROW LEVEL SECURITY;

CREATE POLICY cooperative_number_counters_owner_lane
    ON public.cooperative_number_counters
    TO kooperatif
    USING (true)
    WITH CHECK (true);

-- ── Fail-closed post-checks ───────────────────────────────────────

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname = 'cooperative_number_counters'
          AND c.relrowsecurity AND c.relforcerowsecurity
    ) THEN
        RAISE EXCEPTION 'cooperative_number_counters must be ENABLE+FORCE';
    END IF;

    -- Exactly ONE policy on the counters table: the owner lane. Any
    -- kooperatif_app policy would be a design bug — the table is meant
    -- to be invisible to the runtime role.
    IF (
        SELECT count(*) FROM pg_policy p
        JOIN pg_class c ON c.oid = p.polrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname = 'cooperative_number_counters'
    ) <> 1 THEN
        RAISE EXCEPTION 'counters table must carry exactly the owner lane';
    END IF;

    -- The rollout bound: RLS may exist only on pilot + batch 1-6 + S1.
    IF EXISTS (
        SELECT 1 FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relrowsecurity
          AND c.relname NOT IN (
              'periods','governance_bodies',
              'income_entries','expense_entries','payment_allocations',
              'shareholder_family_memberships','governance_votes',
              'investment_incomes',
              'payments','assessments','assessment_rules',
              'assessment_share_sources','shareholder_credits',
              'credit_applications',
              'investments','investment_fundings','investment_valuations',
              'investment_disposals','investment_disposal_proceeds',
              'governance_decisions',
              'persons','families','shareholders',
              'shares','share_events','share_ownerships',
              'share_returns','share_return_entitlements',
              'share_return_settlements',
              'social_aid_funds','social_aid_donations',
              'social_aid_disbursements',
              'financial_accounts','financial_categories',
              'account_movements','account_transfers',
              'governance_memberships',
              'cooperative_number_counters')
    ) THEN
        RAISE EXCEPTION 'RLS expanded beyond pilot + batch 1-6 + S1';
    END IF;
END $$;
