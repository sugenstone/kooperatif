-- M2 PD-14 hardening — TRUE single-use temporary credentials.
--
-- password_reset_pending/consumed pair: a reset marks the credential
-- pending; the FIRST successful login consumes it atomically inside
-- app_password_consume_reset (UPDATE ... WHERE NOT consumed — two
-- racing logins can never both pass). The consumed flag is cleared
-- together with the credential on rotation in app_password_change_self.
-- Permanent users (pending=false) take a fast no-op path — the normal
-- login flow is unchanged. Failed logins never consume.
--
-- NOT APPLIED to any database.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

ALTER TABLE public.users
    ADD COLUMN IF NOT EXISTS password_reset_pending  boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS password_reset_consumed boolean NOT NULL DEFAULT false;

-- Atomic consume at the credential-bound login lane. Resolves the
-- owner from the JUST-OPENED session row (token hash — never a
-- caller-supplied user id). Returns true when the lane may proceed:
-- no pending reset, or the pending reset was consumed HERE.
CREATE OR REPLACE FUNCTION public.app_password_consume_reset(
    p_token_hash bytea
)
RETURNS boolean
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $function$
DECLARE
    v_user uuid;
BEGIN
    SELECT s.user_id INTO v_user FROM public.user_sessions s
     WHERE s.token_hash = p_token_hash AND s.revoked_at IS NULL
     LIMIT 1;
    IF v_user IS NULL THEN
        RETURN false;
    END IF;
    -- No pending reset: ordinary credential, nothing to consume.
    IF NOT EXISTS (
        SELECT 1 FROM public.users u
         WHERE u.id = v_user AND u.password_reset_pending
    ) THEN
        RETURN true;
    END IF;
    -- Atomic single-use: exactly one racing login flips the flag.
    PERFORM 1 FROM public.users u
     WHERE u.id = v_user
       AND u.password_reset_pending
       AND NOT u.password_reset_consumed
     FOR UPDATE;
    IF NOT FOUND THEN
        RETURN false;
    END IF;
    UPDATE public.users u
       SET password_reset_consumed = true
     WHERE u.id = v_user AND u.password_reset_pending;
    RETURN true;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_password_consume_reset(bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_password_consume_reset(bytea)
    TO kooperatif_app;

-- Re-issue the reset definer: mark the credential pending+unconsumed.
CREATE OR REPLACE FUNCTION public.app_platform_password_reset(
    p_user_id             uuid,
    p_new_password_hash   text,
    p_new_verifier        text
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_actor uuid := nullif(current_setting('app.user_id', true), '')::uuid;
BEGIN
    IF v_actor IS NULL
       OR NOT public.app_has_platform_permission(v_actor, 'users.manage')
    THEN
        RAISE EXCEPTION 'insufficient_privilege: platform users.manage required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF NOT EXISTS (SELECT 1 FROM public.users WHERE id = p_user_id) THEN
        RETURN NULL;
    END IF;

    UPDATE public.users u
       SET password_hash           = p_new_password_hash,
           login_verifier          = crypt(p_new_verifier, gen_salt('bf')),
           must_change_password    = true,
           password_reset_pending  = true,
           password_reset_consumed = false,
           password_changed_at     = now(),
           updated_at              = now()
     WHERE u.id = p_user_id;

    UPDATE public.user_sessions s
       SET revoked_at        = now(),
           revocation_reason = 'admin_password_reset'
     WHERE s.user_id = p_user_id AND s.revoked_at IS NULL;

    PERFORM public.app_audit_emit('password_reset_by_admin',
        jsonb_build_object('target_user_id', p_user_id));

    RETURN p_user_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_platform_password_reset(uuid, text, text)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_platform_password_reset(uuid, text, text)
    TO kooperatif_app;

-- Re-issue the self-rotation definer: clear the one-use flags with the
-- credential itself.
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
       SET password_hash           = p_new_password_hash,
           login_verifier          = crypt(p_new_verifier_password, gen_salt('bf')),
           must_change_password    = false,
           password_reset_pending  = false,
           password_reset_consumed = false,
           password_changed_at     = now(),
           updated_at              = now()
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
