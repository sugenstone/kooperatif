-- 0058 — M1-P4-B1: payment creation integrity
--
-- The closure audit's B1 finding: a sealed actor holding a *.manage
-- permission could INSERT payments / payment_allocations /
-- shareholder_credits / credit_applications / account_movements
-- directly on the kooperatif_app lane and produce states the HTTP
-- repository never produces:
--   * a `posted` Payment with NO account movement (unbacked receipt),
--     or with MORE THAN ONE active movement (double posting);
--   * forged `created_by` attribution on the ledger rows;
--   * a caller-chosen `payment_number` / `credit_number`, bypassing
--     the gapless document counters;
--   * rows born `reversed`, laundering provenance at birth;
--   * allocations/credits/applications that over-dispose a Payment,
--     over-settle an Assessment, or over-apply a Credit — debt
--     written off without backing money.
--
-- This migration binds the payment-domain invariants at the
-- database boundary on the restricted lane only. Owner-lane
-- fixtures, migrations and the SECURITY DEFINER packages are
-- untouched; RLS, grants and the 0056/0057 surfaces are unchanged.
--
-- Semantics preserved (verified against payments/repo.rs and
-- credits/repo.rs):
--   * a posted Payment may legitimately carry ZERO allocations —
--     the unallocated remainder is a supported state
--     (docs/06 disposition path), never forced into credit;
--   * allocations may be added later (`add_allocations`);
--   * remainder may be assigned as shareholder credit
--     (`assign_credit`) and applied (`apply_credit`,
--     `auto_apply_to_assessments`); both payment allocations and
--     credit applications settle assessment debt;
--   * idempotency_key/fingerprint semantics are caller-owned;
--   * the payer need not be a debtor and may be a new Person.

-- ============================================================
-- §1. Birth pinning — actor attribution and legitimate birth
--     status are pinned on the restricted lane.
-- ============================================================
-- A sealed caller may never:
--   * attribute a ledger row to anyone else (`created_by` is
--     overwritten with the sealed actor, matching the
--     account_movements pin from 0057);
--   * mint a row born `reversed`/`disposed`/terminal — lifecycle
--     transitions belong to the verified packages (R3B/R3C);
--   * supply the document number — counters stay server-assigned
--     (§2) so the app lane cannot mint non-sequential or replayed
--     numbers.
CREATE OR REPLACE FUNCTION public.app_birth_pin()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_actor uuid;
BEGIN
    IF session_user <> 'kooperatif_app' THEN
        RETURN NEW;
    END IF;
    v_actor := nullif(current_setting('app.user_id', true), '')::uuid;
    IF v_actor IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed actor required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    NEW.created_by := v_actor;
    -- TG_ARGV[0] = the only legitimate birth status for this table.
    IF NEW.status IS DISTINCT FROM TG_ARGV[0] THEN
        RAISE EXCEPTION
            'insufficient_privilege: rows on this lane are born %, not %',
            TG_ARGV[0], NEW.status
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    NEW.reversed_at     := NULL;
    NEW.reversed_by     := NULL;
    NEW.reversal_reason := NULL;
    RETURN NEW;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_birth_pin() FROM PUBLIC;

CREATE OR REPLACE TRIGGER payments_birth_pin
    BEFORE INSERT ON public.payments
    FOR EACH ROW EXECUTE FUNCTION public.app_birth_pin('posted');
CREATE OR REPLACE TRIGGER payment_allocations_birth_pin
    BEFORE INSERT ON public.payment_allocations
    FOR EACH ROW EXECUTE FUNCTION public.app_birth_pin('active');
CREATE OR REPLACE TRIGGER shareholder_credits_birth_pin
    BEFORE INSERT ON public.shareholder_credits
    FOR EACH ROW EXECUTE FUNCTION public.app_birth_pin('active');
CREATE OR REPLACE TRIGGER credit_applications_birth_pin
    BEFORE INSERT ON public.credit_applications
    FOR EACH ROW EXECUTE FUNCTION public.app_birth_pin('active');

-- ============================================================
-- §2. Server-side document numbering on the restricted lane.
-- ============================================================
-- `allocate_document_number` remains the single counter lane, but
-- its RESULT is now bound by the trigger: a restricted caller may
-- not choose the value. Owner-lane inserts keep their explicit
-- numbers (fixtures/migrations); NULL falls through to assignment
-- on every lane so the repository's INSERT can omit the column.
CREATE OR REPLACE FUNCTION public.app_payment_number_pin()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' AND NEW.payment_number IS NOT NULL
    THEN
        RAISE EXCEPTION
            'insufficient_privilege: document numbers are server-assigned'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF NEW.payment_number IS NULL THEN
        NEW.payment_number := public.allocate_document_number(
            NEW.cooperative_id, 'payments.payment_number');
    END IF;
    RETURN NEW;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_payment_number_pin() FROM PUBLIC;

