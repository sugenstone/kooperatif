-- M1-P4-R1 — RBAC database boundary (closure-audit F1 + F2).
--
-- The audit proved that the sealed cooperative context plus direct
-- write grants let ANY authenticated member escalate privileges at
-- the SQL layer:
--
--   F1  role_permissions had INSERT/DELETE/SELECT grants, no RLS and
--       no cooperative_id — an UNAUTHENTICATED kooperatif_app
--       connection could graft permissions onto any role (including
--       platform/NULL roles and foreign-cooperative roles) or wipe
--       the entire role→permission map (global authorization outage).
--
--   F2  roles INSERT/UPDATE and user_role_assignments INSERT/DELETE
--       grants passed the sealed-scope WITH CHECK as long as the row
--       carried the caller's cooperative — so a member could
--       self-assign the admin role of its own cooperative without
--       holding roles.manage (permission-level checks lived only in
--       the HTTP layer).
--
-- Remediation invariant: a valid session / sealed cooperative context
-- alone does NOT authorize RBAC mutations. Every mutation goes
-- through a narrowly scoped SECURITY DEFINER that re-verifies the
-- administrative permission against the sealed context inside the
-- database, re-checks row scope, and re-enforces the
-- last-administrator protection. Direct write grants are revoked;
-- the app policies shrink to FOR SELECT.
--
-- Internal helpers get NO EXECUTE grant for kooperatif_app (PUBLIC
-- revoked). Mutators keep the codebase convention: for the runtime
-- role the sealed context + permission are mandatory; the owner lane
-- (CLI/migrations/fixtures) is trusted and skips the GUC checks.
BEGIN;
SET LOCAL statement_timeout = '10s';

-- ------------------------------------------------------------------
-- Internal: does a row's cooperative attribution fall inside the
-- verified scope? Mirrors scoped_visibility / the S5 policies
-- exactly, including the bootstrap-NULL arm (marker + flag
-- re-verified, never for a non-bootstrap cooperative) and the
-- pre-tenancy NULL arm.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_rbac_in_scope(
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
            OR (p_row_cooperative_id IS NULL AND EXISTS(
                SELECT 1
                  FROM public.installation_state s
                  JOIN public.cooperatives c
                    ON c.id = s.bootstrap_cooperative_id
                   AND c.is_bootstrap
                 WHERE s.bootstrap_cooperative_id = p_scope))
        ELSE
            p_row_cooperative_id IS NULL
            AND NOT EXISTS (SELECT 1 FROM public.cooperatives)
    END
$function$;
REVOKE ALL ON FUNCTION public.app_rbac_in_scope(uuid, uuid) FROM PUBLIC;

-- ------------------------------------------------------------------
-- Internal: last-administration-path count for a scope. Ports
-- authz::administration_path_count verbatim (roles.manage through an
-- ACTIVE role + ACTIVE assignment + ACTIVE user + ACTIVE membership;
-- NULL attribution counts only for the verified bootstrap
-- cooperative; NULL scope = the pre-tenancy global count).
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_admin_path_count(p_scope uuid)
RETURNS bigint
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
    SELECT CASE WHEN p_scope IS NULL THEN
        (SELECT count(DISTINCT ura.user_id)
           FROM public.user_role_assignments ura
           JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
           JOIN public.role_permissions rp ON rp.role_id = r.id
           JOIN public.permissions p
             ON p.id = rp.permission_id AND p.key = 'roles.manage'
           JOIN public.users u ON u.id = ura.user_id AND u.status = 'active')
    ELSE
        (SELECT count(DISTINCT ura.user_id)
           FROM public.user_role_assignments ura
           JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
                AND public.app_rbac_in_scope(r.cooperative_id, p_scope)
           JOIN public.role_permissions rp ON rp.role_id = r.id
           JOIN public.permissions p
             ON p.id = rp.permission_id AND p.key = 'roles.manage'
           JOIN public.users u ON u.id = ura.user_id AND u.status = 'active'
           JOIN public.cooperative_memberships m
             ON m.user_id = ura.user_id AND m.cooperative_id = p_scope
            AND m.status = 'active'
          WHERE public.app_rbac_in_scope(ura.cooperative_id, p_scope))
    END
$function$;
REVOKE ALL ON FUNCTION public.app_admin_path_count(uuid) FROM PUBLIC;

-- ------------------------------------------------------------------
-- Internal: verify the sealed context AND independently re-check that
-- the sealed actor holds p_permission inside the verified scope.
-- Returns the verified scope (NULL in the never-bootstrapped lane).
-- Raises insufficient_privilege on any failure — callers map it to
-- Forbidden; it is unreachable on the guarded HTTP path.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_rbac_verified_scope(p_permission text)
RETURNS uuid
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_user  uuid;
    v_scope uuid;
    v_ok    boolean;
BEGIN
    v_user  := nullif(current_setting('app.user_id', true), '')::uuid;
    v_scope := nullif(current_setting('app.cooperative_id', true), '')::uuid;

    IF session_user <> 'kooperatif_app' THEN
        -- Owner lane (CLI/migrations/fixtures and the owner-pool
        -- development fallback): trusted for permission checks, but
        -- attribution still comes from the stamped GUCs so a sealed
        -- owner-side request transaction writes its real cooperative
        -- instead of NULL.
        RETURN v_scope;
    END IF;

    IF v_scope IS NOT NULL THEN
        IF NOT public.app_ctx_ok() THEN
            RAISE EXCEPTION 'insufficient_privilege: sealed cooperative context required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        v_ok := EXISTS(
            SELECT 1
              FROM public.user_role_assignments ura
              JOIN public.roles r
                ON r.id = ura.role_id AND r.status = 'active'
               AND public.app_rbac_in_scope(r.cooperative_id, v_scope)
              JOIN public.role_permissions rp ON rp.role_id = r.id
              JOIN public.permissions p
                ON p.id = rp.permission_id AND p.key = p_permission
              JOIN public.cooperative_memberships m
                ON m.cooperative_id = v_scope
               AND m.user_id = ura.user_id
               AND m.status = 'active'
             WHERE ura.user_id = v_user
               AND public.app_rbac_in_scope(ura.cooperative_id, v_scope));
    ELSE
        -- Never-bootstrapped lane: user-only seal AND the installation
        -- must still be pre-tenancy at mutation time.
        IF NOT public.app_user_ctx_ok()
           OR EXISTS (SELECT 1 FROM public.cooperatives) THEN
            RAISE EXCEPTION 'insufficient_privilege: sealed pre-tenancy context required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        v_ok := EXISTS(
            SELECT 1
              FROM public.user_role_assignments ura
              JOIN public.roles r
                ON r.id = ura.role_id AND r.status = 'active'
               AND r.cooperative_id IS NULL
              JOIN public.role_permissions rp ON rp.role_id = r.id
              JOIN public.permissions p
                ON p.id = rp.permission_id AND p.key = p_permission
             WHERE ura.user_id = v_user
               AND ura.cooperative_id IS NULL);
    END IF;

    IF NOT v_ok THEN
        RAISE EXCEPTION 'insufficient_privilege: permission % required', p_permission
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN v_scope;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_rbac_verified_scope(text) FROM PUBLIC;

