-- M1 — user identity vs cooperative membership separation.
--
-- Approved decision (final):
--   * A cooperative administrator may suspend / reactivate / end a
--     membership ONLY inside their own verified cooperative.
--   * A cooperative administrator must NEVER disable or reactivate the
--     global user identity.
--   * Global identity lifecycle belongs to platform authority. The only
--     existing platform-authority representation is the NULL-scope
--     (pre-tenancy) grant lane: an active `users.manage` assignment
--     carrying `cooperative_id IS NULL` on an active NULL-scope role.
--     NULL-attributed roles are created before tenancy exists (CLI /
--     bootstrap) and, post-bootstrap, already resolve as in-scope only
--     for the bootstrap cooperative — they are deliberately NOT treated
--     as cooperative grants here; they are the platform lane.
--   * A membership change in cooperative A must not affect cooperative
--     B — membership status is per-(cooperative, user) by construction.
--   * The last effective administration path is protected at both
--     levels: `app_admin_path_count` for the cooperative, and the new
--     NULL-scope roles.manage path count for the platform.
--
-- What changes:
--   1. `app_user_set_status` — on the app lane a cooperative-scoped
--      context can no longer touch global users.status. Pre-tenancy
--      (NULL verified scope) stays legal; the owner lane (CLI /
--      migrations / fixtures) is unchanged.
--   2. `app_platform_user_set_status` — new platform lane: sealed
--      user context (no cooperative) + NULL-scope users.manage on the
--      actor, last-platform-administrator protection, same
--      global status write.
--   3. `app_admin_revoke_user_sessions` gains a post-bootstrap platform
--      arm so the platform disable path can still revoke sessions.
--   4. `app_membership_set_status` — the cooperative-scoped membership
--      lifecycle mutator: sealed context + users.manage re-verified in
--      the database, target restricted to the verified cooperative,
--      last-cooperative-administrator protection, ended_at semantics
--      (set on 'ended', cleared on reactivation).
--   5. `app_audit_transition` mints cooperative_membership_* events —
--      membership transitions are now audited like every other
--      security-relevant transition.
--
-- No table grants change: kooperatif_app keeps SELECT-only on
-- cooperative_memberships; every membership write goes through the
-- new definer.

BEGIN;
SET LOCAL statement_timeout = '10s';

-- ------------------------------------------------------------------
-- Internal predicate: does p_user hold an active NULL-scope
-- users.manage grant? NULL attribution = the pre-tenancy/platform lane.
-- Not granted to the runtime role.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_is_platform_admin(p_user uuid)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT EXISTS(
        SELECT 1
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id
            AND r.status = 'active'
            AND r.cooperative_id IS NULL
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
            AND p.key = 'users.manage'
         WHERE ura.user_id = p_user
           AND ura.cooperative_id IS NULL)
$function$;
REVOKE ALL ON FUNCTION public.app_is_platform_admin(uuid) FROM PUBLIC;

-- ------------------------------------------------------------------
-- Internal: count of DISTINCT active users holding an active NULL-scope
-- roles.manage path — the platform administration-path guard.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_platform_admin_path_count()
RETURNS bigint
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT count(DISTINCT ura.user_id)
      FROM public.user_role_assignments ura
      JOIN public.roles r ON r.id = ura.role_id
        AND r.status = 'active' AND r.cooperative_id IS NULL
      JOIN public.role_permissions rp ON rp.role_id = r.id
      JOIN public.permissions p ON p.id = rp.permission_id
        AND p.key = 'roles.manage'
      JOIN public.users u ON u.id = ura.user_id AND u.status = 'active'
     WHERE ura.cooperative_id IS NULL
$function$;
REVOKE ALL ON FUNCTION public.app_platform_admin_path_count() FROM PUBLIC;

