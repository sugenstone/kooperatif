-- M1-P4-P2A — runtime principal hardening.
--
-- Migration 0032 granted kooperatif_app blanket CRUD on every public
-- table plus unrestricted future-table default privileges. That surface
-- was deliberately behavior-identical plumbing; this migration narrows
-- it to the minimum verified by the HTTP runtime audit:
--
--   * business tables (cooperative_id NOT NULL) keep full CRUD;
--   * platform/auth/security metadata drops to the exact verb set the
--     HTTP paths exercise — CLI-only writers (create_cooperative,
--     add_member, bootstrap, create_user) keep no runtime write grant;
--   * unrestricted ALTER DEFAULT PRIVILEGES are revoked — every new
--     table must be granted explicitly by its own migration;
--   * _sqlx_migrations stays fully denied.
--
-- The SECURITY DEFINER shims are hardened: fully-qualified object
-- references and a caller-binding guard — when app.user_id is stamped
-- (P2B), the membership lookup may only resolve THAT user; an unset
-- GUC still serves the pre-context bootstrap lane until then.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ------------------------------------------------------------------
-- 1. Drop the blanket grant and the unrestricted future-table defaults.
-- ------------------------------------------------------------------
REVOKE SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public
    FROM kooperatif_app;
REVOKE USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public
    FROM kooperatif_app;
REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA public
    FROM kooperatif_app;

ALTER DEFAULT PRIVILEGES FOR ROLE kooperatif IN SCHEMA public
    REVOKE SELECT, INSERT, UPDATE, DELETE ON TABLES FROM kooperatif_app;
ALTER DEFAULT PRIVILEGES FOR ROLE kooperatif IN SCHEMA public
    REVOKE USAGE, SELECT ON SEQUENCES FROM kooperatif_app;
ALTER DEFAULT PRIVILEGES FOR ROLE kooperatif IN SCHEMA public
    REVOKE EXECUTE ON FUNCTIONS FROM kooperatif_app;

-- ------------------------------------------------------------------
-- 2. Business tables (cooperative_id NOT NULL) get full CRUD. The two
--    cooperative-attributed system tables with narrower runtime needs
--    are excluded here and granted explicitly below.
-- ------------------------------------------------------------------
DO $$
DECLARE
    t record;
BEGIN
    FOR t IN
        SELECT c.relname
          FROM pg_class c
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public'
           AND c.relkind = 'r'
           AND EXISTS (
               SELECT 1 FROM pg_attribute a
                WHERE a.attrelid = c.oid
                  AND a.attname = 'cooperative_id'
                  AND a.attnotnull
                  AND NOT a.attisdropped)
           AND c.relname NOT IN ('cooperative_memberships')
    LOOP
        EXECUTE format(
            'GRANT SELECT, INSERT, UPDATE, DELETE ON public.%I TO kooperatif_app',
            t.relname);
    END LOOP;
END $$;

-- ------------------------------------------------------------------
-- 3. Platform / auth / security metadata — exact minimal verbs.
-- ------------------------------------------------------------------

-- Identity reads + login/status writes only. User creation and session
-- destruction are CLI lanes; sessions are revoked, never deleted.
GRANT SELECT, UPDATE ON public.users TO kooperatif_app;
GRANT SELECT, INSERT, UPDATE ON public.user_sessions TO kooperatif_app;

-- Audit writer: append + read (never_bootstrapped probe, listings).
GRANT SELECT, INSERT ON public.security_events TO kooperatif_app;

-- RBAC surface reachable over HTTP (rbac router): role CRUD via
-- status flips, permission set replacement, assignment replacement.
-- `permissions` and the system templates are read-only catalogs.
GRANT SELECT, INSERT, UPDATE ON public.roles TO kooperatif_app;
GRANT SELECT, INSERT, DELETE ON public.role_permissions TO kooperatif_app;
GRANT SELECT, INSERT, DELETE ON public.user_role_assignments TO kooperatif_app;
GRANT SELECT ON public.permissions TO kooperatif_app;
GRANT SELECT ON public.system_role_templates TO kooperatif_app;
GRANT SELECT ON public.system_role_template_permissions TO kooperatif_app;

-- Pre-context reads only. Membership/cooperative/bootstrap WRITES are
-- admin lanes (create-cooperative, add-member, bootstrap-cooperative).
GRANT SELECT ON public.cooperative_memberships TO kooperatif_app;
GRANT SELECT ON public.cooperatives TO kooperatif_app;
GRANT SELECT ON public.installation_state TO kooperatif_app;

-- Numbering registry is read-only for the runtime; counters live in
-- cooperative_number_counters (business table, CRUD above).
GRANT SELECT ON public.document_number_series TO kooperatif_app;

-- The migration ledger stays fully denied (defense in depth — 0032
-- already revoked it, restated so the post-check below is
-- self-contained).
REVOKE ALL ON TABLE public._sqlx_migrations FROM kooperatif_app;

