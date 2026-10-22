-- M1-P4-S5 — FORCE RLS on `roles` and `user_role_assignments`.
--
-- Access-path inventory (verified in code):
--
--   OWNER/CLI lane — main.rs user administration (role lookup by name,
--   assignment insert) runs on the owner pool: superuser/BYPASSRLS,
--   unaffected by FORCE RLS. Owner policy below is belt-and-braces.
--
--   DEFINER lane — every pre-context/authorization read is already
--   inside a kooperatif-owned SECURITY DEFINER (owner lane, RLS-exempt):
--   app_session_permissions_scoped/legacy,
--   app_session_role_summaries_scoped/legacy (S2A), session opening.
--
--   SEALED-TRANSACTION lane (kooperatif_app, direct) — rbac.rs CRUD
--   and authz::administration_path_count read/mutate both tables
--   inside request_tx transactions, always through the
--   `scoped_visibility` predicate: a cooperative scope sees its own
--   rows plus NULL-attributed legacy rows ONLY when the scope
--   re-verifies as the marker-consistent bootstrap cooperative; the
--   NULL scope (never-bootstrapped user lane, request_user_tx) sees
--   NULL rows only while zero cooperatives exist.
--
--   Pool-based helpers (effective_permissions*, active_role_summaries*,
--   require*) have no live route callers — owner-pool tests only.
--
-- Policy set per table: owner lane + ONE app lane that reproduces the
-- scoped_visibility semantics at the row level:
--   (a) sealed context: cooperative_id = sealed coop, OR NULL rows
--       when the sealed coop is the verified bootstrap cooperative;
--   (b) user-seal context (no cooperative stamped): NULL rows only,
--       and only while zero cooperatives exist — a NULL scope can
--       never become unrestricted once tenancy exists.
-- WITH CHECK mirrors USING: cross-cooperative writes are invisible
-- targets and NULL-attributed writes are impossible outside the two
-- sanctioned lanes (bootstrap scope management, pre-tenancy lane).
--
-- Grants are kept exactly as they are: roles = SELECT/INSERT/UPDATE
-- (list+load, create, update/status), user_role_assignments =
-- SELECT/INSERT/DELETE (load, assign, replace-by-delete) — all are
-- genuinely exercised by rbac.rs; nothing broader exists to revoke.
--
-- New helper: app_user_ctx_ok() — the user-seal counterpart of
-- app_ctx_ok(). The cooperative seal chain (app_open_request_context)
-- always mints user_seal first; request_user_tx deliberately stops
-- there. The predicate verifies that seal exactly like app_ctx_ok
-- verifies ctx_seal, with NULL cooperative in the HMAC payload.
-- It reads GUCs + the owner-only secret only — no circular RLS
-- dependency on the tables it guards.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 1. User-seal predicate (never-bootstrapped lane) ──────────────

CREATE OR REPLACE FUNCTION public.app_user_ctx_ok()
RETURNS boolean
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
    SELECT
        nullif(current_setting('app.user_id', true), '') IS NOT NULL
        AND nullif(current_setting('app.session_id', true), '') IS NOT NULL
        AND nullif(current_setting('app.ctx_txid', true), '')
                = txid_current()::text
        AND nullif(current_setting('app.user_seal', true), '')
                = public.app_seal_of(
                    nullif(current_setting('app.user_id', true), '')::uuid,
                    nullif(current_setting('app.session_id', true), '')::uuid,
                    NULL,
                    nullif(current_setting('app.ctx_txid', true), '')::bigint);
$fn$;
REVOKE ALL ON FUNCTION public.app_user_ctx_ok() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_user_ctx_ok() TO kooperatif_app;

-- ── 2. roles ──────────────────────────────────────────────────────

ALTER TABLE public.roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.roles FORCE ROW LEVEL SECURITY;

CREATE POLICY roles_owner_lane
    ON public.roles
    TO kooperatif
    USING (true)
    WITH CHECK (true);