CREATE OR REPLACE FUNCTION public.app_credit_number_pin()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' AND NEW.credit_number IS NOT NULL
    THEN
        RAISE EXCEPTION
            'insufficient_privilege: document numbers are server-assigned'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF NEW.credit_number IS NULL THEN
        NEW.credit_number := public.allocate_document_number(
            NEW.cooperative_id, 'shareholder_credits.credit_number');
    END IF;
    RETURN NEW;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_credit_number_pin() FROM PUBLIC;

CREATE OR REPLACE TRIGGER payments_number_pin
    BEFORE INSERT ON public.payments
    FOR EACH ROW EXECUTE FUNCTION public.app_payment_number_pin();
CREATE OR REPLACE TRIGGER shareholder_credits_number_pin
    BEFORE INSERT ON public.shareholder_credits
    FOR EACH ROW EXECUTE FUNCTION public.app_credit_number_pin();

-- ============================================================
-- §3. Payment <-> movement pair binding (deferred).
-- ============================================================
-- A `posted` Payment is backed by EXACTLY ONE active inflow
-- movement on its destination account, same cooperative, same
-- amount, same attributed creator — inserted in the same
-- transaction (the deferred source check of 0057 validates the
-- movement -> payment direction; this binds payment -> movement
-- cardinality). Commit-time only: the pair may be written in any
-- order inside the transaction.
CREATE OR REPLACE FUNCTION public.app_payment_pair_check()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_legs bigint;
    v_acct record;
BEGIN
    IF session_user <> 'kooperatif_app' OR NEW.status <> 'posted' THEN
        RETURN NULL;
    END IF;
    SELECT status, currency, cooperative_id INTO v_acct
      FROM public.financial_accounts
     WHERE id = NEW.destination_account_id;
    IF NOT FOUND OR v_acct.status <> 'active'
       OR v_acct.currency <> 'TRY'
       OR v_acct.cooperative_id IS DISTINCT FROM NEW.cooperative_id THEN
        RAISE EXCEPTION 'kooperatif:account_not_found_or_inactive'
            USING ERRCODE = 'P0001';
    END IF;
    SELECT count(*) INTO v_legs
      FROM public.account_movements m
     WHERE m.source_type = 'payment'
       AND m.source_id   = NEW.id
       AND m.account_id  = NEW.destination_account_id
       AND m.direction   = 'inflow'
       AND m.amount      = NEW.amount
       AND m.status      = 'active'
       AND m.created_by  = NEW.created_by
       AND m.cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id;
    IF v_legs <> 1 THEN
        RAISE EXCEPTION 'kooperatif:payment_movement_pair'
            USING ERRCODE = 'P0001',
                  DETAIL = 'a posted payment requires exactly one '
                        || 'active inflow movement on its destination '
                        || 'account, in the same transaction';
    END IF;
    RETURN NULL;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_payment_pair_check() FROM PUBLIC;

DROP TRIGGER IF EXISTS payments_pair_check ON public.payments;
CREATE CONSTRAINT TRIGGER payments_pair_check
    AFTER INSERT ON public.payments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION public.app_payment_pair_check();

-- ============================================================
-- §4. Disposition binding (deferred): allocations, credits and
--     applications can never exceed what funds them.
-- ============================================================
-- Commit-time sums are computed under a FOR NO KEY UPDATE lock on
-- the funded parent. NO KEY UPDATE conflicts with another disposer's
-- NO KEY UPDATE and with lifecycle UPDATEs, but NOT with the
-- FOR KEY SHARE lock a concurrent sibling INSERT's referential
-- check holds — so two racing writers serialize here without a
-- lock-ordering deadlock, and the loser re-sums against the
-- winner's committed rows (READ COMMITTED statement snapshot).
CREATE OR REPLACE FUNCTION public.app_allocation_bound_check()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_pay_amount numeric;
    v_pay_status text;
    v_disposed   numeric;
    v_a_amount   numeric;
    v_a_status   text;
    v_settled    numeric;
