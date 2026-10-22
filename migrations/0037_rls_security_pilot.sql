-- M1-P4-P3A — RLS security pilot on `periods` + `governance_bodies`.
--
-- Bounded pilot, NOT a fleet rollout: row security is enabled and
-- FORCED on exactly two cooperative-owned tables chosen for low
-- blast radius — both are evidence/structure tables that carry no
-- monetary movements (`periods` defines collection windows,
-- `governance_bodies` is governance metadata) and both are the only
-- business tables with a real DELETE surface (`periods` draft
-- deletion exercises all four verbs).
--
-- Policy model:
--   * `TO kooperatif`      — the owner/migration/CLI lane keeps full
--     visibility (FORCE would otherwise fail closed for the owner too).
--   * `TO kooperatif_app`  — the HTTP runtime lane sees only rows of
--     the transaction-stamped `app.cooperative_id`. Missing or unset
--     context fails closed: `current_setting(…, true)` yields NULL or
--     '' after the pool's RESET, `nullif(…,'')::uuid` collapses both
--     to NULL, and `cooperative_id = NULL` matches nothing. No NULL
--     branch exists because 0025 made `cooperative_id NOT NULL` — a
--     database carrying 0037 provably cannot hold unattributed rows.
--
-- KNOWN RESIDUAL RISK (documented, deliberately not claimed solved):
--   `app.cooperative_id` is an ordinary custom GUC — `kooperatif_app`
--   can SET it directly, and it holds SELECT on `user_sessions`
--   (token hashes), so arbitrary SQL executed AS the app role can
--   mint a foreign context. RLS here is a second defensive line
--   against application-layer scope bugs inside the verified
--   `request_tx` envelope, not a hard boundary against a compromised
--   runtime credential. A hard boundary requires sealing the context
--   (definer-minted proof the role cannot forge) plus narrowing
--   session-table reads behind SECURITY DEFINER accessors — that is a
--   tracked follow-up before any fleet rollout.
--
-- Bootstrap: the 37-table orphan scan runs pre-context as the runtime
--   principal; plain EXISTS probes on RLS-covered tables would be
--   silently filtered (fail-open). The scan moves behind a SECURITY
--   DEFINER function owned by the migration principal so it keeps
--   seeing every row.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 1. Bootstrap orphan scan, definer-hardened ───────────────────

