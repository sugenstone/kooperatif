-- M1-P4-P3C — final trust-boundary security gates.
--
--   1. `app_admin_revoke_user_sessions` now requires the STAMPED,
--      SEALED request context to independently prove administrative
--      authorization: caller's user seal must verify, and the caller
--      must hold `users.manage` in the stamped cooperative scope
--      (bootstrap-attribution aware, mirroring
--      `effective_permissions_in_bootstrap`), while the target user
--      must be member-visible in that scope. The never-bootstrapped
--      lane requires zero cooperatives and a global users.manage
--      grant. Unauthenticated or under-privileged invocation raises
--      `insufficient_privilege` — fail-closed and loud, because the
--      legitimate RBAC path always runs inside a sealed request tx.
--
--   2. `app_session_open` gains database-enforced abuse resistance:
--      a per-username xact advisory lock serializes concurrent
--      attempts, and a failure window (10 verdicts / 300 s, app clock)
--      rejects further attempts. Throttled attempts are audited with
--      reason 'throttled' and EXCLUDED from the window count, so a
--      flood cannot perpetually extend its own lockout, and app-side
--      'rate_limited' rows likewise do not feed the DB counter.
--      Failure verdicts are written at p_now so the window math stays
--      in the application-clock domain.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 1. Privileged session revocation ──────────────────────────────

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
    v_user    uuid;
    v_session uuid;
    v_coop    uuid;
    v_txid    text;
    v_seal    text;
    v_ctx     text;
    v_count   bigint;
BEGIN
    -- Sealed caller context is mandatory.
    v_user    := nullif(current_setting('app.user_id', true), '')::uuid;
    v_session := nullif(current_setting('app.session_id', true), '')::uuid;
    v_coop    := nullif(current_setting('app.cooperative_id', true), '')::uuid;
    v_txid    := nullif(current_setting('app.ctx_txid', true), '');
    v_seal    := nullif(current_setting('app.user_seal', true), '');
    v_ctx     := nullif(current_setting('app.ctx_seal', true), '');

    IF v_user IS NULL OR v_session IS NULL OR v_seal IS NULL
       OR v_txid IS NULL OR v_txid <> txid_current()::text
       OR v_seal <> public.app_seal_of(v_user, v_session, NULL, v_txid::bigint)
    THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed caller context required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF v_coop IS NOT NULL THEN
        -- Cooperative-scoped lane: the stamped cooperative must carry
        -- its own valid seal, the caller must hold users.manage there
        -- (NULL-attributed legacy rows count only through the
        -- marker-consistent bootstrap cooperative), and the target
        -- must be member-visible in that scope.
        IF v_ctx IS NULL
           OR v_ctx <> public.app_seal_of(v_user, v_session, v_coop, v_txid::bigint)
        THEN
            RAISE EXCEPTION 'insufficient_privilege: cooperative context not sealed'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        IF NOT EXISTS (
            SELECT 1
              FROM public.user_role_assignments ura
              JOIN public.roles r ON r.id = ura.role_id
                AND r.status = 'active'
                AND (r.cooperative_id = v_coop
                     OR (r.cooperative_id IS NULL AND EXISTS (
                         SELECT 1 FROM public.installation_state s
                          JOIN public.cooperatives c
                            ON c.id = s.bootstrap_cooperative_id
                           AND c.is_bootstrap
                          WHERE s.bootstrap_cooperative_id = v_coop)))
              JOIN public.cooperative_memberships m
                ON m.cooperative_id = v_coop AND m.user_id = ura.user_id
               AND m.status = 'active'
              JOIN public.role_permissions rp ON rp.role_id = r.id
              JOIN public.permissions p ON p.id = rp.permission_id
                AND p.key = 'users.manage'
             WHERE ura.user_id = v_user
               AND (ura.cooperative_id = v_coop
                    OR (ura.cooperative_id IS NULL AND EXISTS (
                        SELECT 1 FROM public.installation_state s
                         JOIN public.cooperatives c
                           ON c.id = s.bootstrap_cooperative_id
                          AND c.is_bootstrap
                         WHERE s.bootstrap_cooperative_id = v_coop)))
        ) THEN
            RAISE EXCEPTION 'insufficient_privilege: users.manage required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM public.cooperative_memberships m
             WHERE m.user_id = p_user_id AND m.cooperative_id = v_coop
        ) THEN
            RAISE EXCEPTION 'insufficient_privilege: target outside scope'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    ELSE
        -- Never-bootstrapped lane: only while zero cooperatives exist,
        -- and only with a global users.manage grant.
        IF v_ctx IS NOT NULL OR EXISTS (SELECT 1 FROM public.cooperatives) THEN
            RAISE EXCEPTION 'insufficient_privilege: no global lane after bootstrap'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        IF NOT EXISTS (
            SELECT 1
              FROM public.user_role_assignments ura
              JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
              JOIN public.role_permissions rp ON rp.role_id = r.id
              JOIN public.permissions p ON p.id = rp.permission_id
                AND p.key = 'users.manage'
             WHERE ura.user_id = v_user
        ) THEN
            RAISE EXCEPTION 'insufficient_privilege: users.manage required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END IF;

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