-- ------------------------------------------------------------------
-- Global identity status — PLATFORM lane only.
--
-- App lane requirements: sealed USER context (request_user_tx — no
-- cooperative GUC) and an active NULL-scope users.manage on the sealed
-- actor. Owner lane stays trusted. Disabling the last active
-- NULL-scope roles.manage path raises check_violation
-- 'lockout_prevented'. Returns the user id, or NULL when the target is
-- already in the requested state (idempotent).
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_platform_user_set_status(
    p_user_id uuid,
    p_status  text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_actor uuid;
    v_had   boolean;
    v_id    uuid;
BEGIN
    IF p_status NOT IN ('active', 'disabled') THEN
        RAISE EXCEPTION 'invalid user status: %', p_status
            USING ERRCODE = 'check_violation';
    END IF;

    IF session_user = 'kooperatif_app' THEN
        IF NOT public.app_user_ctx_ok() THEN
            RAISE EXCEPTION 'insufficient_privilege: sealed user context required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        v_actor := nullif(current_setting('app.user_id', true), '')::uuid;
        IF v_actor IS NULL OR NOT public.app_is_platform_admin(v_actor) THEN
            RAISE EXCEPTION 'insufficient_privilege: platform administration required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END IF;

    v_had := EXISTS(
        SELECT 1
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id
            AND r.status = 'active' AND r.cooperative_id IS NULL
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
            AND p.key = 'roles.manage'
         WHERE ura.user_id = p_user_id AND ura.cooperative_id IS NULL);

    UPDATE public.users u
       SET status = p_status, updated_at = now()
     WHERE u.id = p_user_id AND u.status <> p_status
    RETURNING u.id INTO v_id;

    IF v_id IS NOT NULL
       AND p_status = 'disabled'
       AND v_had
       AND public.app_platform_admin_path_count() = 0 THEN
        RAISE EXCEPTION 'lockout_prevented'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN v_id;
END;
$function$;

REVOKE ALL ON FUNCTION public.app_platform_user_set_status(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_platform_user_set_status(uuid, text)
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- §1 — app_user_set_status: cooperative-scoped app callers lose the
-- global identity write. A sealed cooperative context now raises
-- insufficient_privilege; only the NULL-scope (pre-tenancy) app lane
-- and the trusted owner lane still reach the write.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_user_set_status(
    p_user_id uuid,
    p_status  text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('users.manage');
    v_id    uuid;
BEGIN
    -- M1: global identity is platform authority. On the restricted lane
    -- a cooperative-scoped seal is no longer sufficient — membership
    -- lifecycle moved to app_membership_set_status; global status moves
    -- to app_platform_user_set_status. Post-tenancy even the user-seal
    -- (NULL scope) lane is closed here.
    IF session_user = 'kooperatif_app'
       AND (v_scope IS NOT NULL
            OR EXISTS (SELECT 1 FROM public.cooperatives))
    THEN
        RAISE EXCEPTION 'insufficient_privilege: global user status requires platform authority'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    UPDATE public.users u
       SET status = p_status, updated_at = now()
     WHERE u.id = p_user_id
       AND u.status <> p_status
       AND v_scope IS NULL
       AND NOT EXISTS (SELECT 1 FROM public.cooperatives)
    RETURNING u.id INTO v_id;

    IF v_id IS NOT NULL
       AND p_status = 'disabled'
       AND public.app_admin_path_count(v_scope) = 0 THEN
        RAISE EXCEPTION 'lockout_prevented'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN v_id;
END;
$function$;

-- ------------------------------------------------------------------
-- §3 — session revocation gains a post-bootstrap platform arm: with a
-- sealed user context and no cooperative stamp, an actor holding an
-- active NULL-scope users.manage may revoke a target's sessions
-- (platform disable path). The pre-tenancy arm is unchanged.
-- ------------------------------------------------------------------
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
    -- Sealed caller context is mandatory.
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
        -- Cooperative-scoped lane: the stamped cooperative must carry
        -- its own valid seal, the caller must hold users.manage there
        -- (NULL-attributed legacy rows count only through the
        -- marker-consistent bootstrap cooperative), and the target
        -- must be member-visible in that scope.
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
        -- User-seal lane. Post-bootstrap this is the PLATFORM arm: the
        -- sealed actor must hold an active NULL-scope users.manage
        -- grant. Pre-tenancy keeps the legacy global users.manage rule.
        IF v_ctx IS NOT NULL THEN
            RAISE EXCEPTION 'insufficient_privilege: cooperative seal without cooperative context'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        IF EXISTS (SELECT 1 FROM public.cooperatives) THEN
            IF NOT public.app_is_platform_admin(v_user) THEN
                RAISE EXCEPTION 'insufficient_privilege: platform administration required'
                    USING ERRCODE = 'insufficient_privilege';
            END IF;
        ELSIF NOT EXISTS (
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
END;
$fn$;

-- ------------------------------------------------------------------
-- §4 — cooperative membership lifecycle mutator.
--
-- Sealed cooperative context + users.manage re-verified inside the
-- database (app_rbac_verified_scope). The target row is pinned to the
-- verified scope: a caller can never touch another cooperative's
-- membership, and a user with no row in the verified scope yields NULL
-- (caller maps to 404 / idempotent 204).
--
-- Status domain: active | suspended | ended. 'ended' stamps ended_at;
-- any transition back to 'active' clears it — the row is UNIQUE per
-- (cooperative, user) and is never deleted, so history and attribution
-- survive (mirrors the owner-lane ON CONFLICT revive semantic).
--
-- Suspending/ending the membership that carries the scope's last
-- active roles.manage path raises 'lockout_prevented' (23514) and
-- rolls back atomically.
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
    v_scope     uuid := public.app_rbac_verified_scope('users.manage');
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

REVOKE ALL ON FUNCTION public.app_membership_set_status(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_membership_set_status(uuid, text)
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- Session-bound UX flag: does the SESSION OWNER hold platform
-- authority (active NULL-scope users.manage)? Drives whether the
-- frontend surfaces global-account controls; backend enforcement is
-- the definer gate inside app_platform_user_set_status, never this.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_session_is_platform_admin(
    p_token_hash bytea,
    p_now        timestamptz
)
RETURNS boolean
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user uuid;
BEGIN
    v_user := public.app_session_owner(p_token_hash, p_now);
    IF v_user IS NULL THEN
        RETURN false;
    END IF;
    RETURN public.app_is_platform_admin(v_user);
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_is_platform_admin(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_is_platform_admin(bytea, timestamptz)
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- §5 — membership transitions join the audited transition set. The
-- cooperative-scope attribution is supplied by the sealed context
-- inside app_audit_emit; owner/unsealed lanes mint nothing (CLI keeps
-- its explicit cooperative_membership_granted mint).
-- ------------------------------------------------------------------
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
    WHEN 'cooperative_memberships' THEN
        IF TG_OP = 'UPDATE'
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
    ELSE
        NULL;
    END CASE;

    IF v_type IS NOT NULL THEN
        PERFORM public.app_audit_emit(v_type, v_meta);
    END IF;
    RETURN NULL;
END
$fn$;

CREATE OR REPLACE TRIGGER cooperative_memberships_audit
    AFTER UPDATE ON public.cooperative_memberships
    FOR EACH ROW EXECUTE FUNCTION public.app_audit_transition();

-- ------------------------------------------------------------------
-- Post-checks (fail closed inside the migration transaction).
-- ------------------------------------------------------------------
DO $$
BEGIN
    IF NOT has_function_privilege(
        'kooperatif_app',
        'public.app_membership_set_status(uuid,text)', 'EXECUTE')
    THEN RAISE EXCEPTION 'm1 postcheck: app_membership_set_status not granted';
    END IF;
    IF NOT has_function_privilege(
        'kooperatif_app',
        'public.app_platform_user_set_status(uuid,text)', 'EXECUTE')
    THEN RAISE EXCEPTION 'm1 postcheck: app_platform_user_set_status not granted';
    END IF;
    IF EXISTS (
        SELECT 1 FROM aclexplode(
            (SELECT proacl FROM pg_proc
              WHERE oid = 'public.app_membership_set_status(uuid,text)'::regprocedure)) x
         WHERE x.grantee = 0)
    THEN RAISE EXCEPTION 'm1 postcheck: app_membership_set_status PUBLIC-executable';
    END IF;
    IF EXISTS (
        SELECT 1 FROM aclexplode(
            (SELECT proacl FROM pg_proc
              WHERE oid = 'public.app_platform_user_set_status(uuid,text)'::regprocedure)) x
         WHERE x.grantee = 0)
    THEN RAISE EXCEPTION 'm1 postcheck: app_platform_user_set_status PUBLIC-executable';
    END IF;
    IF has_function_privilege(
        'kooperatif_app',
        'public.app_is_platform_admin(uuid)', 'EXECUTE')
    THEN RAISE EXCEPTION 'm1 postcheck: internal helper granted to runtime';
    END IF;
    IF has_function_privilege(
        'kooperatif_app',
        'public.app_platform_admin_path_count()', 'EXECUTE')
    THEN RAISE EXCEPTION 'm1 postcheck: internal helper granted to runtime';
    END IF;
    IF NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_is_platform_admin(bytea,timestamptz)', 'EXECUTE')
    THEN RAISE EXCEPTION 'm1 postcheck: app_session_is_platform_admin not granted';
    END IF;
    -- Runtime still holds no membership write privilege.
    IF EXISTS (
        SELECT 1 FROM information_schema.column_privileges
         WHERE grantee = 'kooperatif_app'
           AND table_name = 'cooperative_memberships'
           AND privilege_type <> 'SELECT')
    THEN RAISE EXCEPTION 'm1 postcheck: residual non-SELECT grant on cooperative_memberships';
    END IF;
END $$;

COMMIT;
