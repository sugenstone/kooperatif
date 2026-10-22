-- M1-P4-S4 — FORCE RLS on `cooperative_memberships`.
--
-- Access-path inventory (verified in code):
--
--   WRITES — no runtime principal ever writes this table. Bootstrap
--     claim (`bootstrap_initial_cooperative`) and `add_member` run on
--     the owner/CLI lane (main.rs). `kooperatif_app` holds only `r`
--     (SELECT) — INSERT/UPDATE/DELETE were never granted.
--
--   PRE-CONTEXT / SESSION-BOUND reads — all resolve through
--     kooperatif-owned SECURITY DEFINERs (owner lane, RLS-exempt):
--     app_verify_membership, app_stamp_cooperative, request context
--     opening, app_session_membership(s), app_session_memberships,
--     app_session_business_membership_ok, app_session_audit_scoped,
--     app_session_set_cooperative, app_session_legacy_lane_ok, the S2A
--     permission/role-summary readers, realtime revalidation.
--
--   SEALED-TRANSACTION reads (kooperatif_app, direct) — only the
--     cooperative-scoped administration paths join this table, always
--     inside request_tx/begin_guarded_transaction and always
--     coop-scoped: member_visibility EXISTS probes and
--     administration_path_count's membership join (rbac.rs / authz.rs).
--     NULL-attributed membership rows are NEVER read on the app lane
--     (the NULL/legacy membership facts resolve through definers).
--
--   Dead-at-runtime helpers (effective_permissions_in_*,
--   active_role_summaries_*) survive only in owner-pool tests.
--
-- Policy set: owner lane + ONE app lane bound to the sealed
-- cooperative. No NULL arm — there is no legitimate runtime read of
-- NULL-attributed membership rows. The policy calls app_ctx_ok()
-- (GUC/seal verification only — it does not read this table, so no
-- circular RLS dependency). WITH CHECK mirrors USING: should a write
-- grant ever be re-added, cross-cooperative/NULL writes still fail.
--
-- The existing `r` grant is kept — the sealed-tx joins above are the
-- justification; without it queries would fail at the grant wall
-- before RLS could bind them.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

ALTER TABLE public.cooperative_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.cooperative_memberships FORCE ROW LEVEL SECURITY;

CREATE POLICY cooperative_memberships_owner_lane
    ON public.cooperative_memberships
    TO kooperatif
    USING (true)
    WITH CHECK (true);

CREATE POLICY cooperative_memberships_app_scope
    ON public.cooperative_memberships
    FOR ALL
    TO kooperatif_app
    USING (
        public.app_ctx_ok()
        AND cooperative_id =
            nullif(current_setting('app.cooperative_id', true), '')::uuid
    )
    WITH CHECK (
        public.app_ctx_ok()
        AND cooperative_id =
            nullif(current_setting('app.cooperative_id', true), '')::uuid
    );

-- ── Fail-closed post-checks ───────────────────────────────────────

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class
         WHERE relname = 'cooperative_memberships'
           AND relnamespace = 'public'::regnamespace
           AND relrowsecurity AND relforcerowsecurity
    ) THEN
        RAISE EXCEPTION 'cooperative_memberships RLS not enabled/forced';
    END IF;

    IF (SELECT count(*) FROM pg_policy
         WHERE polrelid = 'public.cooperative_memberships'::regclass) <> 2 THEN
        RAISE EXCEPTION 'cooperative_memberships must carry owner + app policies';
    END IF;

    -- The runtime role must hold SELECT (sealed-tx joins) and nothing
    -- beyond it.
    IF NOT has_table_privilege(
        'kooperatif_app', 'public.cooperative_memberships', 'SELECT')
       OR has_table_privilege(
        'kooperatif_app', 'public.cooperative_memberships', 'INSERT')
       OR has_table_privilege(
        'kooperatif_app', 'public.cooperative_memberships', 'UPDATE')
       OR has_table_privilege(
        'kooperatif_app', 'public.cooperative_memberships', 'DELETE') THEN
        RAISE EXCEPTION 'cooperative_memberships grant surface drifted';
    END IF;

    -- Fleet boundary: 39 tables / 76 policies (through S3) + this
    -- table with two policies.
    IF (SELECT count(*) FROM pg_class c
         WHERE c.relrowsecurity AND c.relforcerowsecurity
           AND c.relnamespace = 'public'::regnamespace
           AND c.relkind = 'r') <> 40
       OR (SELECT count(*) FROM pg_policy p
            JOIN pg_class c ON c.oid = p.polrelid
           WHERE c.relnamespace = 'public'::regnamespace) <> 78 THEN
        RAISE EXCEPTION 'RLS fleet boundary drifted';
    END IF;
END $$;
