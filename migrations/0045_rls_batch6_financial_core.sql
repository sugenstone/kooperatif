-- M1-P4 — controlled RLS fleet rollout, BATCH 6: FINANCIAL CORE.
--
-- Extends the sealed-context model (0037–0044) to the financial-core
-- ledger plus the last independent leaf:
--
--   financial_accounts   — cash/bank accounts (referenced by every
--                          financial table; composite FK parents)
--   financial_categories — income/expense categories (referenced by
--                          income_entries/expense_entries)
--   account_movements    — the money ledger itself; polymorphic
--                          source_id (no FK), composite FK to
--                          financial_accounts
--   account_transfers    — net-zero account-to-account moves
--   governance_memberships — body membership windows; only referrer
--                            is already-forced governance_votes
--
-- Selection rationale: every foreign-key referrer into this set is
-- already ENABLE+FORCE (payments, income/expense_entries, all
-- investment legs, share_return_settlements, social_aid_*,
-- shareholders, governance_votes) or inside the set
-- (account_movements/account_transfers→financial_accounts) — the
-- financial core closes completely.
--
-- Pre-context / owner-lane / definer audit (clean):
--   * Every repository write runs inside the sealed request_tx with
--     cooperative_id stamped — financial_accounts, income_expense,
--     share_returns, social_aid, investments, payments verified.
--   * The bootstrap category seed (tenant::repo) and the CLI
--     bootstrap-cooperative path run on the administrative pool —
--     owner lane, unaffected.
--   * The only SECURITY DEFINER touching these tables is
--     public.app_bootstrap_orphan_scan, owned by kooperatif — the
--     owner lane keeps its NULL-orphan probes fail-closed correct.
--   * Realtime triggers on all five tables only call pg_notify.
--   * account_movements.source_id is intentionally polymorphic
--     (no FK) — nothing for RLS to interact with.
--
-- cooperative_memberships, cooperative_number_counters and the three
-- nullable system-lane tables stay OUT — they need dedicated
-- designs.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

DO $$
DECLARE
    t text;
    batch text[] := ARRAY[
        'financial_accounts', 'financial_categories',
        'account_movements', 'account_transfers',
        'governance_memberships'];
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
              'financial_accounts','financial_categories',
              'account_movements','account_transfers',
              'governance_memberships')
          AND NOT (c.relrowsecurity AND c.relforcerowsecurity)
    ) THEN
        RAISE EXCEPTION 'batch tables must be ENABLE+FORCE';
    END IF;

    -- The rollout bound: RLS may exist only on pilot + batch 1-6.
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
              'governance_memberships')
    ) THEN
        RAISE EXCEPTION 'RLS expanded beyond pilot + batch 1-6';
    END IF;

    IF (
        SELECT count(*) FROM pg_policy p
        JOIN pg_class c ON c.oid = p.polrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN (
              'financial_accounts','financial_categories',
              'account_movements','account_transfers',
              'governance_memberships')
    ) <> 10 THEN
        RAISE EXCEPTION 'each batch-6 table must carry exactly owner + app policies';
    END IF;
END $$;