-- Numbering functions are invoked inside business INSERT paths.
GRANT EXECUTE ON FUNCTION public.allocate_document_number(uuid, text)
    TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.reserve_document_number(uuid, text, bigint)
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- 4. Hardened pre-context shims (supersede the 0032 definitions).
--
--    Changes vs 0032:
--      * fully qualified public.* object references — with the pinned
--        search_path this is belt-and-suspenders, but keeps the
--        function correct even if a future edit loosens the path;
--      * caller binding: when app.user_id is present (stamped by the
--        authenticated request context — P2B), the function resolves
--        ONLY that user. An unset/empty GUC preserves the pre-context
--        bootstrap lane until P2B wires session->GUC stamping. The
--        guard turns caller-supplied p_user into a checked claim
--        instead of a free parameter.
--      * no dynamic SQL, no identifier parameters, STABLE only.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_verify_membership(
    p_user uuid,
    p_cooperative uuid
)
RETURNS TABLE (
    membership_status text,
    cooperative_name text,
    cooperative_status text,
    business_enabled boolean
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
    SELECT m.status::text, c.name::text, c.status::text, c.business_enabled
      FROM public.cooperative_memberships m
      JOIN public.cooperatives c ON c.id = m.cooperative_id
     WHERE m.user_id = p_user
       AND m.cooperative_id = p_cooperative
       AND (
            current_setting('app.user_id', true) IS NULL
            OR current_setting('app.user_id', true) = ''
            OR current_setting('app.user_id', true)::uuid = p_user
       )
$$;

CREATE OR REPLACE FUNCTION public.app_resolve_bootstrap()
RETURNS TABLE (cooperative_id uuid, cooperative_name text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
    SELECT c.id, c.name::text
      FROM public.installation_state s
      JOIN public.cooperatives c ON c.id = s.bootstrap_cooperative_id
     LIMIT 1
$$;

REVOKE ALL ON FUNCTION public.app_verify_membership(uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.app_resolve_bootstrap() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_verify_membership(uuid, uuid)
    TO kooperatif_app;
GRANT EXECUTE ON FUNCTION public.app_resolve_bootstrap()
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- 5. Fail-closed post-checks: the restricted surface must be exactly
--    what this migration declared.
-- ------------------------------------------------------------------
DO $$
DECLARE
    bad record;
BEGIN
    -- No write privilege may remain on any of these system tables.
    FOR bad IN
        SELECT tbl
          FROM (VALUES
              ('users',                   'INSERT,DELETE'),
              ('user_sessions',           'DELETE'),
              ('security_events',         'UPDATE,DELETE'),
              ('roles',                   'DELETE'),
              ('permissions',             'INSERT,UPDATE,DELETE'),
              ('system_role_templates',   'INSERT,UPDATE,DELETE'),
              ('system_role_template_permissions','INSERT,UPDATE,DELETE'),
              ('cooperative_memberships', 'INSERT,UPDATE,DELETE'),
              ('cooperatives',            'INSERT,UPDATE,DELETE'),
              ('installation_state',      'INSERT,UPDATE,DELETE'),
              ('document_number_series',  'INSERT,UPDATE,DELETE'),
              ('_sqlx_migrations',        'SELECT,INSERT,UPDATE,DELETE')
          ) AS v(tbl, verbs)
         WHERE EXISTS (
             SELECT 1
               FROM (SELECT string_to_table(v.verbs, ',') AS verb) vv
              WHERE has_table_privilege(
                        'kooperatif_app', 'public.' || v.tbl, vv.verb))
    LOOP
        RAISE EXCEPTION 'kooperatif_app retains forbidden privilege on %', bad.tbl;
    END LOOP;

    -- Sanity: the required verbs are present.
    IF NOT has_table_privilege('kooperatif_app', 'public.users', 'SELECT,UPDATE')
        OR NOT has_table_privilege('kooperatif_app', 'public.user_sessions', 'SELECT,INSERT,UPDATE')
        OR NOT has_table_privilege('kooperatif_app', 'public.security_events', 'SELECT,INSERT')
        OR NOT has_table_privilege('kooperatif_app', 'public.roles', 'SELECT,INSERT,UPDATE')
        OR NOT has_table_privilege('kooperatif_app', 'public.user_role_assignments', 'SELECT,INSERT,DELETE')
        OR NOT has_table_privilege('kooperatif_app', 'public.cooperative_memberships', 'SELECT')
        OR NOT has_function_privilege('kooperatif_app', 'public.app_verify_membership(uuid,uuid)', 'EXECUTE')
        OR NOT has_function_privilege('kooperatif_app', 'public.allocate_document_number(uuid,text)', 'EXECUTE')
    THEN
        RAISE EXCEPTION 'kooperatif_app is missing a required privilege';
    END IF;
END $$;
