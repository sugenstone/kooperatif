-- M1-P4-R2 — users table privilege boundary (closure-audit F3).
--
-- The audit proved that the runtime principal held
-- UPDATE(status, updated_at) plus SELECT over seven non-credential
-- columns on `users` with NO row-level boundary (the table has no
-- cooperative_id and carried no RLS):
--
--   * an UNAUTHENTICATED kooperatif_app connection could run
--     `UPDATE users SET status='disabled'` on ANY user — including
--     users of another cooperative — a global lockout / silent
--     reactivation primitive;
--   * `SELECT id, username, display_name, status, created_at,
--     updated_at, last_login_at` enumerated the entire identity table
--     across all cooperatives;
--   * credential columns (password_hash, login_verifier) were already
--     column-invisible — that part of the boundary held.
--
-- Remediation invariant: a valid session / sealed cooperative context
-- alone does NOT authorize user-status mutations, and a sealed context
-- only ever exposes users that hold a membership row in the verified
-- cooperative.
--
--   * `app_user_set_status` is the ONLY runtime mutation lane: it
--     re-verifies `users.manage` inside the database
--     (app_rbac_verified_scope), applies the member_visibility rule
--     (membership row in the verified scope, any status; NULL scope =
--     the never-bootstrapped lane only while zero cooperatives exist),
--     and re-enforces the last-administrator path for disables —
--     atomically inside one statement.
--   * `users` gains ENABLE + FORCE RLS. The app policy is FOR SELECT:
--     sealed contexts see members of their verified cooperative
--     (membership at ANY status — ending a membership must not
--     silently orphan administrative visibility); the user-seal
--     never-bootstrapped lane sees all rows only while zero
--     cooperatives exist, mirroring member_visibility's NULL arm.
--   * Direct UPDATE is revoked; SELECT narrows to the four columns
--     the runtime actually reads (id, username, display_name, status).
--     created_at / updated_at / last_login_at leave the runtime
--     surface entirely.
--
-- IMPORTANT business-meaning note (recorded, not changed): users.status
-- is a GLOBAL identity attribute — disabling a user kills all their
-- sessions everywhere (app_admin_revoke_user_sessions stays route-side,
-- same guarded transaction). A user holding memberships in several
-- cooperatives can be disabled by an administrator of ANY of them.
-- That is the pre-existing member_visibility semantic; narrowing it to
-- cooperative-scoped status is a domain-spec change, out of R2 scope.
BEGIN;
SET LOCAL statement_timeout = '10s';

-- ------------------------------------------------------------------
-- Boundary mutator: user status change inside the verified scope.
-- Returns the user id, or NULL when the target is outside the
-- verified scope / already in the requested state (idempotent).
-- Disabling the scope's last administration path raises
-- check_violation 'lockout_prevented' (mapped to LockoutPrevented by
-- the caller). users.status CHECK constrains the value domain.
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
    UPDATE public.users u
       SET status = p_status, updated_at = now()
     WHERE u.id = p_user_id
       AND u.status <> p_status
       AND ((v_scope IS NULL
             AND NOT EXISTS (SELECT 1 FROM public.cooperatives))
            OR EXISTS (SELECT 1 FROM public.cooperative_memberships m
                        WHERE m.user_id = u.id
                          AND m.cooperative_id = v_scope))
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

REVOKE ALL ON FUNCTION public.app_user_set_status(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_user_set_status(uuid, text) TO kooperatif_app;

-- ------------------------------------------------------------------
-- Grant boundary: no runtime writes; reads shrink to the columns the
-- sealed lanes actually select (member lists, administration-path
-- counts, audit display joins). Credential columns were never granted.
-- ------------------------------------------------------------------
REVOKE UPDATE (status, updated_at) ON public.users FROM kooperatif_app;
REVOKE SELECT (created_at, updated_at, last_login_at) ON public.users
    FROM kooperatif_app;

-- ------------------------------------------------------------------
-- Row boundary: members-of-the-verified-scope visibility, mirroring
-- member_visibility (users carry no cooperative_id — scope is derived
-- through cooperative_memberships, which is itself FORCE-RLS and
-- filters the policy subquery to the sealed cooperative anyway).
-- FOR SELECT only: should a write grant ever return, default-deny.
-- ------------------------------------------------------------------
ALTER TABLE public.users ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.users FORCE ROW LEVEL SECURITY;

CREATE POLICY users_owner_lane
    ON public.users FOR ALL TO kooperatif
    USING (true) WITH CHECK (true);

CREATE POLICY users_app_scope
    ON public.users FOR SELECT TO kooperatif_app
    USING (
        (public.app_ctx_ok() AND EXISTS(
            SELECT 1 FROM public.cooperative_memberships m
             WHERE m.user_id = users.id
               AND m.cooperative_id =
                   nullif(current_setting('app.cooperative_id', true), '')::uuid))
        OR (public.app_user_ctx_ok()
            AND NOT EXISTS (SELECT 1 FROM public.cooperatives)));

-- ------------------------------------------------------------------
-- Post-checks (fail closed inside the migration transaction).
-- ------------------------------------------------------------------
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relname = 'users'
           AND c.relrowsecurity AND c.relforcerowsecurity)
    THEN RAISE EXCEPTION 'r2 postcheck: users must be ENABLE+FORCE RLS';
    END IF;
    IF (SELECT count(*) FROM pg_policies
         WHERE schemaname = 'public' AND tablename = 'users') <> 2
    THEN RAISE EXCEPTION 'r2 postcheck: users must carry exactly 2 policies';
    END IF;
    -- No residual write privilege.
    IF EXISTS (
        SELECT 1 FROM information_schema.column_privileges
         WHERE grantee = 'kooperatif_app' AND table_name = 'users'
           AND privilege_type <> 'SELECT')
    THEN RAISE EXCEPTION 'r2 postcheck: residual non-SELECT grant on users';
    END IF;
    -- Exactly the four readable columns.
    IF (SELECT count(*) FROM information_schema.column_privileges
         WHERE grantee = 'kooperatif_app' AND table_name = 'users'
           AND privilege_type = 'SELECT'
           AND column_name IN ('id','username','display_name','status')) <> 4
       OR EXISTS (
        SELECT 1 FROM information_schema.column_privileges
         WHERE grantee = 'kooperatif_app' AND table_name = 'users'
           AND privilege_type = 'SELECT'
           AND column_name NOT IN ('id','username','display_name','status'))
    THEN RAISE EXCEPTION 'r2 postcheck: users SELECT surface must be exactly 4 columns';
    END IF;
    -- Boundary mutator is the only new EXECUTE surface.
    IF (SELECT count(*) FROM information_schema.routine_privileges
         WHERE grantee = 'kooperatif_app'
           AND routine_name = 'app_user_set_status') <> 1
    THEN RAISE EXCEPTION 'r2 postcheck: app_user_set_status EXECUTE missing';
    END IF;
END $$;

COMMIT;