CREATE POLICY roles_app_scope
    ON public.roles
    FOR ALL
    TO kooperatif_app
    USING (
        (public.app_ctx_ok() AND (
            cooperative_id =
                nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND EXISTS(
                SELECT 1 FROM public.installation_state s
                JOIN public.cooperatives c
                  ON c.id = s.bootstrap_cooperative_id AND c.is_bootstrap
                WHERE s.bootstrap_cooperative_id =
                    nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR
        (public.app_user_ctx_ok() AND cooperative_id IS NULL
         AND NOT EXISTS (SELECT 1 FROM public.cooperatives))
    )
    WITH CHECK (
        (public.app_ctx_ok() AND (
            cooperative_id =
                nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND EXISTS(
                SELECT 1 FROM public.installation_state s
                JOIN public.cooperatives c
                  ON c.id = s.bootstrap_cooperative_id AND c.is_bootstrap
                WHERE s.bootstrap_cooperative_id =
                    nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR
        (public.app_user_ctx_ok() AND cooperative_id IS NULL
         AND NOT EXISTS (SELECT 1 FROM public.cooperatives))
    );

-- ── 3. user_role_assignments ──────────────────────────────────────

ALTER TABLE public.user_role_assignments ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.user_role_assignments FORCE ROW LEVEL SECURITY;

CREATE POLICY user_role_assignments_owner_lane
    ON public.user_role_assignments
    TO kooperatif
    USING (true)
    WITH CHECK (true);

CREATE POLICY user_role_assignments_app_scope
    ON public.user_role_assignments
    FOR ALL
    TO kooperatif_app
    USING (
        (public.app_ctx_ok() AND (
            cooperative_id =
                nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND EXISTS(
                SELECT 1 FROM public.installation_state s
                JOIN public.cooperatives c
                  ON c.id = s.bootstrap_cooperative_id AND c.is_bootstrap
                WHERE s.bootstrap_cooperative_id =
                    nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR
        (public.app_user_ctx_ok() AND cooperative_id IS NULL
         AND NOT EXISTS (SELECT 1 FROM public.cooperatives))
    )
    WITH CHECK (
        (public.app_ctx_ok() AND (
            cooperative_id =
                nullif(current_setting('app.cooperative_id', true), '')::uuid
            OR (cooperative_id IS NULL AND EXISTS(
                SELECT 1 FROM public.installation_state s
                JOIN public.cooperatives c
                  ON c.id = s.bootstrap_cooperative_id AND c.is_bootstrap
                WHERE s.bootstrap_cooperative_id =
                    nullif(current_setting('app.cooperative_id', true), '')::uuid))))
        OR
        (public.app_user_ctx_ok() AND cooperative_id IS NULL
         AND NOT EXISTS (SELECT 1 FROM public.cooperatives))
    );

-- ── Fail-closed post-checks ───────────────────────────────────────

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class
         WHERE relname IN ('roles', 'user_role_assignments')
           AND relnamespace = 'public'::regnamespace
           AND relrowsecurity AND relforcerowsecurity
        HAVING count(*) = 2
    ) THEN
        RAISE EXCEPTION 'roles/user_role_assignments RLS not enabled+forced';
    END IF;

    IF (SELECT count(*) FROM pg_policy
         WHERE polrelid = 'public.roles'::regclass) <> 2
       OR (SELECT count(*) FROM pg_policy
            WHERE polrelid = 'public.user_role_assignments'::regclass) <> 2 THEN
        RAISE EXCEPTION 'roles/user_role_assignments must carry owner + app policies';
    END IF;

    -- Exact grant surfaces: no new verb may appear, none may drop.
    IF NOT has_table_privilege('kooperatif_app', 'public.roles', 'SELECT')
       OR NOT has_table_privilege('kooperatif_app', 'public.roles', 'INSERT')
       OR NOT has_table_privilege('kooperatif_app', 'public.roles', 'UPDATE')
       OR has_table_privilege('kooperatif_app', 'public.roles', 'DELETE') THEN
        RAISE EXCEPTION 'roles grant surface drifted';
    END IF;
    IF NOT has_table_privilege('kooperatif_app', 'public.user_role_assignments', 'SELECT')
       OR NOT has_table_privilege('kooperatif_app', 'public.user_role_assignments', 'INSERT')
       OR NOT has_table_privilege('kooperatif_app', 'public.user_role_assignments', 'DELETE')
       OR has_table_privilege('kooperatif_app', 'public.user_role_assignments', 'UPDATE') THEN
        RAISE EXCEPTION 'user_role_assignments grant surface drifted';
    END IF;

    IF NOT has_function_privilege(
        'kooperatif_app', 'public.app_user_ctx_ok()', 'EXECUTE') THEN
        RAISE EXCEPTION 'app_user_ctx_ok must be reachable inside policies';
    END IF;

    -- Fleet boundary: 40 tables / 78 policies (through S4)
    -- + 2 tables / 4 policies.
    IF (SELECT count(*) FROM pg_class c
         WHERE c.relrowsecurity AND c.relforcerowsecurity
           AND c.relnamespace = 'public'::regnamespace
           AND c.relkind = 'r') <> 42
       OR (SELECT count(*) FROM pg_policy p
            JOIN pg_class c ON c.oid = p.polrelid
           WHERE c.relnamespace = 'public'::regnamespace) <> 82 THEN
        RAISE EXCEPTION 'RLS fleet boundary drifted';
    END IF;
END $$;
