-- M1-P6 BATCH B2 — platform cooperative lifecycle.
--
-- Approved rules (K2–K5):
--   * Cooperative lifecycle (create/activate/suspend/archive/
--     reactivate) is PLATFORM authority only — a NULL-scope
--     `cooperatives.manage` grant held through an active NULL-scope
--     role + assignment. Cooperative-scoped grants NEVER satisfy it;
--     cooperative administrators manage only their own cooperative and
--     cannot reach these paths.
--   * New cooperatives start in `provisioning` with
--     `business_enabled = false` (0018 CHECK already permits the four
--     states; no schema change needed).
--   * Activation is one atomic package: initial administrator
--     membership + cooperative-scoped administrator role assignment +
--     status/business_enabled transition succeed or fail together.
--   * Suspension blocks business operations while preserving ALL
--     historical/financial rows (status flip only — never deletion).
--   * Reactivation validates the effective-administrator invariant
--     (app_admin_path_count) before business reopens.
--   * Archive is a one-way terminal transition from
--     provisioning/active/suspended; no outbound product path is
--     invented (semantics pending a product decision — reported).
--   * The bootstrap cooperative can be suspended/reactivated
--     (recoverable via the platform lane) but can NEVER be archived —
--     archiving the only business cooperative would brick the
--     installation with no in-product recovery.
--
-- Deliverables:
--   §1 app_has_platform_permission     — NULL-scope grant predicate
--   §2 app_cooperative_create          — sealed platform creation
--   §3 app_cooperative_activate        — atomic activation package
--   §4 app_cooperative_set_status      — suspend/reactivate/archive
--   §5 app_cooperative_list_platform   — platform read model
--   §6 audit: cooperatives branch + cooperative_memberships INSERT
--   §7 realtime: rt_authz_cooperatives — suspension/reactivation
--        emits a signed `authz` signal so bound sockets revalidate and
--        close (verify_business_membership rejects non-active coops).
--   §8 dedicated least-privilege platform role seed
--   §9 postchecks
--
-- Audit attribution: lifecycle mutations mint through the sealed
-- request context — actor = session owner, cooperative_id = NULL
-- (platform event); the TARGET cooperative rides in metadata
-- (platform actors hold no membership seal in the target coop, so the
-- emitter cannot and must not attribute the row to it).
--
-- Append-only. No historical migration is modified.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── §1. Generic NULL-scope permission predicate ─────────────────────
-- Mirrors app_is_platform_admin / the 0065 platform audit predicate
-- exactly: active NULL-scope role + active permission + NULL-scope
-- assignment + ACTIVE user. Cooperative-scoped grants of the same key
-- can never satisfy it.
CREATE OR REPLACE FUNCTION public.app_has_platform_permission(
    p_user       uuid,
    p_permission text
)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT EXISTS (
        SELECT 1
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id
            AND r.status = 'active' AND r.cooperative_id IS NULL
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
            AND p.key = p_permission
          JOIN public.users u ON u.id = ura.user_id
            AND u.status = 'active'
         WHERE ura.user_id = p_user
           AND ura.cooperative_id IS NULL)
$function$;
REVOKE ALL ON FUNCTION public.app_has_platform_permission(uuid, text)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_has_platform_permission(uuid, text)
    TO kooperatif_app;

