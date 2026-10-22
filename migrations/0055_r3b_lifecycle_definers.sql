-- ============================================================
-- M1-P4-R3B — financial reversal & lifecycle authorization boundary
-- (closure-audit F4 residual)
--
-- 0054 proved the SHAPE of lifecycle writes (column-dar grants +
-- forward-only RESTRICTIVE guards) but any sealed cooperative member
-- could still run them: a member with no permission executed
--   UPDATE payments SET status='reversed', reversal_reason='fabricated'
-- inside a sealed transaction (verified on kooperatif_test v54).
-- The permission check lived only at the HTTP layer.
--
-- This migration moves every lifecycle/terminal transition behind
-- narrow SECURITY DEFINER functions that re-verify the sealed
-- context AND the module .manage permission inside the database
-- boundary (app_rbac_verified_scope, 0052). The corresponding
-- direct UPDATE column grants are revoked; what remains for the
-- runtime role is draft/config columns only.
--
-- Invariants:
--   * every definer derives the cooperative from the VERIFIED sealed
--     context — a caller-supplied cooperative_id is never trusted;
--   * definers carry their own transition preconditions
--     (status/domain/terminal checks) — the 0054 RESTRICTIVE guards
--     stay as defense-in-depth for the app role;
--   * no new broad mutation surface: each function performs exactly
--     one domain transition;
--   * lane semantics inside every definer: `v_restricted` is true for
--     the app role and for any sealed context — the row's
--     cooperative_id must then match the VERIFIED stamped scope.
--     An unsealed owner connection (migrations/CLI/fixtures/
--     repo-level tests) supplies p_coop itself, preserving the
--     caller-scoped semantics the repositories had before — while
--     the app role can never smuggle p_coop past the verified scope
--     because it is ignored whenever a restricted lane is active;
--   * owner lane keeps working — app_rbac_verified_scope returns the
--     stamped scope without a permission check for non-app
--     session_user;
--   * EXECUTE is granted per function; helpers stay closed.
-- ============================================================

-- ------------------------------------------------------------------
-- Helper: verify the sealed context and either permission (some
-- operations legitimately run under two module permissions —
-- e.g. share-ownership interval closes happen from both the shares
-- lifecycle and the share-return finalization).
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_rbac_verified_scope_any(
    p_perm_a text,
    p_perm_b text)
RETURNS uuid
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
BEGIN
    BEGIN
        RETURN public.app_rbac_verified_scope(p_perm_a);
    EXCEPTION WHEN insufficient_privilege THEN
        RETURN public.app_rbac_verified_scope(p_perm_b);
    END;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_rbac_verified_scope_any(text, text) FROM PUBLIC;

