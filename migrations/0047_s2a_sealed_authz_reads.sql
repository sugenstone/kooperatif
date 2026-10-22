-- M1-P4-S2A — sealed authorization READ layer.
--
-- The pre-context readers (`resolve_membership`, `list_user_cooperatives`,
-- `has_enabled_business_membership`, `effective_permissions_*`,
-- `active_role_summaries_*`) used to run on the bare application pool
-- with caller-supplied `user_id`/`cooperative_id` arguments — a direct
-- membership/permission enumeration surface for any SQL the runtime
-- principal can issue, and the exact code paths that would deadlock the
-- system once `cooperative_memberships`/`roles`/`user_role_assignments`
-- take FORCE RLS (audit steps S4/S5).
--
-- This migration replaces them with credential-bound SECURITY DEFINER
-- readers, mirroring the P3B session API (`app_session_resolve`,
-- `app_session_list`, ...): the caller presents ONLY the session token
-- hash it already authenticated with, and the database derives the user
-- id from the session row itself. An arbitrary `user_id` or
-- `cooperative_id` parameter is a checked claim, never an authority:
--
--   * an unknown/expired/revoked/idle/disabled session resolves to no
--     user and every reader returns empty — fail closed;
--   * membership lookups return rows only for the session owner;
--   * permission/role resolution joins through an `active` membership
--     in the requested scope, so a foreign cooperative yields an empty
--     set (default DENY);
--   * NULL-attributed legacy role/assignment rows count only through
--     the marker-consistent bootstrap cooperative — encoded inside the
--     queries (same rule as `effective_permissions_in_bootstrap` /
--     `scoped_visibility`), so a caller-supplied non-bootstrap scope can
--     never inherit them;
--   * the legacy global lane additionally re-verifies the
--     never-bootstrapped predicate inside the function — a stale caller
--     cannot reach the global set once tenancy exists.
--
-- No `app.*` GUC is read or written here: these functions serve the
-- pre-cooperative-context lane where no seal exists yet. Identity is
-- carried exclusively by the presented credential.
--
-- Table grants are unchanged in this step: the sealed-transaction RBAC
-- surface (rbac.rs CRUD, `administration_path_count`) still reads these
-- tables as `kooperatif_app` inside `request_tx`; the remaining direct
-- grants are retired by the S3–S5 RLS steps, which this layer unblocks.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 0. Internal session-owner resolver (NOT granted to the app role) ─
--
-- Single source of the session-validity rule for the whole S2A family:
-- same predicates as `app_open_request_context`/`app_session_resolve`
-- (not revoked, absolute + idle expiry against the application clock,
-- user active). Returns NULL for anything unusable — every caller then
-- fails closed.

CREATE OR REPLACE FUNCTION public.app_session_owner(
    p_token_hash bytea,
    p_now        timestamptz
)
RETURNS uuid
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
    SELECT us.user_id
      FROM public.user_sessions us
      JOIN public.users u ON u.id = us.user_id
     WHERE us.token_hash = p_token_hash
       AND us.revoked_at IS NULL
       AND us.expires_at > p_now
       AND us.idle_expires_at > p_now
       AND u.status = 'active'
     LIMIT 1
$fn$;

REVOKE ALL ON FUNCTION public.app_session_owner(bytea, timestamptz) FROM PUBLIC;

-- ── 1. Membership readers ───────────────────────────────────────────

