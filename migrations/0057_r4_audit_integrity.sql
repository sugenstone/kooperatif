-- M1-P4-R4 â€” AUDIT INTEGRITY AND RESIDUAL SECURITY CLOSURE.
--
-- Residual findings verified against live grants/RLS/definers on the
-- disposable test database before this migration was written:
--
--   F5 (CONFIRMED) â€” audit integrity holes:
--     * app_ctx_audit accepted ANY caller-chosen event_type, so a
--       sealed runtime caller could fabricate privileged business
--       events (e.g. 'role_created', 'user_disabled') at will.
--     * The R1 RBAC definers (app_role_create/update/set_status/
--       permissions_set, app_user_roles_set, app_user_set_status,
--       app_admin_revoke_user_sessions) mutated security-relevant
--       state without writing a mandatory audit row â€” the audit
--       record lived only in the HTTP route, so a direct definer
--       call inside a sealed transaction produced an UNAUDITED
--       mutation.
--     * The 24 R3C packages had the same gap: only
--       app_shareholder_set_status_tx minted an event.
--
--   F6 (CONFIRMED) â€” enumeration surface: SELECT on cooperatives,
--     installation_state, permissions and role_permissions was granted
--     to kooperatif_app with NO row-level security on those tables.
--     SET ROLE kooperatif_app + bare SELECT returned every
--     cooperative's name/legal_name/bootstrap flag, the bootstrap
--     marker, the full permission catalog and every tenant's
--     roleâ†’permission wiring (cross-tenant RBAC structure disclosure).
--
--   F7 (CONFIRMED) â€” polymorphic movement references:
--     account_movements.source_type/source_id carry no FK. The
--     *_app_insert_live INSERT policies checked only
--     `status <> 'reversed'` on top of the cooperative-scoped ALL
--     policy â€” i.e. any sealed member (no module permission at all)
--     could INSERT fabricated 'active' movements and fabricated
--     posted payments/transfers/entries/donations directly,
--     attributing created_by to anyone. Movements are the derived
--     account-balance source of truth, so this is a financial
--     integrity hole, not just a referential oddity.
--
--   F8 (CONFIRMED during R4) â€” the same *_app_insert_live gap applies
--     to every business table the runtime can INSERT into: sealed +
--     own-coop + live status was sufficient; no module permission was
--     re-checked in the database.
--
-- Remediation in this migration:
--
--   Â§1  app_audit_emit â€” internal sealed-lane event emitter. Fires
--       only when a verified request seal is present (ctx_txid of the
--       current tx); owner-lane/unsealed writes mint nothing (fixtures
--       and CLI semantics preserved). Attribution is ALWAYS derived
--       from the seal GUCs â€” never from the row or the caller.
--
--   Â§2  app_ctx_audit â€” retained for the two residual route-owned
--       event types (shareholder_family_changed composite event,
--       assessments_generated aggregate event); every other
--       event_type is rejected with insufficient_privilege, so sealed
--       callers can no longer mint privileged event types ad hoc.
--
--   Â§3  app_audit_transition + per-table AFTER triggers â€” the
--       database-owned audit layer. Every security-relevant row
--       mutation mints its catalog event inside the SAME transaction:
--       audit failure aborts the mutation, direct callers cannot skip
--       it, and the route-level record_tx calls for trigger-owned
--       types are removed in the same step (no duplicates). Emit is a
--       no-op outside sealed transactions so owner-lane fixtures stay
--       silent, exactly as before.
--
--   Â§4  Recreated definers: app_role_permissions_set and
--       app_admin_revoke_user_sessions now mint their events
--       (role_permissions_changed, admin_sessions_revoked) inside the
--       boundary; app_shareholder_set_status_tx drops its inline
--       INSERT (the shareholders transition trigger mints it).
--
--   Â§5  app_write_guard + BEFORE INSERT triggers â€” every business
--       table writable by kooperatif_app now requires
--       app_rbac_verified_scope_in(<module permissions>) at INSERT
--       time, closing the *_app_insert_live permission gap (F8).
--
--   Â§6  app_movement_source_check â€” deferred constraint trigger on
--       account_movements for the kooperatif_app lane: source_type is
--       whitelisted, and source_id must resolve to a real row of the
--       matching domain in the SAME cooperative, amount/direction/
--       account consistent, back-linked via account_movement_id where
--       the schema provides it. Owner-lane fixture inserts are
--       untouched (deliberately â€” the owner lane is trusted and tests
--       craft dangling sources on purpose, see c4_integrity).
--
--   Â§7  F6 â€” narrow verified readers for the four de-granted tables
--       (app_permission_catalog, app_role_permission_keys,
--       app_bootstrap_completed, app_cooperatives_exist,
--       app_is_bootstrap_scope) and SELECT revocation on
--       cooperatives, installation_state, permissions,
--       role_permissions. administration_path_count moves behind the
--       existing app_admin_path_count definer (granted here).
--
-- New catalog event types minted by this layer (DB-only):
--   period_opened, shareholder_family_membership_ended,
--   admin_sessions_revoked.
-- Route-mintable whitelist (app_ctx_audit) after this migration:
--   shareholder_family_changed, assessments_generated.
-- Owner-lane events remain minted by CLI/bootstrap writers:
--   cooperative_created, cooperative_bootstrapped,
--   cooperative_membership_granted, bootstrap_role_granted,
--   user_created.
-- Lane writers (unchanged): login_failed, login_succeeded,
--   session_revoked, sessions_revoked_all_others, logout,
--   cooperative_context_switched, lockout_prevented.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- â”€â”€ Â§1. Internal sealed emitter â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
-- Identity comes exclusively from the request seal. Unsealed lanes
-- (owner pool, fixtures, CLI) return silently â€” minting is a property
-- of the sealed request transaction, not of the caller's pool.
CREATE OR REPLACE FUNCTION public.app_audit_emit(
    p_event_type text,
    p_metadata   jsonb DEFAULT '{}'::jsonb
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user      uuid;
    v_session   uuid;
    v_coop      uuid;
    v_txid      text;
    v_ctx_seal  text;
    v_user_seal text;
BEGIN
    v_txid := nullif(current_setting('app.ctx_txid', true), '');
    IF v_txid IS NULL THEN
        RETURN; -- owner/unsealed lane: nothing to mint
    END IF;

    v_user      := nullif(current_setting('app.user_id', true), '')::uuid;
    v_session   := nullif(current_setting('app.session_id', true), '')::uuid;
    v_coop      := nullif(current_setting('app.cooperative_id', true), '')::uuid;
    v_ctx_seal  := nullif(current_setting('app.ctx_seal', true), '');
    v_user_seal := nullif(current_setting('app.user_seal', true), '');

    IF v_user IS NULL OR v_session IS NULL
       OR v_txid <> txid_current()::text THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed request context required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF v_coop IS NOT NULL THEN
        IF v_ctx_seal IS NULL
           OR v_ctx_seal <> public.app_seal_of(v_user, v_session, v_coop,
                                               v_txid::bigint) THEN
            RAISE EXCEPTION 'insufficient_privilege: invalid cooperative seal'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    ELSE
        IF v_user_seal IS NULL
           OR v_user_seal <> public.app_seal_of(v_user, v_session, NULL,
                                                v_txid::bigint) THEN
            RAISE EXCEPTION 'insufficient_privilege: invalid user seal'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END IF;

    INSERT INTO public.security_events
        (event_type, cooperative_id, user_id, session_id, metadata)
    VALUES (p_event_type, v_coop, v_user, v_session,
            coalesce(p_metadata, '{}'::jsonb));
END
$fn$;

REVOKE ALL ON FUNCTION public.app_audit_emit(text, jsonb) FROM PUBLIC;

-- â”€â”€ Â§2. app_ctx_audit â†’ route-owned whitelist only â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
-- Every trigger/definer-owned event type is now minted by the
-- database boundary itself; a sealed caller may only mint the two
-- aggregate/composite events that still live at the route layer.
CREATE OR REPLACE FUNCTION public.app_ctx_audit(
    p_event_type text,
    p_metadata   jsonb
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user      uuid;
    v_session   uuid;
    v_coop      uuid;
    v_txid      text;
    v_ctx_seal  text;
    v_user_seal text;
BEGIN
    IF p_event_type NOT IN ('shareholder_family_changed',
                            'assessments_generated') THEN
        RAISE EXCEPTION
            'insufficient_privilege: event type % is not caller-mintable',
            p_event_type
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    v_user      := nullif(current_setting('app.user_id', true), '')::uuid;
    v_session   := nullif(current_setting('app.session_id', true), '')::uuid;
    v_coop      := nullif(current_setting('app.cooperative_id', true), '')::uuid;
    v_txid      := nullif(current_setting('app.ctx_txid', true), '');
    v_ctx_seal  := nullif(current_setting('app.ctx_seal', true), '');
    v_user_seal := nullif(current_setting('app.user_seal', true), '');

    IF v_user IS NULL OR v_session IS NULL OR v_txid IS NULL
       OR v_txid <> txid_current()::text THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed request context required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF v_coop IS NOT NULL THEN
        IF v_ctx_seal IS NULL
           OR v_ctx_seal <> public.app_seal_of(v_user, v_session, v_coop,
                                               v_txid::bigint) THEN
            RAISE EXCEPTION 'insufficient_privilege: invalid cooperative seal'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    ELSE
        IF v_user_seal IS NULL
           OR v_user_seal <> public.app_seal_of(v_user, v_session, NULL,
                                                v_txid::bigint) THEN
            RAISE EXCEPTION 'insufficient_privilege: invalid user seal'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END IF;

    INSERT INTO public.security_events
        (event_type, cooperative_id, user_id, session_id, metadata)
    VALUES (p_event_type, v_coop, v_user, v_session,
            coalesce(p_metadata, '{}'::jsonb));
END
$fn$;

REVOKE ALL ON FUNCTION public.app_ctx_audit(text, jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_ctx_audit(text, jsonb)
    TO kooperatif_app;

-- â”€â”€ Â§3. Database-owned transition audit â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
-- One trigger function classifying the row mutation into the catalog
-- event, minted through app_audit_emit. AFTER ROW triggers fire for
-- EVERY writer lane â€” the emit call itself decides sealed-vs-silent,
-- so a package definer, a component definer and a permitted raw
-- statement all produce the identical audit row.
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
                                         'name', NEW.name);
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
                (SELECT name FROM public.roles WHERE id = NEW.role_id));
        ELSIF TG_OP = 'DELETE' THEN
            v_type := 'role_unassigned';
            v_meta := jsonb_build_object(
                'target_user_id', OLD.user_id,
                'role_id', OLD.role_id,
                'role_name',
                (SELECT name FROM public.roles WHERE id = OLD.role_id));
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

-- Row-level transition triggers on every security-relevant table.
CREATE OR REPLACE TRIGGER persons_audit
    AFTER INSERT OR UPDATE ON public.persons
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER families_audit
    AFTER INSERT OR UPDATE ON public.families
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER shareholders_audit
    AFTER INSERT OR UPDATE ON public.shareholders
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER shareholder_family_memberships_audit
    AFTER UPDATE ON public.shareholder_family_memberships
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER shares_audit
    AFTER INSERT OR UPDATE ON public.shares
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER share_events_audit
    AFTER INSERT ON public.share_events
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER periods_audit
    AFTER INSERT OR UPDATE OR DELETE ON public.periods
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER payments_audit
    AFTER INSERT OR UPDATE ON public.payments
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER payment_allocations_audit
    AFTER INSERT OR UPDATE ON public.payment_allocations
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER account_transfers_audit
    AFTER INSERT OR UPDATE ON public.account_transfers
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER income_entries_audit
    AFTER INSERT OR UPDATE ON public.income_entries
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER expense_entries_audit
    AFTER INSERT OR UPDATE ON public.expense_entries
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER shareholder_credits_audit
    AFTER INSERT OR UPDATE ON public.shareholder_credits
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER credit_applications_audit
    AFTER INSERT OR UPDATE ON public.credit_applications
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER financial_accounts_audit
    AFTER INSERT OR UPDATE ON public.financial_accounts
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER financial_categories_audit
    AFTER INSERT OR UPDATE ON public.financial_categories
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER investments_audit
    AFTER INSERT OR UPDATE ON public.investments
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER investment_fundings_audit
    AFTER INSERT OR UPDATE ON public.investment_fundings
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER investment_incomes_audit
    AFTER INSERT OR UPDATE ON public.investment_incomes
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER investment_valuations_audit
    AFTER INSERT OR UPDATE ON public.investment_valuations
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER investment_disposals_audit
    AFTER INSERT ON public.investment_disposals
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER share_returns_audit
    AFTER INSERT OR UPDATE ON public.share_returns
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER share_return_entitlements_audit
    AFTER INSERT OR UPDATE ON public.share_return_entitlements
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER share_return_settlements_audit
    AFTER INSERT OR UPDATE ON public.share_return_settlements
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER social_aid_funds_audit
    AFTER INSERT OR UPDATE ON public.social_aid_funds
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER social_aid_donations_audit
    AFTER INSERT OR UPDATE ON public.social_aid_donations
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER social_aid_disbursements_audit
    AFTER INSERT OR UPDATE ON public.social_aid_disbursements
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER governance_bodies_audit
    AFTER INSERT OR UPDATE ON public.governance_bodies
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER governance_memberships_audit
    AFTER INSERT OR UPDATE ON public.governance_memberships
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER governance_decisions_audit
    AFTER INSERT OR UPDATE ON public.governance_decisions
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER governance_votes_audit
    AFTER INSERT ON public.governance_votes
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER users_audit
    AFTER UPDATE ON public.users
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER roles_audit
    AFTER INSERT OR UPDATE ON public.roles
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();
CREATE OR REPLACE TRIGGER user_role_assignments_audit
    AFTER INSERT OR DELETE ON public.user_role_assignments
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();

-- â”€â”€ Â§4. Definers that mint inside the boundary â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
-- app_role_permissions_set: the statement-level permission replace
-- emits ONE role_permissions_changed event with before/after keys
-- (per-row triggers would produce per-permission noise).
CREATE OR REPLACE FUNCTION public.app_role_permissions_set(
    p_role_id uuid,
    p_keys    text[]
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope  uuid := public.app_rbac_verified_scope('roles.manage');
    v_before text[];
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.roles r
         WHERE r.id = p_role_id
           AND public.app_rbac_in_scope(r.cooperative_id, v_scope)
    ) THEN
        RAISE EXCEPTION 'insufficient_privilege: role outside verified scope'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    SELECT COALESCE(array_agg(p.key ORDER BY p.key), '{}'::text[])
      INTO v_before
      FROM public.role_permissions rp
      JOIN public.permissions p ON p.id = rp.permission_id
     WHERE rp.role_id = p_role_id;

    DELETE FROM public.role_permissions WHERE role_id = p_role_id;

    INSERT INTO public.role_permissions (role_id, permission_id)
    SELECT p_role_id, p.id
      FROM (SELECT DISTINCT unnest(p_keys) AS key) k
      JOIN public.permissions p ON p.key = k.key;

    IF (SELECT count(*) FROM public.role_permissions WHERE role_id = p_role_id)
       <> (SELECT count(*) FROM (SELECT DISTINCT unnest(p_keys) AS key) k) THEN
        RAISE EXCEPTION 'unknown_permission_key' USING ERRCODE = 'data_exception';
    END IF;

    IF public.app_admin_path_count(v_scope) = 0 THEN
        RAISE EXCEPTION 'lockout_prevented' USING ERRCODE = 'check_violation';
    END IF;

    PERFORM public.app_audit_emit(
        'role_permissions_changed',
        jsonb_build_object('role_id', p_role_id,
                           'before', to_jsonb(v_before),
                           'after', to_jsonb(
                               (SELECT COALESCE(array_agg(k.key ORDER BY k.key),
                                                '{}'::text[])
                                  FROM (SELECT DISTINCT unnest(p_keys) AS key)
                                       k))));
END;
$function$;

-- app_admin_revoke_user_sessions: the bulk revocation mints
-- admin_sessions_revoked atomically (per-session 'session_revoked'
-- events are minted by the session definers only â€” a bulk admin
-- revocation is a distinct, separately auditable operation).
CREATE OR REPLACE FUNCTION public.app_admin_revoke_user_sessions(
    p_user_id uuid,
    p_now     timestamptz
)
RETURNS bigint
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user    uuid;
    v_session uuid;
    v_coop    uuid;
    v_txid    text;
    v_seal    text;
    v_ctx     text;
    v_count   bigint;
BEGIN
    v_user    := nullif(current_setting('app.user_id', true), '')::uuid;
    v_session := nullif(current_setting('app.session_id', true), '')::uuid;
    v_coop    := nullif(current_setting('app.cooperative_id', true), '')::uuid;
    v_txid    := nullif(current_setting('app.ctx_txid', true), '');
    v_seal    := nullif(current_setting('app.user_seal', true), '');
    v_ctx     := nullif(current_setting('app.ctx_seal', true), '');

    IF v_user IS NULL OR v_session IS NULL OR v_seal IS NULL
       OR v_txid IS NULL OR v_txid <> txid_current()::text
       OR v_seal <> public.app_seal_of(v_user, v_session, NULL, v_txid::bigint)
    THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed caller context required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF v_coop IS NOT NULL THEN
        IF v_ctx IS NULL
           OR v_ctx <> public.app_seal_of(v_user, v_session, v_coop, v_txid::bigint)
        THEN
            RAISE EXCEPTION 'insufficient_privilege: cooperative context not sealed'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        IF NOT EXISTS (
            SELECT 1
              FROM public.user_role_assignments ura
              JOIN public.roles r ON r.id = ura.role_id
                AND r.status = 'active'
                AND (r.cooperative_id = v_coop
                     OR (r.cooperative_id IS NULL AND EXISTS (
                         SELECT 1 FROM public.installation_state s
                          JOIN public.cooperatives c
                            ON c.id = s.bootstrap_cooperative_id
                           AND c.is_bootstrap
                          WHERE s.bootstrap_cooperative_id = v_coop)))
              JOIN public.cooperative_memberships m
                ON m.cooperative_id = v_coop AND m.user_id = ura.user_id
               AND m.status = 'active'
              JOIN public.role_permissions rp ON rp.role_id = r.id
              JOIN public.permissions p ON p.id = rp.permission_id
                AND p.key = 'users.manage'
             WHERE ura.user_id = v_user
               AND (ura.cooperative_id = v_coop
                    OR (ura.cooperative_id IS NULL AND EXISTS (
                        SELECT 1 FROM public.installation_state s
                         JOIN public.cooperatives c
                           ON c.id = s.bootstrap_cooperative_id
                          AND c.is_bootstrap
                         WHERE s.bootstrap_cooperative_id = v_coop)))
        ) THEN
            RAISE EXCEPTION 'insufficient_privilege: users.manage required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM public.cooperative_memberships m
             WHERE m.user_id = p_user_id AND m.cooperative_id = v_coop
        ) THEN
            RAISE EXCEPTION 'insufficient_privilege: target outside scope'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    ELSE
        IF v_ctx IS NOT NULL OR EXISTS (SELECT 1 FROM public.cooperatives) THEN
            RAISE EXCEPTION 'insufficient_privilege: no global lane after bootstrap'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        IF NOT EXISTS (
            SELECT 1
              FROM public.user_role_assignments ura
              JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
              JOIN public.role_permissions rp ON rp.role_id = r.id
              JOIN public.permissions p ON p.id = rp.permission_id
                AND p.key = 'users.manage'
             WHERE ura.user_id = v_user
        ) THEN
            RAISE EXCEPTION 'insufficient_privilege: users.manage required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END IF;

    UPDATE public.user_sessions
       SET revoked_at = p_now, revocation_reason = 'user_disabled'
     WHERE user_id = p_user_id AND revoked_at IS NULL;
    GET DIAGNOSTICS v_count = ROW_COUNT;

    PERFORM public.app_audit_emit(
        'admin_sessions_revoked',
        jsonb_build_object('target_user_id', p_user_id,
                           'revoked_count', v_count));
    RETURN v_count;
