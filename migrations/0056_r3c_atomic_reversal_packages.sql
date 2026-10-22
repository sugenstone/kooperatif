-- ============================================================
-- M1-P4-R3C — atomic financial reversal packages (closure-audit
-- F4 residual of R3B)
--
-- 0055 moved lifecycle transitions behind permission-checked
-- definers, but each definer performed only the STATUS leg of a
-- multi-effect operation. The compensating pieces (movement
-- reversal, balance/reservation checks, credit reconcile, proceeds
-- legs, entitlement crystallization) still lived in repository
-- code. Verified on kooperatif_test v55 with a sealed
-- `kooperatif_app` connection: a member holding payments.manage
-- called `app_payment_reverse` alone — the Payment flipped to
-- `reversed` while its account_movements row stayed `active`,
-- leaving the derived account balance unchanged (the reversed
-- inflow still counted). The same partial-package bypass existed
-- for every movement-bearing reversal.
--
-- This migration adds *_tx package definers that perform the
-- COMPLETE operation inside the database — lock order, domain
-- preconditions, balance/reservation checks, compensating
-- movement reversal, credit reconcile and status flips are one
-- atomic function call — and revokes runtime EXECUTE on the
-- component definers that could create an incomplete reversal.
-- The component functions themselves are kept (they are invoked
-- by the packages / owner lane); they are simply no longer
-- callable by kooperatif_app.
--
-- Domain errors surface as SQLSTATE P0001 with a stable
-- `kooperatif:<token>` message; missing rows use P0002 with
-- `kooperatif:not_found`. Repositories map tokens to their
-- existing typed error variants — API semantics unchanged.
--
-- Lane semantics are identical to 0055: `v_restricted` is true
-- for the app role and for any sealed context — rows must then
-- match the VERIFIED stamped scope; an unsealed owner connection
-- supplies p_coop itself.
-- ============================================================