-- ── 2. Login abuse resistance ─────────────────────────────────────

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
    -- DB-enforced throttle for direct-SQL invocation of this
    -- function: verdict-bearing failures per username within the
    -- window cap further attempts. 'throttled'/'rate_limited' rows
    -- are deliberately excluded so a flood cannot extend its own
    -- lockout and app-side limiter events do not double-count.
    c_fail_window constant interval := interval '300 seconds';
    c_fail_max    constant integer  := 10;
    u       public.users%ROWTYPE;
    v_ok    boolean;
    v_id    uuid;
    v_fails bigint;
BEGIN
    -- Serialize concurrent attempts on the same username — closes the
    -- race where N parallel connections could all slip under the
    -- failure threshold.
    PERFORM pg_advisory_xact_lock(
        hashtextextended('app_session_open:' || p_username, 0));

    SELECT count(*) INTO v_fails
      FROM public.security_events se
     WHERE se.event_type = 'login_failed'
       AND se.occurred_at <= p_now
       AND se.occurred_at > p_now - c_fail_window
       AND se.metadata ->> 'username' = p_username
       AND se.metadata ->> 'reason' NOT IN ('throttled', 'rate_limited');

    IF v_fails >= c_fail_max THEN
        INSERT INTO public.security_events
            (event_type, user_id, metadata, occurred_at)
        VALUES ('login_failed', NULL,
                jsonb_build_object('username', p_username,
                                   'reason', 'throttled'),
                p_now);
        RETURN;
    END IF;

    SELECT * INTO u FROM public.users x WHERE x.username = p_username LIMIT 1;
    -- Timing equalization: crypt runs against a fixed dummy salt when
    -- the user is missing or has no verifier.
    v_ok := crypt(p_password,
                  coalesce(u.login_verifier,
                           '$2a$10$abcdefghijklmnopqrstuu'))
            = coalesce(u.login_verifier, '');
    -- Failed-login audit is written DB-side (at p_now so the window
    -- math stays in the application-clock domain) so the credential
    -- oracle never leaves the definer: the caller sees a uniform empty
    -- set, while the durable audit keeps its reason granularity.
    IF u.id IS NULL THEN
        INSERT INTO public.security_events
            (event_type, user_id, metadata, occurred_at)
        VALUES ('login_failed', NULL,
                jsonb_build_object('username', p_username,
                                   'reason', 'unknown_username'),
                p_now);
        RETURN;
    ELSIF u.status <> 'active' THEN
        INSERT INTO public.security_events
            (event_type, user_id, metadata, occurred_at)
        VALUES ('login_failed', u.id,
                jsonb_build_object('username', p_username,
                                   'reason', 'user_disabled'),
                p_now);
        RETURN;
    ELSIF NOT v_ok THEN
        INSERT INTO public.security_events
            (event_type, user_id, metadata, occurred_at)
        VALUES ('login_failed', u.id,
                jsonb_build_object('username', p_username,
                                   'reason', 'invalid_credentials'),
                p_now);
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

-- ── 3. Fail-closed post-checks ────────────────────────────────────

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_proc p
        WHERE p.pronamespace = 'public'::regnamespace
          AND p.proname IN ('app_admin_revoke_user_sessions', 'app_session_open')
          AND (NOT p.prosecdef
               OR NOT (p.proconfig::text[] @> ARRAY['search_path=public, pg_catalog'])
               OR p.proacl IS NULL
               OR EXISTS (SELECT 1 FROM unnest(p.proacl) AS acl
                           WHERE split_part(acl::text, '=', 1) = ''))
    ) THEN
        RAISE EXCEPTION 'a P3C function is not hardened';
    END IF;
END $$;

-- ── 4. Numbering counter isolation ────────────────────────────────
--
-- `cooperative_number_counters` is the only counter table the runtime
-- touched directly: kooperatif_app held raw INSERT/UPDATE/SELECT on it,
-- and the INVOKER allocators never verified that p_cooperative_id is
-- the sealed request scope. Arbitrary SQL could therefore burn or
-- inflate a FOREIGN cooperative's numbering sequence. The allocators
-- become SECURITY DEFINER and, when the real runtime principal calls
-- them, require the sealed cooperative context to match the requested
-- scope. The owner lane (migrations, fixtures, CLI) is unaffected.

