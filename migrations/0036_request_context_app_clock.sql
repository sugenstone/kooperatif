-- M1-P4-P2B — app-clock evaluation for request identity.
--
-- `app_open_request_context` (0034) compared session expiry against the
-- database wall clock (`now()`). Session rows are written with the
-- APPLICATION clock (`AuthRuntime`/`MutableClock` in tests), so a
-- frozen or skewed app clock made live sessions appear expired —
-- every business request failed closed with `authentication_required`.
--
-- This migration replaces the function with a `(bytea, timestamptz)`
-- signature: `p_now` is the application-clock instant captured at
-- request extraction (`CurrentAuth.authed_at`), keeping expiry
-- evaluation in the same clock domain that wrote the rows. Everything
-- else is unchanged — the database still derives the user id from the
-- session row, stamps `app.user_id` tx-locally, and the 0033
-- caller-binding guard keeps `p_user` a checked claim.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

DROP FUNCTION IF EXISTS public.app_open_request_context(bytea);

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
    v_user uuid;
BEGIN
    SELECT us.user_id INTO v_user
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

    PERFORM set_config('app.user_id', v_user::text, true);
    RETURN v_user;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_open_request_context(bytea, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_open_request_context(bytea, timestamptz)
    TO kooperatif_app;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.proname = 'app_open_request_context'
           AND p.pronamespace = 'public'::regnamespace
           AND p.prosecdef
           AND p.proconfig::text[] @> ARRAY['search_path=public, pg_catalog']
    ) THEN
        RAISE EXCEPTION 'app_open_request_context missing or not hardened';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.proname = 'app_open_request_context'
           AND p.pronamespace = 'public'::regnamespace
           AND (p.proacl IS NULL
                OR EXISTS (
                    SELECT 1 FROM unnest(p.proacl) AS acl
                     WHERE split_part(acl::text, '=', 1) = ''))
    ) THEN
        RAISE EXCEPTION 'app_open_request_context must not be PUBLIC';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.proname = 'app_open_request_context'
           AND p.pronamespace = 'public'::regnamespace
           AND p.pronargs <> 2
    ) THEN
        RAISE EXCEPTION 'stale app_open_request_context signature survived';
    END IF;
END $$;
