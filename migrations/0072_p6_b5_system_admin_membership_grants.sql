-- 0072_p6_b5_system_admin_membership_grants.sql
-- M1-P6 B5 Part A — approved System Administrator membership grants.
--
--   Approved product decision: EVERY cooperative's System Administrator
--   role carries `memberships.read` + `memberships.manage`. No other
--   cooperative role receives them automatically; platform permissions
--   are never derived from cooperative grants.
--
--   Scope of the grant:
--   * the `system-administrator` template — every FUTURE clone made by
--     app_cooperative_activate inherits both keys;
--   * every EXISTING role name-matched to that template (the same rule
--     the activation package applies), in ANY scope — cooperative
--     clones, plus the NULL-scoped legacy `Sistem Yöneticisi`, which is
--     the bootstrap cooperative's administrator role (its permissions
--     resolve in bootstrap scope only, never as platform authority —
--     `memberships.manage` is a cooperative-scoped key, not a platform
--     key);
--   * `app_cooperative_activate` now FORWARD-SYNCs the reused
--     `Sistem Yöneticisi` role to the template: when activation adopts
--     an eponymous role it already requires roles.manage; from B5 it
--     additionally fills missing TEMPLATE permissions (additions only —
--     nothing the role carries is ever revoked) so a reused role cannot
--     stay behind the approved administrator baseline.
--
-- Append-only; no historical migration is edited.

-- ── 1. Template grant (future clones) ───────────────────────────────

INSERT INTO public.system_role_template_permissions (template_id, permission_id)
SELECT t.id, p.id
  FROM public.system_role_templates t
  CROSS JOIN public.permissions p
 WHERE t.key = 'system-administrator'
   AND p.key IN ('memberships.read', 'memberships.manage')
ON CONFLICT DO NOTHING;

-- ── 2. Existing System Administrator roles (all scopes) ─────────────
-- Same name-matching rule app_cooperative_activate uses to recognize
-- the administrator role. Covers cooperative clones AND the NULL-scope
-- legacy `Sistem Yöneticisi` (bootstrap cooperative's admin role —
-- resolves only in bootstrap scope; never a platform privilege).

INSERT INTO public.role_permissions (role_id, permission_id)
SELECT r.id, p.id
  FROM public.roles r
  JOIN public.system_role_templates t
    ON lower(r.name) = lower(t.name)
  CROSS JOIN public.permissions p
 WHERE t.key = 'system-administrator'
   AND p.key IN ('memberships.read', 'memberships.manage')
ON CONFLICT DO NOTHING;

-- ── 3. Activation package: forward-sync the reused admin role ───────
-- Body of 0068 carried verbatim; the only change is the template-delta
-- sync inside the reuse branch (marked B5 below).

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
    --    roles.manage (never adopt an eponymous non-admin role).
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
        -- B5 (approved): a reused System Administrator role is synced
        -- FORWARD to the template — missing template permissions are
        -- added so the role cannot stay behind the approved
        -- administrator baseline (e.g. B5 membership keys). Additions
        -- only: permissions the role already holds are never revoked,
        -- and non-template custom grants are preserved.
        INSERT INTO public.role_permissions (role_id, permission_id)
        SELECT v_role, tp.permission_id
          FROM public.system_role_template_permissions tp
         WHERE tp.template_id = v_template
        ON CONFLICT DO NOTHING;
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
    --    everything above rolls back with it.
    IF public.app_admin_path_count(p_cooperative_id) < 1 THEN
        RAISE EXCEPTION 'activation_admin_invariant'
            USING ERRCODE = 'check_violation';
    END IF;

    RETURN p_cooperative_id;
END;
$function$;

REVOKE ALL ON FUNCTION public.app_cooperative_activate(uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_cooperative_activate(uuid, uuid)
    TO kooperatif_app;

-- ── 4. Postchecks ───────────────────────────────────────────────────

DO $$
BEGIN
    -- Template carries both keys.
    IF EXISTS (
        SELECT 1
          FROM public.system_role_templates t
         WHERE t.key = 'system-administrator'
           AND EXISTS (
               SELECT 1 FROM public.permissions p
                WHERE p.key IN ('memberships.read', 'memberships.manage')
                  AND NOT EXISTS (
                      SELECT 1
                        FROM public.system_role_template_permissions tp
                       WHERE tp.template_id = t.id
                         AND tp.permission_id = p.id)))
    THEN
        RAISE EXCEPTION 'B5 postcheck: system-administrator template missing membership keys';
    END IF;

    -- Every System Administrator role (name-matched, any scope)
    -- carries both keys.
    IF EXISTS (
        SELECT 1
          FROM public.roles r
          JOIN public.system_role_templates t
            ON lower(r.name) = lower(t.name)
         WHERE t.key = 'system-administrator'
           AND EXISTS (
               SELECT 1 FROM public.permissions p
                WHERE p.key IN ('memberships.read', 'memberships.manage')
                  AND NOT EXISTS (
                      SELECT 1 FROM public.role_permissions rp
                       WHERE rp.role_id = r.id
                         AND rp.permission_id = p.id)))
    THEN
        RAISE EXCEPTION 'B5 postcheck: a System Administrator role is missing membership keys';
    END IF;

    -- Least privilege: NO role other than a name-matched System
    -- Administrator holds memberships.manage, and no template other
    -- than system-administrator does.
    IF EXISTS (
        SELECT 1
          FROM public.role_permissions rp
          JOIN public.permissions p ON p.id = rp.permission_id
          JOIN public.roles r ON r.id = rp.role_id
         WHERE p.key = 'memberships.manage'
           AND NOT EXISTS (
               SELECT 1 FROM public.system_role_templates t
                WHERE t.key = 'system-administrator'
                  AND lower(t.name) = lower(r.name)))
    THEN
        RAISE EXCEPTION 'B5 postcheck: memberships.manage granted beyond System Administrator roles';
    END IF;
    IF EXISTS (
        SELECT 1
          FROM public.system_role_template_permissions tp
          JOIN public.permissions p ON p.id = tp.permission_id
          JOIN public.system_role_templates t ON t.id = tp.template_id
         WHERE p.key = 'memberships.manage'
           AND t.key <> 'system-administrator')
    THEN
        RAISE EXCEPTION 'B5 postcheck: memberships.manage on a non-admin template';
    END IF;

    -- Platform boundary: no cooperative-scoped role and no role
    -- template may hold the platform lifecycle key.
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
         WHERE p.key = 'cooperatives.manage')
    THEN
        RAISE EXCEPTION 'B5 postcheck: platform key leaked into cooperative scope';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public'
           AND p.proname = 'app_cooperative_activate'
           AND (NOT p.prosecdef OR p.proacl::text LIKE '%PUBLIC%'))
    THEN
        RAISE EXCEPTION 'B5 postcheck: app_cooperative_activate must stay sealed';
    END IF;

    IF pg_get_functiondef('public.app_cooperative_activate(uuid,uuid)'::regprocedure)
           NOT LIKE '%B5 (approved): a reused System Administrator role is synced%'
    THEN
        RAISE EXCEPTION 'B5 postcheck: template forward-sync missing from activate';
    END IF;
END
$$;
