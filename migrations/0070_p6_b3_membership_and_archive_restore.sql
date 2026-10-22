-- 0070_p6_b3_membership_and_archive_restore.sql
-- M1-P6 Part A + B3:
--
--   PART A (approved: reversible archiving) — archived -> active is a
--   legal transition through the platform lane, gated by the same
--   effective-administrator invariant as suspended -> active. Archival
--   stays explicit (never automatic) and bootstrap-protection stands;
--   every other archived -> * transition remains rejected.
--
--   PART B (B3 membership management) — cooperative membership
--   creation moves behind the dedicated cooperative-scoped
--   `memberships.manage` permission inside the sealed boundary:
--   app_membership_create derives the target cooperative ONLY from the
--   verified sealed scope (never a parameter), requires an eligible
--   active global user and a live ('active') cooperative, refuses
--   duplicate rows, and stamps created_by_user_id with the sealed
--   actor. users.manage keeps backward compatibility on the existing
--   suspend/reactivate/end routes; membership CREATION now requires
--   memberships.manage.
--
--   Invariants preserved: per-(cooperative,user) history rows are never
--   deleted; INSERT mints cooperative_membership_granted through the
--   audited transition trigger with sealed cooperative attribution;
--   rt_authz_memberships (0064) already fans the assignment signal out
--   to bound sockets.