-- ------------------------------------------------------------------
-- Mutators (EXECUTE granted to kooperatif_app below).
-- ------------------------------------------------------------------

-- Create a role attributed to the verified scope (never to a
-- caller-supplied cooperative; NULL only in the pre-tenancy lane).
CREATE OR REPLACE FUNCTION public.app_role_create(
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
    INSERT INTO public.roles (name, description, cooperative_id)
    VALUES (p_name, p_description, v_scope)
    RETURNING id INTO v_id;
    RETURN v_id;
END;
$function$;

-- Update name/description of a role inside the verified scope.
-- Returns the role id, or NULL when the role is outside the scope.
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
       AND public.app_rbac_in_scope(cooperative_id, v_scope)
    RETURNING id INTO v_id;
    RETURN v_id;
END;
$function$;

-- Change a role's status inside the verified scope. Disabling the
-- scope's last administration path raises check_violation
-- 'lockout_prevented' (mapped to LockoutPrevented by the caller).
-- Returns the role id, or NULL when outside scope / already in state.
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
       AND public.app_rbac_in_scope(cooperative_id, v_scope)
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

-- Replace a role's permission set. The role must be inside the
-- verified scope — role_permissions has no cooperative_id of its
-- own, so scope is enforced through the parent role (never through
-- the bare FK). Unknown catalog keys raise data_exception; stripping
-- the last administration path raises 'lockout_prevented'.
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
    v_scope uuid := public.app_rbac_verified_scope('roles.manage');
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM public.roles r
         WHERE r.id = p_role_id
           AND public.app_rbac_in_scope(r.cooperative_id, v_scope)
    ) THEN
        RAISE EXCEPTION 'insufficient_privilege: role outside verified scope'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

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
END;
$function$;

-- Replace a user's role assignments inside the verified scope.
-- Target must hold a membership row in the scope (any status —
-- mirroring member_visibility); every requested role must be active
-- and, in a cooperative scope, OWNED by that cooperative (NULL roles
-- stay visible but are not assignable, matching the handler policy).
-- Removing the last administration path raises 'lockout_prevented'.
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
       AND public.app_rbac_in_scope(cooperative_id, v_scope);

    INSERT INTO public.user_role_assignments (user_id, role_id, cooperative_id)
    SELECT p_user_id, r.id, v_scope
      FROM (SELECT DISTINCT unnest(p_role_ids) AS id) k
      JOIN public.roles r ON r.id = k.id;

    IF public.app_admin_path_count(v_scope) = 0 THEN
        RAISE EXCEPTION 'lockout_prevented' USING ERRCODE = 'check_violation';
    END IF;