BEGIN
    IF session_user <> 'kooperatif_app' THEN
        RETURN NULL;
    END IF;

    SELECT amount, status INTO v_pay_amount, v_pay_status
      FROM public.payments
     WHERE id = NEW.payment_id
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id
     FOR NO KEY UPDATE;
    IF NOT FOUND OR v_pay_status <> 'posted' THEN
        RAISE EXCEPTION 'kooperatif:allocation_on_unposted_payment'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT COALESCE(sum(amount), 0)
             + COALESCE((SELECT sum(amount)
                           FROM public.shareholder_credits
                          WHERE source_payment_id = NEW.payment_id
                            AND status = 'active'
                            AND cooperative_id IS NOT DISTINCT FROM
                                NEW.cooperative_id), 0)
      INTO v_disposed
      FROM public.payment_allocations
     WHERE payment_id = NEW.payment_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id;
    IF v_disposed > v_pay_amount THEN
        RAISE EXCEPTION 'kooperatif:payment_over_disposed'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT amount, status INTO v_a_amount, v_a_status
      FROM public.assessments
     WHERE id = NEW.assessment_id
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id
     FOR NO KEY UPDATE;
    IF NOT FOUND OR v_a_status <> 'active' THEN
        RAISE EXCEPTION 'kooperatif:inactive_assessment'
            USING ERRCODE = 'P0001';
    END IF;

    -- settled = payment allocations + credit applications (both
    -- satisfy debt identically — credits/repo::settled_map).
    SELECT COALESCE(sum(amount), 0)
      INTO v_settled
      FROM (SELECT amount FROM public.payment_allocations
             WHERE assessment_id = NEW.assessment_id
               AND status = 'active'
               AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id
             UNION ALL
            SELECT amount FROM public.credit_applications
             WHERE assessment_id = NEW.assessment_id
               AND status = 'active'
               AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id) s;
    IF v_settled > v_a_amount THEN
        RAISE EXCEPTION 'kooperatif:assessment_over_settled'
            USING ERRCODE = 'P0001';
    END IF;
    RETURN NULL;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_allocation_bound_check() FROM PUBLIC;

DROP TRIGGER IF EXISTS payment_allocations_bound_check
    ON public.payment_allocations;
CREATE CONSTRAINT TRIGGER payment_allocations_bound_check
    AFTER INSERT ON public.payment_allocations
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION public.app_allocation_bound_check();

CREATE OR REPLACE FUNCTION public.app_credit_bound_check()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_pay_amount numeric;
    v_pay_status text;
    v_disposed   numeric;
    v_b_status   text;
BEGIN
    IF session_user <> 'kooperatif_app' THEN
        RETURN NULL;
    END IF;

    SELECT amount, status INTO v_pay_amount, v_pay_status
      FROM public.payments
     WHERE id = NEW.source_payment_id
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id
     FOR NO KEY UPDATE;
    IF NOT FOUND OR v_pay_status <> 'posted' THEN
        RAISE EXCEPTION 'kooperatif:credit_on_unposted_payment'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT COALESCE((SELECT sum(amount)
                       FROM public.payment_allocations
                      WHERE payment_id = NEW.source_payment_id
                        AND status = 'active'
                        AND cooperative_id IS NOT DISTINCT FROM
                            NEW.cooperative_id), 0)
         + COALESCE(sum(amount), 0)
      INTO v_disposed
      FROM public.shareholder_credits
     WHERE source_payment_id = NEW.source_payment_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id;
    IF v_disposed > v_pay_amount THEN
        RAISE EXCEPTION 'kooperatif:payment_over_disposed'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT status INTO v_b_status
      FROM public.shareholders
     WHERE id = NEW.shareholder_id
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id;
    IF v_b_status IS NULL OR v_b_status NOT IN ('active','inactive') THEN
        RAISE EXCEPTION 'kooperatif:beneficiary_invalid'
            USING ERRCODE = 'P0001';
    END IF;
    RETURN NULL;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_credit_bound_check() FROM PUBLIC;

DROP TRIGGER IF EXISTS shareholder_credits_bound_check
    ON public.shareholder_credits;
CREATE CONSTRAINT TRIGGER shareholder_credits_bound_check
    AFTER INSERT ON public.shareholder_credits
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION public.app_credit_bound_check();

CREATE OR REPLACE FUNCTION public.app_application_bound_check()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_c_amount numeric;
    v_c_status text;
    v_c_owner  uuid;
    v_applied  numeric;
    v_a_amount numeric;
    v_a_status text;
    v_a_owner  uuid;
    v_settled  numeric;