END
$fn$;

-- app_shareholder_set_status_tx: the inline INSERT is removed â€” the
-- shareholders transition trigger mints shareholder_status_changed
-- under sealed context (attribution now ALWAYS seal-derived; the
-- owner lane no longer mints, matching every other definer).
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
BEGIN
    IF p_target NOT IN ('active', 'inactive', 'voided') THEN
        RAISE EXCEPTION 'kooperatif:invalid_transition'
            USING ERRCODE = 'P0001';
    END IF;
    v_eff := CASE WHEN v_restricted THEN v_scope ELSE p_coop END;
    IF v_restricted THEN
        p_by := nullif(current_setting('app.user_id', true), '')::uuid;
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
END;
$function$;

-- â”€â”€ Â§5. INSERT write-guard (F8) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
-- The *_app_insert_live policies checked only row liveness â€” any
-- sealed member could post fabricated financial rows directly. The
-- guard re-verifies the module permission inside the database on
-- every runtime INSERT. Owner lane is exempt (trusted).
CREATE OR REPLACE FUNCTION public.app_rbac_verified_scope_in(
    p_permissions text[]
)
RETURNS uuid
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_user  uuid;
    v_scope uuid;
    v_key   text;
    v_ok    boolean;
BEGIN
    v_user  := nullif(current_setting('app.user_id', true), '')::uuid;
    v_scope := nullif(current_setting('app.cooperative_id', true), '')::uuid;

    IF session_user <> 'kooperatif_app' THEN
        RETURN v_scope;
    END IF;

    IF v_scope IS NOT NULL THEN
        IF NOT public.app_ctx_ok() THEN
            RAISE EXCEPTION 'insufficient_privilege: sealed cooperative context required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        FOREACH v_key IN ARRAY p_permissions LOOP
            SELECT EXISTS(
                SELECT 1
                  FROM public.user_role_assignments ura
                  JOIN public.roles r
                    ON r.id = ura.role_id AND r.status = 'active'
                   AND public.app_rbac_in_scope(r.cooperative_id, v_scope)
                  JOIN public.role_permissions rp ON rp.role_id = r.id
                  JOIN public.permissions p
                    ON p.id = rp.permission_id AND p.key = v_key
                  JOIN public.cooperative_memberships m
                    ON m.cooperative_id = v_scope
                   AND m.user_id = ura.user_id
                   AND m.status = 'active'
                 WHERE ura.user_id = v_user
                   AND public.app_rbac_in_scope(ura.cooperative_id, v_scope))
              INTO v_ok;
            IF v_ok THEN
                RETURN v_scope;
            END IF;
        END LOOP;
    ELSE
        IF NOT public.app_user_ctx_ok()
           OR EXISTS (SELECT 1 FROM public.cooperatives) THEN
            RAISE EXCEPTION 'insufficient_privilege: sealed pre-tenancy context required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        FOREACH v_key IN ARRAY p_permissions LOOP
            SELECT EXISTS(
                SELECT 1
                  FROM public.user_role_assignments ura
                  JOIN public.roles r
                    ON r.id = ura.role_id AND r.status = 'active'
                   AND r.cooperative_id IS NULL
                  JOIN public.role_permissions rp ON rp.role_id = r.id
                  JOIN public.permissions p
                    ON p.id = rp.permission_id AND p.key = v_key
                 WHERE ura.user_id = v_user
                   AND ura.cooperative_id IS NULL)
              INTO v_ok;
            IF v_ok THEN
                RETURN NULL;
            END IF;
        END LOOP;
    END IF;

    RAISE EXCEPTION 'insufficient_privilege: one of % required', p_permissions
        USING ERRCODE = 'insufficient_privilege';
