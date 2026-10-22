-- M1-P4-S3 — FORCE RLS on `security_events`.
--
-- S2B (0048) made the write surface exclusive: every runtime event now
-- flows through narrowly scoped SECURITY DEFINER functions
-- (app_ctx_audit, app_session_audit_platform/scoped,
-- app_audit_login_failed, and the session-lifecycle definers that mint
-- their events internally). The direct INSERT grant is already gone.
--
-- The one remaining direct-runtime dependency is the
-- `never_bootstrapped` probe in tenant/repo.rs, which reads
-- security_events on the kooperatif_app pool as part of its
-- tamper-evidence predicate ("zero cooperative-attributed audit
-- events"). Under FORCE RLS with no app policy that arm would
-- silently see zero rows — WEAKENING the check (a wiped `cooperatives`
-- table plus surviving audit rows would no longer be detected). The
-- identical predicate already exists definer-side as
-- `app_session_legacy_lane_ok()` (0047), so this migration exposes it
-- to the runtime role and the probe is repointed — no privilege
-- increase: the app could already evaluate the same rows today.
--
-- Policy choice: OWNER LANE ONLY, mirroring S1
-- (cooperative_number_counters). Every legitimate runtime read/write
-- now executes inside a SECURITY DEFINER owned by `kooperatif`, so no
-- kooperatif_app policy is justified — under FORCE RLS, absence of a
-- policy is a complete deny for the runtime principal, and the SELECT
-- grant is revoked as well so the table is fully closed to direct
-- runtime access.
--
-- `kooperatif` is superuser+BYPASSRLS today, so the owner policy is
-- belt-and-braces: it documents the intended lane and keeps the
-- invariants intact should that role ever lose superuser.
--
-- Event history, attribution, event types, and the S2B
-- mandatory/best-effort semantics are unchanged — this migration
-- touches only the access boundary.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 1. Expose the never-bootstrapped predicate as the probe ───────
--
-- Zero-argument boolean SECURITY DEFINER; returns one boolean, no
-- rows, no parameters to abuse. This is a narrower surface than the
-- SELECT grant it replaces.
GRANT EXECUTE ON FUNCTION public.app_session_legacy_lane_ok()
    TO kooperatif_app;

-- ── 2. Enable + FORCE RLS, owner lane only ────────────────────────

ALTER TABLE public.security_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.security_events FORCE ROW LEVEL SECURITY;

CREATE POLICY security_events_owner_lane ON public.security_events
    TO kooperatif
    USING (true)
    WITH CHECK (true);

-- ── 3. Close the last direct grant ────────────────────────────────
--
-- With the probe on the definer, the runtime principal needs no
-- direct visibility into the audit table at all.
REVOKE SELECT ON public.security_events FROM kooperatif_app;

-- ── 4. Fail-closed post-checks ────────────────────────────────────

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class
         WHERE relname = 'security_events'
           AND relnamespace = 'public'::regnamespace
           AND relrowsecurity AND relforcerowsecurity
    ) THEN
        RAISE EXCEPTION 'security_events RLS not enabled/forced';
    END IF;

    IF (SELECT count(*) FROM pg_policy
         WHERE polrelid = 'public.security_events'::regclass) <> 1 THEN
        RAISE EXCEPTION 'security_events must carry exactly the owner policy';
    END IF;

    IF has_table_privilege(
        'kooperatif_app', 'public.security_events', 'INSERT')
       OR has_table_privilege(
        'kooperatif_app', 'public.security_events', 'UPDATE')
       OR has_table_privilege(
        'kooperatif_app', 'public.security_events', 'DELETE')
       OR has_table_privilege(
        'kooperatif_app', 'public.security_events', 'SELECT') THEN
        RAISE EXCEPTION 'kooperatif_app still holds a direct security_events privilege';
    END IF;

    IF NOT has_function_privilege(
        'kooperatif_app',
        'public.app_session_legacy_lane_ok()', 'EXECUTE') THEN
        RAISE EXCEPTION 'never_bootstrapped probe definer not reachable';
    END IF;

    -- Fleet boundary: 38 tables / 75 policies (S1 fleet) + this table.
    IF (SELECT count(*) FROM pg_class c
         WHERE c.relrowsecurity AND c.relforcerowsecurity
           AND c.relnamespace = 'public'::regnamespace
           AND c.relkind = 'r') <> 39
       OR (SELECT count(*) FROM pg_policy p
            JOIN pg_class c ON c.oid = p.polrelid
           WHERE c.relnamespace = 'public'::regnamespace) <> 76 THEN
        RAISE EXCEPTION 'RLS fleet boundary drifted';
    END IF;
END $$;
