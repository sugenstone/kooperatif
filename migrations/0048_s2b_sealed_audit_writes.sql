-- M1-P4-S2B — sealed security-event WRITE boundary.
--
-- Before this migration every `audit::record`/`record_scoped` call ran a
-- bare `INSERT INTO security_events` on whichever pool was at hand:
--
--   * identity (user_id, session_id, cooperative_id) was supplied by the
--     caller — nothing in the database verified it;
--   * business-route writes executed on the POOL while the request
--     transaction was still open, so an audit row could commit for an
--     operation that later rolled back (phantom audit) — and, once S3
--     forces RLS on security_events, the same unsealed write would be
--     silently filtered (audit loss);
--   * every failure was logged and swallowed, including failures of
--     events that should be atomic with the protected action;
--   * `kooperatif_app` held a direct INSERT grant, so any SQL the
--     runtime principal could issue could fabricate events with
--     arbitrary actor/cooperative attribution.
--
-- This migration replaces that surface with narrowly scoped
-- SECURITY DEFINER writers and then REVOKES the direct INSERT grant,
-- so every runtime write must pass through an identity-deriving
-- boundary:
--
--   * app_ctx_audit(event, metadata)            — sealed-transaction
--     lane. Requires a verified request seal: the cooperative ctx_seal
--     on the cooperative lane, the user_seal on the platform/never-
--     bootstrapped NULL lane (a stamped cooperative can never degrade
--     to NULL attribution). user/session/cooperative are read from the
--     verified seal GUCs, never from arguments. Called inside
--     request_tx → the event is ATOMIC with the protected operation:
--     a write failure aborts the operation (fail closed), a rolled-back
--     operation leaves no audit row. Mandatory lane.
--
--   * app_session_audit_platform(hash, type, meta, now) — session-bound
--     platform lane (cooperative_id forced NULL, actor = session owner).
--     Whitelisted to login_succeeded; used post-login where a mandatory
--     write would 500 an otherwise valid authentication — best-effort
--     with observable error logging at the call site.
--
--   * app_session_audit_scoped(hash, coop, type, meta, now) — session-
--     bound cooperative lane for events that CANNOT join the doomed
--     transaction: currently only lockout_prevented (the protected
--     mutation was just refused and its tx rolled back; the denial
--     itself must still be durable). Requires an active membership of
--     the session owner in the claimed cooperative — cross-cooperative
--     attribution is rejected. A NULL cooperative is accepted only on
--     the re-verified never-bootstrapped lane. Best-effort + loud log.
--
--   * app_audit_login_failed(meta, now) — the ONLY unauthenticated
--     writer: hardcoded event_type 'login_failed', user/session/
--     cooperative forced NULL. A caller can record that a login failed
--     for a claimed username (exactly what it could already do by
--     hitting the endpoint) but can never impersonate a user or
--     attribute to a cooperative.
--
--   * app_session_revoke / app_session_revoke_others /
--     app_session_set_cooperative are REPLACED (same signatures, same
--     grants) so the revocation/context-switch and its audit event are
--     written atomically inside the definer. This also closes the
--     dead-session trap: a just-revoked token cannot resolve an owner,
--     so a post-revocation pool write would always fail closed.
--
-- Event semantics after this migration:
--   MANDATORY/ATOMIC — every cooperative-scoped business and RBAC
--     mutation event, written inside the request transaction;
--     session_revoked/logout/sessions_revoked_all_others/
--     cooperative_context_switched, folded into their session definers;
--     login_failed rows minted inside app_session_open (unchanged,
--     0039).
--   BEST-EFFORT (observable) — login_succeeded (must never block a
--     valid login), lockout_prevented (independent lane; the denial is
--     already final), HTTP-layer login_failed (invalid username /
--     rate_limited, pre-credential — must never expose internals or
--     block authentication). Failures surface as tracing::error at the
--     call site; they are never silently dropped.
--
-- No `app.*` GUC is written here. app_ctx_audit only READS the sealed
-- GUCs after app_ctx_ok() proves them. No RLS is enabled on
-- security_events in this step (S3 scope); the INSERT grant revocation
-- already makes the definer boundary exclusive for the runtime role.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 1. Sealed-transaction writer (mandatory lane) ─────────────────
--
-- Identity comes exclusively from the seal: a caller cannot name an
-- actor, session or cooperative at all. app_ctx_ok() re-verifies the
-- HMAC chain and transaction freshness before anything is read.
CREATE OR REPLACE FUNCTION public.app_ctx_audit(
    p_event_type text,
    p_metadata   jsonb
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user      uuid;
    v_session   uuid;
    v_coop      uuid;
    v_txid      text;
    v_ctx_seal  text;
    v_user_seal text;
BEGIN
    v_user      := nullif(current_setting('app.user_id', true), '')::uuid;
    v_session   := nullif(current_setting('app.session_id', true), '')::uuid;
    v_coop      := nullif(current_setting('app.cooperative_id', true), '')::uuid;
    v_txid      := nullif(current_setting('app.ctx_txid', true), '');
    v_ctx_seal  := nullif(current_setting('app.ctx_seal', true), '');
    v_user_seal := nullif(current_setting('app.user_seal', true), '');

    IF v_user IS NULL OR v_session IS NULL OR v_txid IS NULL
       OR v_txid <> txid_current()::text THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed request context required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF v_coop IS NOT NULL THEN
        -- Cooperative lane: a stamped cooperative must carry its ctx
        -- seal — a forged app.cooperative_id GUC cannot degrade to
        -- platform NULL attribution.
        IF v_ctx_seal IS NULL
           OR v_ctx_seal <> public.app_seal_of(v_user, v_session, v_coop,
                                               v_txid::bigint) THEN
            RAISE EXCEPTION 'insufficient_privilege: invalid cooperative seal'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    ELSE
        -- Platform/never-bootstrapped lane: request_user_tx stamps only
        -- the user seal (cooperative = NULL). Identity is still
        -- session-derived and transaction-fresh; cooperative
        -- attribution is structurally impossible.
        IF v_user_seal IS NULL
           OR v_user_seal <> public.app_seal_of(v_user, v_session, NULL,
                                                v_txid::bigint) THEN
            RAISE EXCEPTION 'insufficient_privilege: invalid user seal'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END IF;

    INSERT INTO public.security_events
        (event_type, cooperative_id, user_id, session_id, metadata)
    VALUES (p_event_type, v_coop, v_user, v_session,
            coalesce(p_metadata, '{}'::jsonb));
END
$fn$;

REVOKE ALL ON FUNCTION public.app_ctx_audit(text, jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_ctx_audit(text, jsonb)
    TO kooperatif_app;

-- ── 2. Session-bound writers (independent-transaction lanes) ──────

-- Platform events for an authenticated caller where the action is not
-- transactional with a business mutation — currently only
-- login_succeeded. The actor and session are derived from the token
-- hash; cooperative attribution is structurally impossible.
CREATE OR REPLACE FUNCTION public.app_session_audit_platform(
    p_token_hash bytea,
    p_event_type text,
    p_metadata   jsonb,
    p_now        timestamptz
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user    uuid;
    v_session uuid;
BEGIN
    IF p_event_type <> 'login_succeeded' THEN
        RAISE EXCEPTION 'insufficient_privilege: event type outside this lane'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    SELECT us.user_id, us.id INTO v_user, v_session
      FROM public.user_sessions us
      JOIN public.users u ON u.id = us.user_id
     WHERE us.token_hash = p_token_hash
       AND us.revoked_at IS NULL
       AND us.expires_at > p_now
       AND us.idle_expires_at > p_now
       AND u.status = 'active'
     LIMIT 1;
    IF v_user IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: valid session required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    INSERT INTO public.security_events
        (event_type, cooperative_id, user_id, session_id, metadata,
         occurred_at)
    VALUES (p_event_type, NULL, v_user, v_session,
            coalesce(p_metadata, '{}'::jsonb), p_now);
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_audit_platform(bytea, text, jsonb, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_audit_platform(bytea, text, jsonb, timestamptz)
    TO kooperatif_app;

-- Cooperative-attributed session-bound writer for events whose
-- protected action was DENIED (the transaction rolled back and cannot
-- carry the row). The claimed cooperative must contain an active
-- membership of the session owner — same predicate as the S2A scoped
-- readers — so a token can never attribute an event to a foreign
-- cooperative. NULL attribution is reachable only on the re-verified
-- never-bootstrapped lane.
CREATE OR REPLACE FUNCTION public.app_session_audit_scoped(
    p_token_hash     bytea,
    p_cooperative_id uuid,
    p_event_type     text,
    p_metadata       jsonb,
    p_now            timestamptz
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user    uuid;
    v_session uuid;
BEGIN
    IF p_event_type <> 'lockout_prevented' THEN
        RAISE EXCEPTION 'insufficient_privilege: event type outside this lane'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    SELECT us.user_id, us.id INTO v_user, v_session
      FROM public.user_sessions us
      JOIN public.users u ON u.id = us.user_id
     WHERE us.token_hash = p_token_hash
       AND us.revoked_at IS NULL
       AND us.expires_at > p_now
       AND us.idle_expires_at > p_now
       AND u.status = 'active'
     LIMIT 1;
    IF v_user IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: valid session required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF p_cooperative_id IS NULL THEN
        IF NOT public.app_session_legacy_lane_ok() THEN
            RAISE EXCEPTION 'insufficient_privilege: NULL attribution requires the never-bootstrapped lane'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1
          FROM public.cooperative_memberships m
          JOIN public.cooperatives c ON c.id = m.cooperative_id
         WHERE m.user_id = v_user
           AND m.cooperative_id = p_cooperative_id
           AND m.status = 'active'
           AND c.status = 'active'
           AND c.business_enabled
    ) THEN
        RAISE EXCEPTION 'insufficient_privilege: cooperative outside caller scope'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    INSERT INTO public.security_events
        (event_type, cooperative_id, user_id, session_id, metadata,
         occurred_at)
    VALUES (p_event_type, p_cooperative_id, v_user, v_session,
            coalesce(p_metadata, '{}'::jsonb), p_now);
END
$fn$;

REVOKE ALL ON FUNCTION public.app_session_audit_scoped(bytea, uuid, text, jsonb, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_audit_scoped(bytea, uuid, text, jsonb, timestamptz)
    TO kooperatif_app;

-- The single unauthenticated lane: login failures raised BEFORE a
-- credential check exists (malformed username, app-side rate limit).
-- Everything except the metadata payload is fixed server-side — no
-- actor, no session, no cooperative, no event-type choice.
CREATE OR REPLACE FUNCTION public.app_audit_login_failed(
    p_metadata jsonb,
    p_now      timestamptz
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
BEGIN
    INSERT INTO public.security_events
        (event_type, cooperative_id, user_id, session_id, metadata,
         occurred_at)
    VALUES ('login_failed', NULL, NULL, NULL,
            coalesce(p_metadata, '{}'::jsonb), p_now);
END
$fn$;

REVOKE ALL ON FUNCTION public.app_audit_login_failed(jsonb, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_audit_login_failed(jsonb, timestamptz)
    TO kooperatif_app;

-- ── 3. Atomic audit inside the session lifecycle definers ─────────
--
-- Same signatures as 0038/0039 — grants are preserved by CREATE OR
-- REPLACE and re-asserted below. The audit INSERT rides the same
-- transaction as the mutation it describes.

-- Reason → event mapping reproduces the former call-site semantics:
--   'logout'       -> logout                       (metadata {})
--   anything else  -> session_revoked              (metadata {"source":"user"})
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
        INSERT INTO public.security_events
            (event_type, cooperative_id, user_id, session_id, metadata,
             occurred_at)
        VALUES (
            CASE WHEN p_reason = 'logout'
                 THEN 'logout' ELSE 'session_revoked' END,
            NULL, v_owner, p_session_id,
            CASE WHEN p_reason = 'logout'
                 THEN '{}'::jsonb
                 ELSE jsonb_build_object('source', 'user') END,
            p_now);
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

    INSERT INTO public.security_events
        (event_type, cooperative_id, user_id, session_id, metadata,
         occurred_at)
    VALUES ('sessions_revoked_all_others', NULL, v_owner, v_current,
            jsonb_build_object('revoked_count', v_count), p_now);
    RETURN v_count;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_revoke_others(bytea, timestamptz, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_revoke_others(bytea, timestamptz, text)
    TO kooperatif_app;

-- Cooperative switching: the audit row is minted inside the same call
-- that persists the session default — the switch can no longer commit
-- while its event evaporates on a separate connection.
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
    v_name    text;
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
    IF FOUND THEN
        SELECT c.name INTO v_name FROM public.cooperatives c
         WHERE c.id = p_coop;
        INSERT INTO public.security_events
            (event_type, cooperative_id, user_id, session_id, metadata)
        VALUES ('cooperative_context_switched', p_coop, v_owner,
                v_session,
                jsonb_build_object('cooperative_name', v_name));
    END IF;
    RETURN FOUND;
END
$fn$;
REVOKE ALL ON FUNCTION public.app_session_set_cooperative(bytea, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_session_set_cooperative(bytea, uuid)
    TO kooperatif_app;

-- ── 4. Close the direct-write surface ─────────────────────────────
--
-- Every runtime write now flows through the definers above (or runs on
-- the owner lane: CLI admin paths, fixtures, migrations). The runtime
-- principal keeps SELECT (the never_bootstrapped probe) but can no
-- longer fabricate rows with arbitrary attribution.
REVOKE INSERT ON public.security_events FROM kooperatif_app;

-- ── 5. Fail-closed post-checks ────────────────────────────────────

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_proc p
        WHERE p.pronamespace = 'public'::regnamespace
          AND p.proname IN (
              'app_ctx_audit',
              'app_session_audit_platform',
              'app_session_audit_scoped',
              'app_audit_login_failed',
              'app_session_revoke',
              'app_session_revoke_others',
              'app_session_set_cooperative')
          AND (NOT p.prosecdef
               OR NOT (p.proconfig::text[] @> ARRAY['search_path=public, pg_catalog'])
               OR p.proacl IS NULL
               OR EXISTS (SELECT 1 FROM unnest(p.proacl) AS acl
                           WHERE split_part(acl::text, '=', 1) = ''))
    ) THEN
        RAISE EXCEPTION 'an S2B function is not hardened';
    END IF;

    IF NOT has_function_privilege(
        'kooperatif_app',
        'public.app_ctx_audit(text,jsonb)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_audit_platform(bytea,text,jsonb,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_audit_scoped(bytea,uuid,text,jsonb,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_audit_login_failed(jsonb,timestamptz)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_revoke(bytea,uuid,timestamptz,text)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_revoke_others(bytea,timestamptz,text)', 'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_set_cooperative(bytea,uuid)', 'EXECUTE')
    THEN
        RAISE EXCEPTION 'kooperatif_app is missing a required S2B EXECUTE grant';
    END IF;

    IF has_table_privilege(
        'kooperatif_app', 'public.security_events', 'INSERT')
       OR NOT has_table_privilege(
        'kooperatif_app', 'public.security_events', 'SELECT')
    THEN
        RAISE EXCEPTION 'security_events grant boundary drifted';
    END IF;
END $$;
