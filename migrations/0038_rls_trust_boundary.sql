-- M1-P4-P3B — RLS trust-boundary hardening.
--
-- P3A demonstrated that a plain `app.cooperative_id` GUC cannot be a
-- trust boundary: the runtime principal can SET it directly and can
-- read `user_sessions.token_hash` to mint any user's context. This
-- migration closes both holes:
--
--   1. SEALED CONTEXT. `app_secrets` holds a random key readable only
--      by the table owner (the migration principal). Context GUCs are
--      now backed by an HMAC chain minted inside SECURITY DEFINER
--      functions:
--        app_open_request_context(hash) -> stamps user_id, session_id,
--            ctx_txid (txid_current) and user_seal = hmac(u|s|tx).
--        app_stamp_cooperative(coop)    -> verifies the user seal AND
--            txid freshness, verifies membership via
--            app_verify_membership, then stamps cooperative_id and
--            ctx_seal = hmac(u|s|c|tx).
--        app_ctx_ok()                   -> recomputes ctx_seal from the
--            current GUCs and compares; used by the pilot policies.
--      A forged or replayed GUC set cannot satisfy a seal it cannot
--      compute, and a captured seal dies with its transaction (txid
--      binding) — reuse on another transaction fails.
--
--   2. SESSION CREDENTIAL ISOLATION. All direct table privileges on
--      `user_sessions` are removed from kooperatif_app; every session
--      operation moves behind a narrow SECURITY DEFINER interface that
--      is bound to the presented credential:
--        app_session_open     — login: verifies the plaintext password
--            against `users.login_verifier` (crypt/bcrypt, set by the
--            privileged user-management lane) INSIDE the database, so
--            a session row can never be fabricated for another user.
--        app_session_resolve  — read-only validity resolution by hash.
--        app_session_touch    — bounded last-seen refresh by hash.
--        app_session_revoke   — revokes only the hash owner's sessions.
--        app_session_revoke_others — revoke-all-except-current, hash-bound.
--        app_session_list     — lists only the hash owner's sessions.
--        app_session_set_cooperative — persists the UX default coop,
--            hash-bound AND membership-checked.
--        app_admin_revoke_user_sessions — RBAC disable path.
--      `users` SELECT/UPDATE narrows to non-credential columns so
--      `password_hash`/`login_verifier` can never be read or written
--      by the runtime principal.
--
--   3. Pilot policies gain `AND public.app_ctx_ok()`.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 0. Secret material (owner-only) ───────────────────────────────

CREATE TABLE IF NOT EXISTS public.app_secrets (
    name  text PRIMARY KEY,
    value bytea NOT NULL
);
REVOKE ALL ON public.app_secrets FROM PUBLIC;
INSERT INTO public.app_secrets (name, value)
VALUES ('ctx-seal', gen_random_bytes(32))
ON CONFLICT (name) DO NOTHING;

-- DB-side login verifier: bcrypt of the plaintext password, written
-- by the privileged user-creation lane only.
ALTER TABLE public.users ADD COLUMN IF NOT EXISTS login_verifier text;

-- ── 1. Seal machinery (internal — NOT granted to the app role) ────

CREATE OR REPLACE FUNCTION public.app_seal_of(
    p_user uuid, p_session uuid, p_coop uuid, p_txid bigint
)
RETURNS text
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
    SELECT encode(hmac(
        convert_to(
            p_user::text || '|' || p_session::text || '|' ||
            coalesce(p_coop::text, '-') || '|' || p_txid::text,
            'UTF8'),
        (SELECT s.value FROM public.app_secrets s WHERE s.name = 'ctx-seal'),
        'sha256'::text), 'hex');
$fn$;
REVOKE ALL ON FUNCTION public.app_seal_of(uuid, uuid, uuid, bigint) FROM PUBLIC;