-- ------------------------------------------------------------------
-- payments (payments.manage): payment + allocations + compensating
-- movement reversal + shareholder-credit reconcile + balance and
-- Social-Aid reservation checks — one atomic call.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_payment_reverse_tx(
    p_payment uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, reversed_allocations bigint,
              payment_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('payments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff       uuid;
    v_status    text;
    v_number    bigint;
    v_account   uuid;
    v_amount    numeric;
    v_acct_coop uuid;
    v_balance   numeric;
    v_reserved  numeric;
    v_credit_ids uuid[];
    v_n         bigint;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT p.status, p.payment_number, p.destination_account_id, p.amount
      INTO v_status, v_number, v_account, v_amount
      FROM public.payments p
     WHERE p.id = p_payment
       AND p.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        -- Idempotent replay (docs/19): already reversed/cancelled.
        RETURN QUERY SELECT true, 0::bigint, v_number;
        RETURN;
    END IF;

    IF v_account IS NOT NULL THEN
        SELECT a.cooperative_id INTO v_acct_coop
          FROM public.financial_accounts a
         WHERE a.id = v_account FOR UPDATE;
        IF NOT FOUND OR v_acct_coop IS DISTINCT FROM v_eff THEN
            RAISE EXCEPTION 'kooperatif:account_not_found_or_inactive'
                USING ERRCODE = 'P0001';
        END IF;
        SELECT COALESCE(sum(CASE m.direction WHEN 'inflow'
                            THEN m.amount ELSE -m.amount END), 0)
          INTO v_balance
          FROM public.account_movements m
         WHERE m.account_id = v_account AND m.status = 'active'
           AND m.cooperative_id IS NOT DISTINCT FROM v_eff;
        IF v_balance - v_amount < 0 THEN
            RAISE EXCEPTION 'kooperatif:account_effect_blocked'
                USING ERRCODE = 'P0001';
        END IF;
        -- Removing the inflow must not leave the Social Aid
        -- reservation physically unbacked (PILOT-FIX-001).
        SELECT COALESCE((SELECT sum(d.amount)
                           FROM public.social_aid_donations d
                          WHERE d.financial_account_id = v_account
                            AND d.status = 'posted'
                            AND d.cooperative_id IS NOT DISTINCT FROM v_eff), 0)
             - COALESCE((SELECT sum(s.amount)
                           FROM public.social_aid_disbursements s
                          WHERE s.financial_account_id = v_account
                            AND s.status = 'posted'
                            AND s.cooperative_id IS NOT DISTINCT FROM v_eff), 0)
          INTO v_reserved;
        IF v_balance - v_amount < v_reserved THEN
            RAISE EXCEPTION 'kooperatif:account_effect_blocked'
                USING ERRCODE = 'P0001';
        END IF;
        UPDATE public.account_movements
           SET status = 'reversed', reversed_at = p_at,
               reversed_by = p_by, reversal_reason = p_reason
         WHERE source_type = 'payment' AND source_id = p_payment
           AND status = 'active'
           AND cooperative_id IS NOT DISTINCT FROM v_eff;
    END IF;

    -- Credit effect: the Payment's active Shareholder Credits are
    -- reversed unless one is already CONSUMED by an active
    -- application — then the whole package is rejected.
    SELECT array_agg(c.id) INTO v_credit_ids
      FROM (SELECT id FROM public.shareholder_credits
             WHERE source_payment_id = p_payment AND status = 'active'
               AND cooperative_id IS NOT DISTINCT FROM v_eff
             ORDER BY credit_number FOR UPDATE) c;
    IF v_credit_ids IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM public.credit_applications a
                    WHERE a.credit_id = ANY(v_credit_ids)
                      AND a.status = 'active'
                      AND a.cooperative_id IS NOT DISTINCT FROM v_eff) THEN
            RAISE EXCEPTION 'kooperatif:credit_effect_blocked'
                USING ERRCODE = 'P0001';
        END IF;
        UPDATE public.shareholder_credits
           SET status = 'reversed', reversed_at = p_at,
               reversed_by = p_by, reversal_reason = p_reason,
               updated_at = p_at
         WHERE id = ANY(v_credit_ids) AND status = 'active'
           AND cooperative_id IS NOT DISTINCT FROM v_eff;
    END IF;

    UPDATE public.payment_allocations
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE payment_id = p_payment AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    GET DIAGNOSTICS v_n = ROW_COUNT;
    UPDATE public.payments
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_payment AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_n, v_number;
END;
$function$;

-- ------------------------------------------------------------------
-- account transfers (financial_accounts.manage): transfer + both
-- movement legs + destination balance/reservation checks.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_transfer_reverse_tx(
    p_transfer uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, transfer_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('financial_accounts.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_status   text;
    v_number   bigint;
    v_src      uuid;
    v_dst      uuid;
    v_amount   numeric;
    v_balance  numeric;
    v_reserved numeric;
    v_lock     uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT t.status, t.transfer_number, t.source_account_id,
           t.destination_account_id, t.amount
      INTO v_status, v_number, v_src, v_dst, v_amount
      FROM public.account_transfers t
     WHERE t.id = p_transfer
       AND t.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;

    -- Lock both accounts in canonical ascending-id order.
    IF v_src < v_dst THEN
        SELECT a.id INTO v_lock FROM public.financial_accounts a
         WHERE a.id = v_src FOR UPDATE;
        SELECT a.id INTO v_lock FROM public.financial_accounts a
         WHERE a.id = v_dst FOR UPDATE;
    ELSE
        SELECT a.id INTO v_lock FROM public.financial_accounts a
         WHERE a.id = v_dst FOR UPDATE;
        SELECT a.id INTO v_lock FROM public.financial_accounts a
         WHERE a.id = v_src FOR UPDATE;
    END IF;

    -- The reversal removes the DESTINATION inflow: if the value
    -- already left, reversing would fabricate a negative balance.
    SELECT COALESCE(sum(CASE m.direction WHEN 'inflow'
                        THEN m.amount ELSE -m.amount END), 0)
      INTO v_balance
      FROM public.account_movements m
     WHERE m.account_id = v_dst AND m.status = 'active'
       AND m.cooperative_id IS NOT DISTINCT FROM v_eff;
    IF v_balance - v_amount < 0 THEN
        RAISE EXCEPTION 'kooperatif:insufficient_funds'
            USING ERRCODE = 'P0001';
    END IF;
    SELECT COALESCE((SELECT sum(d.amount)
                       FROM public.social_aid_donations d
                      WHERE d.financial_account_id = v_dst
                        AND d.status = 'posted'
                        AND d.cooperative_id IS NOT DISTINCT FROM v_eff), 0)
         - COALESCE((SELECT sum(s.amount)
                       FROM public.social_aid_disbursements s
                      WHERE s.financial_account_id = v_dst
                        AND s.status = 'posted'
                        AND s.cooperative_id IS NOT DISTINCT FROM v_eff), 0)
      INTO v_reserved;
    IF v_balance - v_amount < v_reserved THEN
        RAISE EXCEPTION 'kooperatif:insufficient_unrestricted_funds'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE source_type = 'transfer' AND source_id = p_transfer
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    UPDATE public.account_transfers
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_transfer AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

-- ------------------------------------------------------------------
-- income / expense entries (income_expense.manage): entry + movement
-- + balance/reservation checks (income reversal removes an inflow).
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_income_entry_reverse_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, entry_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('income_expense.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_status   text;
    v_number   bigint;
    v_account  uuid;
    v_amount   numeric;
    v_acct_coop uuid;
    v_balance  numeric;
    v_reserved numeric;
    v_lock     uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT e.status, e.income_number, e.financial_account_id, e.amount
      INTO v_status, v_number, v_account, v_amount
      FROM public.income_entries e
     WHERE e.id = p_id
       AND e.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;

    SELECT a.cooperative_id INTO v_acct_coop
      FROM public.financial_accounts a
     WHERE a.id = v_account FOR UPDATE;
    IF v_acct_coop IS DISTINCT FROM v_eff THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;

    -- Removing an inflow: the post-reversal balance must stay
    -- non-negative AND still cover the Social Aid reservation.
    SELECT COALESCE(sum(CASE m.direction WHEN 'inflow'
                        THEN m.amount ELSE -m.amount END), 0)
      INTO v_balance
      FROM public.account_movements m
     WHERE m.account_id = v_account AND m.status = 'active'
       AND m.cooperative_id IS NOT DISTINCT FROM v_acct_coop;
    IF v_balance - v_amount < 0 THEN
        RAISE EXCEPTION 'kooperatif:insufficient_funds'
            USING ERRCODE = 'P0001';
    END IF;
    SELECT COALESCE((SELECT sum(d.amount)
                       FROM public.social_aid_donations d
                      WHERE d.financial_account_id = v_account
                        AND d.status = 'posted'
                        AND d.cooperative_id IS NOT DISTINCT FROM v_eff), 0)
         - COALESCE((SELECT sum(s.amount)
                       FROM public.social_aid_disbursements s
                      WHERE s.financial_account_id = v_account
                        AND s.status = 'posted'
                        AND s.cooperative_id IS NOT DISTINCT FROM v_eff), 0)
      INTO v_reserved;
    IF v_balance - v_amount < v_reserved THEN
        RAISE EXCEPTION 'kooperatif:insufficient_unrestricted_funds'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE source_type = 'income' AND source_id = p_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_acct_coop;
    UPDATE public.income_entries
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_expense_entry_reverse_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, entry_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('income_expense.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_status   text;
    v_number   bigint;
    v_account  uuid;
    v_acct_coop uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT e.status, e.expense_number, e.financial_account_id
      INTO v_status, v_number, v_account
      FROM public.expense_entries e
     WHERE e.id = p_id
       AND e.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;

    SELECT a.cooperative_id INTO v_acct_coop
      FROM public.financial_accounts a
     WHERE a.id = v_account FOR UPDATE;
    IF v_acct_coop IS DISTINCT FROM v_eff THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;

    -- Reversal restores the outflow — the balance only grows, so no
    -- funds check is needed (same semantics as the repository).
    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE source_type = 'expense' AND source_id = p_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_acct_coop;
    UPDATE public.expense_entries
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

-- ------------------------------------------------------------------
-- shareholder credits (credits.manage): the consumed-application
-- check moves INSIDE the boundary — a credit funding an active
-- application can no longer be reversed around the check.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_credit_reverse_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('credits.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff    uuid;
    v_status text;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT c.status INTO v_status
      FROM public.shareholder_credits c
     WHERE c.id = p_id
       AND c.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'active' THEN
        RETURN true; -- idempotent replay
    END IF;
    IF EXISTS (SELECT 1 FROM public.credit_applications a
                WHERE a.credit_id = p_id AND a.status = 'active'
                  AND a.cooperative_id IS NOT DISTINCT FROM v_eff) THEN
        RAISE EXCEPTION 'kooperatif:consumed' USING ERRCODE = 'P0001';
    END IF;
    UPDATE public.shareholder_credits
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN false;
END;
$function$;

-- ------------------------------------------------------------------
-- investments (investments.manage)
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_investment_funding_reverse_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, funding_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_status   text;
    v_number   bigint;
    v_invest   uuid;
    v_account  uuid;
    v_istatus  text;
    v_acct_coop uuid;
    v_mcoop    uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT f.status, f.funding_number, f.investment_id,
           f.financial_account_id
      INTO v_status, v_number, v_invest, v_account
      FROM public.investment_fundings f
     WHERE f.id = p_id
       AND f.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;

    SELECT i.status INTO v_istatus
      FROM public.investments i
     WHERE i.id = v_invest
       AND i.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_istatus = 'disposed' THEN
        RAISE EXCEPTION 'kooperatif:invalid_state' USING ERRCODE = 'P0001';
    END IF;

    SELECT a.cooperative_id INTO v_acct_coop
      FROM public.financial_accounts a
     WHERE a.id = v_account FOR UPDATE;
    v_mcoop := CASE WHEN v_restricted THEN v_eff ELSE v_acct_coop END;

    -- Funding reversal removes an outflow — the balance only grows.
    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE source_type = 'investment_funding' AND source_id = p_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_mcoop;
    UPDATE public.investment_fundings
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_investment_income_reverse_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, income_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_status   text;
    v_number   bigint;
    v_account  uuid;
    v_amount   numeric;
    v_acct_coop uuid;
    v_mcoop    uuid;
    v_balance  numeric;
    v_reserved numeric;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT n.status, n.income_number, n.financial_account_id, n.amount
      INTO v_status, v_number, v_account, v_amount
      FROM public.investment_incomes n
     WHERE n.id = p_id
       AND n.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;

    SELECT a.cooperative_id INTO v_acct_coop
      FROM public.financial_accounts a
     WHERE a.id = v_account FOR UPDATE;
    v_mcoop := CASE WHEN v_restricted THEN v_eff ELSE v_acct_coop END;

    -- Removing an inflow: balance must stay non-negative and cover
    -- the Social Aid reservation.
    SELECT COALESCE(sum(CASE m.direction WHEN 'inflow'
                        THEN m.amount ELSE -m.amount END), 0)
      INTO v_balance
      FROM public.account_movements m
     WHERE m.account_id = v_account AND m.status = 'active'
       AND m.cooperative_id IS NOT DISTINCT FROM v_mcoop;
    IF v_balance - v_amount < 0 THEN
        RAISE EXCEPTION 'kooperatif:insufficient_funds'
            USING ERRCODE = 'P0001';
    END IF;
    SELECT COALESCE((SELECT sum(d.amount)
                       FROM public.social_aid_donations d
                      WHERE d.financial_account_id = v_account
                        AND d.status = 'posted'
                        AND d.cooperative_id IS NOT DISTINCT FROM v_mcoop), 0)
         - COALESCE((SELECT sum(s.amount)
                       FROM public.social_aid_disbursements s
                      WHERE s.financial_account_id = v_account
                        AND s.status = 'posted'
                        AND s.cooperative_id IS NOT DISTINCT FROM v_mcoop), 0)
      INTO v_reserved;
    IF v_balance - v_amount < v_reserved THEN
        RAISE EXCEPTION 'kooperatif:insufficient_unrestricted_funds'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE source_type = 'investment_income' AND source_id = p_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_mcoop;
    UPDATE public.investment_incomes
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

-- Disposal package: derecognition event + one inflow movement per
-- proceeds leg + investment status flip, all-or-nothing.
-- `p_proceeds` is a jsonb array:
--   [{account_id, amount, occurred_at, reference}]
CREATE OR REPLACE FUNCTION public.app_investment_dispose_tx(
    p_investment    uuid,
    p_disposed_at   date,
    p_consideration numeric,
    p_counterparty  text,
    p_reference     text,
    p_note          text,
    p_proceeds      jsonb,
    p_idem_key      text,
    p_fingerprint   text,
    p_by            uuid,
    p_at            timestamptz,
    p_coop          uuid)
RETURNS TABLE(event_id uuid, event_number bigint, replayed boolean)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff     uuid;
    v_exist   record;
    v_istatus text;
    v_leg     record;
    v_acct    record;
    v_disposal uuid;
    v_number  bigint;
    v_leg_id  uuid;
    v_mov_id  uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    -- Idempotent replay by caller key.
    SELECT d.id, d.disposal_number, d.idempotency_fingerprint
      INTO v_exist
      FROM public.investment_disposals d
     WHERE d.idempotency_key = p_idem_key
       AND d.cooperative_id IS NOT DISTINCT FROM v_eff;
    IF FOUND THEN
        IF v_exist.idempotency_fingerprint <> p_fingerprint THEN
            RAISE EXCEPTION 'kooperatif:idempotency_conflict'
                USING ERRCODE = 'P0001';
        END IF;
        RETURN QUERY SELECT v_exist.id, v_exist.disposal_number, true;
        RETURN;
    END IF;

    SELECT i.status INTO v_istatus
      FROM public.investments i
     WHERE i.id = p_investment
       AND i.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_istatus <> 'active' THEN
        RAISE EXCEPTION 'kooperatif:invalid_state' USING ERRCODE = 'P0001';
    END IF;

    -- Lock every proceeds account in canonical ascending-id order,
    -- then validate existence, scope and active status per leg.
    FOR v_acct IN
        SELECT a.id, a.status, a.cooperative_id
          FROM public.financial_accounts a
         WHERE a.id = ANY (ARRAY(
                   SELECT (l->>'account_id')::uuid
                     FROM jsonb_array_elements(p_proceeds) l))
         ORDER BY a.id FOR UPDATE
    LOOP
        NULL; -- locking pass only
    END LOOP;

    FOR v_leg IN
        SELECT (l->>'account_id')::uuid        AS account_id,
               (l->>'amount')::numeric         AS amount,
               (l->>'occurred_at')::timestamptz AS occurred_at,
               l->>'reference'                 AS reference
          FROM jsonb_array_elements(p_proceeds) l
    LOOP
        SELECT a.status, a.cooperative_id INTO v_acct
          FROM public.financial_accounts a WHERE a.id = v_leg.account_id;
        IF NOT FOUND OR v_acct.cooperative_id IS DISTINCT FROM v_eff THEN
            RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
        END IF;
        IF v_acct.status <> 'active' THEN
            RAISE EXCEPTION 'kooperatif:inactive_account'
                USING ERRCODE = 'P0001';
        END IF;
    END LOOP;

    INSERT INTO public.investment_disposals
        (investment_id, disposed_at, consideration_amount, currency,
         counterparty_name, reference, note, status,
         idempotency_key, idempotency_fingerprint, created_by,
         cooperative_id, disposal_number)
    VALUES (p_investment, p_disposed_at, p_consideration, 'TRY',
            p_counterparty, p_reference, p_note, 'posted',
            p_idem_key, p_fingerprint, p_by, v_eff,
            public.allocate_document_number(
                v_eff, 'investment_disposals.disposal_number'))
    ON CONFLICT (cooperative_id, idempotency_key)
        WHERE idempotency_key IS NOT NULL DO NOTHING
    RETURNING id, disposal_number INTO v_disposal, v_number;

    IF v_disposal IS NULL THEN
        -- Lost the insert race: answer the committed winner.
        SELECT d.id, d.disposal_number, d.idempotency_fingerprint
          INTO v_exist
          FROM public.investment_disposals d
         WHERE d.idempotency_key = p_idem_key
           AND d.cooperative_id IS NOT DISTINCT FROM v_eff;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
        END IF;
        IF v_exist.idempotency_fingerprint <> p_fingerprint THEN
            RAISE EXCEPTION 'kooperatif:idempotency_conflict'
                USING ERRCODE = 'P0001';
        END IF;
        RETURN QUERY SELECT v_exist.id, v_exist.disposal_number, true;
        RETURN;
    END IF;

    FOR v_leg IN
        SELECT (l->>'account_id')::uuid        AS account_id,
               (l->>'amount')::numeric         AS amount,
               (l->>'occurred_at')::timestamptz AS occurred_at,
               l->>'reference'                 AS reference
          FROM jsonb_array_elements(p_proceeds) l
    LOOP
        v_leg_id := gen_random_uuid();
        SELECT a.cooperative_id INTO v_acct
          FROM public.financial_accounts a WHERE a.id = v_leg.account_id;
        INSERT INTO public.account_movements
            (account_id, direction, amount, source_type, source_id,
             occurred_at, created_by, cooperative_id)
        VALUES (v_leg.account_id, 'inflow', v_leg.amount,
                'investment_disposal', v_leg_id, v_leg.occurred_at,
                p_by, v_acct.cooperative_id)
        RETURNING id INTO v_mov_id;
        INSERT INTO public.investment_disposal_proceeds
            (id, disposal_id, financial_account_id, amount, currency,
             occurred_at, reference, account_movement_id,
             cooperative_id)
        VALUES (v_leg_id, v_disposal, v_leg.account_id, v_leg.amount,
                'TRY', v_leg.occurred_at, v_leg.reference, v_mov_id,
                v_eff);
    END LOOP;

    UPDATE public.investments
       SET status = 'disposed', disposed_at = p_at,
           disposed_by = p_by, updated_at = p_at
     WHERE id = p_investment AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;

    RETURN QUERY SELECT v_disposal, v_number, false;
END;
$function$;

-- ------------------------------------------------------------------
-- share returns (share_returns.manage)
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_share_return_settlement_reverse_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, settlement_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_ent      uuid;
    v_account  uuid;
    v_acct_coop uuid;
    v_mcoop    uuid;
    v_status   text;
    v_number   bigint;
    v_lock     uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT s.entitlement_id, s.financial_account_id
      INTO v_ent, v_account
      FROM public.share_return_settlements s
     WHERE s.id = p_id
       AND s.cooperative_id IS NOT DISTINCT FROM v_eff;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;

    -- Canonical lock order: account -> entitlement -> settlement.
    SELECT a.cooperative_id INTO v_acct_coop
      FROM public.financial_accounts a
     WHERE a.id = v_account FOR UPDATE;
    v_mcoop := CASE WHEN v_restricted THEN v_eff ELSE v_acct_coop END;
    SELECT e.id INTO v_lock FROM public.share_return_entitlements e
     WHERE e.id = v_ent
       AND e.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    SELECT s.status, s.settlement_number INTO v_status, v_number
      FROM public.share_return_settlements s
     WHERE s.id = p_id
       AND s.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;

    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE source_type = 'share_return_settlement' AND source_id = p_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_mcoop;
    UPDATE public.share_return_settlements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;

    -- Derived entitlement status recompute (open|partially_settled|
    -- settled) — same recompute as app_share_return_entitlement_refresh.
    UPDATE public.share_return_entitlements e
       SET status = CASE
            WHEN e.amount IS NOT NULL AND settled.s >= e.amount THEN 'settled'
            WHEN settled.s > 0 THEN 'partially_settled'
            ELSE 'open' END,
           updated_at = now()
      FROM (SELECT COALESCE(sum(amount), 0) AS s
              FROM public.share_return_settlements
             WHERE entitlement_id = v_ent AND status = 'posted') settled
     WHERE e.id = v_ent AND e.status <> 'cancelled'
       AND e.cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

-- Finalization package (pending -> finalized): ownership interval
-- close + share event + return/share flips + entitlement
-- crystallization — the complete operation, not just the status leg.
-- `p_entitlements` is a jsonb array:
--   [{type, amount, due_date, policy_reference, description}]
CREATE OR REPLACE FUNCTION public.app_share_return_finalize_tx(
    p_return          uuid,
    p_expected_updated_at timestamptz,
    p_effective_date  date,
    p_entitlements    jsonb,
    p_at              timestamptz,
    p_by              uuid,
    p_coop            uuid)
RETURNS TABLE(return_number bigint, entitlement_ids uuid[])
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_share    uuid;
    v_share_status text;
    v_status   text;
    v_number   bigint;
    v_owner    uuid;
    v_updated  timestamptz;
    v_own_id   uuid;
    v_spec     record;
    v_ids      uuid[] := '{}';
    v_new_id   uuid;
    v_dupes    int;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT r.share_id INTO v_share
      FROM public.share_returns r
     WHERE r.id = p_return
       AND r.cooperative_id IS NOT DISTINCT FROM v_eff;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;

    -- Lock order: share -> return case.
    SELECT s.status INTO v_share_status
      FROM public.shares s
     WHERE s.id = v_share
       AND s.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    SELECT r.status, r.return_number, r.shareholder_id, r.updated_at
      INTO v_status, v_number, v_owner, v_updated
      FROM public.share_returns r
     WHERE r.id = p_return
       AND r.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_updated <> p_expected_updated_at THEN
        RAISE EXCEPTION 'kooperatif:stale_state' USING ERRCODE = 'P0001';
    END IF;
    IF v_status <> 'pending' OR v_share_status <> 'return_pending' THEN
        RAISE EXCEPTION 'kooperatif:invalid_state' USING ERRCODE = 'P0001';
    END IF;

    -- One right per type per case.
    SELECT count(*) - count(DISTINCT t.etype) INTO v_dupes
      FROM (SELECT e->>'type' AS etype
              FROM jsonb_array_elements(p_entitlements) e) t;
    IF v_dupes > 0 THEN
        RAISE EXCEPTION 'kooperatif:entitlement_exists'
            USING ERRCODE = 'P0001';
    END IF;

    -- Close the ownership interval at the economic cutoff:
    -- ended_at = effective_return_date 00:00 Europe/Istanbul.
    SELECT o.ownership_id INTO v_own_id
      FROM public.app_share_ownership_end_open_interval(
               v_share,
               (p_effective_date::timestamp AT TIME ZONE 'Europe/Istanbul'),
               v_eff) o;
    IF v_own_id IS NULL THEN
        RAISE EXCEPTION 'kooperatif:share_not_eligible'
            USING ERRCODE = 'P0001';
    END IF;

    INSERT INTO public.share_events
        (share_id, event_type, occurred_at, from_shareholder_id,
         status_from, status_to, reason, actor_user_id, cooperative_id)
    VALUES (v_share, 'return_finalized', p_at, v_owner,
            'return_pending', 'closed', NULL, p_by, v_eff);

    UPDATE public.share_returns
       SET status = 'finalized', finalized_at = p_at,
           finalized_by = p_by, updated_at = p_at
     WHERE id = p_return AND status = 'pending'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    UPDATE public.shares
       SET status = 'closed', updated_at = p_at
     WHERE id = v_share AND status = 'return_pending'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;

    -- Crystallize each right — snapshot amount/due/policy evidence.
    FOR v_spec IN
        SELECT e->>'type'               AS etype,
               (e->>'amount')::numeric   AS amount,
               (e->>'due_date')::date    AS due_date,
               e->>'policy_reference'    AS policy_reference,
               e->>'description'         AS description
          FROM jsonb_array_elements(p_entitlements) e
    LOOP
        INSERT INTO public.share_return_entitlements
            (share_return_id, entitlement_type,
             beneficiary_shareholder_id, amount, due_date,
             policy_reference, description, recognized_at,
             determined_at, status, created_by, cooperative_id,
             entitlement_number)
        VALUES (p_return, v_spec.etype, v_owner, v_spec.amount,
                v_spec.due_date, v_spec.policy_reference,
                v_spec.description, p_at,
                CASE WHEN v_spec.amount IS NULL THEN NULL ELSE p_at END,
                'open', p_by, v_eff,
                public.allocate_document_number(
                    v_eff, 'share_return_entitlements.entitlement_number'))
        RETURNING id INTO v_new_id;
        v_ids := v_ids || v_new_id;
    END LOOP;

    RETURN QUERY SELECT v_number, v_ids;
END;
$function$;

-- ------------------------------------------------------------------
-- social aid (social_aid.manage): donation/disbursement reversal +
-- fund lock + restricted/physical availability checks + movement.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_social_aid_donation_reverse_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, donation_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('social_aid.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_status   text;
    v_number   bigint;
    v_fund     uuid;
    v_account  uuid;
    v_amount   numeric;
    v_fstatus  text;
    v_acct_coop uuid;
    v_mcoop    uuid;
    v_restrict numeric;
    v_balance  numeric;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT d.status, d.donation_number, d.fund_id,
           d.financial_account_id, d.amount
      INTO v_status, v_number, v_fund, v_account, v_amount
      FROM public.social_aid_donations d
     WHERE d.id = p_id
       AND d.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;

    -- Canonical lock prefix: fund row first, then the account row.
    SELECT f.status INTO v_fstatus
      FROM public.social_aid_funds f
     WHERE f.id = v_fund
       AND f.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    SELECT a.cooperative_id INTO v_acct_coop
      FROM public.financial_accounts a
     WHERE a.id = v_account FOR UPDATE;
    v_mcoop := CASE WHEN v_restricted THEN v_eff ELSE v_acct_coop END;

    -- Restricted availability of THIS fund in THIS account must still
    -- cover the donation: consumed restricted money cannot be
    -- invalidated. Deliberately not cooperative-filtered — mirrors the
    -- repository reservation formula (the account id already confines
    -- the ledger to this cooperative).
    SELECT COALESCE((SELECT sum(d.amount)
                       FROM public.social_aid_donations d
                      WHERE d.fund_id = v_fund
                        AND d.financial_account_id = v_account
                        AND d.status = 'posted'), 0)
         - COALESCE((SELECT sum(s.amount)
                       FROM public.social_aid_disbursements s
                      WHERE s.fund_id = v_fund
                        AND s.financial_account_id = v_account
                        AND s.status = 'posted'), 0)
      INTO v_restrict;
    IF v_restrict < v_amount THEN
        RAISE EXCEPTION 'kooperatif:insufficient_restricted'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT COALESCE(sum(CASE m.direction WHEN 'inflow'
                        THEN m.amount ELSE -m.amount END), 0)
      INTO v_balance
      FROM public.account_movements m
     WHERE m.account_id = v_account AND m.status = 'active'
       AND m.cooperative_id IS NOT DISTINCT FROM v_mcoop;
    IF v_balance < v_amount THEN
        RAISE EXCEPTION 'kooperatif:insufficient_funds'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE source_type = 'social_aid_donation' AND source_id = p_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_mcoop;
    UPDATE public.social_aid_donations
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_social_aid_disbursement_reverse_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS TABLE(replayed boolean, disbursement_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('social_aid.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_status   text;
    v_number   bigint;
    v_fund     uuid;
    v_account  uuid;
    v_fstatus  text;
    v_acct_coop uuid;
    v_mcoop    uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT s.status, s.disbursement_number, s.fund_id,
           s.financial_account_id
      INTO v_status, v_number, v_fund, v_account
      FROM public.social_aid_disbursements s
     WHERE s.id = p_id
       AND s.cooperative_id IS NOT DISTINCT FROM v_eff
     FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'posted' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;

    SELECT f.status INTO v_fstatus
      FROM public.social_aid_funds f
     WHERE f.id = v_fund
       AND f.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    SELECT a.cooperative_id INTO v_acct_coop
      FROM public.financial_accounts a
     WHERE a.id = v_account FOR UPDATE;
    v_mcoop := CASE WHEN v_restricted THEN v_eff ELSE v_acct_coop END;

    -- Disbursement reversal restores restricted money — always safe.
    UPDATE public.account_movements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason
     WHERE source_type = 'social_aid_disbursement' AND source_id = p_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_mcoop;
    UPDATE public.social_aid_disbursements
       SET status = 'reversed', reversed_at = p_at,
           reversed_by = p_by, reversal_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'posted'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

-- ------------------------------------------------------------------
-- governance (governance.manage): finalization evidence (electorate
-- size + vote tally) is computed INSIDE the boundary — a caller can
-- no longer supply a fabricated snapshot. The previous signature,
-- which trusted caller-computed tallies, is dropped.
-- ------------------------------------------------------------------
DROP FUNCTION IF EXISTS public.app_governance_decision_finalize(
    uuid, text, uuid, int, int, int, int, uuid);

CREATE OR REPLACE FUNCTION public.app_governance_decision_finalize_tx(
    p_id uuid, p_outcome text, p_by uuid,
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
    v_eff     uuid;
    v_status  text;
    v_body    uuid;
    v_eligible int;
    v_approve int;
    v_reject  int;
    v_abstain int;
BEGIN
    IF p_outcome NOT IN ('approved', 'rejected') THEN
        RAISE EXCEPTION 'kooperatif:invalid_state'
            USING ERRCODE = 'P0001';
    END IF;
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    -- Verified actor: in the sealed lane the caller can never
    -- name a different actor — attribution comes from the seal.
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT d.status, d.body_id INTO v_status, v_body
      FROM public.governance_decisions d
     WHERE d.id = p_id
       AND d.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'open' THEN
        RAISE EXCEPTION 'kooperatif:invalid_state' USING ERRCODE = 'P0001';
    END IF;

    SELECT count(*) INTO v_eligible
      FROM public.governance_memberships m
     WHERE m.body_id = v_body AND m.ended_at IS NULL
       AND m.cooperative_id IS NOT DISTINCT FROM v_eff;
    SELECT count(*) FILTER (WHERE v.choice = 'approve'),
           count(*) FILTER (WHERE v.choice = 'reject'),
           count(*) FILTER (WHERE v.choice = 'abstain')
      INTO v_approve, v_reject, v_abstain
      FROM public.governance_votes v
     WHERE v.decision_id = p_id
       AND v.cooperative_id IS NOT DISTINCT FROM v_eff;

    UPDATE public.governance_decisions
       SET status = p_outcome, finalized_at = now(),
           finalized_by = p_by,
           eligible_count = v_eligible, approve_count = v_approve,
           reject_count = v_reject, abstain_count = v_abstain,
           updated_at = now()
     WHERE id = p_id AND status = 'open'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
END;
$function$;

-- ------------------------------------------------------------------
-- shares (shares.manage): ownership transfer is the complete package —
-- share/ownership/target locks, effective-date and active-owner rules,
-- interval close, the share event and the successor interval. A caller
-- can no longer close an interval without its successor or write the
-- transfer without the event.
CREATE OR REPLACE FUNCTION public.app_share_transfer_tx(
    p_share uuid, p_to uuid, p_kind text, p_amount numeric,
    p_reason text, p_effective_at timestamptz,
    p_expected_updated_at timestamptz, p_at timestamptz, p_by uuid,
    p_coop uuid)
RETURNS TABLE(from_shareholder uuid, share_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('shares.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff      uuid;
    v_status   text;
    v_updated  timestamptz;
    v_number   bigint;
    v_own      uuid;
    v_from     uuid;
    v_started  timestamptz;
    v_target   text;
    v_event    uuid;
BEGIN
    IF p_kind NOT IN ('transfer', 'sale') THEN
        RAISE EXCEPTION 'kooperatif:invalid_state'
            USING ERRCODE = 'P0001';
    END IF;
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;
    IF p_effective_at > p_at THEN
        RAISE EXCEPTION 'kooperatif:future_effective_at'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT s.status, s.updated_at, s.share_number
      INTO v_status, v_updated, v_number
      FROM public.shares s
     WHERE s.id = p_share
       AND s.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_updated <> p_expected_updated_at THEN
        RAISE EXCEPTION 'kooperatif:stale_state' USING ERRCODE = 'P0001';
    END IF;
    IF v_status <> 'active' THEN
        RAISE EXCEPTION 'kooperatif:share_not_active'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT o.id, o.shareholder_id, o.started_at
      INTO v_own, v_from, v_started
      FROM public.share_ownerships o
     WHERE o.share_id = p_share AND o.ended_at IS NULL
       AND o.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:no_current_owner'
            USING ERRCODE = 'P0001';
    END IF;
    IF v_from = p_to THEN
        RAISE EXCEPTION 'kooperatif:same_owner' USING ERRCODE = 'P0001';
    END IF;
    IF p_effective_at < v_started THEN
        RAISE EXCEPTION 'kooperatif:invalid_effective_at'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT sh.status INTO v_target
      FROM public.shareholders sh
     WHERE sh.id = p_to
       AND sh.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:shareholder_not_found'
            USING ERRCODE = 'P0002';
    END IF;
    IF v_target <> 'active' THEN
        RAISE EXCEPTION 'kooperatif:shareholder_not_active'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.share_ownerships
       SET ended_at = p_effective_at
     WHERE id = v_own
       AND cooperative_id IS NOT DISTINCT FROM v_eff;

    INSERT INTO public.share_events
        (share_id, event_type, occurred_at, from_shareholder_id,
         to_shareholder_id, amount, currency, reason, actor_user_id,
         cooperative_id)
    VALUES (p_share, p_kind, p_effective_at, v_from, p_to, p_amount,
            'TRY', p_reason, p_by, v_eff)
    RETURNING id INTO v_event;

    INSERT INTO public.share_ownerships
        (share_id, shareholder_id, started_at, acquisition_type,
         source_event_id, created_by, cooperative_id)
    VALUES (p_share, p_to, p_effective_at, p_kind, v_event, p_by,
            v_eff);

    UPDATE public.shares SET updated_at = p_at
     WHERE id = p_share
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT v_from, v_number;
END;
$function$;

-- Share status transition + mandatory share_events row (+ the void
-- handling: no-history check and initial-interval close) — one call.
CREATE OR REPLACE FUNCTION public.app_share_set_status_tx(
    p_share uuid, p_target text, p_reason text, p_at timestamptz,
    p_by uuid, p_coop uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('shares.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff     uuid;
    v_from    text;
    v_number  bigint;
    v_owner   uuid;
    v_history bigint;
BEGIN
    IF p_target NOT IN ('active', 'suspended', 'voided') THEN
        RAISE EXCEPTION 'kooperatif:invalid_transition'
            USING ERRCODE = 'P0001';
    END IF;
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT s.status, s.share_number INTO v_from, v_number
      FROM public.shares s
     WHERE s.id = p_share
       AND s.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF NOT ((v_from = 'active'    AND p_target IN ('suspended', 'voided'))
         OR (v_from = 'suspended' AND p_target IN ('active', 'voided')))
    THEN
        RAISE EXCEPTION 'kooperatif:invalid_transition'
            USING ERRCODE = 'P0001';
    END IF;

    IF p_target = 'voided' THEN
        -- Only a mistaken record that never went beyond its initial
        -- allocation may be voided (docs/16, §30).
        SELECT count(*) INTO v_history
          FROM public.share_events e
         WHERE e.share_id = p_share
           AND e.event_type <> 'initial_acquisition'
           AND e.cooperative_id IS NOT DISTINCT FROM v_eff;
        IF v_history > 0 THEN
            RAISE EXCEPTION 'kooperatif:has_history'
                USING ERRCODE = 'P0001';
        END IF;
        UPDATE public.share_ownerships o
           SET ended_at = p_at
         WHERE o.share_id = p_share AND o.ended_at IS NULL
           AND o.cooperative_id IS NOT DISTINCT FROM v_eff
        RETURNING o.shareholder_id INTO v_owner;
        INSERT INTO public.share_events
            (share_id, event_type, occurred_at, from_shareholder_id,
             status_from, status_to, reason, actor_user_id,
             cooperative_id)
        VALUES (p_share, 'voided', p_at, v_owner, v_from, 'voided',
                p_reason, p_by, v_eff);
    ELSE
        INSERT INTO public.share_events
            (share_id, event_type, occurred_at, status_from, status_to,
             reason, actor_user_id, cooperative_id)
        VALUES (p_share, 'status_change', p_at, v_from, p_target,
                p_reason, p_by, v_eff);
    END IF;

    UPDATE public.shares
       SET status = p_target, updated_at = p_at
     WHERE id = p_share
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN v_number;
END;
$function$;

-- ------------------------------------------------------------------
-- share returns: the shares.status leg now requires the return case
-- and its audit event to already exist in this transaction — a naked
-- flip to return_pending is impossible.
CREATE OR REPLACE FUNCTION public.app_share_return_mark_pending_tx(
    p_share uuid, p_at timestamptz, p_coop uuid)
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
    v_eff uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;

    -- Completeness guard: the return case and its 'return_requested'
    -- audit event must already exist for THIS share in scope.
    IF NOT EXISTS (SELECT 1 FROM public.share_returns r
                    WHERE r.share_id = p_share AND r.status = 'pending'
                      AND r.cooperative_id IS NOT DISTINCT FROM v_eff)
       OR NOT EXISTS (SELECT 1 FROM public.share_events e
                       WHERE e.share_id = p_share
                         AND e.event_type = 'return_requested'
                         AND e.cooperative_id IS NOT DISTINCT FROM v_eff)
    THEN
        RAISE EXCEPTION 'kooperatif:invalid_state'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.shares
       SET status = 'return_pending', updated_at = p_at
     WHERE id = p_share AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:invalid_state'
            USING ERRCODE = 'P0001';
    END IF;
END;
$function$;

-- Return cancellation package: pending case + return_pending share,
-- the 'return_cancelled' event and both flips are indivisible.
CREATE OR REPLACE FUNCTION public.app_share_return_cancel_tx(
    p_return uuid, p_at timestamptz, p_by uuid, p_reason text,
    p_coop uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff    uuid;
    v_share  uuid;
    v_status text;
    v_number bigint;
    v_share_status text;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT r.share_id INTO v_share
      FROM public.share_returns r
     WHERE r.id = p_return
       AND r.cooperative_id IS NOT DISTINCT FROM v_eff;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;

    -- Lock order: share -> return case.
    SELECT s.status INTO v_share_status
      FROM public.shares s
     WHERE s.id = v_share
       AND s.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    SELECT r.status, r.return_number INTO v_status, v_number
      FROM public.share_returns r
     WHERE r.id = p_return
       AND r.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF v_status <> 'pending' OR v_share_status <> 'return_pending' THEN
        RAISE EXCEPTION 'kooperatif:invalid_state'
            USING ERRCODE = 'P0001';
    END IF;

    INSERT INTO public.share_events
        (share_id, event_type, occurred_at, status_from, status_to,
         reason, actor_user_id, cooperative_id)
    VALUES (v_share, 'return_cancelled', p_at, 'return_pending',
            'active', p_reason, p_by, v_eff);

    UPDATE public.share_returns
       SET status = 'cancelled', cancelled_at = p_at,
           cancelled_by = p_by, cancellation_reason = p_reason,
           updated_at = p_at
     WHERE id = p_return AND status = 'pending'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    UPDATE public.shares
       SET status = 'active', updated_at = p_at
     WHERE id = v_share AND status = 'return_pending'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN v_number;
END;
$function$;

-- Entitlement cancellation package: a settled right can never be
-- erased — posted settlements forbid the cancel (docs/19).
CREATE OR REPLACE FUNCTION public.app_share_return_entitlement_cancel_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text, p_coop uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('share_returns.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff    uuid;
    v_status text;
    v_number bigint;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT e.status, e.entitlement_number INTO v_status, v_number
      FROM public.share_return_entitlements e
     WHERE e.id = p_id
       AND e.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'open' THEN
        RAISE EXCEPTION 'kooperatif:invalid_state'
            USING ERRCODE = 'P0001';
    END IF;
    IF EXISTS (SELECT 1 FROM public.share_return_settlements s
                WHERE s.entitlement_id = p_id AND s.status = 'posted'
                  AND s.cooperative_id IS NOT DISTINCT FROM v_eff) THEN
        RAISE EXCEPTION 'kooperatif:has_settlements'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.share_return_entitlements
       SET status = 'cancelled', cancelled_at = p_at,
           cancelled_by = p_by, cancellation_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'open'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN v_number;
END;
$function$;

-- ------------------------------------------------------------------
-- parties (shareholders.manage): shareholder status transition + the
-- mandatory 'shareholder_status_changed' security event — indivisible.
CREATE OR REPLACE FUNCTION public.app_shareholder_set_status_tx(
    p_id uuid, p_target text, p_reason text, p_by uuid, p_coop uuid)
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
    v_eff     uuid;
    v_from    text;
    v_session uuid;
BEGIN
    IF p_target NOT IN ('active', 'inactive', 'voided') THEN
        RAISE EXCEPTION 'kooperatif:invalid_transition'
            USING ERRCODE = 'P0001';
    END IF;
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
        v_session := nullif(current_setting('app.session_id', true),
                            '')::uuid;
    END IF;

    SELECT s.status INTO v_from
      FROM public.shareholders s
     WHERE s.id = p_id
       AND s.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF NOT ((v_from = 'active'   AND p_target IN ('inactive', 'voided'))
         OR (v_from = 'inactive' AND p_target IN ('active', 'voided')))
    THEN
        RAISE EXCEPTION 'kooperatif:invalid_transition'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.shareholders
       SET status = p_target, updated_at = now()
     WHERE id = p_id
       AND cooperative_id IS NOT DISTINCT FROM v_eff;

    INSERT INTO public.security_events
        (event_type, cooperative_id, user_id, session_id, metadata)
    VALUES ('shareholder_status_changed', v_eff, p_by, v_session,
            jsonb_build_object('shareholder_id', p_id, 'from', v_from,
                               'to', p_target, 'reason', p_reason));
END;
$function$;

-- ------------------------------------------------------------------
-- periods (periods.manage): a period may only open WITH its generated
-- obligations — a bare flip leaving an open period without any
-- assessment is impossible.
CREATE OR REPLACE FUNCTION public.app_period_open_tx(
    p_id uuid, p_coop uuid)
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
    v_eff uuid;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;

    UPDATE public.periods
       SET status = 'open', updated_at = now()
     WHERE id = p_id AND status = 'draft'
       AND cooperative_id IS NOT DISTINCT FROM v_eff
       AND EXISTS (SELECT 1 FROM public.assessments a
                    WHERE a.period_id = p_id
                      AND a.cooperative_id IS NOT DISTINCT FROM v_eff);
END;
$function$;

-- ------------------------------------------------------------------
-- governance (governance.manage): a decision opens only under an
-- ACTIVE body — the check is inside the boundary now.
CREATE OR REPLACE FUNCTION public.app_governance_decision_open_tx(
    p_id uuid, p_by uuid, p_coop uuid)
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
    v_eff    uuid;
    v_status text;
    v_body   uuid;
    v_bstat  text;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT d.status, d.body_id INTO v_status, v_body
      FROM public.governance_decisions d
     WHERE d.id = p_id
       AND d.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'draft' THEN
        RAISE EXCEPTION 'kooperatif:invalid_state'
            USING ERRCODE = 'P0001';
    END IF;
    SELECT b.status INTO v_bstat
      FROM public.governance_bodies b
     WHERE b.id = v_body
       AND b.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF v_bstat IS DISTINCT FROM 'active' THEN
        RAISE EXCEPTION 'kooperatif:inactive_body'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.governance_decisions
       SET status = 'open', opened_at = now(), opened_by = p_by,
           updated_at = now()
     WHERE id = p_id AND status = 'draft'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
END;
$function$;

-- ------------------------------------------------------------------
-- investments (investments.manage): cancellation is identity-only —
-- an investment with ANY financial event can never be cancelled.
CREATE OR REPLACE FUNCTION public.app_investment_cancel_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text, p_coop uuid)
RETURNS TABLE(replayed boolean, investment_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('investments.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff    uuid;
    v_status text;
    v_number bigint;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT i.status, i.investment_number INTO v_status, v_number
      FROM public.investments i
     WHERE i.id = p_id
       AND i.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'active' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;
    IF EXISTS (SELECT 1 FROM public.investment_fundings f
                WHERE f.investment_id = p_id
                  AND f.cooperative_id IS NOT DISTINCT FROM v_eff)
       OR EXISTS (SELECT 1 FROM public.investment_incomes n
                   WHERE n.investment_id = p_id
                     AND n.cooperative_id IS NOT DISTINCT FROM v_eff)
       OR EXISTS (SELECT 1 FROM public.investment_disposals d
                   WHERE d.investment_id = p_id
                     AND d.cooperative_id IS NOT DISTINCT FROM v_eff)
    THEN
        RAISE EXCEPTION 'kooperatif:has_financial_events'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.investments
       SET status = 'cancelled', cancelled_at = p_at,
           cancelled_by = p_by, cancellation_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

-- ------------------------------------------------------------------
-- social aid funds (social_aid.manage): close requires zero remaining
-- restricted availability; cancel requires zero financial events. The
-- invariants live inside the boundary now.
CREATE OR REPLACE FUNCTION public.app_social_aid_fund_close_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_coop uuid)
RETURNS TABLE(replayed boolean, fund_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('social_aid.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff    uuid;
    v_status text;
    v_number bigint;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT f.status, f.fund_number INTO v_status, v_number
      FROM public.social_aid_funds f
     WHERE f.id = p_id
       AND f.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'active' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;
    -- No remaining restricted availability anywhere on the fund.
    IF EXISTS (
        SELECT 1 FROM (
            SELECT financial_account_id, sum(amount) AS available FROM (
                SELECT financial_account_id, amount
                  FROM public.social_aid_donations
                 WHERE fund_id = p_id AND status = 'posted'
                   AND cooperative_id IS NOT DISTINCT FROM v_eff
                UNION ALL
                SELECT financial_account_id, -amount
                  FROM public.social_aid_disbursements
                 WHERE fund_id = p_id AND status = 'posted'
                   AND cooperative_id IS NOT DISTINCT FROM v_eff
            ) s GROUP BY financial_account_id
        ) t WHERE t.available <> 0) THEN
        RAISE EXCEPTION 'kooperatif:has_restricted_balance'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.social_aid_funds
       SET status = 'closed', closed_at = p_at, closed_by = p_by,
           updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

CREATE OR REPLACE FUNCTION public.app_social_aid_fund_cancel_tx(
    p_id uuid, p_at timestamptz, p_by uuid, p_reason text, p_coop uuid)
RETURNS TABLE(replayed boolean, fund_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('social_aid.manage');
    v_restricted boolean := session_user = 'kooperatif_app'
                          OR public.app_ctx_ok()
                          OR public.app_user_ctx_ok();
    v_eff    uuid;
    v_status text;
    v_number bigint;
BEGIN
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
    END IF;

    SELECT f.status, f.fund_number INTO v_status, v_number
      FROM public.social_aid_funds f
     WHERE f.id = p_id
       AND f.cooperative_id IS NOT DISTINCT FROM v_eff FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif:not_found' USING ERRCODE = 'P0002';
    END IF;
    IF v_status <> 'active' THEN
        RETURN QUERY SELECT true, v_number;
        RETURN;
    END IF;
    IF EXISTS (SELECT 1 FROM public.social_aid_donations d
                WHERE d.fund_id = p_id
                  AND d.cooperative_id IS NOT DISTINCT FROM v_eff)
       OR EXISTS (SELECT 1 FROM public.social_aid_disbursements s
                   WHERE s.fund_id = p_id
                     AND s.cooperative_id IS NOT DISTINCT FROM v_eff)
    THEN
        RAISE EXCEPTION 'kooperatif:has_restricted_balance'
            USING ERRCODE = 'P0001';
    END IF;

    UPDATE public.social_aid_funds
       SET status = 'cancelled', cancelled_at = p_at,
           cancelled_by = p_by, cancellation_reason = p_reason,
           updated_at = p_at
     WHERE id = p_id AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM v_eff;
    RETURN QUERY SELECT false, v_number;
END;
$function$;

-- ------------------------------------------------------------------
-- EXECUTE surface: packages callable by the runtime role; the
-- component definers that could create an incomplete financial
-- reversal lose runtime EXECUTE entirely. Newly created functions
-- carry PUBLIC EXECUTE by default — that is why revoking from
-- kooperatif_app alone would be a no-op (the role inherits PUBLIC).
-- Every component below is therefore revoked from PUBLIC, and each
-- new package is likewise revoked from PUBLIC then granted only to
-- kooperatif_app. The component functions remain invocable by the
-- owner lane and from inside the packages (definer = owner).
-- ------------------------------------------------------------------
REVOKE EXECUTE ON FUNCTION public.app_movements_reverse_of_source(text, uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_payment_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_transfer_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_income_entry_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_expense_entry_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_credit_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_credits_reverse_by_ids(uuid[], timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_investment_funding_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_investment_income_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_investment_dispose(uuid, timestamptz, uuid, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_share_return_settlement_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_share_return_finalize(uuid, uuid, timestamptz, uuid, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_social_aid_donation_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_social_aid_disbursement_reverse(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_share_mark_return_pending(uuid, timestamptz, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_share_return_cancel(uuid, uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_share_return_entitlement_cancel(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_share_set_status(uuid, text, timestamptz, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_share_ownership_end(uuid, timestamptz, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_share_ownership_end_open_interval(uuid, timestamptz, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_shareholder_set_status(uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_period_open(uuid, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_governance_decision_open(uuid, uuid, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_investment_cancel(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_social_aid_fund_close(uuid, timestamptz, uuid, uuid) FROM PUBLIC, kooperatif_app;
REVOKE EXECUTE ON FUNCTION public.app_social_aid_fund_cancel(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC, kooperatif_app;

REVOKE ALL ON FUNCTION public.app_payment_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_transfer_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_income_entry_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_expense_entry_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_credit_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_investment_funding_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_investment_income_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_investment_dispose_tx(uuid, date, numeric, text, text, text, jsonb, text, text, uuid, timestamptz, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_share_return_settlement_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_share_return_finalize_tx(uuid, timestamptz, date, jsonb, timestamptz, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_social_aid_donation_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_social_aid_disbursement_reverse_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_governance_decision_finalize_tx(uuid, text, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_share_transfer_tx(uuid, uuid, text, numeric, text, timestamptz, timestamptz, timestamptz, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_share_set_status_tx(uuid, text, text, timestamptz, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_share_return_mark_pending_tx(uuid, timestamptz, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_share_return_cancel_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_share_return_entitlement_cancel_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_shareholder_set_status_tx(uuid, text, text, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_period_open_tx(uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_governance_decision_open_tx(uuid, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_investment_cancel_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_social_aid_fund_close_tx(uuid, timestamptz, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_social_aid_fund_cancel_tx(uuid, timestamptz, uuid, text, uuid) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION public.app_payment_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_transfer_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_income_entry_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_expense_entry_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_credit_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_funding_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_income_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_dispose_tx(uuid, date, numeric, text, text, text, jsonb, text, text, uuid, timestamptz, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_settlement_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_finalize_tx(uuid, timestamptz, date, jsonb, timestamptz, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_social_aid_donation_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_social_aid_disbursement_reverse_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_governance_decision_finalize_tx(uuid, text, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_transfer_tx(uuid, uuid, text, numeric, text, timestamptz, timestamptz, timestamptz, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_set_status_tx(uuid, text, text, timestamptz, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_mark_pending_tx(uuid, timestamptz, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_cancel_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_share_return_entitlement_cancel_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_shareholder_set_status_tx(uuid, text, text, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_period_open_tx(uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_governance_decision_open_tx(uuid, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_investment_cancel_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_social_aid_fund_close_tx(uuid, timestamptz, uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_social_aid_fund_cancel_tx(uuid, timestamptz, uuid, text, uuid) TO kooperatif_app;
