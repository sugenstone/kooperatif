-- M1-P5-B5 — cooperative-switch realtime invalidation.
--
-- Finding (B5 audit): a WebSocket binds its verified cooperative scope
-- at handshake; `revalidate` re-checks membership in THAT bound
-- cooperative per signal, but never compared it with the session's
-- current `active_cooperative_id`. After `POST /api/auth/cooperative`
-- (A → B), a socket bound to A kept receiving A's invalidation signals
-- while the membership in A was still valid — a stale subscription, not
-- an unauthorized one, but stale nonetheless (B3 contract: switching
-- must not retain stale subscriptions).
--
-- Two-part fix:
--   1. This migration widens `rt_authz_session_revocation` so an
--      `active_cooperative_id` change on the session row ALSO emits the
--      signed tenant-scoped `authz` event. `kooperatif_emit_authz_for_
--      user` fans out to every cooperative where the user holds an
--      active membership — including the cooperative the socket just
--      switched away from — so the stale socket is notified promptly
--      instead of waiting for the next heartbeat tick.
--   2. `revalidate` (realtime/routes.rs) compares the session's current
--      `active_cooperative_id` with the socket's bound scope and closes
--      when they diverge — the authoritative close is Rust-side, the
--      NOTIFY merely makes it prompt.
--
-- Idle-touch writes (last_seen_at refresh) still change neither column
-- and stay silent. Login pin and logout-revoke paths are unaffected.
-- Append-only; no historical migration or checksum changes.

BEGIN;

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '60s';

DROP TRIGGER rt_authz_session_revocation ON public.user_sessions;

CREATE TRIGGER rt_authz_session_revocation
    AFTER UPDATE ON public.user_sessions
    FOR EACH ROW
    WHEN (
        (NEW.revoked_at IS NOT NULL AND OLD.revoked_at IS NULL)
        OR (NEW.active_cooperative_id IS DISTINCT FROM OLD.active_cooperative_id)
    )
    EXECUTE FUNCTION public.kooperatif_notify_authz_session();

-- Postchecks: trigger inventory unchanged (43), function still a
-- revokable SECURITY DEFINER, no direct grant to the runtime role.
DO $$
BEGIN
    IF (SELECT count(*) FROM pg_trigger
         WHERE tgname LIKE 'rt_%' AND NOT tgisinternal) <> 43 THEN
        RAISE EXCEPTION 'rt_* trigger inventory must stay 43 after B5';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_trigger
         WHERE tgname = 'rt_authz_session_revocation'
           AND NOT tgisinternal) THEN
        RAISE EXCEPTION 'rt_authz_session_revocation missing';
    END IF;
    IF EXISTS (
        SELECT 1
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE p.proname = 'kooperatif_notify_authz_session'
           AND (NOT p.prosecdef
                OR has_function_privilege('public', p.oid, 'EXECUTE'))) THEN
        RAISE EXCEPTION 'kooperatif_notify_authz_session must stay sealed';
    END IF;
END $$;

COMMIT;