END;
$function$;

-- ------------------------------------------------------------------
-- Grants: mutators executable by the runtime role; internal helpers
-- stay unreachable; direct write privileges are revoked.
-- ------------------------------------------------------------------
REVOKE ALL ON FUNCTION public.app_role_create(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_role_update(uuid, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_role_set_status(uuid, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_role_permissions_set(uuid, text[]) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_user_roles_set(uuid, uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_role_create(text, text) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_role_update(uuid, text, text) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_role_set_status(uuid, text) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_role_permissions_set(uuid, text[]) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_user_roles_set(uuid, uuid[]) TO kooperatif_app;

REVOKE INSERT, UPDATE ON public.roles FROM kooperatif_app;
REVOKE INSERT, DELETE ON public.user_role_assignments FROM kooperatif_app;
REVOKE INSERT, DELETE ON public.role_permissions FROM kooperatif_app;

-- ------------------------------------------------------------------
-- Policy narrowing: after grant revocation the app policies exist
-- only for reads. FOR SELECT keeps the identical visibility
-- predicate and turns any future re-granted write attempt into a
-- default-deny instead of a policy-evaluated write.
-- ------------------------------------------------------------------
DROP POLICY roles_app_scope ON public.roles;
CREATE POLICY roles_app_scope
    ON public.roles FOR SELECT TO kooperatif_app
    USING (
        (public.app_ctx_ok() AND (
            cooperative_id = nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND EXISTS(
                SELECT 1 FROM public.installation_state s
                JOIN public.cooperatives c
                  ON c.id = s.bootstrap_cooperative_id AND c.is_bootstrap
                 WHERE s.bootstrap_cooperative_id =
                       nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR (public.app_user_ctx_ok()
            AND cooperative_id IS NULL
            AND NOT EXISTS (SELECT 1 FROM public.cooperatives)));

DROP POLICY user_role_assignments_app_scope ON public.user_role_assignments;
CREATE POLICY user_role_assignments_app_scope
    ON public.user_role_assignments FOR SELECT TO kooperatif_app
    USING (
        (public.app_ctx_ok() AND (
            cooperative_id = nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND EXISTS(
                SELECT 1 FROM public.installation_state s
                JOIN public.cooperatives c
                  ON c.id = s.bootstrap_cooperative_id AND c.is_bootstrap
                 WHERE s.bootstrap_cooperative_id =
                       nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR (public.app_user_ctx_ok()
            AND cooperative_id IS NULL
            AND NOT EXISTS (SELECT 1 FROM public.cooperatives)));

-- ------------------------------------------------------------------
-- Post-checks (fail closed inside the migration transaction).
-- ------------------------------------------------------------------
DO $$
BEGIN
    -- Direct write privileges are gone; SELECT survives.
    IF EXISTS (
        SELECT 1 FROM information_schema.role_table_grants
         WHERE grantee = 'kooperatif_app'
           AND ((table_name = 'roles' AND privilege_type IN ('INSERT','UPDATE','DELETE'))
             OR (table_name = 'user_role_assignments' AND privilege_type IN ('INSERT','DELETE','UPDATE'))
             OR (table_name = 'role_permissions' AND privilege_type IN ('INSERT','DELETE','UPDATE'))))
    THEN RAISE EXCEPTION 'r1 postcheck: residual app write grant';
    END IF;
    IF (SELECT count(*) FROM information_schema.role_table_grants
         WHERE grantee = 'kooperatif_app' AND privilege_type = 'SELECT'
           AND table_name IN ('roles','user_role_assignments','role_permissions')) <> 3
    THEN RAISE EXCEPTION 'r1 postcheck: expected SELECT retained on 3 rbac tables';
    END IF;
    -- Mutator EXECUTE surface: exactly the five boundary functions.
    IF (SELECT count(*) FROM information_schema.routine_privileges
         WHERE grantee = 'kooperatif_app'
           AND routine_name IN ('app_role_create','app_role_update','app_role_set_status',
                                'app_role_permissions_set','app_user_roles_set')) <> 5
    THEN RAISE EXCEPTION 'r1 postcheck: mutator EXECUTE surface mismatch';
    END IF;
    -- Internal helpers must NOT be executable by the runtime role.
    IF EXISTS (
        SELECT 1 FROM information_schema.routine_privileges
         WHERE grantee = 'kooperatif_app'
           AND routine_name IN ('app_rbac_in_scope','app_rbac_verified_scope','app_admin_path_count'))
    THEN RAISE EXCEPTION 'r1 postcheck: internal helper granted to app';
    END IF;
END $$;

COMMIT;
