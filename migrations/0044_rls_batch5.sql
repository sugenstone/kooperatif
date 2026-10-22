-- M1-P4 — controlled RLS fleet rollout, BATCH 5.
--
-- Extends the sealed-context model (0037–0043) to the share-return
-- and social-aid domains — two fully closed chains:
--
--   share_returns             — the return request / lifecycle record
--   share_return_entitlements — per-entitlement rights crystallized
--                               at finalization
--   share_return_settlements  — cash outflow settling an entitlement
--   social_aid_funds          — the social-aid fund
--   social_aid_donations      — cash inflow into a fund
--   social_aid_disbursements  — cash outflow to a beneficiary
--
-- Selection rationale: the ONLY foreign-key referrers into this set
-- are inside the set (entitlements→returns, settlements→entitlements,
-- donations/disbursements→funds) — both chains close completely.
-- Outbound references point at already-forced parents (shares,
-- shareholders, persons) or deferred financial-core tables
-- (financial_accounts, account_movements) via composite/single FKs.
--
-- `cooperative_memberships` and `cooperative_number_counters` are
-- deliberately NOT in this batch — they need a dedicated security
-- design (membership drives the seal chain itself; counters are
-- already fully behind P3C definer allocators).

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

DO $$
DECLARE
    t text;
    batch text[] := ARRAY[
        'share_returns', 'share_return_entitlements',
        'share_return_settlements',
        'social_aid_funds', 'social_aid_donations',
        'social_aid_disbursements'];
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
              'share_returns','share_return_entitlements',
              'share_return_settlements',
              'social_aid_funds','social_aid_donations',
              'social_aid_disbursements')
          AND NOT (c.relrowsecurity AND c.relforcerowsecurity)
    ) THEN
        RAISE EXCEPTION 'batch tables must be ENABLE+FORCE';
    END IF;

    -- The rollout bound: RLS may exist only on pilot + batch 1-5.
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
              'social_aid_disbursements')
    ) THEN
        RAISE EXCEPTION 'RLS expanded beyond pilot + batch 1-5';
    END IF;

    IF (
        SELECT count(*) FROM pg_policy p
        JOIN pg_class c ON c.oid = p.polrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN (
              'share_returns','share_return_entitlements',
              'share_return_settlements',
              'social_aid_funds','social_aid_donations',
              'social_aid_disbursements')
    ) <> 12 THEN
        RAISE EXCEPTION 'each batch-5 table must carry exactly owner + app policies';
    END IF;
END $$;