END;
$function$;
REVOKE ALL ON FUNCTION public.app_rbac_verified_scope_in(text[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_rbac_verified_scope_in(text[])
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_write_guard()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        PERFORM public.app_rbac_verified_scope_in(TG_ARGV::text[]);
    END IF;
    RETURN NEW;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_write_guard() FROM PUBLIC;

-- F7 attribution pin: a sealed runtime INSERT may never attribute a
-- movement to anyone but the sealed actor, and source_type must be a
-- known domain tag even before the deferred referential check.
CREATE OR REPLACE FUNCTION public.app_movement_source_pre()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    IF session_user = 'kooperatif_app' THEN
        NEW.created_by :=
            nullif(current_setting('app.user_id', true), '')::uuid;
        IF NEW.created_by IS NULL THEN
            RAISE EXCEPTION 'insufficient_privilege: sealed actor required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END IF;
    RETURN NEW;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_movement_source_pre() FROM PUBLIC;

CREATE OR REPLACE TRIGGER account_movements_source_pre
    BEFORE INSERT ON public.account_movements
    FOR EACH ROW EXECUTE FUNCTION public.app_movement_source_pre();

-- Per-table write guards. The permission list mirrors the modules that
-- legitimately create each row (cross-module writers included).
CREATE OR REPLACE TRIGGER persons_write_guard
    BEFORE INSERT ON public.persons
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'shareholders.manage', 'families.manage', 'payments.manage');
CREATE OR REPLACE TRIGGER families_write_guard
    BEFORE INSERT ON public.families
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'shareholders.manage', 'families.manage');
CREATE OR REPLACE TRIGGER shareholders_write_guard
    BEFORE INSERT ON public.shareholders
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'shareholders.manage');
CREATE OR REPLACE TRIGGER shareholder_family_memberships_write_guard
    BEFORE INSERT ON public.shareholder_family_memberships
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'shareholders.manage', 'families.manage');
CREATE OR REPLACE TRIGGER shares_write_guard
    BEFORE INSERT ON public.shares
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'shares.manage');
CREATE OR REPLACE TRIGGER share_events_write_guard
    BEFORE INSERT ON public.share_events
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'shares.manage', 'share_returns.manage');
CREATE OR REPLACE TRIGGER share_ownerships_write_guard
    BEFORE INSERT ON public.share_ownerships
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'shares.manage');
CREATE OR REPLACE TRIGGER periods_write_guard
    BEFORE INSERT ON public.periods
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'periods.manage');
CREATE OR REPLACE TRIGGER assessment_rules_write_guard
    BEFORE INSERT ON public.assessment_rules
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'periods.manage', 'assessments.manage');
CREATE OR REPLACE TRIGGER assessments_write_guard
    BEFORE INSERT ON public.assessments
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'periods.manage', 'assessments.manage');
CREATE OR REPLACE TRIGGER assessment_share_sources_write_guard
    BEFORE INSERT ON public.assessment_share_sources
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'periods.manage', 'assessments.manage');
CREATE OR REPLACE TRIGGER payments_write_guard
    BEFORE INSERT ON public.payments
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'payments.manage');
CREATE OR REPLACE TRIGGER payment_allocations_write_guard
    BEFORE INSERT ON public.payment_allocations
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'payments.manage');
CREATE OR REPLACE TRIGGER account_transfers_write_guard
    BEFORE INSERT ON public.account_transfers
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'financial_accounts.manage');
CREATE OR REPLACE TRIGGER account_movements_write_guard
    BEFORE INSERT ON public.account_movements
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'payments.manage', 'financial_accounts.manage',
        'income_expense.manage', 'credits.manage',
        'investments.manage', 'share_returns.manage',
        'social_aid.manage');