BEGIN
    IF session_user <> 'kooperatif_app' THEN
        RETURN NULL;
    END IF;

    SELECT amount, status, shareholder_id
      INTO v_c_amount, v_c_status, v_c_owner
      FROM public.shareholder_credits
     WHERE id = NEW.credit_id
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id
     FOR NO KEY UPDATE;
    IF NOT FOUND OR v_c_status <> 'active' THEN
        RAISE EXCEPTION 'kooperatif:application_on_inactive_credit'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT COALESCE(sum(amount), 0) INTO v_applied
      FROM public.credit_applications
     WHERE credit_id = NEW.credit_id
       AND status = 'active'
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id;
    IF v_applied > v_c_amount THEN
        RAISE EXCEPTION 'kooperatif:credit_over_applied'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT amount, status, shareholder_id
      INTO v_a_amount, v_a_status, v_a_owner
      FROM public.assessments
     WHERE id = NEW.assessment_id
       AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id
     FOR NO KEY UPDATE;
    IF NOT FOUND OR v_a_status <> 'active' THEN
        RAISE EXCEPTION 'kooperatif:inactive_assessment'
            USING ERRCODE = 'P0001';
    END IF;
    -- A credit settles only ITS OWN shareholder's debt
    -- (credits/repo::apply_credit).
    IF v_a_owner IS DISTINCT FROM v_c_owner THEN
        RAISE EXCEPTION 'kooperatif:application_owner_mismatch'
            USING ERRCODE = 'P0001';
    END IF;

    SELECT COALESCE(sum(amount), 0)
      INTO v_settled
      FROM (SELECT amount FROM public.payment_allocations
             WHERE assessment_id = NEW.assessment_id
               AND status = 'active'
               AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id
             UNION ALL
            SELECT amount FROM public.credit_applications
             WHERE assessment_id = NEW.assessment_id
               AND status = 'active'
               AND cooperative_id IS NOT DISTINCT FROM NEW.cooperative_id) s;
    IF v_settled > v_a_amount THEN
        RAISE EXCEPTION 'kooperatif:assessment_over_settled'
            USING ERRCODE = 'P0001';
    END IF;
    RETURN NULL;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_application_bound_check() FROM PUBLIC;

DROP TRIGGER IF EXISTS credit_applications_bound_check
    ON public.credit_applications;
CREATE CONSTRAINT TRIGGER credit_applications_bound_check
    AFTER INSERT ON public.credit_applications
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION public.app_application_bound_check();