-- ------------------------------------------------------------------
-- account_movements — status-based reversal is only reachable through
-- a source-domain reversal. The required permission is derived from
-- source_type INSIDE the function so a payments.manage holder cannot
-- reverse movements that belong to a social-aid disbursement.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_movements_reverse_of_source(
    p_source_type text,
    p_source_id   uuid,
    p_at          timestamptz,
    p_by          uuid,
    p_reason      text,
    p_coop uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_perm  text;
    v_scope uuid;
    v_n     bigint;
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    v_perm := CASE p_source_type
        WHEN 'payment'                  THEN 'payments.manage'
        WHEN 'transfer'                 THEN 'financial_accounts.manage'
        WHEN 'income'                   THEN 'income_expense.manage'
        WHEN 'expense'                  THEN 'income_expense.manage'
        WHEN 'share_return_settlement'  THEN 'share_returns.manage'
        WHEN 'investment_funding'       THEN 'investments.manage'
        WHEN 'investment_income'        THEN 'investments.manage'
        WHEN 'investment_disposal'      THEN 'investments.manage'
        WHEN 'social_aid_donation'      THEN 'social_aid.manage'
        WHEN 'social_aid_disbursement'  THEN 'social_aid.manage'
        ELSE NULL END;
    IF v_perm IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: unknown movement source %', p_source_type
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    v_scope := public.app_rbac_verified_scope(v_perm);
    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason
     WHERE source_type = p_source_type AND source_id = p_source_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    GET DIAGNOSTICS v_n = ROW_COUNT;
    RETURN v_n;
END;
$function$;

-- ------------------------------------------------------------------
-- payments + allocations (payments.manage)
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_payment_reverse(
    p_payment uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('payments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_n     bigint;
BEGIN
    UPDATE public.payment_allocations
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason
     WHERE payment_id = p_payment AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    GET DIAGNOSTICS v_n = ROW_COUNT;
    UPDATE public.payments
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_payment AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    RETURN v_n;
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_payment_allocation_reverse(
    p_allocation uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('payments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.payment_allocations
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason
     WHERE id = p_allocation AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- ------------------------------------------------------------------
-- financial_accounts (financial_accounts.manage)
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_transfer_reverse(
    p_transfer uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('financial_accounts.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.account_transfers
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_transfer AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_financial_account_set_status(
    p_id uuid, p_status text,
    p_coop uuid)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('financial_accounts.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    IF p_status NOT IN ('active', 'inactive') THEN
        RAISE EXCEPTION 'check_violation: invalid account status %', p_status
            USING ERRCODE = 'check_violation';
    END IF;
    UPDATE public.financial_accounts
       SET status = p_status, updated_at = now()
     WHERE id = p_id
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    RETURN FOUND;
END;
$function$;

-- ------------------------------------------------------------------
-- income / expense entries + categories (income_expense.manage)
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_income_entry_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('income_expense.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.income_entries
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_expense_entry_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('income_expense.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.expense_entries
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_financial_category_set_status(
    p_id uuid, p_status text,
    p_coop uuid)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('income_expense.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    IF p_status NOT IN ('active', 'inactive') THEN
        RAISE EXCEPTION 'check_violation: invalid category status %', p_status
            USING ERRCODE = 'check_violation';
    END IF;
    UPDATE public.financial_categories
       SET status = p_status, updated_at = now()
     WHERE id = p_id
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    RETURN FOUND;
END;
$function$;

-- ------------------------------------------------------------------
-- credits (credits.manage). The bulk reconcile runs inside the
-- payments reversal — its permission is payments.manage because the
-- credits exist only as a side effect of that payment.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_credit_application_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('credits.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.credit_applications
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_credit_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('credits.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.shareholder_credits
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_credits_reverse_by_ids(
    p_ids uuid[], p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('payments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_n     bigint;
BEGIN
    UPDATE public.shareholder_credits
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = ANY(p_ids) AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    GET DIAGNOSTICS v_n = ROW_COUNT;
    RETURN v_n;
END;
$function$;

-- ------------------------------------------------------------------
-- investments (investments.manage)
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_investment_cancel(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.investments
       SET status = 'cancelled', cancelled_at = p_at, cancelled_by = p_by,
           cancellation_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_investment_funding_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.investment_fundings
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_investment_valuation_cancel(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.investment_valuations
       SET status = 'cancelled', cancelled_at = p_at, cancelled_by = p_by,
           cancellation_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'recorded'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_investment_income_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.investment_incomes
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_investment_dispose(
    p_id uuid, p_at timestamptz, p_by uuid,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.investments
       SET status = 'disposed', disposed_at = p_at, disposed_by = p_by,
           updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- ------------------------------------------------------------------
-- share returns (share_returns.manage). The shares.status side
-- effects fold into the case definers — shares is never writable
-- directly.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_share_mark_return_pending(
    p_share uuid, p_at timestamptz,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.shares
       SET status = 'return_pending', updated_at = p_at
     WHERE id = p_share AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_share_return_cancel(
    p_return uuid, p_share uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.share_returns
       SET status = 'cancelled', cancelled_at = p_at, cancelled_by = p_by,
           cancellation_reason = p_reason, updated_at = p_at
     WHERE id = p_return AND status = 'pending'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    UPDATE public.shares
       SET status = 'active', updated_at = p_at
     WHERE id = p_share AND status = 'return_pending'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_share_return_finalize(
    p_return uuid, p_share uuid, p_at timestamptz, p_by uuid,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.share_returns
       SET status = 'finalized', finalized_at = p_at, finalized_by = p_by,
           updated_at = p_at
     WHERE id = p_return AND status = 'pending'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    UPDATE public.shares
       SET status = 'closed', updated_at = p_at
     WHERE id = p_share AND status = 'return_pending'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_share_return_entitlement_determine(
    p_id       uuid,
    p_amount   numeric,
    p_due      date,
    p_policy   text,
    p_desc     text,
    p_now      timestamptz,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.share_return_entitlements
       SET amount = p_amount,
           due_date = COALESCE(p_due, due_date),
           policy_reference = COALESCE(p_policy, policy_reference),
           description = COALESCE(p_desc, description),
           determined_at = p_now, updated_at = p_now
     WHERE id = p_id AND status = 'open' AND amount IS NULL
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_share_return_entitlement_cancel(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.share_return_entitlements
       SET status = 'cancelled', cancelled_at = p_at, cancelled_by = p_by,
           cancellation_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'open'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- Derived recompute (open|partially_settled|settled) — never a
-- hand-edit; cancelled stays terminal.
CREATE OR REPLACE FUNCTION public.app_share_return_entitlement_refresh(
    p_id uuid,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.share_return_entitlements e
       SET status = CASE
            WHEN e.amount IS NOT NULL AND settled.s >= e.amount THEN 'settled'
            WHEN settled.s > 0 THEN 'partially_settled'
            ELSE 'open' END,
           updated_at = now()
      FROM (SELECT COALESCE(sum(amount), 0) AS s
              FROM public.share_return_settlements
             WHERE entitlement_id = p_id AND status = 'posted') settled
     WHERE e.id = p_id AND e.status <> 'cancelled'
       AND e.cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_share_return_settlement_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.share_return_settlements
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- ------------------------------------------------------------------
-- social aid (social_aid.manage)
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_social_aid_fund_close(
    p_id uuid, p_at timestamptz, p_by uuid,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('social_aid.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.social_aid_funds
       SET status = 'closed', closed_at = p_at, closed_by = p_by,
           updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_social_aid_fund_cancel(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('social_aid.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.social_aid_funds
       SET status = 'cancelled', cancelled_at = p_at, cancelled_by = p_by,
           cancellation_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_social_aid_donation_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('social_aid.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.social_aid_donations
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_social_aid_disbursement_reverse(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('social_aid.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.social_aid_disbursements
       SET status = 'reversed', reversed_at = p_at, reversed_by = p_by,
           reversal_reason = p_reason, updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- ------------------------------------------------------------------
-- periods (periods.manage) — draft -> open -> closed only.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_period_open(p_id uuid,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('periods.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.periods
       SET status = 'open', updated_at = now()
     WHERE id = p_id AND status = 'draft'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_period_close(
    p_id uuid, p_at timestamptz,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('periods.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.periods
       SET status = 'closed', updated_at = p_at
     WHERE id = p_id AND status = 'open'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- ------------------------------------------------------------------
-- shares / shareholders / parties. The generic share status command
-- can only produce active|suspended|voided — return_pending/closed are
-- reachable exclusively through the share-return definers above
-- (docs/16 rule, now enforced inside the database).
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_share_set_status(
    p_id uuid, p_status text, p_at timestamptz,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('shares.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_from  text;
BEGIN
    IF p_status NOT IN ('active', 'suspended', 'voided') THEN
        RAISE EXCEPTION 'check_violation: invalid share status %', p_status
            USING ERRCODE = 'check_violation';
    END IF;
    SELECT status INTO v_from FROM public.shares
     WHERE id = p_id AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    IF v_from IS NULL OR NOT (
           (v_from = 'active'    AND p_status IN ('suspended', 'voided'))
        OR (v_from = 'suspended' AND p_status IN ('active', 'voided'))) THEN
        RAISE EXCEPTION 'check_violation: invalid share transition % -> %',
            v_from, p_status USING ERRCODE = 'check_violation';
    END IF;
    UPDATE public.shares
       SET status = p_status, updated_at = p_at
     WHERE id = p_id AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- Ownership interval close: by interval id, or the open interval of a
-- share. Both the shares lifecycle and the share-return finalization
-- legitimately end ownerships — either permission suffices.
CREATE OR REPLACE FUNCTION public.app_share_ownership_end(
    p_ownership uuid, p_ended_at timestamptz,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope_any(
        'shares.manage', 'share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.share_ownerships
       SET ended_at = p_ended_at
     WHERE id = p_ownership
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_share_ownership_end_open_interval(
    p_share uuid, p_at_ts timestamptz,
    p_coop uuid)
RETURNS TABLE(ownership_id uuid, shareholder_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope_any(
        'shares.manage', 'share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    RETURN QUERY
    UPDATE public.share_ownerships o
       SET ended_at = p_at_ts
     WHERE o.share_id = p_share AND o.ended_at IS NULL
       AND o.cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END)
    RETURNING o.id, o.shareholder_id;
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_family_membership_end(
    p_shareholder uuid, p_family uuid, p_ended_at timestamptz, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('families.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.shareholder_family_memberships
       SET ended_at = p_ended_at, reason = COALESCE(p_reason, reason)
     WHERE shareholder_id = p_shareholder AND family_id = p_family
       AND ended_at IS NULL
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_shareholder_set_status(
    p_id uuid, p_status text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('shareholders.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_from  text;
BEGIN
    IF p_status NOT IN ('active', 'inactive', 'voided') THEN
        RAISE EXCEPTION 'check_violation: invalid shareholder status %', p_status
            USING ERRCODE = 'check_violation';
    END IF;
    SELECT status INTO v_from FROM public.shareholders
     WHERE id = p_id AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
    IF v_from IS NULL OR NOT (
           (v_from = 'active'   AND p_status IN ('inactive', 'voided'))
        OR (v_from = 'inactive' AND p_status IN ('active', 'voided'))) THEN
        RAISE EXCEPTION 'check_violation: invalid shareholder transition % -> %',
            v_from, p_status USING ERRCODE = 'check_violation';
    END IF;
    UPDATE public.shareholders
       SET status = p_status, updated_at = now()
     WHERE id = p_id AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- ------------------------------------------------------------------
-- governance (governance.manage)
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_governance_body_close(
    p_id uuid, p_by uuid,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('governance.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.governance_bodies
       SET status = 'closed', closed_at = now(), closed_by = p_by,
           updated_at = now()
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_governance_decision_open(
    p_id uuid, p_by uuid,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('governance.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.governance_decisions
       SET status = 'open', opened_at = now(), opened_by = p_by,
           updated_at = now()
     WHERE id = p_id AND status = 'draft'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_governance_decision_cancel(
    p_id uuid, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('governance.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.governance_decisions
       SET status = 'cancelled', cancelled_at = now(),
           cancelled_by = p_by, cancellation_reason = p_reason,
           updated_at = now()
     WHERE id = p_id AND status = 'draft'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_governance_decision_finalize(
    p_id       uuid,
    p_outcome  text,
    p_by       uuid,
    p_eligible int,
    p_approve  int,
    p_reject   int,
    p_abstain  int,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('governance.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    IF p_outcome NOT IN ('approved', 'rejected') THEN
        RAISE EXCEPTION 'check_violation: invalid decision outcome %', p_outcome
            USING ERRCODE = 'check_violation';
    END IF;
    UPDATE public.governance_decisions
       SET status = p_outcome, finalized_at = now(), finalized_by = p_by,
           eligible_count = p_eligible, approve_count = p_approve,
           reject_count = p_reject, abstain_count = p_abstain,
           updated_at = now()
     WHERE id = p_id AND status = 'open'
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_governance_membership_end(
    p_id uuid, p_ended_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('governance.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
BEGIN
    UPDATE public.governance_memberships
       SET ended_at = p_ended_at, ended_by = p_by, end_reason = p_reason,
           updated_at = now()
     WHERE id = p_id AND ended_at IS NULL
       AND cooperative_id IS NOT DISTINCT FROM
           (CASE WHEN v_restricted THEN v_scope ELSE p_coop END);
END;
$function$;

-- ------------------------------------------------------------------
-- EXECUTE surface: definers callable by the runtime role, helpers not.
-- ------------------------------------------------------------------
GRANT EXECUTE ON FUNCTION public.app_movements_reverse_of_source(text, uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_payment_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_payment_allocation_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_transfer_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_financial_account_set_status(uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_income_entry_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_expense_entry_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_financial_category_set_status(uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_credit_application_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_credit_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_credits_reverse_by_ids(uuid[], timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_cancel(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_funding_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_valuation_cancel(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_income_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_dispose(uuid, timestamptz, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_mark_return_pending(uuid, timestamptz, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_cancel(uuid, uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_finalize(uuid, uuid, timestamptz, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_entitlement_determine(uuid, numeric, date, text, text, timestamptz, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_entitlement_cancel(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_entitlement_refresh(uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_settlement_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_social_aid_fund_close(uuid, timestamptz, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_social_aid_fund_cancel(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_social_aid_donation_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_social_aid_disbursement_reverse(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_period_open(uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_period_close(uuid, timestamptz, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_set_status(uuid, text, timestamptz, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_ownership_end(uuid, timestamptz, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_ownership_end_open_interval(uuid, timestamptz, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_family_membership_end(uuid, uuid, timestamptz, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_shareholder_set_status(uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_governance_body_close(uuid, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_governance_decision_open(uuid, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_governance_decision_cancel(uuid, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_governance_decision_finalize(uuid, text, uuid, int, int, int, int, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_governance_membership_end(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;

-- ------------------------------------------------------------------
-- Column-grant narrowing: every lifecycle/terminal column leaves the
-- direct UPDATE surface. Surviving grants are draft/config fields
-- only (assessment_rules, financial_accounts name/desc/bank/iban,
-- financial_categories name/desc, governance_decisions draft fields,
-- periods draft config, persons, shareholders guardian/account).
-- ------------------------------------------------------------------
-- NB: column-level grants live in attacl — a bare REVOKE UPDATE ON
-- table does NOT remove them. Every granted column must be named.
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason)
    ON account_movements FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON account_transfers FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason)
    ON credit_applications FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON expense_entries FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON income_entries FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON investment_fundings FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON investment_incomes FROM kooperatif_app;
REVOKE UPDATE (status, cancelled_at, cancelled_by, cancellation_reason,
               updated_at)
    ON investment_valuations FROM kooperatif_app;
REVOKE UPDATE (status, cancelled_at, cancelled_by, cancellation_reason,
               disposed_at, disposed_by, updated_at)
    ON investments FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason)
    ON payment_allocations FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON payments FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON share_return_settlements FROM kooperatif_app;
REVOKE UPDATE (status, cancelled_at, cancelled_by, cancellation_reason,
               finalized_at, finalized_by, updated_at)
    ON share_returns FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON shareholder_credits FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON social_aid_disbursements FROM kooperatif_app;
REVOKE UPDATE (status, reversed_at, reversed_by, reversal_reason,
               updated_at)
    ON social_aid_donations FROM kooperatif_app;
REVOKE UPDATE (status, closed_at, closed_by, cancelled_at, cancelled_by,
               cancellation_reason, updated_at)
    ON social_aid_funds FROM kooperatif_app;
REVOKE UPDATE (status, closed_at, closed_by, updated_at)
    ON governance_bodies FROM kooperatif_app;
REVOKE UPDATE (ended_at, ended_by, end_reason, updated_at)
    ON governance_memberships FROM kooperatif_app;
REVOKE UPDATE (ended_at) ON share_ownerships FROM kooperatif_app;
REVOKE UPDATE (amount, due_date, policy_reference, description,
               determined_at, status, cancelled_at, cancelled_by,
               cancellation_reason, updated_at)
    ON share_return_entitlements FROM kooperatif_app;
REVOKE UPDATE (ended_at, reason)
    ON shareholder_family_memberships FROM kooperatif_app;
REVOKE UPDATE (status, updated_at) ON shares FROM kooperatif_app;
-- shares.updated_at alone is a non-sensitive touch column — the
-- ownership-change path bumps it after closing an interval via the
-- ownership definers; status transitions stay definer-only.
GRANT UPDATE (updated_at) ON shares TO kooperatif_app;

REVOKE UPDATE (status) ON financial_accounts FROM kooperatif_app;
REVOKE UPDATE (status) ON financial_categories FROM kooperatif_app;
REVOKE UPDATE (status) ON periods FROM kooperatif_app;
REVOKE UPDATE (status) ON shareholders FROM kooperatif_app;
REVOKE UPDATE (status, opened_at, opened_by, cancelled_at, cancelled_by,
               cancellation_reason, finalized_at, finalized_by,
               eligible_count, approve_count, reject_count,
               abstain_count)
    ON governance_decisions FROM kooperatif_app;