CREATE OR REPLACE FUNCTION public.allocate_document_number(
    p_cooperative_id uuid,
    p_series_key     text
)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_next bigint;
BEGIN
    IF session_user = 'kooperatif_app'
       AND (NOT public.app_ctx_ok()
            OR nullif(current_setting('app.cooperative_id', true), '')::uuid
                   IS DISTINCT FROM p_cooperative_id)
    THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed cooperative context required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    INSERT INTO public.cooperative_number_counters
        (cooperative_id, series_key, next_value)
    VALUES (p_cooperative_id, p_series_key, 2)
    ON CONFLICT (cooperative_id, series_key) DO UPDATE
        SET next_value = cooperative_number_counters.next_value + 1
    RETURNING next_value - 1 INTO v_next;
    RETURN v_next;
END
$fn$;

CREATE OR REPLACE FUNCTION public.reserve_document_number(
    p_cooperative_id uuid,
    p_series_key     text,
    p_value          bigint
)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_next bigint;
BEGIN
    IF session_user = 'kooperatif_app'
       AND (NOT public.app_ctx_ok()
            OR nullif(current_setting('app.cooperative_id', true), '')::uuid
                   IS DISTINCT FROM p_cooperative_id)
    THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed cooperative context required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    INSERT INTO public.cooperative_number_counters
        (cooperative_id, series_key, next_value)
    VALUES (p_cooperative_id, p_series_key, p_value + 1)
    ON CONFLICT (cooperative_id, series_key) DO UPDATE
        SET next_value = GREATEST(
            cooperative_number_counters.next_value,
            EXCLUDED.next_value)
    RETURNING next_value INTO v_next;
    RETURN v_next;
END
$fn$;

REVOKE ALL ON FUNCTION public.allocate_document_number(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.allocate_document_number(uuid, text) TO kooperatif_app;
REVOKE ALL ON FUNCTION public.reserve_document_number(uuid, text, bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.reserve_document_number(uuid, text, bigint) TO kooperatif_app;

-- The counters move fully behind the definer allocators — the runtime
-- principal no longer writes the table directly.
REVOKE INSERT, UPDATE, DELETE, SELECT, TRUNCATE, REFERENCES, TRIGGER
    ON public.cooperative_number_counters FROM kooperatif_app;

DO $$
BEGIN
    IF has_table_privilege('kooperatif_app', 'public.cooperative_number_counters', 'SELECT')
        OR has_table_privilege('kooperatif_app', 'public.cooperative_number_counters', 'INSERT')
        OR has_table_privilege('kooperatif_app', 'public.cooperative_number_counters', 'UPDATE')
        OR has_table_privilege('kooperatif_app', 'public.cooperative_number_counters', 'DELETE')
    THEN
        RAISE EXCEPTION 'kooperatif_app still holds counter privileges';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_proc p
        WHERE p.pronamespace = 'public'::regnamespace
          AND p.proname IN ('allocate_document_number', 'reserve_document_number')
          AND (NOT p.prosecdef
               OR NOT (p.proconfig::text[] @> ARRAY['search_path=public, pg_catalog']))
    ) THEN
        RAISE EXCEPTION 'a numbering function is not hardened';
    END IF;
END $$;

-- ── 5. Membership-resolution oracle ───────────────────────────────
--
-- `app_verify_membership` was EXECUTE-granted to the runtime and, while
-- no context is stamped, resolves ARBITRARY (user, cooperative) pairs —
-- a membership/enumeration oracle over security_events-adjacent facts
-- (membership status + cooperative name/status). No production path
-- calls it directly: `app_stamp_cooperative` and
-- `app_session_set_cooperative` invoke it internally under definer
-- rights (the caller's EXECUTE privilege is not needed inside a
-- SECURITY DEFINER chain). Direct EXECUTE is revoked; internal use is
-- unaffected.

REVOKE ALL ON FUNCTION public.app_verify_membership(uuid, uuid) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION public.app_verify_membership(uuid, uuid) FROM kooperatif_app;

DO $$
BEGIN
    IF has_function_privilege(
        'kooperatif_app', 'public.app_verify_membership(uuid,uuid)', 'EXECUTE')
    THEN
        RAISE EXCEPTION 'app_verify_membership still executable by kooperatif_app';
    END IF;
END $$;