-- Replaces 0036: additionally stamps session id, tx epoch and the
-- user seal that authorizes cooperative stamping.
CREATE OR REPLACE FUNCTION public.app_open_request_context(
    p_session_token_hash bytea,
    p_now                timestamptz
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user    uuid;
    v_session uuid;
    v_txid    bigint;
BEGIN
    SELECT us.user_id, us.id INTO v_user, v_session
      FROM public.user_sessions us
      JOIN public.users u ON u.id = us.user_id
     WHERE us.token_hash = p_session_token_hash
       AND us.revoked_at IS NULL
       AND us.expires_at > p_now
       AND us.idle_expires_at > p_now
       AND u.status = 'active'
     LIMIT 1;

    IF v_user IS NULL THEN
        RETURN NULL;
    END IF;

    v_txid := txid_current();
    PERFORM set_config('app.user_id', v_user::text, true);
    PERFORM set_config('app.session_id', v_session::text, true);
    PERFORM set_config('app.ctx_txid', v_txid::text, true);
    PERFORM set_config(
        'app.user_seal',
        public.app_seal_of(v_user, v_session, NULL, v_txid),
        true);
    RETURN v_user;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_open_request_context(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_open_request_context(bytea, timestamptz)
    TO kooperatif_app;

-- Mint the cooperative seal: refuses unless the stamped user seal is
-- authentic (forged user/coop GUCs alone cannot pass) and fresh
-- (same transaction), and membership actually verifies.
CREATE OR REPLACE FUNCTION public.app_stamp_cooperative(p_coop uuid)
RETURNS TABLE(membership_status text, cooperative_name text,
              cooperative_status text, business_enabled boolean)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user    uuid;
    v_session uuid;
    v_txid    text;
    v_seal    text;
BEGIN
    v_user    := nullif(current_setting('app.user_id', true), '')::uuid;
    v_session := nullif(current_setting('app.session_id', true), '')::uuid;
    v_txid    := nullif(current_setting('app.ctx_txid', true), '');
    v_seal    := nullif(current_setting('app.user_seal', true), '');

    -- Forgery/replay guard: the stamped user seal must recompute from
    -- the presented GUCs, and the seal must belong to THIS tx.
    IF v_user IS NULL OR v_session IS NULL OR v_seal IS NULL
       OR v_txid IS NULL OR v_txid <> txid_current()::text
       OR v_seal <> public.app_seal_of(v_user, v_session, NULL, v_txid::bigint)
    THEN
        RETURN;
    END IF;

    RETURN QUERY
        SELECT m.membership_status, m.cooperative_name,
               m.cooperative_status, m.business_enabled
          FROM public.app_verify_membership(v_user, p_coop) m;
    IF NOT FOUND THEN
        RETURN;
    END IF;

    PERFORM set_config('app.cooperative_id', p_coop::text, true);
    PERFORM set_config(
        'app.ctx_seal',
        public.app_seal_of(v_user, v_session, p_coop, v_txid::bigint),
        true);
END
$fn$;

REVOKE ALL ON FUNCTION public.app_stamp_cooperative(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_stamp_cooperative(uuid)
    TO kooperatif_app;

-- Policy predicate: the stamped cooperative context must carry a
-- valid, transaction-fresh seal.
CREATE OR REPLACE FUNCTION public.app_ctx_ok()
RETURNS boolean
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
    SELECT
        nullif(current_setting('app.user_id', true), '') IS NOT NULL
        AND nullif(current_setting('app.session_id', true), '') IS NOT NULL
        AND nullif(current_setting('app.cooperative_id', true), '') IS NOT NULL
        AND nullif(current_setting('app.ctx_txid', true), '')
                = txid_current()::text
        AND nullif(current_setting('app.ctx_seal', true), '')
                = public.app_seal_of(
                    nullif(current_setting('app.user_id', true), '')::uuid,
                    nullif(current_setting('app.session_id', true), '')::uuid,
                    nullif(current_setting('app.cooperative_id', true), '')::uuid,
                    nullif(current_setting('app.ctx_txid', true), '')::bigint);
$fn$;
REVOKE ALL ON FUNCTION public.app_ctx_ok() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_ctx_ok() TO kooperatif_app;

-- ── 2. Narrow session interface (credential-bound definer API) ────

-- Resolve a session by token hash. Returns exactly one row; when the
-- session is unusable, `rejection` carries the classification and the
-- remaining columns are NULL. The token hash itself is never returned.
CREATE OR REPLACE FUNCTION public.app_session_resolve(
    p_token_hash bytea,
    p_now        timestamptz
)
RETURNS TABLE(
    rejection text, session_id uuid, user_id uuid, username text,
    display_name text, user_status text, csrf_token text,
    created_at timestamptz, last_seen_at timestamptz,
    expires_at timestamptz, idle_expires_at timestamptz,
    active_cooperative_id uuid, client_label text
)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    s public.user_sessions%ROWTYPE;
    u public.users%ROWTYPE;
BEGIN
    SELECT * INTO s FROM public.user_sessions us
     WHERE us.token_hash = p_token_hash LIMIT 1;
    IF NOT FOUND THEN
        RETURN QUERY SELECT 'unknown'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text;
        RETURN;
    END IF;
    SELECT * INTO u FROM public.users WHERE id = s.user_id;
    IF s.revoked_at IS NOT NULL THEN
        RETURN QUERY SELECT 'revoked'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text;
    ELSIF s.expires_at <= p_now THEN
        RETURN QUERY SELECT 'expired'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text;
    ELSIF s.idle_expires_at <= p_now THEN
        RETURN QUERY SELECT 'idle_expired'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text;
    ELSIF u.id IS NULL OR u.status <> 'active' THEN
        RETURN QUERY SELECT 'user_disabled'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text;
    ELSE
        RETURN QUERY SELECT NULL, s.id, s.user_id, u.username,
            u.display_name, u.status, s.csrf_token, s.created_at,
            s.last_seen_at, s.expires_at, s.idle_expires_at,
            s.active_cooperative_id, s.client_label;
    END IF;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_resolve(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_resolve(bytea, timestamptz)
    TO kooperatif_app;

-- Bounded last-seen refresh, bound to the presented token hash.
CREATE OR REPLACE FUNCTION public.app_session_touch(
    p_token_hash    bytea,
    p_now           timestamptz,
    p_idle_ttl_secs integer,
    p_min_age_secs  integer
)
RETURNS boolean
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    UPDATE public.user_sessions
       SET last_seen_at = p_now,
           idle_expires_at = p_now + make_interval(secs => p_idle_ttl_secs)
     WHERE token_hash = p_token_hash
       AND revoked_at IS NULL
       AND last_seen_at < p_now - make_interval(secs => p_min_age_secs);
    RETURN FOUND;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_touch(bytea, timestamptz, integer, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_touch(bytea, timestamptz, integer, integer)
    TO kooperatif_app;

-- Login: credential-bound session creation. The plaintext password is
-- verified against `users.login_verifier` INSIDE the database — the
-- runtime principal can neither read the verifier nor fabricate a
-- session for a user whose password it does not know. Returns the new
-- session's fields, or zero rows on failure (uniform — no
-- username/password oracle).
CREATE OR REPLACE FUNCTION public.app_session_open(
    p_username      text,
    p_password      text,
    p_token_hash    bytea,
    p_csrf          text,
    p_now           timestamptz,
    p_abs_ttl_secs  integer,
    p_idle_ttl_secs integer,
    p_label         text
)
RETURNS TABLE(
    session_id uuid, user_id uuid, username text, display_name text,
    active_cooperative_id uuid, created_at timestamptz,
    expires_at timestamptz
)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    u      public.users%ROWTYPE;
    v_ok   boolean;
    v_id   uuid;
BEGIN
    SELECT * INTO u FROM public.users x WHERE x.username = p_username LIMIT 1;
    -- Timing equalization: crypt runs against a fixed dummy salt when
    -- the user is missing or has no verifier.
    v_ok := crypt(p_password,
                  coalesce(u.login_verifier,
                           '$2a$10$abcdefghijklmnopqrstuu'))
            = coalesce(u.login_verifier, '');
    -- Failed-login audit is written DB-side so the credential oracle
    -- never leaves the definer: the caller sees a uniform empty set,
    -- while the durable audit keeps its reason granularity.
    IF u.id IS NULL THEN
        INSERT INTO public.security_events (event_type, user_id, metadata)
        VALUES ('login_failed', NULL,
                jsonb_build_object('username', p_username,
                                   'reason', 'unknown_username'));
        RETURN;
    ELSIF u.status <> 'active' THEN
        INSERT INTO public.security_events (event_type, user_id, metadata)
        VALUES ('login_failed', u.id,
                jsonb_build_object('username', p_username,
                                   'reason', 'user_disabled'));
        RETURN;
    ELSIF NOT v_ok THEN
        INSERT INTO public.security_events (event_type, user_id, metadata)
        VALUES ('login_failed', u.id,
                jsonb_build_object('username', p_username,
                                   'reason', 'invalid_credentials'));
        RETURN;
    END IF;

    INSERT INTO public.user_sessions
        (user_id, token_hash, csrf_token, created_at, last_seen_at,
         expires_at, idle_expires_at, client_label)
    VALUES (u.id, p_token_hash, p_csrf, p_now, p_now,
            p_now + make_interval(secs => p_abs_ttl_secs),
            p_now + make_interval(secs => p_idle_ttl_secs),
            p_label)
    RETURNING id INTO v_id;

    UPDATE public.users SET last_login_at = p_now, updated_at = p_now
     WHERE id = u.id;

    RETURN QUERY
        SELECT s.id, s.user_id, u.username, u.display_name,
               s.active_cooperative_id, s.created_at, s.expires_at
          FROM public.user_sessions s WHERE s.id = v_id;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_open(text, text, bytea, text, timestamptz, integer, integer, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_open(text, text, bytea, text, timestamptz, integer, integer, text)
    TO kooperatif_app;

-- Revoke one session of the hash owner (self-service). Returns
-- 'revoked', 'already_revoked' (own session, idempotent 204) or
-- 'not_found' (foreign/unknown — caller maps to 404 without leaking).
CREATE OR REPLACE FUNCTION public.app_session_revoke(
    p_token_hash bytea,
    p_session_id uuid,
    p_now        timestamptz,
    p_reason     text
)
RETURNS text
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_owner uuid;
    v_state text;
BEGIN
    SELECT us.user_id INTO v_owner
      FROM public.user_sessions us
      JOIN public.users u ON u.id = us.user_id
     WHERE us.token_hash = p_token_hash
       AND us.revoked_at IS NULL
       AND us.expires_at > p_now
       AND u.status = 'active'
     LIMIT 1;
    IF v_owner IS NULL THEN
        RETURN 'not_found';
    END IF;

    UPDATE public.user_sessions
       SET revoked_at = p_now, revocation_reason = p_reason
     WHERE id = p_session_id AND user_id = v_owner
       AND revoked_at IS NULL;
    IF FOUND THEN
        RETURN 'revoked';
    END IF;
    SELECT CASE WHEN EXISTS (
            SELECT 1 FROM public.user_sessions
             WHERE id = p_session_id AND user_id = v_owner)
        THEN 'already_revoked' ELSE 'not_found' END
      INTO v_state;
    RETURN v_state;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_revoke(bytea, uuid, timestamptz, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_revoke(bytea, uuid, timestamptz, text)
    TO kooperatif_app;

-- Revoke every other active session of the hash owner.
CREATE OR REPLACE FUNCTION public.app_session_revoke_others(
    p_token_hash bytea,
    p_now        timestamptz,
    p_reason     text
)
RETURNS bigint
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_owner    uuid;
    v_current  uuid;
    v_count    bigint;
BEGIN
    SELECT us.user_id, us.id INTO v_owner, v_current
      FROM public.user_sessions us
      JOIN public.users u ON u.id = us.user_id
     WHERE us.token_hash = p_token_hash
       AND us.revoked_at IS NULL
       AND us.expires_at > p_now
       AND u.status = 'active'
     LIMIT 1;
    IF v_owner IS NULL THEN
        RETURN 0;
    END IF;
    UPDATE public.user_sessions
       SET revoked_at = p_now, revocation_reason = p_reason
     WHERE user_id = v_owner AND id <> v_current AND revoked_at IS NULL;
    GET DIAGNOSTICS v_count = ROW_COUNT;
    RETURN v_count;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_revoke_others(bytea, timestamptz, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_revoke_others(bytea, timestamptz, text)
    TO kooperatif_app;

-- List the hash owner's active sessions (never returns token_hash).
CREATE OR REPLACE FUNCTION public.app_session_list(
    p_token_hash bytea,
    p_now        timestamptz
)
RETURNS TABLE(id uuid, created_at timestamptz, last_seen_at timestamptz,
              expires_at timestamptz, client_label text, is_current boolean)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_owner   uuid;
    v_current uuid;
BEGIN
    SELECT us.user_id, us.id INTO v_owner, v_current
      FROM public.user_sessions us
      JOIN public.users u ON u.id = us.user_id
     WHERE us.token_hash = p_token_hash
       AND us.revoked_at IS NULL
       AND us.expires_at > p_now
       AND u.status = 'active'
     LIMIT 1;
    IF v_owner IS NULL THEN
        RETURN;
    END IF;
    RETURN QUERY
        SELECT s.id, s.created_at, s.last_seen_at, s.expires_at,
               s.client_label, (s.id = v_current)
          FROM public.user_sessions s
         WHERE s.user_id = v_owner AND s.revoked_at IS NULL
           AND s.expires_at > p_now AND s.idle_expires_at > p_now
         ORDER BY s.created_at DESC;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_list(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_list(bytea, timestamptz)
    TO kooperatif_app;

-- Persist the UX-level default cooperative on the CURRENT session —
-- bound to the presented hash and verified membership (M1-K1: never a
-- security decision on its own).
CREATE OR REPLACE FUNCTION public.app_session_set_cooperative(
    p_token_hash bytea,
    p_coop       uuid
)
RETURNS boolean
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_owner   uuid;
    v_session uuid;
BEGIN
    SELECT us.user_id, us.id INTO v_owner, v_session
      FROM public.user_sessions us
      JOIN public.users u ON u.id = us.user_id
     WHERE us.token_hash = p_token_hash
       AND us.revoked_at IS NULL
       AND u.status = 'active'
     LIMIT 1;
    IF v_owner IS NULL THEN
        RETURN false;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM public.app_verify_membership(v_owner, p_coop)
         WHERE membership_status = 'active'
           AND cooperative_status = 'active'
           AND business_enabled
    ) THEN
        RETURN false;
    END IF;
    UPDATE public.user_sessions
       SET active_cooperative_id = p_coop
     WHERE id = v_session;
    RETURN FOUND;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_set_cooperative(bytea, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_set_cooperative(bytea, uuid)
    TO kooperatif_app;

-- Administrative revoke-all for the RBAC user-disable path (invoked
-- inside a request transaction after app-side permission checks).
CREATE OR REPLACE FUNCTION public.app_admin_revoke_user_sessions(
    p_user_id uuid,
    p_now     timestamptz
)
RETURNS bigint
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_count bigint;
BEGIN
    UPDATE public.user_sessions
       SET revoked_at = p_now, revocation_reason = 'user_disabled'
     WHERE user_id = p_user_id AND revoked_at IS NULL;
    GET DIAGNOSTICS v_count = ROW_COUNT;
    RETURN v_count;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_admin_revoke_user_sessions(uuid, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_admin_revoke_user_sessions(uuid, timestamptz)
    TO kooperatif_app;

-- ── 3. Grant narrowing ───────────────────────────────────────────

REVOKE SELECT, INSERT, UPDATE, DELETE ON public.user_sessions
    FROM kooperatif_app;

REVOKE SELECT, UPDATE ON public.users FROM kooperatif_app;
GRANT SELECT (id, username, display_name, status,
              created_at, updated_at, last_login_at)
    ON public.users TO kooperatif_app;
GRANT UPDATE (status, updated_at)
    ON public.users TO kooperatif_app;

-- ── 4. Pilot policies → sealed context ───────────────────────────

DROP POLICY IF EXISTS periods_app_cooperative ON public.periods;
CREATE POLICY periods_app_cooperative ON public.periods
    TO kooperatif_app
    USING (
        cooperative_id
            = nullif(current_setting('app.cooperative_id', true), '')::uuid
        AND public.app_ctx_ok()
    )
    WITH CHECK (
        cooperative_id
            = nullif(current_setting('app.cooperative_id', true), '')::uuid
        AND public.app_ctx_ok()
    );

DROP POLICY IF EXISTS governance_bodies_app_cooperative ON public.governance_bodies;
CREATE POLICY governance_bodies_app_cooperative ON public.governance_bodies
    TO kooperatif_app
    USING (
        cooperative_id
            = nullif(current_setting('app.cooperative_id', true), '')::uuid
        AND public.app_ctx_ok()
    )
    WITH CHECK (
        cooperative_id
            = nullif(current_setting('app.cooperative_id', true), '')::uuid
        AND public.app_ctx_ok()
    );

-- ── 5. Fail-closed post-checks ───────────────────────────────────

DO $$
BEGIN
    -- The seal helper must never be callable by the runtime role.
    IF has_function_privilege(
        'kooperatif_app', 'public.app_seal_of(uuid,uuid,uuid,bigint)', 'EXECUTE')
    THEN
        RAISE EXCEPTION 'app_seal_of must not be granted to kooperatif_app';
    END IF;

    -- Session table fully closed; user credentials column-invisible.
    IF has_table_privilege('kooperatif_app', 'public.user_sessions', 'SELECT')
       OR has_table_privilege('kooperatif_app', 'public.user_sessions', 'INSERT')
       OR has_table_privilege('kooperatif_app', 'public.user_sessions', 'UPDATE')
       OR has_table_privilege('kooperatif_app', 'public.user_sessions', 'DELETE')
    THEN
        RAISE EXCEPTION 'kooperatif_app must not touch user_sessions';
    END IF;
    IF has_column_privilege('kooperatif_app', 'public.users', 'password_hash', 'SELECT')
       OR has_column_privilege('kooperatif_app', 'public.users', 'login_verifier', 'SELECT')
       OR has_column_privilege('kooperatif_app', 'public.users', 'password_hash', 'UPDATE')
       OR has_column_privilege('kooperatif_app', 'public.users', 'login_verifier', 'UPDATE')
    THEN
        RAISE EXCEPTION 'credential columns must stay invisible to kooperatif_app';
    END IF;
    IF has_table_privilege('kooperatif_app', 'public.app_secrets', 'SELECT')
    THEN
        RAISE EXCEPTION 'app_secrets must stay owner-only';
    END IF;

    -- Every new definer function hardened: secdef + pinned path + no PUBLIC.
    IF EXISTS (
        SELECT 1 FROM pg_proc p
        WHERE p.pronamespace = 'public'::regnamespace
          AND p.proname IN ('app_open_request_context','app_stamp_cooperative',
              'app_ctx_ok','app_session_resolve','app_session_touch',
              'app_session_open','app_session_revoke','app_session_revoke_others',
              'app_session_list','app_session_set_cooperative',
              'app_admin_revoke_user_sessions','app_seal_of')
          AND (NOT p.prosecdef
               OR NOT (p.proconfig::text[] @> ARRAY['search_path=public, pg_catalog'])
               OR p.proacl IS NULL
               OR EXISTS (SELECT 1 FROM unnest(p.proacl) AS acl
                           WHERE split_part(acl::text, '=', 1) = ''))
    ) THEN
        RAISE EXCEPTION 'a trust-boundary function is not hardened';
    END IF;
END $$;
