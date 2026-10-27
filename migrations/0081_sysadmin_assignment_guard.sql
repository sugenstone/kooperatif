-- 0081_sysadmin_assignment_guard.sql
-- M1/M2 hardening: only a platform administrator
-- (canonical app_is_platform_admin — NULL-scope users.manage) may add or remove the canonical System Administrator
-- role inside a cooperative. Cooperative users.manage holders retain
-- every other assignable role; the guard is enforced inside the
-- mutating definer so no API path can bypass it.
--
-- Canonical identity: the cooperative role linked to
-- system_role_templates.key = 'system-administrator' by the same
-- name-matching rule cooperative activation uses to locate it.
--
-- app_user_roles_set replaces the full per-scope set, so the guard
-- must fire on BOTH membership of the role in the incoming set and
-- presence in the outgoing set (a removal is the privileged action).

CREATE OR REPLACE FUNCTION public.app_user_roles_set(
    p_user_id  uuid,
    p_role_ids uuid[]
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('users.manage');
    v_actor uuid := nullif(current_setting('app.user_id', true), '')::uuid;
    v_role  uuid;
    v_sysadmin uuid;
BEGIN
    IF NOT (
        (v_scope IS NULL AND NOT EXISTS (SELECT 1 FROM public.cooperatives))
        OR EXISTS (
            SELECT 1 FROM public.cooperative_memberships m
             WHERE m.user_id = p_user_id
               AND m.cooperative_id = v_scope)
    ) THEN
        RAISE EXCEPTION 'insufficient_privilege: user outside verified scope'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    FOR v_role IN SELECT DISTINCT unnest(p_role_ids) LOOP
        IF NOT EXISTS (
            SELECT 1 FROM public.roles r
             WHERE r.id = v_role
               AND r.status = 'active'
               AND (v_scope IS NULL OR r.cooperative_id = v_scope)
        ) THEN
            RAISE EXCEPTION 'invalid_role_assignment' USING ERRCODE = 'check_violation';
        END IF;
    END LOOP;

    -- System Administrator guard (0081): the canonical administrator
    -- role may change hands only under platform-administrator
    -- authority. Cooperative users.manage alone is insufficient for
    -- both assignment and revocation.
    SELECT r.id INTO v_sysadmin
      FROM public.roles r
      JOIN public.system_role_templates t
        ON lower(r.name) = lower(t.name)
     WHERE r.cooperative_id = v_scope
       AND t.key = 'system-administrator'
       AND t.status = 'active'
     LIMIT 1;
    IF v_sysadmin IS NOT NULL
       AND (
           v_sysadmin = ANY(p_role_ids)
           OR EXISTS (
               SELECT 1 FROM public.user_role_assignments
                WHERE user_id = p_user_id
                  AND role_id = v_sysadmin
                  AND cooperative_id = v_scope)
       )
       AND NOT public.app_is_platform_admin(v_actor)
    THEN
        RAISE EXCEPTION 'insufficient_privilege: system administrator assignment requires platform administrator'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    DELETE FROM public.user_role_assignments
     WHERE user_id = p_user_id
       AND public.app_rbac_write_scope(cooperative_id, v_scope);

    INSERT INTO public.user_role_assignments (user_id, role_id, cooperative_id)
    SELECT p_user_id, r.id, v_scope
      FROM (SELECT DISTINCT unnest(p_role_ids) AS id) k
      JOIN public.roles r ON r.id = k.id;

    IF public.app_admin_path_count(v_scope) = 0 THEN
        RAISE EXCEPTION 'lockout_prevented' USING ERRCODE = 'check_violation';
    END IF;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_user_roles_set(uuid, uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_user_roles_set(uuid, uuid[]) TO kooperatif_app;

-- ── Delegation boundary (approved direction): a cooperative
-- administrator may only write permission keys they themselves hold
-- in the same cooperative scope. Platform administrators keep the
-- full catalog via app_is_platform_admin. The platform-only key
-- block (0069) stays first and unchanged; the existing catalog,
-- role-scope, lockout and audit checks are preserved verbatim.
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
    v_actor  uuid := nullif(current_setting('app.user_id', true), '')::uuid;
    v_before text[];
BEGIN
    IF session_user = 'kooperatif_app'
       AND v_scope IS NOT NULL
       AND 'cooperatives.manage' = ANY (p_keys)
    THEN
        RAISE EXCEPTION 'insufficient_privilege: platform permission not grantable in cooperative scope'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    -- 0081: fail closed before any write — in a cooperative scope a
    -- non-platform actor may not delegate a key absent from their own
    -- in-scope grant set. Mirrors app_rbac_verified_scope's join shape
    -- (membership + in_scope on both assignment and role) so anything
    -- the scope check accepts is delegatable here; platform-scoped
    -- grants count the same way in the bootstrap lane, while the
    -- platform-admin exemption covers the NULL-scope authority path.
    IF session_user = 'kooperatif_app'
       AND v_scope IS NOT NULL
       AND NOT public.app_is_platform_admin(v_actor)
       AND EXISTS (
           SELECT 1 FROM (SELECT DISTINCT unnest(p_keys) AS key) k
            WHERE NOT EXISTS (
                SELECT 1
                  FROM public.user_role_assignments ura
                  JOIN public.roles r
                    ON r.id = ura.role_id AND r.status = 'active'
                   AND public.app_rbac_in_scope(r.cooperative_id, v_scope)
                  JOIN public.role_permissions rp ON rp.role_id = r.id
                  JOIN public.permissions p
                    ON p.id = rp.permission_id AND p.key = k.key
                  JOIN public.cooperative_memberships m
                    ON m.cooperative_id = v_scope
                   AND m.user_id = ura.user_id
                   AND m.status = 'active'
                 WHERE ura.user_id = v_actor
                   AND public.app_rbac_in_scope(ura.cooperative_id, v_scope)))
    THEN
        RAISE EXCEPTION 'insufficient_privilege: cannot delegate permissions outside own grant set'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM public.roles r
         WHERE r.id = p_role_id
           AND public.app_rbac_write_scope(r.cooperative_id, v_scope)
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
REVOKE ALL ON FUNCTION public.app_role_permissions_set(uuid, text[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_role_permissions_set(uuid, text[]) TO kooperatif_app;