-- ------------------------------------------------------------------
-- Part A — reversible archive: archived -> active joins the restore
-- arm. Same sealed platform gate, same administrator invariant, same
-- atomic UPDATE; the only change is the source-state set.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_cooperative_set_status(
    p_cooperative_id uuid,
    p_status         text
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
-- Explicit transition matrix (invalid source -> check_violation, never
-- silent):
--   active       -> suspended   (business gate closes immediately)
--   suspended    -> active      (requires >=1 effective administrator)
--   archived     -> active      (restore: same invariant; an archived
--                                cooperative WITHOUT an effective
--                                administrator is not restorable
--                                through the app lane)
--   provisioning|active|suspended -> archived (business gate closes;
--                              the bootstrap cooperative is refused)
-- Every other transition, including archived -> suspended and
-- provisioning -> active (which belongs to app_cooperative_activate),
-- is rejected. Nothing reactivates an archived row implicitly.
DECLARE
    v_actor      uuid;
    v_status     text;
    v_bootstrap  boolean;
BEGIN
    IF p_status NOT IN ('active', 'suspended', 'archived') THEN
        RAISE EXCEPTION 'invalid_cooperative_transition: unknown target %', p_status
            USING ERRCODE = 'check_violation';
    END IF;

    IF session_user = 'kooperatif_app' THEN
        IF NOT public.app_user_ctx_ok() THEN
            RAISE EXCEPTION 'insufficient_privilege: sealed user context required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        v_actor := nullif(current_setting('app.user_id', true), '')::uuid;
        IF v_actor IS NULL
           OR NOT public.app_has_platform_permission(v_actor, 'cooperatives.manage')
        THEN
            RAISE EXCEPTION 'insufficient_privilege: platform cooperatives.manage required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END IF;

    SELECT c.status, c.is_bootstrap INTO v_status, v_bootstrap
      FROM public.cooperatives c
     WHERE c.id = p_cooperative_id
     FOR UPDATE OF c;
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;

    IF v_status = p_status THEN
        RETURN p_cooperative_id; -- already in target state: idempotent
    END IF;

    IF p_status = 'suspended' THEN
        IF v_status <> 'active' THEN
            RAISE EXCEPTION 'invalid_cooperative_transition: % -> suspended', v_status
                USING ERRCODE = 'check_violation';
        END IF;
    ELSIF p_status = 'active' THEN
        IF v_status NOT IN ('suspended', 'archived') THEN
            RAISE EXCEPTION 'invalid_cooperative_transition: % -> active', v_status
                USING ERRCODE = 'check_violation';
        END IF;
        -- Reactivation/restore validates the effective-administrator
        -- AND membership invariant: app_admin_path_count counts only
        -- active-member + active-role + active-user administration
        -- paths inside this exact cooperative scope.
        IF public.app_admin_path_count(p_cooperative_id) < 1 THEN
            RAISE EXCEPTION 'reactivation_admin_invariant'
                USING ERRCODE = 'check_violation';
        END IF;
    ELSE -- archived
        IF v_bootstrap THEN
            RAISE EXCEPTION 'invalid_cooperative_transition: bootstrap cooperative cannot be archived'
                USING ERRCODE = 'check_violation';
        END IF;
    END IF;

    UPDATE public.cooperatives
       SET status = p_status,
           business_enabled = (p_status = 'active'),
           updated_at = now()
     WHERE id = p_cooperative_id;

    RETURN p_cooperative_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_cooperative_set_status(uuid, text)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_cooperative_set_status(uuid, text)
    TO kooperatif_app;

-- archived -> active mints cooperative_reactivated like suspended ->
-- active (restore IS a reactivation); the rest of the audited
-- transition function is carried verbatim from 0068.
CREATE OR REPLACE FUNCTION public.app_audit_transition()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_type text;
    v_meta jsonb;
BEGIN
    CASE TG_TABLE_NAME
    WHEN 'persons' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'person_created';
            v_meta := jsonb_build_object('person_id', NEW.id);
        ELSIF NEW.first_name IS DISTINCT FROM OLD.first_name
           OR NEW.last_name  IS DISTINCT FROM OLD.last_name
           OR NEW.search_name IS DISTINCT FROM OLD.search_name THEN
            v_type := 'person_updated';
            v_meta := jsonb_build_object(
                'person_id', NEW.id,
                'before', jsonb_build_object(
                    'first_name', OLD.first_name,
                    'last_name', OLD.last_name),
                'after', jsonb_build_object(
                    'first_name', NEW.first_name,
                    'last_name', NEW.last_name));
        END IF;
    WHEN 'families' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'family_created';
            v_meta := jsonb_build_object('family_id', NEW.id);
        END IF;
    WHEN 'shareholders' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'shareholder_created';
            v_meta := jsonb_build_object(
                'shareholder_id', NEW.id, 'person_id', NEW.person_id);
        ELSIF NEW.status IS DISTINCT FROM OLD.status THEN
            v_type := 'shareholder_status_changed';
            v_meta := jsonb_build_object(
                'shareholder_id', NEW.id,
                'from', OLD.status, 'to', NEW.status);
        ELSIF NEW.default_collection_account_id
              IS DISTINCT FROM OLD.default_collection_account_id THEN
            v_type := 'shareholder_default_account_changed';
            v_meta := jsonb_build_object(
                'shareholder_id', NEW.id,
                'default_collection_account_id',
                NEW.default_collection_account_id);
        ELSIF NEW.guardian_person_id
              IS DISTINCT FROM OLD.guardian_person_id THEN
            v_type := 'shareholder_updated';
            v_meta := jsonb_build_object(
                'shareholder_id', NEW.id,
                'change', 'guardian',
                'before', OLD.guardian_person_id,
                'after', NEW.guardian_person_id);
        ELSIF NEW.person_id IS DISTINCT FROM OLD.person_id THEN
            v_type := 'shareholder_updated';
            v_meta := jsonb_build_object('shareholder_id', NEW.id);
        END IF;
    WHEN 'shareholder_family_memberships' THEN
        IF TG_OP = 'UPDATE'
           AND OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL THEN
            v_type := 'shareholder_family_membership_ended';
            v_meta := jsonb_build_object(
                'membership_id', NEW.id,
                'shareholder_id', NEW.shareholder_id,
                'family_id', NEW.family_id,
                'reason', NEW.reason);
        END IF;
    WHEN 'shares' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'share_created';
            v_meta := jsonb_build_object(
                'share_id', NEW.id, 'share_number', NEW.share_number);
        ELSIF NEW.status IS DISTINCT FROM OLD.status THEN
            v_type := 'share_status_changed';
            v_meta := jsonb_build_object(
                'share_id', NEW.id, 'share_number', NEW.share_number,
                'from', OLD.status, 'to', NEW.status);
        END IF;
    WHEN 'share_events' THEN
        IF TG_OP = 'INSERT' AND NEW.event_type = 'transfer' THEN
            v_type := 'share_transferred';
            v_meta := jsonb_build_object(
                'share_id', NEW.share_id,
                'share_number',
                (SELECT share_number FROM public.shares
                  WHERE id = NEW.share_id),
                'from_shareholder_id', NEW.from_shareholder_id,
                'to_shareholder_id', NEW.to_shareholder_id,
                'amount', NEW.amount::text,
                'reason', NEW.reason,
                'effective_at', NEW.occurred_at);
        ELSIF TG_OP = 'INSERT' AND NEW.event_type = 'sale' THEN
            v_type := 'share_sold';
            v_meta := jsonb_build_object(
                'share_id', NEW.share_id,
                'share_number',
                (SELECT share_number FROM public.shares
                  WHERE id = NEW.share_id),
                'from_shareholder_id', NEW.from_shareholder_id,
                'to_shareholder_id', NEW.to_shareholder_id,
                'amount', NEW.amount::text,
                'reason', NEW.reason,
                'effective_at', NEW.occurred_at);
        END IF;
        -- initial_acquisition is minted by the shares INSERT trigger
        -- (share_created); status_change/voided/return_* events are
        -- minted by the shares/share_returns transition triggers â€”
        -- minting here would duplicate them.
    WHEN 'periods' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'period_created';
            v_meta := jsonb_build_object(
                'period_id', NEW.id, 'period_number', NEW.period_number,
                'name', NEW.name);
        ELSIF TG_OP = 'DELETE' THEN
            v_type := 'period_deleted';
            v_meta := jsonb_build_object(
                'period_id', OLD.id, 'period_number', OLD.period_number);
        ELSIF NEW.status IS DISTINCT FROM OLD.status
           AND NEW.status = 'open' THEN
            v_type := 'period_opened';
            v_meta := jsonb_build_object(
                'period_id', NEW.id, 'period_number', NEW.period_number);
        ELSIF NEW.status IS DISTINCT FROM OLD.status
           AND NEW.status = 'closed' THEN
            v_type := 'period_closed';
            v_meta := jsonb_build_object(
                'period_id', NEW.id, 'period_number', NEW.period_number);
        ELSIF NEW.name IS DISTINCT FROM OLD.name
           OR NEW.collection_start_date
              IS DISTINCT FROM OLD.collection_start_date
           OR NEW.due_date IS DISTINCT FROM OLD.due_date THEN
            v_type := 'period_updated';
            v_meta := jsonb_build_object(
                'period_id', NEW.id, 'period_number', NEW.period_number);
        END IF;
    WHEN 'payments' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'payment_posted';
            v_meta := jsonb_build_object(
                'payment_id', NEW.id, 'payment_number', NEW.payment_number,
                'payer_person_id', NEW.payer_person_id,
                'amount', NEW.amount);
        ELSIF TG_OP = 'UPDATE' AND NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'payment_reversed';
            v_meta := jsonb_build_object(
                'payment_id', NEW.id, 'payment_number', NEW.payment_number,
                'reason', NEW.reversal_reason,
                'reversed_allocation_count',
                (SELECT count(*) FROM public.payment_allocations a
                  WHERE a.payment_id = NEW.id AND a.status = 'reversed'));
        END IF;
    WHEN 'payment_allocations' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'payment_allocations_added';
            v_meta := jsonb_build_object(
                'payment_id', NEW.payment_id,
                'allocation_id', NEW.id,
                'assessment_id', NEW.assessment_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'payment_allocation_reversed';
            v_meta := jsonb_build_object(
                'payment_id', NEW.payment_id,
                'allocation_id', NEW.id,
                'reason', NEW.reversal_reason);
        END IF;
    WHEN 'account_transfers' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'account_transfer_posted';
            v_meta := jsonb_build_object(
                'transfer_id', NEW.id, 'transfer_number',
                NEW.transfer_number,
                'source_account_id', NEW.source_account_id,
                'destination_account_id', NEW.destination_account_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'account_transfer_reversed';
            v_meta := jsonb_build_object(
                'transfer_id', NEW.id, 'transfer_number',
                NEW.transfer_number, 'reason', NEW.reversal_reason);
        END IF;
    WHEN 'income_entries' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'income_posted';
            v_meta := jsonb_build_object(
                'income_id', NEW.id, 'income_number', NEW.income_number,
                'financial_account_id', NEW.financial_account_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'income_reversed';
            v_meta := jsonb_build_object(
                'income_id', NEW.id, 'income_number', NEW.income_number,
                'reason', NEW.reversal_reason);
        END IF;
    WHEN 'expense_entries' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'expense_posted';
            v_meta := jsonb_build_object(
                'expense_id', NEW.id, 'expense_number',
                NEW.expense_number,
                'financial_account_id', NEW.financial_account_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'expense_reversed';
            v_meta := jsonb_build_object(
                'expense_id', NEW.id, 'expense_number',
                NEW.expense_number, 'reason', NEW.reversal_reason);
        END IF;
    WHEN 'shareholder_credits' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'active' THEN
            v_type := 'shareholder_credit_assigned';
            v_meta := jsonb_build_object(
                'credit_id', NEW.id, 'credit_number', NEW.credit_number,
                'shareholder_id', NEW.shareholder_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'shareholder_credit_reversed';
            v_meta := jsonb_build_object(
                'credit_id', NEW.id, 'credit_number', NEW.credit_number,
                'reason', NEW.reversal_reason);
        END IF;
    WHEN 'credit_applications' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'credit_applied';
            v_meta := jsonb_build_object(
                'application_id', NEW.id, 'credit_id', NEW.credit_id,
                'assessment_id', NEW.assessment_id,
                'amount', NEW.amount, 'mode', NEW.mode);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'credit_application_reversed';
            v_meta := jsonb_build_object(
                'application_id', NEW.id, 'credit_id', NEW.credit_id,
                'reason', NEW.reversal_reason);
        END IF;
    WHEN 'financial_accounts' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'financial_account_created';
            v_meta := jsonb_build_object(
                'account_id', NEW.id, 'name', NEW.name,
                'account_type', NEW.account_type);
        ELSIF NEW.status IS DISTINCT FROM OLD.status THEN
            v_type := 'financial_account_status_changed';
            v_meta := jsonb_build_object(
                'account_id', NEW.id,
                'from', OLD.status, 'to', NEW.status);
        ELSIF NEW.name IS DISTINCT FROM OLD.name
           OR NEW.account_type IS DISTINCT FROM OLD.account_type
           OR NEW.description IS DISTINCT FROM OLD.description
           OR NEW.bank_name IS DISTINCT FROM OLD.bank_name
           OR NEW.iban IS DISTINCT FROM OLD.iban THEN
            v_type := 'financial_account_updated';
            v_meta := jsonb_build_object('account_id', NEW.id);
        END IF;
    WHEN 'financial_categories' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'financial_category_created';
            v_meta := jsonb_build_object(
                'category_id', NEW.id, 'name', NEW.name,
                'category_type', NEW.category_type);
        ELSIF NEW.status IS DISTINCT FROM OLD.status THEN
            v_type := 'financial_category_status_changed';
            v_meta := jsonb_build_object(
                'category_id', NEW.id,
                'from', OLD.status, 'to', NEW.status);
        ELSIF NEW.name IS DISTINCT FROM OLD.name
           OR NEW.description IS DISTINCT FROM OLD.description
           OR NEW.category_type IS DISTINCT FROM OLD.category_type THEN
            v_type := 'financial_category_updated';
            v_meta := jsonb_build_object('category_id', NEW.id);
        END IF;
    WHEN 'investments' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'investment_created';
            v_meta := jsonb_build_object(
                'investment_id', NEW.id,
                'investment_number', NEW.investment_number,
                'name', NEW.name);
        ELSIF NEW.status = 'cancelled'
           AND OLD.status IS DISTINCT FROM 'cancelled' THEN
            v_type := 'investment_cancelled';
            v_meta := jsonb_build_object(
                'investment_id', NEW.id,
                'investment_number', NEW.investment_number,
                'reason', NEW.cancellation_reason);
        END IF;
    WHEN 'investment_fundings' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'investment_funding_posted';
            v_meta := jsonb_build_object(
                'funding_id', NEW.id, 'investment_id', NEW.investment_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'investment_funding_reversed';
            v_meta := jsonb_build_object(
                'funding_id', NEW.id,
                'reason', NEW.reversal_reason);
        END IF;
    WHEN 'investment_incomes' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'investment_income_posted';
            v_meta := jsonb_build_object(
                'income_id', NEW.id, 'investment_id', NEW.investment_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'investment_income_reversed';
            v_meta := jsonb_build_object(
                'income_id', NEW.id, 'reason', NEW.reversal_reason);
        END IF;
    WHEN 'investment_valuations' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'investment_valuation_recorded';
            v_meta := jsonb_build_object(
                'valuation_id', NEW.id,
                'investment_id', NEW.investment_id,
                'valuation_date', NEW.valuation_date,
                'amount', NEW.amount);
        ELSIF NEW.status = 'cancelled'
           AND OLD.status IS DISTINCT FROM 'cancelled' THEN
            v_type := 'investment_valuation_cancelled';
            v_meta := jsonb_build_object(
                'valuation_id', NEW.id,
                'reason', NEW.cancellation_reason);
        END IF;
    WHEN 'investment_disposals' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'investment_disposed';
            v_meta := jsonb_build_object(
                'disposal_id', NEW.id, 'investment_id', NEW.investment_id,
                'consideration_amount', NEW.consideration_amount);
        END IF;
    WHEN 'investment_disposal_proceeds' THEN
        NULL; -- covered by investment_disposed + the movement row
    WHEN 'share_returns' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'share_return_requested';
            v_meta := jsonb_build_object(
                'return_id', NEW.id, 'share_id', NEW.share_id,
                'shareholder_id', NEW.shareholder_id);
        ELSIF NEW.status = 'finalized'
           AND OLD.status IS DISTINCT FROM 'finalized' THEN
            v_type := 'share_return_finalized';
            v_meta := jsonb_build_object(
                'return_id', NEW.id, 'return_number', NEW.return_number);
        ELSIF NEW.status = 'cancelled'
           AND OLD.status IS DISTINCT FROM 'cancelled' THEN
            v_type := 'share_return_cancelled';
            v_meta := jsonb_build_object(
                'return_id', NEW.id, 'return_number', NEW.return_number,
                'reason', NEW.cancellation_reason);
        END IF;
    WHEN 'share_return_entitlements' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'share_return_entitlement_recognized';
            v_meta := jsonb_build_object(
                'entitlement_id', NEW.id,
                'share_return_id', NEW.share_return_id,
                'entitlement_type', NEW.entitlement_type,
                'amount', NEW.amount);
        ELSIF NEW.status = 'cancelled'
           AND OLD.status IS DISTINCT FROM 'cancelled' THEN
            v_type := 'share_return_entitlement_cancelled';
            v_meta := jsonb_build_object(
                'entitlement_id', NEW.id,
                'reason', NEW.cancellation_reason);
        ELSIF OLD.determined_at IS NULL
           AND NEW.determined_at IS NOT NULL THEN
            v_type := 'share_return_entitlement_determined';
            v_meta := jsonb_build_object(
                'entitlement_id', NEW.id, 'amount', NEW.amount,
                'due_date', NEW.due_date);
        END IF;
    WHEN 'share_return_settlements' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'share_return_settlement_posted';
            v_meta := jsonb_build_object(
                'settlement_id', NEW.id,
                'entitlement_id', NEW.entitlement_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'share_return_settlement_reversed';
            v_meta := jsonb_build_object(
                'settlement_id', NEW.id,
                'reason', NEW.reversal_reason);
        END IF;
    WHEN 'social_aid_funds' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'social_aid_fund_created';
            v_meta := jsonb_build_object(
                'fund_id', NEW.id, 'fund_number', NEW.fund_number,
                'name', NEW.name);
        ELSIF NEW.status = 'closed'
           AND OLD.status IS DISTINCT FROM 'closed' THEN
            v_type := 'social_aid_fund_closed';
            v_meta := jsonb_build_object(
                'fund_id', NEW.id, 'fund_number', NEW.fund_number);
        ELSIF NEW.status = 'cancelled'
           AND OLD.status IS DISTINCT FROM 'cancelled' THEN
            v_type := 'social_aid_fund_cancelled';
            v_meta := jsonb_build_object(
                'fund_id', NEW.id, 'fund_number', NEW.fund_number,
                'reason', NEW.cancellation_reason);
        END IF;
    WHEN 'social_aid_donations' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'social_aid_donation_posted';
            v_meta := jsonb_build_object(
                'donation_id', NEW.id, 'fund_id', NEW.fund_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'social_aid_donation_reversed';
            v_meta := jsonb_build_object(
                'donation_id', NEW.id,
                'reason', NEW.reversal_reason);
        END IF;
    WHEN 'social_aid_disbursements' THEN
        IF TG_OP = 'INSERT' AND NEW.status = 'posted' THEN
            v_type := 'social_aid_disbursement_posted';
            v_meta := jsonb_build_object(
                'disbursement_id', NEW.id, 'fund_id', NEW.fund_id,
                'amount', NEW.amount);
        ELSIF NEW.status = 'reversed'
           AND OLD.status IS DISTINCT FROM 'reversed' THEN
            v_type := 'social_aid_disbursement_reversed';
            v_meta := jsonb_build_object(
                'disbursement_id', NEW.id,
                'reason', NEW.reversal_reason);
        END IF;
    WHEN 'governance_bodies' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'governance_body_created';
            v_meta := jsonb_build_object(
                'body_id', NEW.id, 'body_number', NEW.body_number,
                'name', NEW.name);
        ELSIF NEW.status = 'closed'
           AND OLD.status IS DISTINCT FROM 'closed' THEN
            v_type := 'governance_body_closed';
            v_meta := jsonb_build_object(
                'body_id', NEW.id, 'body_number', NEW.body_number);
        END IF;
    WHEN 'governance_memberships' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'governance_membership_started';
            v_meta := jsonb_build_object(
                'membership_id', NEW.id, 'body_id', NEW.body_id,
                'person_id', NEW.person_id, 'title', NEW.title);
        ELSIF OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL THEN
            v_type := 'governance_membership_ended';
            v_meta := jsonb_build_object(
                'membership_id', NEW.id, 'body_id', NEW.body_id,
                'reason', NEW.end_reason);
        END IF;
    WHEN 'governance_decisions' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'governance_decision_created';
            v_meta := jsonb_build_object(
                'decision_id', NEW.id,
                'decision_number', NEW.decision_number,
                'title', NEW.title);
        ELSIF NEW.status = 'open'
           AND OLD.status IS DISTINCT FROM 'open' THEN
            v_type := 'governance_decision_opened';
            v_meta := jsonb_build_object(
                'decision_id', NEW.id,
                'decision_number', NEW.decision_number);
        ELSIF NEW.status IN ('approved','rejected')
           AND OLD.status IS DISTINCT FROM NEW.status
           AND OLD.status NOT IN ('approved','rejected') THEN
            v_type := 'governance_decision_finalized';
            v_meta := jsonb_build_object(
                'decisionId', NEW.id,
                'decision_number', NEW.decision_number,
                'outcome', NEW.status,
                'approve_count', NEW.approve_count,
                'reject_count', NEW.reject_count,
                'abstain_count', NEW.abstain_count);
        ELSIF NEW.status = 'cancelled'
           AND OLD.status IS DISTINCT FROM 'cancelled' THEN
            v_type := 'governance_decision_cancelled';
            v_meta := jsonb_build_object(
                'decision_id', NEW.id,
                'decision_number', NEW.decision_number,
                'reason', NEW.cancellation_reason);
        ELSIF NEW.title IS DISTINCT FROM OLD.title
           OR NEW.decision_text IS DISTINCT FROM OLD.decision_text
           OR NEW.decision_on IS DISTINCT FROM OLD.decision_on
           OR NEW.effective_on IS DISTINCT FROM OLD.effective_on THEN
            v_type := 'governance_decision_updated';
            v_meta := jsonb_build_object(
                'decision_id', NEW.id,
                'decision_number', NEW.decision_number);
        END IF;
    WHEN 'governance_votes' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'governance_vote_recorded';
            v_meta := jsonb_build_object(
                'vote_id', NEW.id, 'decision_id', NEW.decision_id,
                'membership_id', NEW.membership_id,
                'choice', NEW.choice);
        END IF;
    WHEN 'users' THEN
        IF TG_OP = 'UPDATE'
           AND NEW.status IS DISTINCT FROM OLD.status THEN
            v_type := CASE WHEN NEW.status = 'disabled'
                           THEN 'user_disabled' ELSE 'user_enabled' END;
            v_meta := jsonb_build_object('user_id', NEW.id);
        END IF;
    WHEN 'roles' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'role_created';
            v_meta := jsonb_build_object('role_id', NEW.id,
                                         'name', NEW.name,
                                         'cooperative_id', NEW.cooperative_id);
        ELSIF NEW.status IS DISTINCT FROM OLD.status THEN
            v_type := CASE WHEN NEW.status = 'disabled'
                           THEN 'role_disabled' ELSE 'role_enabled' END;
            v_meta := jsonb_build_object('role_id', NEW.id);
        ELSIF NEW.name IS DISTINCT FROM OLD.name
           OR NEW.description IS DISTINCT FROM OLD.description THEN
            v_type := 'role_updated';
            v_meta := jsonb_build_object(
                'role_id', NEW.id,
                'before', jsonb_build_object(
                    'name', OLD.name, 'description', OLD.description),
                'after', jsonb_build_object(
                    'name', NEW.name, 'description', NEW.description));
        END IF;
    WHEN 'user_role_assignments' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'role_assigned';
            v_meta := jsonb_build_object(
                'target_user_id', NEW.user_id,
                'role_id', NEW.role_id,
                'role_name',
                (SELECT name FROM public.roles WHERE id = NEW.role_id),
                'cooperative_id', NEW.cooperative_id);
        ELSIF TG_OP = 'DELETE' THEN
            v_type := 'role_unassigned';
            v_meta := jsonb_build_object(
                'target_user_id', OLD.user_id,
                'role_id', OLD.role_id,
                'role_name',
                (SELECT name FROM public.roles WHERE id = OLD.role_id),
                'cooperative_id', OLD.cooperative_id);
        END IF;
    WHEN 'cooperative_memberships' THEN
        IF TG_OP = 'INSERT' THEN
            -- B2: sealed-lane grants (initial administrator) mint too;
            -- the target cooperative rides in metadata because the
            -- platform actor holds no membership seal there.
            v_type := 'cooperative_membership_granted';
            v_meta := jsonb_build_object(
                'membership_id', NEW.id,
                'target_user_id', NEW.user_id,
                'cooperative_id', NEW.cooperative_id);
        ELSIF TG_OP = 'UPDATE'
           AND NEW.status IS DISTINCT FROM OLD.status THEN
            v_type := CASE NEW.status
                          WHEN 'suspended' THEN 'cooperative_membership_suspended'
                          WHEN 'ended'     THEN 'cooperative_membership_ended'
                          ELSE 'cooperative_membership_reactivated' END;
            v_meta := jsonb_build_object(
                'membership_id', NEW.id,
                'target_user_id', NEW.user_id,
                'cooperative_id', NEW.cooperative_id,
                'from', OLD.status, 'to', NEW.status);
        END IF;
    WHEN 'cooperatives' THEN
        IF TG_OP = 'INSERT' THEN
            v_type := 'cooperative_created';
            v_meta := jsonb_build_object(
                'cooperative_id', NEW.id, 'name', NEW.name,
                'is_bootstrap', NEW.is_bootstrap);
        ELSIF NEW.status IS DISTINCT FROM OLD.status THEN
            v_type := CASE
                WHEN OLD.status = 'provisioning' AND NEW.status = 'active'
                    THEN 'cooperative_activated'
                WHEN OLD.status = 'active' AND NEW.status = 'suspended'
                    THEN 'cooperative_suspended'
                WHEN OLD.status IN ('suspended', 'archived') AND NEW.status = 'active'
                    THEN 'cooperative_reactivated'
                WHEN NEW.status = 'archived'
                    THEN 'cooperative_archived'
                ELSE 'cooperative_status_changed' END;
            v_meta := jsonb_build_object(
                'cooperative_id', NEW.id,
                'from', OLD.status, 'to', NEW.status);
        ELSIF NEW.business_enabled IS DISTINCT FROM OLD.business_enabled THEN
            v_type := 'cooperative_business_gate_changed';
            v_meta := jsonb_build_object(
                'cooperative_id', NEW.id,
                'business_enabled', NEW.business_enabled);
        END IF;
    ELSE
        NULL;
    END CASE;

    IF v_type IS NOT NULL THEN
        PERFORM public.app_audit_emit(v_type, v_meta);
    END IF;
    RETURN NULL;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_audit_transition() FROM PUBLIC;

-- ------------------------------------------------------------------
-- Part B — app_membership_create: cooperative membership creation
-- behind the cooperative-scoped `memberships.manage` grant.
--
-- Lane rules (app_membership_set_status convention):
--   * the cooperative is ALWAYS the sealed verified scope — callers
--     can never name a target cooperative, so cross-cooperative member
--     creation is structurally impossible;
--   * the NULL (platform/pre-tenancy) scope is refused: the permission
--     is cooperative-scoped by contract;
--   * the cooperative must be 'active' — member creation is a business
--     operation and is refused on provisioning/suspended/archived;
--   * the target user must exist and be globally 'active' (same
--     eligibility rule as the activation initial administrator);
--   * an existing row in ANY status raises 'membership_exists' —
--     lifecycle changes belong to app_membership_set_status, the row
--     is UNIQUE per (cooperative,user) and history is preserved;
--   * created_by_user_id is stamped from the sealed actor (NULL on the
--     trusted owner lane, like every other owner writer).
-- Returns the member user id.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_membership_create(
    p_user_id uuid
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope       uuid := public.app_rbac_verified_scope('memberships.manage');
    v_actor       uuid := nullif(current_setting('app.user_id', true), '')::uuid;
    v_coop_status text;
BEGIN
    IF v_scope IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: memberships.manage is cooperative-scoped'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    SELECT c.status INTO v_coop_status
      FROM public.cooperatives c
     WHERE c.id = v_scope;
    IF v_coop_status IS DISTINCT FROM 'active' THEN
        RAISE EXCEPTION 'cooperative_not_ready: %', v_coop_status
            USING ERRCODE = 'check_violation';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM public.users u
         WHERE u.id = p_user_id AND u.status = 'active')
    THEN
        RAISE EXCEPTION 'membership_user_ineligible'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1 FROM public.cooperative_memberships m
         WHERE m.cooperative_id = v_scope AND m.user_id = p_user_id)
    THEN
        RAISE EXCEPTION 'membership_exists'
            USING ERRCODE = 'check_violation';
    END IF;

    INSERT INTO public.cooperative_memberships
        (cooperative_id, user_id, status, created_by_user_id)
    VALUES (v_scope, p_user_id, 'active', v_actor);

    RETURN p_user_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_membership_create(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_membership_create(uuid)
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- Part B (compat) — membership lifecycle accepts the dedicated
-- permission. Existing suspend/reactivate/end callers holding the
-- legacy users.manage grant keep working; memberships.manage holders
-- can now run the same lane without the broader identity permission.
-- The 0060 body is carried verbatim — only the verifier widened.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_membership_set_status(
    p_user_id uuid,
    p_status  text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope     uuid := public.app_rbac_verified_scope_in(
        ARRAY['memberships.manage', 'users.manage']);
    v_had_admin boolean;
    v_id        uuid;
BEGIN
    IF p_status NOT IN ('active', 'suspended', 'ended') THEN
        RAISE EXCEPTION 'invalid membership status: %', p_status
            USING ERRCODE = 'check_violation';
    END IF;

    v_had_admin := EXISTS(
        SELECT 1
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
            AND public.app_rbac_in_scope(r.cooperative_id, v_scope)
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
            AND p.key = 'roles.manage'
          JOIN public.cooperative_memberships m
            ON m.user_id = ura.user_id AND m.cooperative_id = v_scope
           AND m.status = 'active'
         WHERE ura.user_id = p_user_id
           AND public.app_rbac_in_scope(ura.cooperative_id, v_scope));

    UPDATE public.cooperative_memberships cm
       SET status     = p_status,
           ended_at   = CASE WHEN p_status = 'ended' THEN now() ELSE NULL END,
           updated_at = now()
     WHERE cm.user_id = p_user_id
       AND cm.cooperative_id = v_scope
       AND cm.status <> p_status
    RETURNING cm.user_id INTO v_id;

    IF v_id IS NOT NULL
       AND p_status <> 'active'
       AND v_had_admin
       AND public.app_admin_path_count(v_scope) = 0 THEN
        RAISE EXCEPTION 'lockout_prevented'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN v_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_membership_set_status(uuid, text)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_membership_set_status(uuid, text)
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- Postchecks (fail closed inside the migration transaction).
-- ------------------------------------------------------------------
DO $$
BEGIN
    -- Boundary definers are SECURITY DEFINER, never PUBLIC-executable.
    IF EXISTS (
        SELECT 1
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public'
           AND p.proname IN ('app_cooperative_set_status',
                             'app_membership_create',
                             'app_audit_transition')
           AND NOT p.prosecdef)
    THEN
        RAISE EXCEPTION 'B3 postcheck: definer must be SECURITY DEFINER';
    END IF;

    -- The runtime role holds EXECUTE on the two app-lane definers and
    -- nothing else new.
    IF NOT has_function_privilege(
           'kooperatif_app',
           'public.app_membership_create(uuid)', 'EXECUTE')
       OR NOT has_function_privilege(
           'kooperatif_app',
           'public.app_cooperative_set_status(uuid,text)', 'EXECUTE')
    THEN
        RAISE EXCEPTION 'B3 postcheck: definer grant missing';
    END IF;

    -- No PUBLIC execute leaks on the touched definers.
    IF EXISTS (
        SELECT 1
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public'
           AND p.proname IN ('app_membership_create',
                             'app_cooperative_set_status',
                             'app_membership_set_status')
           AND p.proacl::text LIKE '%PUBLIC%')
    THEN
        RAISE EXCEPTION 'B3 postcheck: PUBLIC execute leaked';
    END IF;

    -- app_membership_create must derive scope via the sealed verifier —
    -- a cooperative parameter is structurally impossible (signature is
    -- a single uuid) AND the body must call the scoped verifier.
    IF pg_get_functiondef(
           'public.app_membership_create(uuid)'::regprocedure)
           NOT LIKE '%app_rbac_verified_scope(''memberships.manage'')%'
    THEN
        RAISE EXCEPTION 'B3 postcheck: membership_create must verify memberships.manage scope';
    END IF;

    -- The lifecycle definer must accept memberships.manage alongside
    -- the legacy users.manage grant.
    IF pg_get_functiondef(
           'public.app_membership_set_status(uuid,text)'::regprocedure)
           NOT LIKE '%memberships.manage%'
    THEN
        RAISE EXCEPTION 'B3 postcheck: membership_set_status must accept memberships.manage';
    END IF;

    -- Restoration must now accept archived as a source state for
    -- 'active' and still enforce the administrator invariant.
    IF pg_get_functiondef(
           'public.app_cooperative_set_status(uuid,text)'::regprocedure)
           NOT LIKE '%NOT IN (''suspended'', ''archived'')%'
    THEN
        RAISE EXCEPTION 'B3 postcheck: archived -> active restore arm missing';
    END IF;
END
$$;