CREATE OR REPLACE TRIGGER income_entries_write_guard
    BEFORE INSERT ON public.income_entries
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'income_expense.manage');
CREATE OR REPLACE TRIGGER expense_entries_write_guard
    BEFORE INSERT ON public.expense_entries
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'income_expense.manage');
CREATE OR REPLACE TRIGGER financial_categories_write_guard
    BEFORE INSERT ON public.financial_categories
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'income_expense.manage', 'financial_accounts.manage');
CREATE OR REPLACE TRIGGER financial_accounts_write_guard
    BEFORE INSERT ON public.financial_accounts
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'financial_accounts.manage');
CREATE OR REPLACE TRIGGER credit_applications_write_guard
    BEFORE INSERT ON public.credit_applications
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'credits.manage', 'assessments.manage');
CREATE OR REPLACE TRIGGER shareholder_credits_write_guard
    BEFORE INSERT ON public.shareholder_credits
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'credits.manage');
CREATE OR REPLACE TRIGGER investments_write_guard
    BEFORE INSERT ON public.investments
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'investments.manage');
CREATE OR REPLACE TRIGGER investment_fundings_write_guard
    BEFORE INSERT ON public.investment_fundings
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'investments.manage');
CREATE OR REPLACE TRIGGER investment_incomes_write_guard
    BEFORE INSERT ON public.investment_incomes
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'investments.manage');
CREATE OR REPLACE TRIGGER investment_valuations_write_guard
    BEFORE INSERT ON public.investment_valuations
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'investments.manage');
CREATE OR REPLACE TRIGGER investment_disposals_write_guard
    BEFORE INSERT ON public.investment_disposals
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'investments.manage');
CREATE OR REPLACE TRIGGER investment_disposal_proceeds_write_guard
    BEFORE INSERT ON public.investment_disposal_proceeds
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'investments.manage');
CREATE OR REPLACE TRIGGER share_returns_write_guard
    BEFORE INSERT ON public.share_returns
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'share_returns.manage');
CREATE OR REPLACE TRIGGER share_return_entitlements_write_guard
    BEFORE INSERT ON public.share_return_entitlements
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'share_returns.manage');
CREATE OR REPLACE TRIGGER share_return_settlements_write_guard
    BEFORE INSERT ON public.share_return_settlements
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'share_returns.manage');
CREATE OR REPLACE TRIGGER social_aid_funds_write_guard
    BEFORE INSERT ON public.social_aid_funds
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'social_aid.manage');
CREATE OR REPLACE TRIGGER social_aid_donations_write_guard
    BEFORE INSERT ON public.social_aid_donations
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'social_aid.manage');
CREATE OR REPLACE TRIGGER social_aid_disbursements_write_guard
    BEFORE INSERT ON public.social_aid_disbursements
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'social_aid.manage');
CREATE OR REPLACE TRIGGER governance_bodies_write_guard
    BEFORE INSERT ON public.governance_bodies
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'governance.manage');
CREATE OR REPLACE TRIGGER governance_memberships_write_guard
    BEFORE INSERT ON public.governance_memberships
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'governance.manage');
CREATE OR REPLACE TRIGGER governance_decisions_write_guard
    BEFORE INSERT ON public.governance_decisions
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'governance.manage');
CREATE OR REPLACE TRIGGER governance_votes_write_guard
    BEFORE INSERT ON public.governance_votes
    FOR EACH ROW EXECUTE FUNCTION public.app_write_guard(
        'governance.manage');