-- ============================================================
-- §6. Definer lock helpers — the legitimate command path on the
--     restricted lane.
-- ============================================================
-- The repository commands lock parent rows `SELECT ... FOR UPDATE`
-- before mutating. Row locks of every strength require the UPDATE
-- privilege, which `kooperatif_app` deliberately does NOT hold on
-- the financial tables (F4: financial history is update/delete
-- closed). The command path on the real runtime lane therefore
-- needs its row locks taken inside the owner security boundary —
-- the same model the 0056 `_tx` packages already use.
--
-- Each helper:
--   * runs as the owner, so the FOR UPDATE lock succeeds;
--   * on the restricted lane requires the verified sealed context
--     AND one of the listed business permissions — the same gate
--     `app_write_guard` applies to writes;
--   * returns only the columns the command reads; the caller still
--     performs every cooperative-scope comparison itself (C3).
-- Row locks are held to the END of the caller's transaction, so
-- the deterministic lock ordering of the commands is preserved
-- exactly.
CREATE OR REPLACE FUNCTION public.app_lock_accounts(p_ids uuid[])
RETURNS TABLE(id uuid, status text, currency text, cooperative_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(ARRAY[
            'payments.manage', 'financial_accounts.manage',
            'income_expense.manage', 'investments.manage',
            'share_returns.manage', 'social_aid.manage']);
    END IF;
    RETURN QUERY
        SELECT a.id, a.status, a.currency, a.cooperative_id
          FROM public.financial_accounts a
         WHERE a.id = ANY(p_ids)
         ORDER BY a.id
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_accounts(uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_accounts(uuid[])
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_assessments(p_ids uuid[])
RETURNS TABLE(id uuid, amount numeric, status text, cooperative_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(ARRAY[
            'payments.manage', 'assessments.manage',
            'credits.manage', 'periods.manage']);
    END IF;
    RETURN QUERY
        SELECT a.id, a.amount, a.status, a.cooperative_id
          FROM public.assessments a
         WHERE a.id = ANY(p_ids)
         ORDER BY a.id
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_assessments(uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_assessments(uuid[])
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_payment(p_id uuid, p_coop uuid)
RETURNS TABLE(status text, amount numeric, payment_number bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(ARRAY[
            'payments.manage', 'credits.manage']);
    END IF;
    RETURN QUERY
        SELECT p.status, p.amount, p.payment_number
          FROM public.payments p
         WHERE p.id = p_id
           AND p.cooperative_id IS NOT DISTINCT FROM p_coop
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_payment(uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_payment(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_allocation(
    p_id uuid, p_payment uuid, p_coop uuid)
RETURNS TABLE(status text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(ARRAY[
            'payments.manage']);
    END IF;
    RETURN QUERY
        SELECT a.status
          FROM public.payment_allocations a
         WHERE a.id = p_id
           AND a.payment_id = p_payment
           AND a.cooperative_id IS NOT DISTINCT FROM p_coop
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_allocation(uuid, uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_allocation(uuid, uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_credits(
    p_coop uuid, p_shareholder uuid)
RETURNS TABLE(id uuid, amount numeric)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(ARRAY[
            'credits.manage']);
    END IF;
    RETURN QUERY
        SELECT c.id, c.amount
          FROM public.shareholder_credits c
         WHERE c.shareholder_id = p_shareholder
           AND c.status = 'active'
           AND c.cooperative_id IS NOT DISTINCT FROM p_coop
         ORDER BY c.credit_number
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_credits(uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_credits(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_assessment(p_id uuid, p_coop uuid)
RETURNS TABLE(id uuid, amount numeric, status text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(ARRAY[
            'credits.manage', 'payments.manage',
            'assessments.manage', 'periods.manage']);
    END IF;
    RETURN QUERY
        SELECT a.id, a.amount, a.status
          FROM public.assessments a
         WHERE a.id = p_id
           AND a.cooperative_id IS NOT DISTINCT FROM p_coop
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_assessment(uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_assessment(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_credit_application(
    p_id uuid, p_coop uuid)
RETURNS TABLE(status text, credit_id uuid, assessment_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(ARRAY[
            'credits.manage']);
    END IF;
    RETURN QUERY
        SELECT a.status, a.credit_id, a.assessment_id
          FROM public.credit_applications a
         WHERE a.id = p_id
           AND a.cooperative_id IS NOT DISTINCT FROM p_coop
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_credit_application(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_credit_application(uuid, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_lock_credit(p_id uuid, p_coop uuid)
RETURNS TABLE(id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(ARRAY[
            'credits.manage']);
    END IF;
    RETURN QUERY
        SELECT c.id
          FROM public.shareholder_credits c
         WHERE c.id = p_id
           AND c.cooperative_id IS NOT DISTINCT FROM p_coop
         FOR UPDATE;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_lock_credit(uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_lock_credit(uuid, uuid)
    TO kooperatif_app;

-- ============================================================
-- §7. Postchecks
-- ============================================================
DO $$
DECLARE
    v_missing text;
BEGIN
    SELECT string_agg(e.tbl || '.' || e.tg, ', ') INTO v_missing
      FROM (VALUES
        ('payments','payments_birth_pin'),
        ('payments','payments_number_pin'),
        ('payments','payments_pair_check'),
        ('payment_allocations','payment_allocations_birth_pin'),
        ('payment_allocations','payment_allocations_bound_check'),
        ('shareholder_credits','shareholder_credits_birth_pin'),
        ('shareholder_credits','shareholder_credits_number_pin'),
        ('shareholder_credits','shareholder_credits_bound_check'),
        ('credit_applications','credit_applications_birth_pin'),
        ('credit_applications','credit_applications_bound_check')) e(tbl, tg)
     WHERE NOT EXISTS (
        SELECT 1 FROM pg_trigger
         WHERE tgname = e.tg AND NOT tgisinternal
           AND tgrelid = e.tbl::regclass);
    IF v_missing IS NOT NULL THEN
        RAISE EXCEPTION 'missing B1 trigger(s): %', v_missing;
    END IF;

    -- Lock helpers must be callable by the runtime role only via
    -- EXECUTE (definer boundary) and never through a direct grant on
    -- the underlying tables.
    PERFORM 1
      FROM pg_proc p
     WHERE p.proname = 'app_lock_accounts'
       AND NOT has_function_privilege(
               'kooperatif_app', p.oid, 'EXECUTE');
    IF FOUND THEN
        RAISE EXCEPTION 'app_lock_accounts not executable by kooperatif_app';
    END IF;
END
$$;
