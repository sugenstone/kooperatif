-- M1-P5 B3 — membership/authorization realtime invalidation.
--
-- Discovery RT-4: the tables that decide WHO may see data have no
-- rt_ triggers — membership suspension, role revocation and
-- permission changes produced no signal, so a live socket only
-- revalidated on the next unrelated domain event or the 30 s
-- heartbeat, and administration screens never refreshed.
--
-- What changes: a new domain tag `authz` is emitted on the existing
-- signed v2 channel (kooperatif_domain_changed) when authorization
-- state changes. The tag carries no user ids, permission names or
-- session material — it only means "authorization context in this
-- cooperative changed; revalidate and re-read". On delivery the
-- server runs the normal per-signal revalidation (fresh session +
-- membership + permission resolution) and closes the socket when the
-- caller is no longer authorized; the event additionally reaches
-- every socket bound to that cooperative regardless of subscribed
-- read scope, so revocations take effect promptly instead of at the
-- next heartbeat.
--
-- Scope derivation (never client input):
--   cooperative_memberships   row cooperative_id — memberships are
--                             always cooperative-local.
--   user_role_assignments     row cooperative_id when set; a NULL-
--                             scoped (platform/bootstrap) assignment
--                             instead reaches the '-' lane AND every
--                             cooperative where that USER holds an
--                             active membership — a NULL-scope grant
--                             takes effect inside each of them.
--   roles                     row cooperative_id ('-' for NULL-scope
--                             role rows).
--   role_permissions          resolved through roles.id: the role's
--                             home coop (or '-'), every cooperative
--                             holding a scoped assignment of the
--                             role, and every cooperative where a
--                             NULL-scope assignee is an active member.
--   user_sessions             revocation only (revoked_at NULL→set):
--                             the revoked session may be bound to any
--                             coop, so the signal reaches every coop
--                             where the user is an active member.
--   users                     status change → same user-wide fan-out;
--                             disable and re-enable both emit.
--
-- A cooperative-A authorization change cannot reach B: the v2 MAC
-- binds the derived cooperative id, and delivery still filters on
-- cooperative before the WebSocket send.
--
-- Append-only. No historical migration is modified.

