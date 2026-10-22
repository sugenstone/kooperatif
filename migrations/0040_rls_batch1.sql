-- M1-P4 — controlled RLS fleet rollout, BATCH 1.
--
-- Expands the sealed-context RLS model (0037/0038/0039) to six
-- cooperative-owned tables, all chosen as dependency LEAVES — no other
-- table holds a foreign key INTO them, so enabling FORCE RLS here
-- cannot break referential checks elsewhere, and every application
-- access already runs inside the sealed `request_tx` envelope:
--
--   income_entries                  — financial inflow ledger rows
--   expense_entries                 — financial outflow ledger rows
--   payment_allocations             — payment→debt allocation history
--   shareholder_family_memberships  — shareholder↔family join facts
--   governance_votes                — recorded vote evidence
--   investment_incomes              — investment yield records
--
-- Policies mirror the pilot exactly: an unrestricted owner lane for
-- `kooperatif` (migrations, fixtures, CLI) and a sealed-cooperative
-- lane for `kooperatif_app` requiring the HMAC ctx seal
-- (`app_ctx_ok()`), not merely the GUC value.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

DO $$
DECLARE
    t text;
    batch text[] := ARRAY[
        'income_entries', 'expense_entries', 'payment_allocations',
        'shareholder_family_memberships', 'governance_votes',
        'investment_incomes'];
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
    -- Every batch table must be ENABLE+FORCE with exactly two policies.
    IF EXISTS (
        SELECT 1 FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN (
              'income_entries','expense_entries','payment_allocations',
              'shareholder_family_memberships','governance_votes',
              'investment_incomes')
          AND NOT (c.relrowsecurity AND c.relforcerowsecurity)
    ) THEN
        RAISE EXCEPTION 'batch tables must be ENABLE+FORCE';
    END IF;

    -- The rollout bound: RLS may exist only on pilot + batch tables.
    IF EXISTS (
        SELECT 1 FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relrowsecurity
          AND c.relname NOT IN (
              'periods','governance_bodies',
              'income_entries','expense_entries','payment_allocations',
              'shareholder_family_memberships','governance_votes',
              'investment_incomes')
    ) THEN
        RAISE EXCEPTION 'RLS expanded beyond pilot + batch 1';
    END IF;

    IF (
        SELECT count(*) FROM pg_policy p
        JOIN pg_class c ON c.oid = p.polrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN (
              'income_entries','expense_entries','payment_allocations',
              'shareholder_family_memberships','governance_votes',
              'investment_incomes')
    ) <> 12 THEN
        RAISE EXCEPTION 'each batch table must carry exactly owner + app policies';
    END IF;
END $$;
