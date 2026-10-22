-- M1-P4-P2B — session-derived request identity.
--
-- `app_open_request_context` is the identity-binding half of the
-- pre-context lane: the application hands over ONLY the session token
-- hash (the credential it already verified), and the database derives
-- the user id from the session row itself — never trusting a
-- caller-supplied UUID. On success the function stamps
-- `app.user_id` transaction-locally via set_config(…, true) and
-- returns the derived user id; on any invalid/expired/revoked/disabled
-- session it returns NULL and stamps nothing.
--
-- This closes the P2A finding that a caller could present an arbitrary
-- user UUID to `app_verify_membership`: with the user GUC stamped here
-- from the session row, the membership shim's caller-binding guard
-- (P2A, migration 0033) makes `p_user` a checked claim instead of a
-- free parameter — and the cooperative GUC is stamped only AFTER
-- membership resolves, so a valid session can never hop into a
-- cooperative it does not belong to.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

CREATE OR REPLACE FUNCTION public.app_open_request_context(
    p_session_token_hash bytea
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
       AND us.expires_at > now()
       AND us.idle_expires_at > now()
       AND u.status = 'active'
     LIMIT 1;

    IF v_user IS NULL THEN
        RETURN NULL;
    END IF;

    PERFORM set_config('app.user_id', v_user::text, true);
    RETURN v_user;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_open_request_context(bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_open_request_context(bytea)
    TO kooperatif_app;

-- Fail-closed post-checks: the function exists, is SECURITY DEFINER,
-- has a pinned search_path, and is executable only by the app role.
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
    -- PUBLIC is a keyword, not a role: inspect proacl directly. A NULL
    -- proacl means default privileges (PUBLIC EXECUTE) still apply;
    -- a non-NULL acl grants PUBLIC when the grantee part is empty.
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
END $$;