-- 1. Shared signed emitter — identical wire contract as 0062/0063.
CREATE OR REPLACE FUNCTION public.kooperatif_emit_domain(
    p_tag text, p_coop uuid
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
DECLARE
    v_key   bytea;
    v_scope text;
    v_nonce uuid;
    v_ts    text;
BEGIN
    v_scope := coalesce(p_coop::text, '-');
    v_nonce := gen_random_uuid();
    v_ts := floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint::text;

    SELECT s.value INTO v_key
      FROM public.app_secrets s
     WHERE s.name = 'realtime_notify_key';
    IF v_key IS NULL THEN
        RAISE EXCEPTION 'realtime_notify_key missing — cannot emit signed domain signal';
    END IF;

    PERFORM pg_notify(
        'kooperatif_domain_changed',
        'v2|' || v_scope || '|' || p_tag || '|' || v_nonce::text || '|' || v_ts || '|' ||
        encode(hmac(convert_to(p_tag || '|' || v_scope || '|' || v_nonce::text || '|' || v_ts, 'UTF8'),
                    v_key, 'sha256'), 'hex'));
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_emit_domain(text, uuid) FROM PUBLIC;

-- 2. Row-coop data-domain trigger refactored onto the shared emitter.
CREATE OR REPLACE FUNCTION public.kooperatif_notify_domain()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
BEGIN
    PERFORM public.kooperatif_emit_domain(
        TG_ARGV[0],
        CASE WHEN TG_OP = 'DELETE'
             THEN OLD.cooperative_id
             ELSE NEW.cooperative_id END);
    RETURN NULL;
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_notify_domain() FROM PUBLIC;

-- 3. User-fan-out emitter: one signed `authz` per cooperative where
--    the user holds an ACTIVE membership (the only coops where that
--    user can hold live sockets), plus the '-' never-bootstrapped
--    lane. Used by session revocation and global identity lifecycle —
--    the row itself does not carry a cooperative.
CREATE OR REPLACE FUNCTION public.kooperatif_emit_authz_for_user(
    p_user uuid
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
DECLARE
    v_coop uuid;
BEGIN
    PERFORM public.kooperatif_emit_domain('authz', NULL);
    FOR v_coop IN
        SELECT m.cooperative_id
          FROM public.cooperative_memberships m
         WHERE m.user_id = p_user
           AND m.status = 'active'
    LOOP
        PERFORM public.kooperatif_emit_domain('authz', v_coop);
    END LOOP;
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_emit_authz_for_user(uuid) FROM PUBLIC;

-- 4. Trigger functions.

--    user_role_assignments: coop-scoped rows are coop-local; NULL-
--    scoped rows fan out to the affected user's active memberships.
CREATE OR REPLACE FUNCTION public.kooperatif_notify_authz_assignment()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
DECLARE
    v_coop uuid;
    v_user uuid;
BEGIN
    IF TG_OP = 'DELETE' THEN
        v_coop := OLD.cooperative_id; v_user := OLD.user_id;
    ELSE
        v_coop := NEW.cooperative_id; v_user := NEW.user_id;
    END IF;
    IF v_coop IS NOT NULL THEN
        PERFORM public.kooperatif_emit_domain('authz', v_coop);
    ELSE
        PERFORM public.kooperatif_emit_authz_for_user(v_user);
    END IF;
    RETURN NULL;
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_notify_authz_assignment() FROM PUBLIC;

--    role_permissions: the role's own coop (or '-' for NULL-scope)
--    plus every cooperative where the role is actually in effect:
--    scoped assignment coops, and membership coops of NULL-scope
--    assignees.
CREATE OR REPLACE FUNCTION public.kooperatif_notify_authz_role()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
DECLARE
    v_role uuid;
    v_coop uuid;
BEGIN
    v_role := CASE WHEN TG_OP = 'DELETE' THEN OLD.role_id ELSE NEW.role_id END;
    SELECT r.cooperative_id INTO v_coop
      FROM public.roles r WHERE r.id = v_role;

    IF v_coop IS NOT NULL THEN
        PERFORM public.kooperatif_emit_domain('authz', v_coop);
    ELSE
        PERFORM public.kooperatif_emit_domain('authz', NULL);
    END IF;
    FOR v_coop IN
        SELECT DISTINCT a.cooperative_id
          FROM public.user_role_assignments a
         WHERE a.role_id = v_role
           AND a.cooperative_id IS NOT NULL
           AND a.cooperative_id IS DISTINCT FROM
               (SELECT r2.cooperative_id FROM public.roles r2
                 WHERE r2.id = v_role)
    LOOP
        PERFORM public.kooperatif_emit_domain('authz', v_coop);
    END LOOP;
    FOR v_coop IN
        SELECT DISTINCT m.cooperative_id
          FROM public.user_role_assignments a
          JOIN public.cooperative_memberships m
            ON m.user_id = a.user_id AND m.status = 'active'
         WHERE a.role_id = v_role
           AND a.cooperative_id IS NULL
    LOOP
        PERFORM public.kooperatif_emit_domain('authz', v_coop);
    END LOOP;
    RETURN NULL;
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_notify_authz_role() FROM PUBLIC;

--    user_sessions: only the NULL → set transition of revoked_at is
--    an authorization change; idle-touch writes stay silent.
CREATE OR REPLACE FUNCTION public.kooperatif_notify_authz_session()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
BEGIN
    PERFORM public.kooperatif_emit_authz_for_user(NEW.user_id);
    RETURN NULL;
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_notify_authz_session() FROM PUBLIC;

--    users.status is global (disable AND re-enable both emit).
CREATE OR REPLACE FUNCTION public.kooperatif_notify_authz_user()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
BEGIN
    PERFORM public.kooperatif_emit_authz_for_user(NEW.id);
    RETURN NULL;
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_notify_authz_user() FROM PUBLIC;

-- 5. Triggers.
CREATE TRIGGER rt_authz_memberships
    AFTER INSERT OR UPDATE OR DELETE ON public.cooperative_memberships
    FOR EACH ROW EXECUTE FUNCTION public.kooperatif_notify_domain('authz');

CREATE TRIGGER rt_authz_role_assignments
    AFTER INSERT OR UPDATE OR DELETE ON public.user_role_assignments
    FOR EACH ROW EXECUTE FUNCTION public.kooperatif_notify_authz_assignment();

CREATE TRIGGER rt_authz_roles
    AFTER INSERT OR UPDATE OR DELETE ON public.roles
    FOR EACH ROW EXECUTE FUNCTION public.kooperatif_notify_domain('authz');

CREATE TRIGGER rt_authz_role_permissions
    AFTER INSERT OR DELETE ON public.role_permissions
    FOR EACH ROW EXECUTE FUNCTION public.kooperatif_notify_authz_role();

CREATE TRIGGER rt_authz_session_revocation
    AFTER UPDATE ON public.user_sessions
    FOR EACH ROW
    WHEN (NEW.revoked_at IS NOT NULL AND OLD.revoked_at IS NULL)
    EXECUTE FUNCTION public.kooperatif_notify_authz_session();

CREATE TRIGGER rt_authz_user_status
    AFTER UPDATE ON public.users
    FOR EACH ROW
    WHEN (OLD.status IS DISTINCT FROM NEW.status)
    EXECUTE FUNCTION public.kooperatif_notify_authz_user();

-- ── Postchecks ─────────────────────────────────────────────────────

DO $$
BEGIN
    IF (SELECT count(*) FROM pg_trigger
         WHERE tgname LIKE 'rt_%' AND NOT tgisinternal) <> 43 THEN
        RAISE EXCEPTION 'rt_* trigger inventory must be 43 after B3';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_trigger t JOIN pg_proc p ON p.oid = t.tgfoid
         WHERE t.tgname LIKE 'rt_authz_%' AND NOT p.prosecdef) THEN
        RAISE EXCEPTION 'authz realtime triggers must be SECURITY DEFINER';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.proname IN ('kooperatif_emit_domain',
                             'kooperatif_emit_authz_for_user',
                             'kooperatif_notify_domain',
                             'kooperatif_notify_authz_assignment',
                             'kooperatif_notify_authz_role',
                             'kooperatif_notify_authz_session',
                             'kooperatif_notify_authz_user')
           AND p.pronamespace = 'public'::regnamespace
           AND (p.proacl IS NULL
                OR EXISTS (SELECT 1 FROM aclexplode(p.proacl) x
                            WHERE x.grantee = 0))) THEN
        RAISE EXCEPTION 'realtime emitters must not be PUBLIC-executable';
    END IF;
END $$;