-- One membership row for the SESSION OWNER in the requested
-- cooperative — replaces `resolve_membership` for HTTP callers. An
-- unknown session or a cooperative the owner does not belong to
-- returns zero rows (callers map that to the uniform
-- cooperative-access denial).
CREATE OR REPLACE FUNCTION public.app_session_membership(
    p_token_hash     bytea,
    p_cooperative_id uuid,
    p_now            timestamptz
)
RETURNS TABLE(
    membership_status  text,
    cooperative_name   text,
    cooperative_status text,
    business_enabled   boolean
)
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
        RETURN;
    END IF;

    RETURN QUERY
        SELECT m.status::text, c.name::text, c.status::text, c.business_enabled
          FROM public.cooperative_memberships m
          JOIN public.cooperatives c ON c.id = m.cooperative_id
         WHERE m.user_id = v_user
           AND m.cooperative_id = p_cooperative_id;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_membership(bytea, uuid, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_membership(bytea, uuid, timestamptz)
    TO kooperatif_app;

-- Every cooperative the session owner belongs to — replaces
-- `list_user_cooperatives` (`/api/auth/me`, login response,
-- `/api/cooperatives`). The result can never contain another user's
-- memberships: the user id is derived, not supplied.
CREATE OR REPLACE FUNCTION public.app_session_memberships(
    p_token_hash bytea,
    p_now        timestamptz
)
RETURNS TABLE(
    cooperative_id    uuid,
    name              text,
    status            text,
    business_enabled  boolean,
    membership_status text
)
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
        RETURN;
    END IF;

    RETURN QUERY
        SELECT m.cooperative_id, c.name::text, c.status::text,
               c.business_enabled, m.status::text
          FROM public.cooperative_memberships m
          JOIN public.cooperatives c ON c.id = m.cooperative_id
         WHERE m.user_id = v_user
         ORDER BY c.name;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_memberships(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_memberships(bytea, timestamptz)
    TO kooperatif_app;

-- The M1 §5 gate predicate for the session owner — replaces
-- `has_enabled_business_membership` (tenant gate, headerless lane).
CREATE OR REPLACE FUNCTION public.app_session_business_membership_ok(
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

    RETURN EXISTS(
        SELECT 1
          FROM public.cooperative_memberships m
          JOIN public.cooperatives c ON c.id = m.cooperative_id
         WHERE m.user_id = v_user
           AND m.status = 'active'
           AND c.status = 'active'
           AND c.business_enabled);
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_business_membership_ok(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_business_membership_ok(bytea, timestamptz)
    TO kooperatif_app;

-- ── 2. Scoped permission / role resolution ──────────────────────────
--
-- Identical WHERE clauses to `effective_permissions_in_bootstrap` /
-- `active_role_summaries_in_bootstrap` (NULL-attribution arm encoded,
-- active membership join mandatory) — only the user id changes from a
-- caller-supplied parameter to the derived session owner. An
-- inapplicable scope (non-member, suspended, foreign cooperative)
-- resolves to an empty set.

CREATE OR REPLACE FUNCTION public.app_session_permissions_scoped(
    p_token_hash     bytea,
    p_cooperative_id uuid,
    p_now            timestamptz
)
RETURNS SETOF text
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
        RETURN;
    END IF;

    RETURN QUERY
        SELECT DISTINCT p.key
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
              AND (r.cooperative_id = p_cooperative_id
                   OR (r.cooperative_id IS NULL AND EXISTS(
                       SELECT 1 FROM public.installation_state s
                        JOIN public.cooperatives c
                          ON c.id = s.bootstrap_cooperative_id
                         AND c.is_bootstrap
                        WHERE s.bootstrap_cooperative_id = p_cooperative_id)))
          JOIN public.cooperative_memberships m
            ON m.cooperative_id = p_cooperative_id
           AND m.user_id = ura.user_id
           AND m.status = 'active'
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
         WHERE ura.user_id = v_user
           AND (ura.cooperative_id = p_cooperative_id
                OR (ura.cooperative_id IS NULL AND EXISTS(
                    SELECT 1 FROM public.installation_state s
                     JOIN public.cooperatives c
                       ON c.id = s.bootstrap_cooperative_id
                      AND c.is_bootstrap
                     WHERE s.bootstrap_cooperative_id = p_cooperative_id)));
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_permissions_scoped(bytea, uuid, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_permissions_scoped(bytea, uuid, timestamptz)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_session_role_summaries_scoped(
    p_token_hash     bytea,
    p_cooperative_id uuid,
    p_now            timestamptz
)
RETURNS TABLE(role_id uuid, role_name text)
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
        RETURN;
    END IF;

    RETURN QUERY
        SELECT r.id, r.name::text
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
              AND (r.cooperative_id = p_cooperative_id
                   OR (r.cooperative_id IS NULL AND EXISTS(
                       SELECT 1 FROM public.installation_state s
                        JOIN public.cooperatives c
                          ON c.id = s.bootstrap_cooperative_id
                         AND c.is_bootstrap
                        WHERE s.bootstrap_cooperative_id = p_cooperative_id)))
          JOIN public.cooperative_memberships m
            ON m.cooperative_id = p_cooperative_id
           AND m.user_id = ura.user_id
           AND m.status = 'active'
         WHERE ura.user_id = v_user
           AND (ura.cooperative_id = p_cooperative_id
                OR (ura.cooperative_id IS NULL AND EXISTS(
                    SELECT 1 FROM public.installation_state s
                     JOIN public.cooperatives c
                       ON c.id = s.bootstrap_cooperative_id
                      AND c.is_bootstrap
                     WHERE s.bootstrap_cooperative_id = p_cooperative_id)))
         ORDER BY r.name;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_role_summaries_scoped(bytea, uuid, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_role_summaries_scoped(bytea, uuid, timestamptz)
    TO kooperatif_app;

-- ── 3. Legacy global lane (never-bootstrapped only) ─────────────────
--
-- `effective_permissions`/`active_role_summaries` served the pre-tenant
-- single-cooperative installation. The definer re-verifies the exact
-- `never_bootstrapped` predicate before resolving anything — after
-- bootstrap (or on any broken/migrated residue) the global set is
-- unreachable and the functions return empty, matching the callers'
-- fail-closed extraction of `BusinessScope::NeverBootstrapped`.

CREATE OR REPLACE FUNCTION public.app_session_legacy_lane_ok()
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
    SELECT EXISTS(SELECT 1 FROM public.installation_state)
       AND NOT EXISTS(SELECT 1 FROM public.installation_state
                      WHERE bootstrap_cooperative_id IS NOT NULL)
       AND NOT EXISTS(SELECT 1 FROM public.cooperatives)
       AND NOT EXISTS(SELECT 1 FROM public.security_events
                      WHERE cooperative_id IS NOT NULL)
$fn$;

REVOKE ALL ON FUNCTION public.app_session_legacy_lane_ok() FROM PUBLIC;

CREATE OR REPLACE FUNCTION public.app_session_permissions_legacy(
    p_token_hash bytea,
    p_now        timestamptz
)
RETURNS SETOF text
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user uuid;
BEGIN
    v_user := public.app_session_owner(p_token_hash, p_now);
    IF v_user IS NULL OR NOT public.app_session_legacy_lane_ok() THEN
        RETURN;
    END IF;

    RETURN QUERY
        SELECT DISTINCT p.key
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
         WHERE ura.user_id = v_user;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_permissions_legacy(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_permissions_legacy(bytea, timestamptz)
    TO kooperatif_app;

CREATE OR REPLACE FUNCTION public.app_session_role_summaries_legacy(
    p_token_hash bytea,
    p_now        timestamptz
)
RETURNS TABLE(role_id uuid, role_name text)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user uuid;
BEGIN
    v_user := public.app_session_owner(p_token_hash, p_now);
    IF v_user IS NULL OR NOT public.app_session_legacy_lane_ok() THEN
        RETURN;
    END IF;

    RETURN QUERY
        SELECT r.id, r.name::text
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
         WHERE ura.user_id = v_user
         ORDER BY r.name;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_role_summaries_legacy(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_role_summaries_legacy(bytea, timestamptz)
    TO kooperatif_app;

-- ── 4. Fail-closed post-checks ──────────────────────────────────────
--
-- Every S2A function: SECURITY DEFINER, pinned search_path, never
-- PUBLIC-executable. The runtime role holds EXECUTE only on the five
-- request-facing readers; the two internal helpers
-- (`app_session_owner`, `app_session_legacy_lane_ok`) are reached only
-- through the definer chain — like `app_seal_of`/`app_verify_membership`
-- they are deliberately NOT executable by kooperatif_app.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_proc p
        WHERE p.pronamespace = 'public'::regnamespace
          AND p.proname IN (
              'app_session_owner',
              'app_session_membership',
              'app_session_memberships',
              'app_session_business_membership_ok',
              'app_session_permissions_scoped',
              'app_session_role_summaries_scoped',
              'app_session_legacy_lane_ok',
              'app_session_permissions_legacy',
              'app_session_role_summaries_legacy')
          AND (NOT p.prosecdef
               OR NOT (p.proconfig::text[] @> ARRAY['search_path=public, pg_catalog'])
               OR p.proacl IS NULL
               OR EXISTS (SELECT 1 FROM unnest(p.proacl) AS acl
                           WHERE split_part(acl::text, '=', 1) = ''))
    ) THEN
        RAISE EXCEPTION 'an S2A function is not hardened';
    END IF;

    IF has_function_privilege(
        'kooperatif_app', 'public.app_session_owner(bytea,timestamptz)', 'EXECUTE')
       OR has_function_privilege(
        'kooperatif_app', 'public.app_session_legacy_lane_ok()', 'EXECUTE')
    THEN
        RAISE EXCEPTION 'an S2A internal helper is executable by kooperatif_app';
    END IF;

    IF NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_membership(bytea,uuid,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_memberships(bytea,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_business_membership_ok(bytea,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_permissions_scoped(bytea,uuid,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_role_summaries_scoped(bytea,uuid,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_permissions_legacy(bytea,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_role_summaries_legacy(bytea,timestamptz)', 'EXECUTE')
    THEN
        RAISE EXCEPTION 'kooperatif_app is missing a required S2A EXECUTE grant';
    END IF;
END $$;
