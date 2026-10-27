-- M2 — self password change + forced-change session gating.
--
-- Two changes, both append-only over existing boundaries:
--
--   1. app_session_resolve gains a `must_change_password` column in its
--      RETURNS TABLE. A RETURNS TABLE change cannot ride CREATE OR
--      REPLACE, so the function is dropped and recreated with the 0038
--      body carried verbatim plus the new field. Privileges are
--      re-issued identically.
--
--   2. app_password_change_self — credential-bound SECURITY DEFINER
--      writer behind the sealed user context (app.user_id stamped by
--      app_open_request_context). It verifies the CURRENT password
--      against users.login_verifier inside the database, rotates both
--      credential columns, clears must_change_password, stamps
--      password_changed_at, and revokes EVERY session of the user —
--      including the calling one (re-login required, PD-14 flow).
--
-- Audit: 'password_changed' mints through app_audit_emit inside the
-- sealed tx; a wrong current password mints 'password_change_failed'
-- and the CALLER commits so the row survives. Passwords, hashes and
-- verifiers never enter metadata.
--
-- NOT APPLIED to any database; clean-replay verification pending.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ------------------------------------------------------------------
-- 1. app_session_resolve with must_change_password
-- ------------------------------------------------------------------
DROP FUNCTION public.app_session_resolve(bytea, timestamptz);

CREATE FUNCTION public.app_session_resolve(
    p_token_hash bytea,
    p_now        timestamptz
)
RETURNS TABLE(
    rejection text, session_id uuid, user_id uuid, username text,
    display_name text, user_status text, csrf_token text,
    created_at timestamptz, last_seen_at timestamptz,
    expires_at timestamptz, idle_expires_at timestamptz,
    active_cooperative_id uuid, client_label text,
    must_change_password boolean
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
            NULL::timestamptz, NULL::uuid, NULL::text, NULL::boolean;
        RETURN;
    END IF;
    SELECT * INTO u FROM public.users WHERE id = s.user_id;
    IF s.revoked_at IS NOT NULL THEN
        RETURN QUERY SELECT 'revoked'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text, NULL::boolean;
    ELSIF s.expires_at <= p_now THEN
        RETURN QUERY SELECT 'expired'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text, NULL::boolean;
    ELSIF s.idle_expires_at <= p_now THEN
        RETURN QUERY SELECT 'idle_expired'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text, NULL::boolean;
    ELSIF u.id IS NULL OR u.status <> 'active' THEN
        RETURN QUERY SELECT 'user_disabled'::text, NULL::uuid, NULL::uuid,
            NULL::text, NULL::text, NULL::text, NULL::text,
            NULL::timestamptz, NULL::timestamptz, NULL::timestamptz,
            NULL::timestamptz, NULL::uuid, NULL::text, NULL::boolean;
    ELSE
        RETURN QUERY SELECT NULL, s.id, s.user_id, u.username,
            u.display_name, u.status, s.csrf_token, s.created_at,
            s.last_seen_at, s.expires_at, s.idle_expires_at,
            s.active_cooperative_id, s.client_label,
            u.must_change_password;
    END IF;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_resolve(bytea, timestamptz)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_resolve(bytea, timestamptz)
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- 2. app_password_change_self — atomic self password rotation.
-- Returns 'ok' | 'invalid_current_password'. Caller commits either
-- way so the failure audit row survives; the definer never raises on
-- a wrong password (that path must not abort the sealed tx).
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_password_change_self(
    p_current_password      text,
    p_new_password_hash     text,
    p_new_verifier_password text
)
RETURNS text
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_user  uuid := nullif(current_setting('app.user_id', true), '')::uuid;
    v_row   public.users%ROWTYPE;
BEGIN
    IF v_user IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed session required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    SELECT * INTO v_row FROM public.users u WHERE u.id = v_user;
    IF v_row.id IS NULL OR v_row.status <> 'active' THEN
        RAISE EXCEPTION 'insufficient_privilege: user inactive'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF crypt(p_current_password, v_row.login_verifier)
         IS DISTINCT FROM v_row.login_verifier
    THEN
        PERFORM public.app_audit_emit('password_change_failed',
            jsonb_build_object('user_id', v_user, 'reason', 'current_mismatch'));
        RETURN 'invalid_current_password';
    END IF;

    UPDATE public.users u
       SET password_hash        = p_new_password_hash,
           login_verifier       = crypt(p_new_verifier_password, gen_salt('bf')),
           must_change_password = false,
           password_changed_at  = now(),
           updated_at           = now()
     WHERE u.id = v_user;

    UPDATE public.user_sessions s
       SET revoked_at        = now(),
           revocation_reason = 'password_changed'
     WHERE s.user_id = v_user AND s.revoked_at IS NULL;

    PERFORM public.app_audit_emit('password_changed',
        jsonb_build_object('user_id', v_user));

    RETURN 'ok';
END;
$function$;
REVOKE ALL ON FUNCTION public.app_password_change_self(text, text, text)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_password_change_self(text, text, text)
    TO kooperatif_app;
