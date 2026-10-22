-- M1-P4 — controlled RLS fleet rollout, BATCH 4.
--
-- Extends the sealed-context model (0037–0042) to the identity and
-- share-ownership cluster:
--
--   persons           — the physical person record
--   families          — the family unit
--   shareholders      — the cooperative member / shareholding party
--   shares            — the share certificate
--   share_events      — append-only share lifecycle events
--   share_ownerships  — temporal ownership assignments
--
-- Selection rationale: every referrer into this set is already under
-- RLS (payments, assessments, shareholder_credits,
-- shareholder_family_memberships, governance_votes,
-- assessment_share_sources) or inside the set itself
-- (share_events, share_ownerships). Deferred referrers
-- (governance_memberships, social_aid_donations/disbursements,
-- share_returns/entitlements) are composite-FK-bound to the same
-- cooperative, so no partial-visibility anomaly can arise; they gain
-- their own RLS in a later batch.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

DO $$
DECLARE
    t text;
    batch text[] := ARRAY[
        'persons', 'families', 'shareholders',
        'shares', 'share_events', 'share_ownerships'];
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
              'persons','families','shareholders',
              'shares','share_events','share_ownerships')
          AND NOT (c.relrowsecurity AND c.relforcerowsecurity)
    ) THEN
        RAISE EXCEPTION 'batch tables must be ENABLE+FORCE';
    END IF;

    -- The rollout bound: RLS may exist only on pilot + batch 1-4.
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
              'shares','share_events','share_ownerships')
    ) THEN
        RAISE EXCEPTION 'RLS expanded beyond pilot + batch 1-4';
    END IF;

    IF (
        SELECT count(*) FROM pg_policy p
        JOIN pg_class c ON c.oid = p.polrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN (
              'persons','families','shareholders',
              'shares','share_events','share_ownerships')
    ) <> 12 THEN
        RAISE EXCEPTION 'each batch-4 table must carry exactly owner + app policies';
    END IF;
END $$;
