-- M1-P4-P1 — restricted application role for the upcoming RLS phase.
--
-- `kooperatif` stays the schema owner / migration / administrative
-- lane. `kooperatif_app` is the runtime principal the API will connect
-- as once `KOOPERATIF_APP_DATABASE_URL` points at it: NOSUPERUSER,
-- NOBYPASSRLS, NOCREATEDB — row-level security will apply to it the
-- moment policies are enabled (M1-P4-P3). This migration grants the
-- same CRUD surface the app already exercises so behavior is identical
-- until RLS lands.
--
-- The role is cluster-global while migrations are per-database: every
-- disposable test database runs this file too, so creation is guarded
-- by IF NOT EXISTS under a cluster-wide advisory lock (concurrent test
-- setups would otherwise race CREATE ROLE). Grants are per-database
-- and simply re-applied on repeat runs. The password is intentionally
-- NOT set here — it is provisioned out-of-band per environment
-- (`ALTER ROLE kooperatif_app PASSWORD …`), never committed.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- Serialize concurrent test-database migration runs on one shared
-- cluster-global key — CREATE/ALTER ROLE touches the shared pg_authid
-- catalog. The lock must be a top-level statement so the lock wait
-- completes before any snapshot below is taken. xact variant
-- auto-releases at transaction end.
SELECT pg_advisory_xact_lock(hashtextextended('kooperatif_app_role_create', 0));

DO $$
DECLARE
    attempt int;
BEGIN
    -- Bounded retry: catalog updates ("tuple concurrently updated",
    -- lock_timeout hits) can still race snapshots on shared catalogs
    -- even under the advisory lock. A caught exception re-snapshots on
    -- the next attempt (READ COMMITTED), so the loop converges.
    FOR attempt IN 1..20 LOOP
        BEGIN
            IF NOT EXISTS (
                SELECT 1 FROM pg_roles WHERE rolname = 'kooperatif_app'
            ) THEN
                CREATE ROLE kooperatif_app
                    LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
            END IF;
            -- Harden the role even if it already existed with weaker attributes.
            ALTER ROLE kooperatif_app
                NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
            RETURN;
        EXCEPTION
            WHEN serialization_failure OR deadlock_detected
                OR lock_not_available OR internal_error THEN
                PERFORM pg_sleep(0.05 * attempt);
        END;
    END LOOP;
    RAISE EXCEPTION 'kooperatif_app role could not be provisioned after 20 attempts';
END $$;

-- Read/write on everything the API uses today. Platform tables
-- (users, user_sessions, cooperatives, …) stay readable/writable —
-- they have no cooperative_id and are NOT RLS targets.
GRANT USAGE ON SCHEMA public TO kooperatif_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO kooperatif_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO kooperatif_app;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO kooperatif_app;

-- The migration ledger is DDL territory — the runtime role must never
-- touch it, and pg_*/system catalogs are untouched.
REVOKE ALL ON TABLE _sqlx_migrations FROM kooperatif_app;

-- Objects the owner creates later inherit the same grant shape.
ALTER DEFAULT PRIVILEGES FOR ROLE kooperatif IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO kooperatif_app;
ALTER DEFAULT PRIVILEGES FOR ROLE kooperatif IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO kooperatif_app;
ALTER DEFAULT PRIVILEGES FOR ROLE kooperatif IN SCHEMA public
    GRANT EXECUTE ON FUNCTIONS TO kooperatif_app;

-- ------------------------------------------------------------------
-- Pre-context membership verification lane (M1-P4-P1).
--
-- RLS cannot read cooperative_memberships to decide whether a tenant
-- context may be established — the lookup itself is cooperative-scoped
-- data. The one sanctioned bypass is this SECURITY DEFINER shim, owned
-- by `kooperatif`: it answers ONLY "is (user, cooperative) an
-- active/enabled membership" and returns exactly the fields the
-- extractor needs. `SET search_path` pins resolution; callers cannot
-- widen it (no parameters reach identifiers). Grant is narrowed below
-- so only the app role may call it.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION app_verify_membership(p_user uuid, p_cooperative uuid)
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
      FROM cooperative_memberships m
      JOIN cooperatives c ON c.id = m.cooperative_id
     WHERE m.user_id = p_user AND m.cooperative_id = p_cooperative
$$;

-- Bootstrap resolution follows the same pre-context problem: the
-- is_bootstrap marker read must work before any cooperative GUC exists.
CREATE OR REPLACE FUNCTION app_resolve_bootstrap()
RETURNS TABLE (cooperative_id uuid, cooperative_name text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
    SELECT c.id, c.name::text
      FROM installation_state s
      JOIN cooperatives c ON c.id = s.bootstrap_cooperative_id
     LIMIT 1
$$;

REVOKE ALL ON FUNCTION app_verify_membership(uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_resolve_bootstrap() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app_verify_membership(uuid, uuid) TO kooperatif_app;
GRANT EXECUTE ON FUNCTION app_resolve_bootstrap() TO kooperatif_app;

-- Fail-closed post-check: the role must exist with the hardened
-- attribute set in THIS database's cluster view.
DO $$
DECLARE
    app_role record;
BEGIN
    SELECT rolsuper, rolcreaterole, rolcreatedb, rolreplication, rolbypassrls, rolcanlogin
      INTO app_role FROM pg_roles WHERE rolname = 'kooperatif_app';
    IF NOT FOUND THEN
        RAISE EXCEPTION 'kooperatif_app role was not created';
    END IF;
    IF app_role.rolsuper OR app_role.rolcreaterole OR app_role.rolcreatedb
        OR app_role.rolreplication OR app_role.rolbypassrls
        OR NOT app_role.rolcanlogin THEN
        RAISE EXCEPTION 'kooperatif_app has unsafe role attributes';
    END IF;
END $$;