-- ── §2. Cooperative creation ────────────────────────────────────────
-- Sealed user context + platform cooperatives.manage on the
-- restricted lane; the trusted owner lane (CLI/migrations/test
-- fixtures) skips the seal exactly like app_platform_user_set_status.
-- Always lands provisioning + business_enabled=false — the
-- business_enabled column has no parameter here so no caller can
-- smuggle an enabled cooperative into existence.
CREATE OR REPLACE FUNCTION public.app_cooperative_create(
    p_name       text,
    p_legal_name text
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_actor uuid;
    v_name  text := btrim(coalesce(p_name, ''));
    v_id    uuid;
BEGIN
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

    IF length(v_name) = 0 OR length(v_name) > 200 THEN
        RAISE EXCEPTION 'cooperative_name_invalid'
            USING ERRCODE = 'check_violation';
    END IF;

    INSERT INTO public.cooperatives (name, legal_name, status, business_enabled)
    VALUES (v_name, nullif(btrim(coalesce(p_legal_name, '')), ''),
            'provisioning', false)
    RETURNING id INTO v_id;

    RETURN v_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_cooperative_create(text, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_cooperative_create(text, text)
    TO kooperatif_app;

-- ── §3. Atomic activation package ───────────────────────────────────
-- provisioning → active with the initial administrator folded into the
-- same transaction:
--   1. target cooperative must exist and be `provisioning`
--      (FOR UPDATE — a concurrent transition cannot interleave);
--   2. the initial administrator must be an existing ACTIVE global
--      user (disabled/unknown users fail the whole package);
--   3. membership upsert (active, created_by = sealed actor);
--   4. administrator role: clone the `system-administrator` template
--      into a cooperative-scoped role. A same-named role already
--      present in the cooperative is reused ONLY when it already
--      grants roles.manage — silently upgrading an eponymous role's
--      permission set is refused;
--   5. cooperative-scoped assignment (composite FK keeps the triple
--      consistent);
--   6. status → active + business_enabled → true;
--   7. postcondition: at least one effective administrator
--      (app_admin_path_count) — otherwise the package raises and every
--      step rolls back.
-- Returns the cooperative id; NULL only when the cooperative does not
-- exist (mapped to not_found).
CREATE OR REPLACE FUNCTION public.app_cooperative_activate(
    p_cooperative_id uuid,
    p_admin_user_id  uuid
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_actor      uuid;
    v_status     text;
    v_admin_ok   boolean;
    v_template   uuid;
    v_role       uuid;
BEGIN
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
    -- Owner lane has no sealed actor; attribution lands in audit
    -- metadata/user columns as NULL exactly like other owner writers.

    SELECT c.status INTO v_status
      FROM public.cooperatives c
     WHERE c.id = p_cooperative_id
     FOR UPDATE OF c;
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;
    IF v_status <> 'provisioning' THEN
        RAISE EXCEPTION 'invalid_cooperative_transition: % -> active', v_status
            USING ERRCODE = 'check_violation';
    END IF;

    SELECT u.status = 'active' INTO v_admin_ok
      FROM public.users u WHERE u.id = p_admin_user_id;
    IF v_admin_ok IS DISTINCT FROM true THEN
        RAISE EXCEPTION 'initial_admin_ineligible'
            USING ERRCODE = 'check_violation';
    END IF;

    -- 3. Membership first — the composite assignment FK requires it.
    INSERT INTO public.cooperative_memberships
        (cooperative_id, user_id, status, created_by_user_id)
    VALUES (p_cooperative_id, p_admin_user_id, 'active', v_actor)
    ON CONFLICT (cooperative_id, user_id) DO UPDATE
        SET status = 'active', ended_at = NULL, updated_at = now()
        WHERE public.cooperative_memberships.status IS DISTINCT FROM 'active';

    -- 4. Administrator role: template clone, or reuse an existing
    --    same-named cooperative role only when it already carries
    --    roles.manage (never mutate a role's permission set silently).
    SELECT t.id INTO v_template
      FROM public.system_role_templates t
     WHERE t.key = 'system-administrator' AND t.status = 'active';
    IF v_template IS NULL THEN
        RAISE EXCEPTION 'admin_template_missing'
            USING ERRCODE = 'check_violation';
    END IF;

    SELECT r.id INTO v_role
      FROM public.roles r
      JOIN public.system_role_templates t
        ON lower(r.name) = lower(t.name)
     WHERE r.cooperative_id = p_cooperative_id
       AND t.id = v_template
     LIMIT 1;
    IF v_role IS NOT NULL THEN
        IF NOT EXISTS (
            SELECT 1
              FROM public.role_permissions rp
              JOIN public.permissions p ON p.id = rp.permission_id
             WHERE rp.role_id = v_role AND p.key = 'roles.manage')
        THEN
            RAISE EXCEPTION 'initial_admin_role_conflict'
                USING ERRCODE = 'check_violation';
        END IF;
    ELSE
        INSERT INTO public.roles (name, description, cooperative_id)
        SELECT t.name, t.description, p_cooperative_id
          FROM public.system_role_templates t WHERE t.id = v_template
        RETURNING id INTO v_role;
        INSERT INTO public.role_permissions (role_id, permission_id)
        SELECT v_role, tp.permission_id
          FROM public.system_role_template_permissions tp
         WHERE tp.template_id = v_template;
    END IF;

    -- 5. Cooperative-scoped assignment (idempotent on re-runs inside
    --    the same provisioning state).
    INSERT INTO public.user_role_assignments (user_id, role_id, cooperative_id)
    VALUES (p_admin_user_id, v_role, p_cooperative_id)
    ON CONFLICT (user_id, role_id) DO NOTHING;

    -- 6. Transition + gate, atomically with all of the above.
    UPDATE public.cooperatives
       SET status = 'active', business_enabled = true, updated_at = now()
     WHERE id = p_cooperative_id;

    -- 7. Effective-administrator postcondition — if it cannot hold,
    --    everything above rolls back with this statement.
    IF public.app_admin_path_count(p_cooperative_id) < 1 THEN
        RAISE EXCEPTION 'activation_admin_invariant'
            USING ERRCODE = 'check_violation';
    END IF;

    RETURN p_cooperative_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_cooperative_activate(uuid, uuid)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_cooperative_activate(uuid, uuid)
    TO kooperatif_app;

-- ── §4. Status transitions: suspend / reactivate / archive ─────────
-- Explicit transition matrix (invalid source → check_violation, never
-- silent):
--   active       → suspended   (business gate closes immediately)
--   suspended    → active      (requires ≥1 effective administrator)
--   provisioning|active|suspended → archived (terminal; business gate
--                              closes; the bootstrap cooperative is
--                              refused — archiving it has no in-product
--                              recovery path)
-- Every other transition, including archived → anything and
-- provisioning → active (which belongs to app_cooperative_activate),
-- is rejected.
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
        IF v_status <> 'suspended' THEN
            RAISE EXCEPTION 'invalid_cooperative_transition: % -> active', v_status
                USING ERRCODE = 'check_violation';
        END IF;
        -- Reactivation validates the effective-administrator AND
        -- membership invariant: app_admin_path_count counts only
        -- active-member + active-role + active-user administration
        -- paths inside this exact cooperative scope.
        IF public.app_admin_path_count(p_cooperative_id) < 1 THEN
            RAISE EXCEPTION 'reactivation_admin_invariant'
                USING ERRCODE = 'check_violation';
        END IF;
    ELSE -- archived
        IF v_status = 'archived' THEN
            RAISE EXCEPTION 'invalid_cooperative_transition: archived is terminal'
                USING ERRCODE = 'check_violation';
        END IF;
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

-- ── §5. Platform read model ─────────────────────────────────────────
-- Session-derived (B4 convention — no GUC seal needed for reads):
-- session owner must hold NULL-scope cooperatives.manage. Returns the
-- allowlisted platform projection; member_count counts ACTIVE
-- memberships only.
CREATE OR REPLACE FUNCTION public.app_cooperative_list_platform(
    p_token_hash      bytea,
    p_now             timestamptz,
    p_cooperative_id  uuid DEFAULT NULL,
    p_status          text DEFAULT NULL,
    p_limit           integer DEFAULT 200
)
RETURNS TABLE (
    id               uuid,
    name             text,
    legal_name       text,
    status           text,
    is_bootstrap     boolean,
    business_enabled boolean,
    member_count     bigint,
    created_at       timestamptz
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_user uuid;
BEGIN
    v_user := public.app_session_owner(p_token_hash, p_now);
    IF v_user IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: valid session required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF NOT public.app_has_platform_permission(v_user, 'cooperatives.manage') THEN
        RAISE EXCEPTION 'insufficient_privilege: platform cooperatives.manage required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    RETURN QUERY
        SELECT c.id, c.name, c.legal_name, c.status, c.is_bootstrap,
               c.business_enabled,
               (SELECT count(*) FROM public.cooperative_memberships m
                 WHERE m.cooperative_id = c.id AND m.status = 'active'),
               c.created_at
          FROM public.cooperatives c
         WHERE (p_cooperative_id IS NULL OR c.id = p_cooperative_id)
           AND (p_status IS NULL OR c.status = p_status)
         ORDER BY c.created_at, c.id
         LIMIT least(greatest(coalesce(p_limit, 200), 1), 500);
END
$function$;
REVOKE ALL ON FUNCTION public.app_cooperative_list_platform(
    bytea, timestamptz, uuid, text, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_cooperative_list_platform(
    bytea, timestamptz, uuid, text, integer) TO kooperatif_app;

-- ── §6. Audit: cooperatives + membership grants join the audited ───
-- transition set. The emit call itself decides sealed-vs-silent, so
-- owner-lane fixtures/CLI stay quiet and every sealed mutation mints
-- with actor attribution.
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
                WHEN OLD.status = 'suspended' AND NEW.status = 'active'
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

CREATE OR REPLACE TRIGGER cooperatives_audit
    AFTER INSERT OR UPDATE ON public.cooperatives
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();

-- Membership INSERTs join the audited set (the trigger previously
-- covered UPDATE only — cooperative_membership_granted was owner-lane
-- minted at bootstrap and is now also sealed-lane minted).
DROP TRIGGER IF EXISTS cooperative_memberships_audit
    ON public.cooperative_memberships;
CREATE TRIGGER cooperative_memberships_audit
    AFTER INSERT OR UPDATE ON public.cooperative_memberships
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();

-- ── §7. Realtime invalidation for lifecycle changes ─────────────────
-- A cooperative status/business_enabled change emits one signed
-- `authz` signal scoped to that cooperative: bound sockets revalidate
-- immediately (session + membership + cooperative status +
-- business_enabled all re-checked) and close when business access is
-- lost — suspension takes effect promptly, reactivation re-opens.
CREATE OR REPLACE FUNCTION public.kooperatif_notify_authz_cooperative()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
BEGIN
    PERFORM public.kooperatif_emit_domain('authz', NEW.id);
    RETURN NULL;
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_notify_authz_cooperative()
    FROM PUBLIC;

CREATE TRIGGER rt_authz_cooperatives
    AFTER UPDATE ON public.cooperatives
    FOR EACH ROW
    WHEN (OLD.status IS DISTINCT FROM NEW.status
          OR OLD.business_enabled IS DISTINCT FROM NEW.business_enabled)
    EXECUTE FUNCTION public.kooperatif_notify_authz_cooperative();

-- ── §8. Least-privilege platform role seed ──────────────────────────
-- A dedicated NULL-scope role carrying ONLY cooperatives.manage. No
-- user is assigned — granting lifecycle authority is an explicit
-- operator act (owner-lane assignment), never automatic. Cooperative-
-- scoped roles and templates remain untouched: cooperatives.manage is
-- platform authority and must never appear inside a cooperative.
INSERT INTO public.roles (name, description, cooperative_id)
SELECT 'platform-lifecycle',
       'Kooperatif yaşam döngüsü yönetimi (oluşturma/aktivasyon/askıya alma/arşivleme); platform kapsamlı, hiçbir kooperatife bağlı değildir.',
       NULL
WHERE NOT EXISTS (
    SELECT 1 FROM public.roles
     WHERE cooperative_id IS NULL
       AND lower(name) = lower('platform-lifecycle'));

INSERT INTO public.role_permissions (role_id, permission_id)
SELECT r.id, p.id
  FROM public.roles r, public.permissions p
 WHERE r.cooperative_id IS NULL
   AND lower(r.name) = lower('platform-lifecycle')
   AND p.key = 'cooperatives.manage'
ON CONFLICT (role_id, permission_id) DO NOTHING;

-- ── §9. Postchecks (fail closed inside the migration transaction) ───
DO $$
DECLARE
    v_sig text;
BEGIN
    -- Definer hardening: every new function is SECURITY DEFINER with
    -- no PUBLIC execute, and exactly the intended app grants exist.
    IF EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.pronamespace = 'public'::regnamespace
           AND p.proname IN (
               'app_has_platform_permission',
               'app_cooperative_create',
               'app_cooperative_activate',
               'app_cooperative_set_status',
               'app_cooperative_list_platform',
               'kooperatif_notify_authz_cooperative')
           AND (NOT p.prosecdef
                OR p.proacl IS NULL
                OR EXISTS (SELECT 1 FROM aclexplode(p.proacl) x
                            WHERE x.grantee = 0)))
    THEN
        RAISE EXCEPTION 'B2 definers must be SECURITY DEFINER without PUBLIC execute';
    END IF;

    FOR v_sig IN
        SELECT sig FROM (VALUES
            ('public.app_has_platform_permission(uuid,text)'),
            ('public.app_cooperative_create(text,text)'),
            ('public.app_cooperative_activate(uuid,uuid)'),
            ('public.app_cooperative_set_status(uuid,text)'),
            ('public.app_cooperative_list_platform(bytea,timestamptz,uuid,text,integer)')
        ) AS t(sig)
    LOOP
        IF NOT has_function_privilege('kooperatif_app', v_sig, 'EXECUTE') THEN
            RAISE EXCEPTION 'B2 postcheck: % not granted to kooperatif_app', v_sig;
        END IF;
    END LOOP;

    -- Realtime trigger inventory: 43 → 44.
    IF (SELECT count(*) FROM pg_trigger
         WHERE tgname LIKE 'rt_%' AND NOT tgisinternal) <> 44 THEN
        RAISE EXCEPTION 'rt_* trigger inventory must be 44 after B2';
    END IF;

    -- The audit trigger exists on cooperatives.
    IF NOT EXISTS (
        SELECT 1 FROM pg_trigger t
         JOIN pg_class c ON c.oid = t.tgrelid
         WHERE t.tgname = 'cooperatives_audit'
           AND c.relname = 'cooperatives' AND NOT t.tgisinternal)
    THEN
        RAISE EXCEPTION 'cooperatives_audit trigger missing';
    END IF;

    -- Least privilege: the dedicated platform role exists, is
    -- NULL-scope, and carries EXACTLY cooperatives.manage.
    IF NOT EXISTS (
        SELECT 1
          FROM public.roles r
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
         WHERE r.cooperative_id IS NULL
           AND lower(r.name) = lower('platform-lifecycle')
           AND p.key = 'cooperatives.manage')
    THEN
        RAISE EXCEPTION 'platform lifecycle role seed missing';
    END IF;
    IF EXISTS (
        SELECT 1
          FROM public.roles r
          JOIN public.role_permissions rp ON rp.role_id = r.id
         WHERE r.cooperative_id IS NULL
           AND lower(r.name) = lower('platform-lifecycle')
           AND rp.permission_id <> (SELECT id FROM public.permissions
                                     WHERE key = 'cooperatives.manage'))
    THEN
        RAISE EXCEPTION 'platform lifecycle role must carry only cooperatives.manage';
    END IF;

    -- The platform permission must never sit on a cooperative-scoped
    -- role or any template.
    IF EXISTS (
        SELECT 1
          FROM public.role_permissions rp
          JOIN public.permissions p ON p.id = rp.permission_id
          JOIN public.roles r ON r.id = rp.role_id
         WHERE p.key = 'cooperatives.manage'
           AND r.cooperative_id IS NOT NULL)
    OR EXISTS (
        SELECT 1
          FROM public.system_role_template_permissions tp
          JOIN public.permissions p ON p.id = tp.permission_id
         WHERE p.key IN ('cooperatives.manage', 'memberships.manage'))
    THEN
        RAISE EXCEPTION 'platform lifecycle permission leaked into cooperative scope';
    END IF;
END
$$;