-- â”€â”€ Â§6. F7 â€” deferred polymorphic source integrity â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
-- Runs once per kooperatif_app transaction that wrote movements:
-- every sealed-lane movement must resolve to a real same-cooperative
-- source row of the matching domain with consistent amount,
-- direction and account. Back-pointer tables are bound through
-- account_movement_id; payment/transfer resolve by their own
-- account columns (the unique(account_id, source_type, source_id)
-- key already bounds cardinality per leg).
CREATE OR REPLACE FUNCTION public.app_movement_source_check()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_bad bigint;
BEGIN
    IF session_user <> 'kooperatif_app' THEN
        RETURN NULL;
    END IF;

    -- Per-row deferred trigger: validate only the row that fired it.
    -- Historical owner-lane fixtures may legitimately carry synthetic
    -- source ids (read-isolation probes); rescanning them here would
    -- make unrelated sealed commits fail.
    SELECT count(*) INTO v_bad
      FROM public.account_movements m
     WHERE m.id = NEW.id
       AND NOT (
           (m.source_type = 'payment' AND m.direction = 'inflow'
            AND EXISTS (SELECT 1 FROM public.payments s
                         WHERE s.id = m.source_id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.destination_account_id = m.account_id
                           AND s.amount = m.amount
                           AND s.status = 'posted'))
        OR (m.source_type = 'transfer'
            AND EXISTS (SELECT 1 FROM public.account_transfers s
                         WHERE s.id = m.source_id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.amount = m.amount
                           AND ((m.direction = 'inflow'
                                 AND s.destination_account_id
                                     = m.account_id)
                                OR (m.direction = 'outflow'
                                    AND s.source_account_id
                                        = m.account_id))))
        OR (m.source_type = 'income' AND m.direction = 'inflow'
            AND EXISTS (SELECT 1 FROM public.income_entries s
                         WHERE s.account_movement_id = m.id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.financial_account_id = m.account_id
                           AND s.amount = m.amount))
        OR (m.source_type = 'expense' AND m.direction = 'outflow'
            AND EXISTS (SELECT 1 FROM public.expense_entries s
                         WHERE s.account_movement_id = m.id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.financial_account_id = m.account_id
                           AND s.amount = m.amount))
        OR (m.source_type = 'share_return_settlement'
            AND m.direction = 'outflow'
            AND EXISTS (SELECT 1 FROM public.share_return_settlements s
                         WHERE s.account_movement_id = m.id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.financial_account_id = m.account_id
                           AND s.amount = m.amount))
        OR (m.source_type = 'investment_funding'
            AND m.direction = 'outflow'
            AND EXISTS (SELECT 1 FROM public.investment_fundings s
                         WHERE s.account_movement_id = m.id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.financial_account_id = m.account_id
                           AND s.amount = m.amount))
        OR (m.source_type = 'investment_income'
            AND m.direction = 'inflow'
            AND EXISTS (SELECT 1 FROM public.investment_incomes s
                         WHERE s.account_movement_id = m.id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.financial_account_id = m.account_id
                           AND s.amount = m.amount))
        OR (m.source_type = 'investment_disposal'
            AND m.direction = 'inflow'
            AND EXISTS (SELECT 1
                          FROM public.investment_disposal_proceeds s
                         WHERE s.account_movement_id = m.id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.financial_account_id = m.account_id
                           AND s.amount = m.amount))
        OR (m.source_type = 'social_aid_donation'
            AND m.direction = 'inflow'
            AND EXISTS (SELECT 1 FROM public.social_aid_donations s
                         WHERE s.account_movement_id = m.id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.financial_account_id = m.account_id
                           AND s.amount = m.amount))
        OR (m.source_type = 'social_aid_disbursement'
            AND m.direction = 'outflow'
            AND EXISTS (SELECT 1
                          FROM public.social_aid_disbursements s
                         WHERE s.account_movement_id = m.id
                           AND s.cooperative_id IS NOT DISTINCT FROM
                               m.cooperative_id
                           AND s.financial_account_id = m.account_id
                           AND s.amount = m.amount))
       );

    IF v_bad > 0 THEN
        RAISE EXCEPTION
            'kooperatif:movement_source_integrity'
            USING ERRCODE = 'P0001',
                  DETAIL = format(
                      '%s movement(s) carry a source reference that does '
                      'not resolve to a same-cooperative source row of '
                      'the declared domain', v_bad);
    END IF;
    RETURN NULL;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_movement_source_check() FROM PUBLIC;