CREATE OR REPLACE FUNCTION public.app_bootstrap_orphan_scan()
RETURNS text
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    -- Mirrors the 0021 claim list / the former repo-side loop exactly.
    -- Returns the first offending table name, NULL when clean.
    IF EXISTS (SELECT 1 FROM public.persons WHERE cooperative_id IS NULL)
        THEN RETURN 'persons'; END IF;
    IF EXISTS (SELECT 1 FROM public.families WHERE cooperative_id IS NULL)
        THEN RETURN 'families'; END IF;
    IF EXISTS (SELECT 1 FROM public.shareholders WHERE cooperative_id IS NULL)
        THEN RETURN 'shareholders'; END IF;
    IF EXISTS (SELECT 1 FROM public.shareholder_family_memberships WHERE cooperative_id IS NULL)
        THEN RETURN 'shareholder_family_memberships'; END IF;
    IF EXISTS (SELECT 1 FROM public.shares WHERE cooperative_id IS NULL)
        THEN RETURN 'shares'; END IF;
    IF EXISTS (SELECT 1 FROM public.share_events WHERE cooperative_id IS NULL)
        THEN RETURN 'share_events'; END IF;
    IF EXISTS (SELECT 1 FROM public.share_ownerships WHERE cooperative_id IS NULL)
        THEN RETURN 'share_ownerships'; END IF;
    IF EXISTS (SELECT 1 FROM public.periods WHERE cooperative_id IS NULL)
        THEN RETURN 'periods'; END IF;
    IF EXISTS (SELECT 1 FROM public.assessment_rules WHERE cooperative_id IS NULL)
        THEN RETURN 'assessment_rules'; END IF;
    IF EXISTS (SELECT 1 FROM public.assessments WHERE cooperative_id IS NULL)
        THEN RETURN 'assessments'; END IF;
    IF EXISTS (SELECT 1 FROM public.assessment_share_sources WHERE cooperative_id IS NULL)
        THEN RETURN 'assessment_share_sources'; END IF;
    IF EXISTS (SELECT 1 FROM public.payments WHERE cooperative_id IS NULL)
        THEN RETURN 'payments'; END IF;
    IF EXISTS (SELECT 1 FROM public.payment_allocations WHERE cooperative_id IS NULL)
        THEN RETURN 'payment_allocations'; END IF;
    IF EXISTS (SELECT 1 FROM public.financial_accounts WHERE cooperative_id IS NULL)
        THEN RETURN 'financial_accounts'; END IF;
    IF EXISTS (SELECT 1 FROM public.account_movements WHERE cooperative_id IS NULL)
        THEN RETURN 'account_movements'; END IF;
    IF EXISTS (SELECT 1 FROM public.account_transfers WHERE cooperative_id IS NULL)
        THEN RETURN 'account_transfers'; END IF;
    IF EXISTS (SELECT 1 FROM public.shareholder_credits WHERE cooperative_id IS NULL)
        THEN RETURN 'shareholder_credits'; END IF;
    IF EXISTS (SELECT 1 FROM public.credit_applications WHERE cooperative_id IS NULL)
        THEN RETURN 'credit_applications'; END IF;
    IF EXISTS (SELECT 1 FROM public.financial_categories WHERE cooperative_id IS NULL)
        THEN RETURN 'financial_categories'; END IF;
    IF EXISTS (SELECT 1 FROM public.income_entries WHERE cooperative_id IS NULL)
        THEN RETURN 'income_entries'; END IF;
    IF EXISTS (SELECT 1 FROM public.expense_entries WHERE cooperative_id IS NULL)
        THEN RETURN 'expense_entries'; END IF;
    IF EXISTS (SELECT 1 FROM public.share_returns WHERE cooperative_id IS NULL)
        THEN RETURN 'share_returns'; END IF;
    IF EXISTS (SELECT 1 FROM public.share_return_entitlements WHERE cooperative_id IS NULL)
        THEN RETURN 'share_return_entitlements'; END IF;
    IF EXISTS (SELECT 1 FROM public.share_return_settlements WHERE cooperative_id IS NULL)
        THEN RETURN 'share_return_settlements'; END IF;
    IF EXISTS (SELECT 1 FROM public.investments WHERE cooperative_id IS NULL)
        THEN RETURN 'investments'; END IF;
    IF EXISTS (SELECT 1 FROM public.investment_fundings WHERE cooperative_id IS NULL)
        THEN RETURN 'investment_fundings'; END IF;
    IF EXISTS (SELECT 1 FROM public.investment_valuations WHERE cooperative_id IS NULL)
        THEN RETURN 'investment_valuations'; END IF;
    IF EXISTS (SELECT 1 FROM public.investment_incomes WHERE cooperative_id IS NULL)
        THEN RETURN 'investment_incomes'; END IF;
    IF EXISTS (SELECT 1 FROM public.investment_disposals WHERE cooperative_id IS NULL)
        THEN RETURN 'investment_disposals'; END IF;
    IF EXISTS (SELECT 1 FROM public.investment_disposal_proceeds WHERE cooperative_id IS NULL)
        THEN RETURN 'investment_disposal_proceeds'; END IF;
    IF EXISTS (SELECT 1 FROM public.social_aid_funds WHERE cooperative_id IS NULL)
        THEN RETURN 'social_aid_funds'; END IF;
    IF EXISTS (SELECT 1 FROM public.social_aid_donations WHERE cooperative_id IS NULL)
        THEN RETURN 'social_aid_donations'; END IF;
    IF EXISTS (SELECT 1 FROM public.social_aid_disbursements WHERE cooperative_id IS NULL)
        THEN RETURN 'social_aid_disbursements'; END IF;
    IF EXISTS (SELECT 1 FROM public.governance_bodies WHERE cooperative_id IS NULL)
        THEN RETURN 'governance_bodies'; END IF;
    IF EXISTS (SELECT 1 FROM public.governance_memberships WHERE cooperative_id IS NULL)
        THEN RETURN 'governance_memberships'; END IF;
    IF EXISTS (SELECT 1 FROM public.governance_decisions WHERE cooperative_id IS NULL)
        THEN RETURN 'governance_decisions'; END IF;
    IF EXISTS (SELECT 1 FROM public.governance_votes WHERE cooperative_id IS NULL)
        THEN RETURN 'governance_votes'; END IF;
    RETURN NULL;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_bootstrap_orphan_scan() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_bootstrap_orphan_scan()
    TO kooperatif_app;

