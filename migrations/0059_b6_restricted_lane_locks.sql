-- ============================================================
-- M1-P4-B6 — Restricted-lane lock remediation
-- ============================================================
-- The restricted principal kooperatif_app holds SELECT+INSERT on the
-- business tables — no UPDATE privilege.  `SELECT ... FOR UPDATE`
-- requires an UPDATE grant on at least one column, so every repo
-- lock site on a grant-less table failed with 42501 on the real
-- application lane (only the owner-pool HTTP suite masked it).
--
-- Confirmed failing sites (13) by locked table:
--   governance_bodies            (3) close_body, create_membership,
--                                    create_decision
--   governance_memberships       (2) end_membership, cast_vote
--   investments                  (1) lock_investment
--   investment_valuations        (1) cancel_valuation
--   shareholder_family_memberships(1) family_change
--   share_ownerships             (1) share_return initiate
--   share_returns                (1) entitlement insert case lock
--   share_return_entitlements    (2) determine, settle
--   social_aid_funds             (1) lock_fund
--
-- Working sites (14 — column-scoped UPDATE grants exist): periods,
-- shareholders, shares, financial_accounts, financial_categories,
-- governance_decisions — left untouched.
--
-- §1 re-creates the 0058 lock helpers so the cooperative filter is
-- the VERIFIED sealed scope on the application lane — a forged
-- p_coop argument can no longer lock or read another cooperative's
-- rows.  §2 adds narrow scope-pinned helpers for the nine failing
-- tables.  Every helper verifies the sealed context plus the
-- business permission inside the function; owner lanes keep the
-- p_coop semantics so fixtures and migration tooling are unaffected.
-- ============================================================

-- ---------------------------------------------------------
-- §1. Scope-pinned re-creations (0058 helpers)
-- ---------------------------------------------------------