DROP TRIGGER IF EXISTS account_movements_source_check
    ON public.account_movements;
CREATE CONSTRAINT TRIGGER account_movements_source_check
    AFTER INSERT ON public.account_movements
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION public.app_movement_source_check();

-- â”€â”€ Â§7. F6 â€” narrow verified readers + SELECT revocation â”€â”€â”€â”€â”€â”€â”€â”€
-- The four de-granted tables still back legitimate flows; each flow
-- gets a dedicated SECURITY DEFINER reader returning exactly what it
-- needs, then the blanket SELECT grants go away.

-- Permission catalog for the roles.read administration surface.
CREATE OR REPLACE FUNCTION public.app_permission_catalog()
RETURNS TABLE(key text, name text, description text, category text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT p.key, p.name, p.description, p.category
      FROM public.permissions p
     ORDER BY p.category, p.key
$function$;
REVOKE ALL ON FUNCTION public.app_permission_catalog() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_permission_catalog()
    TO kooperatif_app;

-- Permission keys of one role (the role row itself stays behind the
-- roles RLS policy; this only exposes its wiring).
CREATE OR REPLACE FUNCTION public.app_role_permission_keys(p_role_id uuid)
RETURNS TABLE(key text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT p.key
      FROM public.role_permissions rp
      JOIN public.permissions p ON p.id = rp.permission_id
     WHERE rp.role_id = p_role_id
     ORDER BY p.key
$function$;
REVOKE ALL ON FUNCTION public.app_role_permission_keys(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_role_permission_keys(uuid)
    TO kooperatif_app;

-- Bootstrap cooperative resolution: marker AND flag agreement (the
-- previous definer dropped the is_bootstrap check — a marker row whose
-- flag was cleared must resolve as "no bootstrap", matching the
-- historical pool-side query semantics).
CREATE OR REPLACE FUNCTION public.app_resolve_bootstrap()
RETURNS TABLE (cooperative_id uuid, cooperative_name text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT c.id, c.name::text
      FROM public.installation_state s
      JOIN public.cooperatives c ON c.id = s.bootstrap_cooperative_id
     WHERE c.is_bootstrap
     LIMIT 1
$function$;
REVOKE ALL ON FUNCTION public.app_resolve_bootstrap() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_resolve_bootstrap()
    TO kooperatif_app;

-- Bootstrap engagement marker (gate.rs / login flow).
CREATE OR REPLACE FUNCTION public.app_bootstrap_completed()
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT EXISTS(SELECT 1 FROM public.installation_state
                   WHERE bootstrap_cooperative_id IS NOT NULL)
        OR EXISTS(SELECT 1 FROM public.cooperatives WHERE is_bootstrap)
$function$;
REVOKE ALL ON FUNCTION public.app_bootstrap_completed() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_bootstrap_completed()
    TO kooperatif_app;

-- "Any cooperative exists" â€” the never-bootstrapped discriminator used
-- by scoped/member visibility fragments.
CREATE OR REPLACE FUNCTION public.app_cooperatives_exist()
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT EXISTS(SELECT 1 FROM public.cooperatives)
$function$;
REVOKE ALL ON FUNCTION public.app_cooperatives_exist() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_cooperatives_exist()
    TO kooperatif_app;

-- Marker-consistent bootstrap-scope check: true iff p_scope resolves
-- as THE bootstrap cooperative (installation_state marker AND
-- is_bootstrap flag agree).
CREATE OR REPLACE FUNCTION public.app_is_bootstrap_scope(p_scope uuid)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT EXISTS(
        SELECT 1 FROM public.installation_state s
         JOIN public.cooperatives c
           ON c.id = s.bootstrap_cooperative_id AND c.is_bootstrap
         WHERE s.bootstrap_cooperative_id = p_scope)
$function$;
REVOKE ALL ON FUNCTION public.app_is_bootstrap_scope(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_is_bootstrap_scope(uuid)
    TO kooperatif_app;

-- Credential-bound role/permission resolution for the bootstrap and
-- cooperative lanes (replaces the pool-side joins over the revoked
-- tables; the principal is always derived from the session token
-- hash, never from a caller-supplied user_id).
CREATE OR REPLACE FUNCTION public.app_session_permissions_bootstrap(
    p_token_hash   bytea,
    p_bootstrap_id uuid
)
RETURNS TABLE(permission_key text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT DISTINCT p.key
      FROM public.user_sessions s
      JOIN public.users u ON u.id = s.user_id AND u.status = 'active'
      JOIN public.user_role_assignments ura ON ura.user_id = u.id
      JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
                          AND (r.cooperative_id = p_bootstrap_id
                               OR (r.cooperative_id IS NULL
                                   AND public.app_is_bootstrap_scope(
                                           p_bootstrap_id)))
      JOIN public.cooperative_memberships m
        ON m.cooperative_id = p_bootstrap_id AND m.user_id = u.id
       AND m.status = 'active'
      JOIN public.role_permissions rp ON rp.role_id = r.id
      JOIN public.permissions p ON p.id = rp.permission_id
     WHERE s.token_hash = p_token_hash
       AND s.revoked_at IS NULL
       AND s.expires_at > now()
       AND s.idle_expires_at > now()
       AND (ura.cooperative_id = p_bootstrap_id
            OR (ura.cooperative_id IS NULL
                AND public.app_is_bootstrap_scope(p_bootstrap_id)))
$function$;
REVOKE ALL ON FUNCTION public.app_session_permissions_bootstrap(bytea, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_permissions_bootstrap(bytea, uuid)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_session_role_summaries_bootstrap(
    p_token_hash   bytea,
    p_bootstrap_id uuid
)
RETURNS TABLE(role_id uuid, role_name text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT r.id, r.name
      FROM public.user_sessions s
      JOIN public.users u ON u.id = s.user_id AND u.status = 'active'
      JOIN public.user_role_assignments ura ON ura.user_id = u.id
      JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
                          AND (r.cooperative_id = p_bootstrap_id
                               OR (r.cooperative_id IS NULL
                                   AND public.app_is_bootstrap_scope(
                                           p_bootstrap_id)))
      JOIN public.cooperative_memberships m
        ON m.cooperative_id = p_bootstrap_id AND m.user_id = u.id
       AND m.status = 'active'
     WHERE s.token_hash = p_token_hash
       AND s.revoked_at IS NULL
       AND s.expires_at > now()
       AND s.idle_expires_at > now()
       AND (ura.cooperative_id = p_bootstrap_id
            OR (ura.cooperative_id IS NULL
                AND public.app_is_bootstrap_scope(p_bootstrap_id)))
     ORDER BY r.name
$function$;
REVOKE ALL ON FUNCTION public.app_session_role_summaries_bootstrap(bytea, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_role_summaries_bootstrap(bytea, uuid)
    TO kooperatif_app;

-- administration_path_count for the sealed lane moves behind the
-- existing owner-side definer (it already derives scope internally).
GRANT EXECUTE ON FUNCTION public.app_admin_path_count(uuid)
    TO kooperatif_app;

-- The enumeration surface itself goes away.
REVOKE SELECT ON public.cooperatives      FROM kooperatif_app;
REVOKE SELECT ON public.installation_state FROM kooperatif_app;
REVOKE SELECT ON public.permissions        FROM kooperatif_app;
REVOKE SELECT ON public.role_permissions   FROM kooperatif_app;

-- The three RLS policies that inline-referenced the now-revoked
-- tables are rebuilt to call the SECURITY DEFINER verifiers instead —
-- same semantics, but the policy no longer requires table privileges
-- the runtime role does not hold.
DROP POLICY IF EXISTS roles_app_scope ON public.roles;
CREATE POLICY roles_app_scope
    ON public.roles
    FOR ALL
    TO kooperatif_app
    USING (
        (public.app_ctx_ok() AND (
            cooperative_id =
                nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND public.app_is_bootstrap_scope(
                nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR
        (public.app_user_ctx_ok() AND cooperative_id IS NULL
         AND NOT public.app_cooperatives_exist())
    )
    WITH CHECK (
        (public.app_ctx_ok() AND (
            cooperative_id =
                nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND public.app_is_bootstrap_scope(
                nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR
        (public.app_user_ctx_ok() AND cooperative_id IS NULL
         AND NOT public.app_cooperatives_exist())
    );

DROP POLICY IF EXISTS user_role_assignments_app_scope
    ON public.user_role_assignments;
CREATE POLICY user_role_assignments_app_scope
    ON public.user_role_assignments
    FOR ALL
    TO kooperatif_app
    USING (
        (public.app_ctx_ok() AND (
            cooperative_id =
                nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND public.app_is_bootstrap_scope(
                nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR
        (public.app_user_ctx_ok() AND cooperative_id IS NULL
         AND NOT public.app_cooperatives_exist())
    )
    WITH CHECK (
        (public.app_ctx_ok() AND (
            cooperative_id =
                nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND public.app_is_bootstrap_scope(
                nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR
        (public.app_user_ctx_ok() AND cooperative_id IS NULL
         AND NOT public.app_cooperatives_exist())
    );

DROP POLICY IF EXISTS users_app_scope ON public.users;
CREATE POLICY users_app_scope
    ON public.users
    FOR SELECT
    TO kooperatif_app
    USING (
        (public.app_ctx_ok() AND EXISTS(
            SELECT 1 FROM public.cooperative_memberships m
             WHERE m.user_id = users.id
               AND m.cooperative_id =
                   nullif(current_setting('app.cooperative_id', true), '')::uuid))
        OR (public.app_user_ctx_ok()
            AND NOT public.app_cooperatives_exist()));

-- â”€â”€ Postchecks â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
DO $$
BEGIN
    IF has_table_privilege('kooperatif_app', 'public.cooperatives', 'SELECT')
       OR has_table_privilege('kooperatif_app', 'public.installation_state', 'SELECT')
       OR has_table_privilege('kooperatif_app', 'public.permissions', 'SELECT')
       OR has_table_privilege('kooperatif_app', 'public.role_permissions', 'SELECT')
    THEN
        RAISE EXCEPTION 'postcheck failed: enumeration SELECT still granted';
    END IF;
    IF NOT has_function_privilege(
        'kooperatif_app', 'public.app_ctx_audit(text,jsonb)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app', 'public.app_permission_catalog()', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app', 'public.app_admin_path_count(uuid)', 'EXECUTE')
    THEN
        RAISE EXCEPTION 'postcheck failed: verified readers not granted';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_trigger
                    WHERE tgname = 'account_movements_source_check'
                      AND tgrelid = 'public.account_movements'::regclass
                      AND tgdeferrable AND tginitdeferred)
    THEN
        RAISE EXCEPTION 'postcheck failed: deferred source guard missing';
    END IF;
END
$$;