-- ── 2. Pilot tables: enable + FORCE ──────────────────────────────

ALTER TABLE public.periods ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.periods FORCE ROW LEVEL SECURITY;
ALTER TABLE public.governance_bodies ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.governance_bodies FORCE ROW LEVEL SECURITY;

-- Owner / migration / CLI lane: unrestricted under FORCE.
CREATE POLICY periods_owner_lane ON public.periods
    TO kooperatif
    USING (true)
    WITH CHECK (true);
CREATE POLICY governance_bodies_owner_lane ON public.governance_bodies
    TO kooperatif
    USING (true)
    WITH CHECK (true);

-- Runtime lane: the row's cooperative must equal the
-- transaction-stamped, membership-verified context GUC. Unset or
-- empty context fails closed (NULL never equals anything).
CREATE POLICY periods_app_cooperative ON public.periods
    TO kooperatif_app
    USING (
        cooperative_id
            = nullif(current_setting('app.cooperative_id', true), '')::uuid
    )
    WITH CHECK (
        cooperative_id
            = nullif(current_setting('app.cooperative_id', true), '')::uuid
    );
CREATE POLICY governance_bodies_app_cooperative ON public.governance_bodies
    TO kooperatif_app
    USING (
        cooperative_id
            = nullif(current_setting('app.cooperative_id', true), '')::uuid
    )
    WITH CHECK (
        cooperative_id
            = nullif(current_setting('app.cooperative_id', true), '')::uuid
    );

-- ── 3. Fail-closed post-checks ───────────────────────────────────

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN ('periods', 'governance_bodies')
          AND NOT (c.relrowsecurity AND c.relforcerowsecurity)
    ) THEN
        RAISE EXCEPTION 'RLS pilot tables must be ENABLE+FORCE';
    END IF;

    -- Pilot bound: no other table may carry row security.
    IF EXISTS (
        SELECT 1 FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relrowsecurity
          AND c.relname NOT IN ('periods', 'governance_bodies')
    ) THEN
        RAISE EXCEPTION 'RLS must not be enabled outside the pilot tables';
    END IF;

    IF (
        SELECT count(*) FROM pg_policy p
        JOIN pg_class c ON c.oid = p.polrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relname IN ('periods', 'governance_bodies')
    ) <> 4 THEN
        RAISE EXCEPTION 'each pilot table must carry exactly owner + app policies';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_proc p
        WHERE p.proname = 'app_bootstrap_orphan_scan'
          AND p.pronamespace = 'public'::regnamespace
          AND p.prosecdef
          AND p.proconfig::text[] @> ARRAY['search_path=public, pg_catalog']
    ) THEN
        RAISE EXCEPTION 'app_bootstrap_orphan_scan missing or not hardened';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_proc p
        WHERE p.proname = 'app_bootstrap_orphan_scan'
          AND p.pronamespace = 'public'::regnamespace
          AND (p.proacl IS NULL
               OR EXISTS (
                   SELECT 1 FROM unnest(p.proacl) AS acl
                    WHERE split_part(acl::text, '=', 1) = ''))
    ) THEN
        RAISE EXCEPTION 'app_bootstrap_orphan_scan must not be PUBLIC';
    END IF;
END $$;
