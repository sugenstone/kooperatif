-- 0069_p6_b2_platform_role_boundary.sql
-- M1-P6 B2 hardening: platform-scoped (NULL-cooperative) role and
-- assignment rows are READ-inherited by the bootstrap scope but must
-- never be MUTABLE from any cooperative scope.
--
-- Before this migration the boundary definers evaluated row scope with
-- public.app_rbac_in_scope(row_cooperative_id, scope), whose NULL arm
-- deliberately counts NULL rows as inside the bootstrap scope (the
-- legacy pre-tenancy lane). That arm is correct for SELECT visibility,
-- but it also let a bootstrap-scope administrator holding roles.manage
-- or users.manage:
--   * rename/disable the NULL-scope 'platform-lifecycle' role or strip
--     its permission set (app_role_update / app_role_set_status /
--     app_role_permissions_set);
--   * delete a platform administrator's NULL-scope role assignment via
--     the cooperative user-roles replace (app_user_roles_set DELETE).
--
-- No privilege escalation existed — NULL-scope assignments cannot be
-- minted through any app-lane definer — but mutating or stripping the
-- platform lane is a boundary violation. The fix narrows only the
-- mutation predicates: app_rbac_write_scope is strict ownership, so
-- cooperative scopes touch exclusively cooperative-owned rows, while
-- the never-bootstrapped lane (NULL scope before any cooperative
-- exists) keeps managing NULL rows exactly as before. Read
-- inheritance, the RLS policies, owner lane and every other definer
-- are untouched.

-- ------------------------------------------------------------------
-- Internal: mutation scope. STRICTER than app_rbac_in_scope — used by
-- boundary definers for writes. A row is writable only when it is
-- OWNED by the verified cooperative scope; the NULL (pre-tenancy)
-- scope may still manage NULL rows only while no cooperative exists.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_rbac_write_scope(
    p_row_cooperative_id uuid,
    p_scope              uuid
)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT CASE
        WHEN p_scope IS NOT NULL THEN
            p_row_cooperative_id = p_scope
        ELSE
            p_row_cooperative_id IS NULL
            AND NOT EXISTS (SELECT 1 FROM public.cooperatives)
    END
$function$;
REVOKE ALL ON FUNCTION public.app_rbac_write_scope(uuid, uuid) FROM PUBLIC;

-- ------------------------------------------------------------------
-- app_role_update — rename/description only on rows OWNED by the
-- verified scope. NULL-scope platform rows are no longer writable from
-- the bootstrap scope. Returns NULL when outside the write scope.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_role_update(
    p_role_id     uuid,
    p_name        text,
    p_description text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('roles.manage');
    v_id    uuid;
BEGIN
    UPDATE public.roles
       SET name        = COALESCE(p_name, name),
           description = COALESCE(p_description, description),
           updated_at  = now()
     WHERE id = p_role_id
       AND public.app_rbac_write_scope(cooperative_id, v_scope)
    RETURNING id INTO v_id;
    RETURN v_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_role_update(uuid, text, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_role_update(uuid, text, text) TO kooperatif_app;

-- ------------------------------------------------------------------
-- app_role_set_status — status flips only on rows OWNED by the
-- verified scope. A bootstrap-scope administrator can no longer
-- disable the platform lifecycle role (or any NULL-scope platform
-- grant carrier). Lockout accounting is unchanged.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_role_set_status(
    p_role_id uuid,
    p_status  text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope('roles.manage');
    v_id    uuid;
BEGIN
    UPDATE public.roles
       SET status = p_status, updated_at = now()
     WHERE id = p_role_id
       AND status <> p_status
       AND public.app_rbac_write_scope(cooperative_id, v_scope)
    RETURNING id INTO v_id;
    IF v_id IS NOT NULL
       AND p_status = 'disabled'
       AND public.app_admin_path_count(v_scope) = 0 THEN
        RAISE EXCEPTION 'lockout_prevented'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN v_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_role_set_status(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_role_set_status(uuid, text) TO kooperatif_app;

-- ------------------------------------------------------------------
-- app_role_permissions_set — replaces the permission set of a role
-- OWNED by the verified scope (0057 audit-minting body preserved
-- verbatim except the scope predicate). Additionally, on the app lane
-- the platform-only key 'cooperatives.manage' can never be granted to
-- a cooperative-owned role: the grant would be inert for the platform
-- predicate (which requires a NULL-scope chain), but platform
-- authority must not even appear inside a cooperative.
-- ------------------------------------------------------------------
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
    IF session_user = 'kooperatif_app'
       AND v_scope IS NOT NULL
       AND 'cooperatives.manage' = ANY (p_keys)
    THEN
        RAISE EXCEPTION 'insufficient_privilege: platform permission not grantable in cooperative scope'
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

-- ------------------------------------------------------------------
-- app_user_roles_set — assignment replace within the verified scope.
-- The role-validation arm already required cooperative ownership for
-- cooperative scopes (NULL roles were never assignable); this version
-- additionally narrows the DELETE so a cooperative-scope replace can
-- never strip NULL-scope (platform lane) assignments — including a
-- platform administrator's lifecycle grant. Pre-tenancy NULL-scope
-- behavior is preserved.
-- ------------------------------------------------------------------
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
    v_role  uuid;
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

-- ------------------------------------------------------------------
-- Postchecks (fail closed inside the migration transaction).
-- ------------------------------------------------------------------
DO $$
BEGIN
    -- Every touched definer is SECURITY DEFINER, unreachable by PUBLIC,
    -- and executable by the runtime role only (helper stays internal).
    IF EXISTS (
        SELECT 1
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public'
           AND p.proname IN ('app_role_update', 'app_role_set_status',
                             'app_role_permissions_set', 'app_user_roles_set',
                             'app_rbac_write_scope')
           AND (NOT p.prosecdef
                OR p.proacl::text LIKE '%PUBLIC%'))
    THEN
        RAISE EXCEPTION 'B2 boundary definers must be SECURITY DEFINER without PUBLIC execute';
    END IF;

    -- The mutation predicates must be the strict write-scope variant.
    IF EXISTS (
        SELECT 1
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public'
           AND p.proname IN ('app_role_update', 'app_role_set_status',
                             'app_role_permissions_set', 'app_user_roles_set')
           AND pg_get_functiondef(p.oid) NOT LIKE '%app_rbac_write_scope%')
    THEN
        RAISE EXCEPTION 'B2 boundary definers must use app_rbac_write_scope';
    END IF;

    -- app_rbac_in_scope itself is untouched — read inheritance for the
    -- bootstrap scope is preserved by contract.
    IF pg_get_functiondef(
           'public.app_rbac_in_scope(uuid, uuid)'::regprocedure)
           LIKE '%app_rbac_write_scope%'
    THEN
        RAISE EXCEPTION 'read-scope predicate must not change';
    END IF;
END
$$;
