-- M1-P4 — controlled RLS fleet rollout, BATCH 3.
--
-- Extends the sealed-context model (0037–0041) to the investment
-- lifecycle cluster plus the governance decision record, closing both
-- chains end-to-end:
--
--   investments                  — the investment asset itself
--   investment_fundings          — cash outflow funding the asset
--   investment_valuations        — recorded valuation snapshots
--   investment_disposals         — the disposal event
--   investment_disposal_proceeds — cash inflow from a disposal
--   governance_decisions         — the decision a vote belongs to
--
-- Selection rationale: every table that references INTO this set is
-- already under RLS (`investment_incomes`, `governance_votes` —
-- batch 1) or inside the set, so no partial-visibility anomaly can
-- appear along either chain. All writes already run inside the sealed
-- request_tx envelope; the SECURITY DEFINER surfaces execute as
-- `kooperatif` and stay on the owner lane.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

DO $$
DECLARE
    t text;
    batch text[] := ARRAY[
        'investments', 'investment_fundings', 'investment_valuations',
        'investment_disposals', 'investment_disposal_proceeds',
        'governance_decisions'];
BEGIN
    FOREACH t IN ARRAY batch LOOP
        -- Sanity: every batch table must already be cooperative-owned.
        IF NOT EXISTS (
            SELECT 1 FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = t
               AND column_name = 'cooperative_id' AND is_nullable = 'NO'
        ) THEN
            RAISE EXCEPTION 'batch table % lacks NOT NULL cooperative_id', t;
        END IF;

        EXECUTE format('ALTER TABLE public.%I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE public.%I FORCE ROW LEVEL SECURITY', t);

        EXECUTE format(
            'CREATE POLICY %I ON public.%I TO kooperatif USING (true) WITH CHECK (true)',
            t || '_owner_lane', t);
        EXECUTE format(
            'CREATE POLICY %I ON public.%I TO kooperatif_app USING ('
          || ' cooperative_id = nullif(current_setting(''app.cooperative_id'', true), '''')::uuid'
          || ' AND public.app_ctx_ok())'
          || ' WITH CHECK ('
          || ' cooperative_id = nullif(current_setting(''app.cooperative_id'', true), '''')::uuid'
          || ' AND public.app_ctx_ok())',
            t || '_app_cooperative', t);
    END LOOP;
END $$;

-- ── Fail-closed post-checks ───────────────────────────────────────

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN (
              'investments','investment_fundings','investment_valuations',
              'investment_disposals','investment_disposal_proceeds',
              'governance_decisions')
          AND NOT (c.relrowsecurity AND c.relforcerowsecurity)
    ) THEN
        RAISE EXCEPTION 'batch tables must be ENABLE+FORCE';
    END IF;

    -- The rollout bound: RLS may exist only on pilot + batch1 + batch2
    -- + batch3.
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
              'governance_decisions')
    ) THEN
        RAISE EXCEPTION 'RLS expanded beyond pilot + batch 1-3';
    END IF;

    IF (
        SELECT count(*) FROM pg_policy p
        JOIN pg_class c ON c.oid = p.polrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN (
              'investments','investment_fundings','investment_valuations',
              'investment_disposals','investment_disposal_proceeds',
              'governance_decisions')
    ) <> 12 THEN
        RAISE EXCEPTION 'each batch-3 table must carry exactly owner + app policies';
    END IF;
END $$;