CREATE OR REPLACE FUNCTION public.app_lock_accounts(p_ids uuid[])
RETURNS TABLE(id uuid, status text, currency text, cooperative_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    IF session_user = 'kooperatif_app' THEN
        v_scope := public.app_rbac_verified_scope_in(ARRAY[
            'payments.manage', 'financial_accounts.manage',
            'income_expense.manage', 'investments.manage',
            'share_returns.manage', 'social_aid.manage']);
    END IF;
    RETURN QUERY
        SELECT a.id, a.status, a.currency, a.cooperative_id
          FROM public.financial_accounts a
         WHERE a.id = ANY(p_ids)
           AND (v_scope IS NULL OR a.cooperative_id = v_scope)
         ORDER BY a.id
         FOR UPDATE;
END
$fn$;

CREATE OR REPLACE FUNCTION public.app_lock_assessments(p_ids uuid[])
RETURNS TABLE(id uuid, amount numeric, status text, cooperative_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    IF session_user = 'kooperatif_app' THEN
        v_scope := public.app_rbac_verified_scope_in(ARRAY[
            'payments.manage', 'assessments.manage',
            'credits.manage', 'periods.manage']);
    END IF;
    RETURN QUERY
        SELECT a.id, a.amount, a.status, a.cooperative_id
          FROM public.assessments a
         WHERE a.id = ANY(p_ids)
           AND (v_scope IS NULL OR a.cooperative_id = v_scope)
         ORDER BY a.id
         FOR UPDATE;
END
$fn$;

CREATE OR REPLACE FUNCTION public.app_lock_payment(p_id uuid, p_coop uuid)
RETURNS TABLE(status text, amount numeric, payment_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY[
            'payments.manage', 'credits.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT p.status, p.amount, p.payment_number
          FROM public.payments p
         WHERE p.id = p_id
           AND p.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;

CREATE OR REPLACE FUNCTION public.app_lock_allocation(
    p_id uuid, p_payment uuid, p_coop uuid)
RETURNS TABLE(status text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['payments.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT a.status
          FROM public.payment_allocations a
         WHERE a.id = p_id
           AND a.payment_id = p_payment
           AND a.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;

CREATE OR REPLACE FUNCTION public.app_lock_credits(
    p_coop uuid, p_shareholder uuid)
RETURNS TABLE(id uuid, amount numeric)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['credits.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT c.id, c.amount
          FROM public.shareholder_credits c
         WHERE c.shareholder_id = p_shareholder
           AND c.status = 'active'
           AND c.cooperative_id IS NOT DISTINCT FROM v_scope
         ORDER BY c.credit_number
         FOR UPDATE;
END
$fn$;

CREATE OR REPLACE FUNCTION public.app_lock_assessment(p_id uuid, p_coop uuid)
RETURNS TABLE(id uuid, amount numeric, status text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY[
            'credits.manage', 'payments.manage',
            'assessments.manage', 'periods.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT a.id, a.amount, a.status
          FROM public.assessments a
         WHERE a.id = p_id
           AND a.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;

CREATE OR REPLACE FUNCTION public.app_lock_credit_application(
    p_id uuid, p_coop uuid)
RETURNS TABLE(status text, credit_id uuid, assessment_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['credits.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT a.status, a.credit_id, a.assessment_id
          FROM public.credit_applications a
         WHERE a.id = p_id
           AND a.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;

CREATE OR REPLACE FUNCTION public.app_lock_credit(p_id uuid, p_coop uuid)
RETURNS TABLE(id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['credits.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT c.id
          FROM public.shareholder_credits c
         WHERE c.id = p_id
           AND c.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;

-- ---------------------------------------------------------
-- §2. Lock helpers for the nine grant-less tables
-- ---------------------------------------------------------

CREATE OR REPLACE FUNCTION public.app_lock_governance_body(
    p_id uuid, p_coop uuid)
RETURNS TABLE(status text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['governance.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT b.status
          FROM public.governance_bodies b
         WHERE b.id = p_id
           AND b.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_governance_body(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_governance_body(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_governance_membership(
    p_id uuid, p_coop uuid)
RETURNS TABLE(started_at timestamptz, ended_at timestamptz)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['governance.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT m.started_at, m.ended_at
          FROM public.governance_memberships m
         WHERE m.id = p_id
           AND m.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_governance_membership(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_governance_membership(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_governance_voter(
    p_body uuid, p_person uuid, p_coop uuid)
RETURNS TABLE(id uuid, started_at timestamptz)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['governance.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT m.id, m.started_at
          FROM public.governance_memberships m
         WHERE m.body_id = p_body
           AND m.person_id = p_person
           AND m.ended_at IS NULL
           AND m.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_governance_voter(uuid, uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_governance_voter(uuid, uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_investment(
    p_id uuid, p_coop uuid)
RETURNS TABLE(status text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['investments.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT i.status
          FROM public.investments i
         WHERE i.id = p_id
           AND i.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_investment(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_investment(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_investment_valuation(
    p_id uuid, p_coop uuid)
RETURNS TABLE(status text, valuation_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['investments.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT v.status, v.valuation_number
          FROM public.investment_valuations v
         WHERE v.id = p_id
           AND v.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_investment_valuation(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_investment_valuation(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_family_membership(
    p_shareholder uuid, p_coop uuid)
RETURNS TABLE(family_id uuid, sequence_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY[
            'families.manage', 'shareholders.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT m.family_id, f.sequence_number
          FROM public.shareholder_family_memberships m
          JOIN public.families f ON f.id = m.family_id
         WHERE m.shareholder_id = p_shareholder
           AND m.ended_at IS NULL
           AND m.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE OF m;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_family_membership(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_family_membership(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_share_ownership(
    p_share uuid, p_coop uuid)
RETURNS TABLE(id uuid, shareholder_id uuid, started_at timestamptz)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY[
            'share_returns.manage', 'shares.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT o.id, o.shareholder_id, o.started_at
          FROM public.share_ownerships o
         WHERE o.share_id = p_share
           AND o.ended_at IS NULL
           AND o.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_share_ownership(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_share_ownership(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_share_return(
    p_id uuid, p_coop uuid)
RETURNS TABLE(status text, shareholder_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['share_returns.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT r.status, r.shareholder_id
          FROM public.share_returns r
         WHERE r.id = p_id
           AND r.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_share_return(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_share_return(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_share_return_entitlement(
    p_id uuid, p_coop uuid)
RETURNS TABLE(amount numeric, entitlement_number bigint,
              status text, updated_at timestamptz)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['share_returns.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT e.amount, e.entitlement_number, e.status, e.updated_at
          FROM public.share_return_entitlements e
         WHERE e.id = p_id
           AND e.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_share_return_entitlement(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_share_return_entitlement(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_finalized_entitlement(
    p_id uuid, p_coop uuid)
RETURNS TABLE(status text, amount numeric, entitlement_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['share_returns.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT e.status, e.amount, e.entitlement_number
          FROM public.share_return_entitlements e
          JOIN public.share_returns r
            ON r.id = e.share_return_id AND r.status = 'finalized'
         WHERE e.id = p_id
           AND e.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE OF e;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_finalized_entitlement(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_finalized_entitlement(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_social_aid_fund(
    p_id uuid, p_coop uuid)
RETURNS TABLE(status text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_scope uuid;
BEGIN
    v_scope := CASE WHEN session_user = 'kooperatif_app'
        THEN public.app_rbac_verified_scope_in(ARRAY['social_aid.manage'])
        ELSE p_coop END;
    RETURN QUERY
        SELECT f.status
          FROM public.social_aid_funds f
         WHERE f.id = p_id
           AND f.cooperative_id IS NOT DISTINCT FROM v_scope
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_social_aid_fund(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_social_aid_fund(uuid, uuid)
    TO kooperatif_app;

-- ---------------------------------------------------------
-- §3. Postchecks
-- ---------------------------------------------------------
DO $$
DECLARE
    fn record;
BEGIN
    FOR fn IN
        SELECT p.proname AS name,
               oidvectortypes(p.proargtypes) AS args
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public'
           AND p.proname IN (
               'app_lock_governance_body',
               'app_lock_governance_membership',
               'app_lock_governance_voter',
               'app_lock_investment',
               'app_lock_investment_valuation',
               'app_lock_family_membership',
               'app_lock_share_ownership',
               'app_lock_share_return',
               'app_lock_share_return_entitlement',
               'app_lock_finalized_entitlement',
               'app_lock_social_aid_fund')
    LOOP
        IF NOT has_function_privilege(
            'kooperatif_app',
            format('public.%I(%s)', fn.name, fn.args),
            'EXECUTE') THEN
            RAISE EXCEPTION '% not executable by kooperatif_app', fn.name;
        END IF;
    END LOOP;

    -- All lock helpers must be SECURITY DEFINER — the whole point is
    -- owner-privileged row locking inside the verified scope.
    IF EXISTS (
        SELECT 1 FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public'
           AND p.proname LIKE 'app\_lock\_%' ESCAPE '\'
           AND NOT p.prosecdef) THEN
        RAISE EXCEPTION 'a lock helper is not SECURITY DEFINER';
    END IF;
END $$;
