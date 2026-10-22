-- M1-P5 B4 — audit read API (discovery: M1-P5-B4-AUDIT-READ-DESIGN.md).
--
-- security_events has had NO read surface until now: the runtime role
-- holds neither SELECT nor INSERT, and FORCE RLS plus the owner-lane
-- policy keep the table sealed. This migration opens exactly two
-- credential-bound SECURITY DEFINER readers — the same boundary shape
-- as app_session_permissions_scoped (0047) and the mutation definers
-- (0052+):
--
--   * app_audit_read_scoped — cooperative lane. The caller presents a
--     claimed cooperative; the definer re-derives the principal from
--     the session token hash, re-verifies an ACTIVE membership in an
--     ACTIVE cooperative, and re-checks `audit.read` inside the
--     effective scoped permission set. Rows are restricted to
--     `cooperative_id = p_cooperative_id` — a caller can never widen
--     the scope. In the provably never-bootstrapped lane (p_cooperative_id
--     NULL) the legacy permission set is consulted and only NULL-scope
--     (pre-tenancy/platform) events are returned.
--
--   * app_audit_read_platform — platform lane (decision D4). Requires
--     an explicit NULL-scope `audit.read` grant on an active user via
--     an active NULL-scope role/assignment — a cooperative-scoped
--     audit.read can never reach this path. An optional cooperativeId
--     filter is caller-selected (platform administrators may inspect
--     any cooperative), NULL means all events including the platform
--     lane.
--
-- Both readers: keyset pagination (occurred_at DESC, id DESC) driven
-- by (p_before_at, p_before_id); LIMIT is caller-bounded (≤100 enforced
-- by the route, clamped here defensively). Read-only STABLE functions
-- — they cannot write, delete or forge audit rows, and the runtime
-- role still has zero direct privileges on security_events.
--
-- Permission model (D2): `audit.read` is a NEW catalog key. It is
-- deliberately NOT granted to any seed role — assignment is an
-- explicit administrative act.
--
-- Append-only. No historical migration is modified.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 1. Permission seed ──────────────────────────────────────────────
INSERT INTO public.permissions (key, name, description, category) VALUES
    ('audit.read',
     'Denetim Kayıtlarını Görüntüle',
     'Kooperatifin denetim (audit) kayıtlarını okuyabilir; platform kapsamında verilirse platform denetim kayıtlarını görebilir.',
     'Denetim');

-- Explicit migration-time grant to the system-seeded administrative
-- role (fixed id) — same convention as every permission migration.
-- No wildcard inheritance; custom roles untouched.
INSERT INTO public.role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM public.permissions p
WHERE p.key = 'audit.read'
ON CONFLICT DO NOTHING;

-- 0019 contract: any permission granted to the seeded admin role must
-- also extend the `system-administrator` template (provisioning mirror).
INSERT INTO public.system_role_template_permissions (template_id, permission_id)
SELECT t.id, p.id
FROM public.system_role_templates t, public.permissions p
WHERE t.key = 'system-administrator'
  AND p.key = 'audit.read'
ON CONFLICT DO NOTHING;

-- ── 2. Keyset index ─────────────────────────────────────────────────
-- Scoped reads are always (cooperative_id, occurred_at DESC, id DESC)
-- scans; the platform lane reuses the same index when a cooperativeId
-- filter is given and the existing occurred_at DESC index otherwise.
CREATE INDEX security_events_coop_occurred_id_idx
    ON public.security_events (cooperative_id, occurred_at DESC, id DESC);

-- ── 3. Cooperative-scoped reader ────────────────────────────────────
CREATE FUNCTION public.app_audit_read_scoped(
    p_token_hash      bytea,
    p_cooperative_id  uuid,
    p_now             timestamptz,
    p_event_type      text,
    p_actor_user_id   uuid,
    p_from            timestamptz,
    p_to              timestamptz,
    p_before_at       timestamptz,
    p_before_id       uuid,
    p_limit           integer
)
RETURNS TABLE (
    id              uuid,
    occurred_at     timestamptz,
    event_type      text,
    actor_user_id   uuid,
    actor_username  text,
    cooperative_id  uuid,
    metadata        jsonb
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user uuid;
BEGIN
    v_user := public.app_session_owner(p_token_hash, p_now);
    IF v_user IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: valid session required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF p_cooperative_id IS NOT NULL THEN
        -- Cooperative lane: verified active membership in an active
        -- cooperative AND audit.read inside the effective scoped set
        -- (bootstrap NULL-attribution arm included via the canonical
        -- resolver — never a caller-chosen scope).
        IF NOT EXISTS (
            SELECT 1
              FROM public.cooperative_memberships m
              JOIN public.cooperatives c ON c.id = m.cooperative_id
             WHERE m.user_id = v_user
               AND m.cooperative_id = p_cooperative_id
               AND m.status = 'active'
               AND c.status = 'active') THEN
            RAISE EXCEPTION 'insufficient_privilege: active membership required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;
        IF NOT EXISTS (
            SELECT 1
              FROM public.app_session_permissions_scoped(
                       p_token_hash, p_cooperative_id, p_now) AS k(key)
             WHERE k.key = 'audit.read') THEN
            RAISE EXCEPTION 'insufficient_privilege: audit.read required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;

        RETURN QUERY
            SELECT e.id, e.occurred_at, e.event_type,
                   e.user_id, u.username, e.cooperative_id, e.metadata
              FROM public.security_events e
              LEFT JOIN public.users u ON u.id = e.user_id
             WHERE e.cooperative_id = p_cooperative_id
               AND (p_event_type IS NULL OR e.event_type = p_event_type)
               AND (p_actor_user_id IS NULL OR e.user_id = p_actor_user_id)
               AND (p_from IS NULL OR e.occurred_at >= p_from)
               AND (p_to IS NULL OR e.occurred_at <= p_to)
               AND (p_before_at IS NULL
                    OR e.occurred_at < p_before_at
                    OR (e.occurred_at = p_before_at AND e.id < p_before_id))
             ORDER BY e.occurred_at DESC, e.id DESC
             LIMIT least(greatest(p_limit, 1), 100);
    ELSE
        -- Never-bootstrapped lane only: the legacy global permission
        -- set is consulted (it is empty once a bootstrap cooperative
        -- exists), and only NULL-attributed pre-tenancy events are
        -- visible — platform events of a tenanted installation never
        -- leak through the cooperative lane.
        IF NOT public.app_session_legacy_lane_ok()
           OR NOT EXISTS (
               SELECT 1
                 FROM public.app_session_permissions_legacy(
                          p_token_hash, p_now) AS k(key)
                WHERE k.key = 'audit.read') THEN
            RAISE EXCEPTION 'insufficient_privilege: audit.read required'
                USING ERRCODE = 'insufficient_privilege';
        END IF;

        RETURN QUERY
            SELECT e.id, e.occurred_at, e.event_type,
                   e.user_id, u.username, e.cooperative_id, e.metadata
              FROM public.security_events e
              LEFT JOIN public.users u ON u.id = e.user_id
             WHERE e.cooperative_id IS NULL
               AND (p_event_type IS NULL OR e.event_type = p_event_type)
               AND (p_actor_user_id IS NULL OR e.user_id = p_actor_user_id)
               AND (p_from IS NULL OR e.occurred_at >= p_from)
               AND (p_to IS NULL OR e.occurred_at <= p_to)
               AND (p_before_at IS NULL
                    OR e.occurred_at < p_before_at
                    OR (e.occurred_at = p_before_at AND e.id < p_before_id))
             ORDER BY e.occurred_at DESC, e.id DESC
             LIMIT least(greatest(p_limit, 1), 100);
    END IF;
END
$fn$;

REVOKE ALL ON FUNCTION public.app_audit_read_scoped(
    bytea, uuid, timestamptz, text, uuid,
    timestamptz, timestamptz, timestamptz, uuid, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_audit_read_scoped(
    bytea, uuid, timestamptz, text, uuid,
    timestamptz, timestamptz, timestamptz, uuid, integer)
    TO kooperatif_app;

-- ── 4. Platform reader (D4) ─────────────────────────────────────────
CREATE FUNCTION public.app_audit_read_platform(
    p_token_hash      bytea,
    p_now             timestamptz,
    p_cooperative_id  uuid,          -- optional caller filter
    p_event_type      text,
    p_actor_user_id   uuid,
    p_from            timestamptz,
    p_to              timestamptz,
    p_before_at       timestamptz,
    p_before_id       uuid,
    p_limit           integer
)
RETURNS TABLE (
    id              uuid,
    occurred_at     timestamptz,
    event_type      text,
    actor_user_id   uuid,
    actor_username  text,
    cooperative_id  uuid,
    metadata        jsonb
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user uuid;
BEGIN
    v_user := public.app_session_owner(p_token_hash, p_now);
    IF v_user IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: valid session required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    -- Explicit platform-scope permission: NULL-scope audit.read via an
    -- active NULL-scope role + assignment held by an ACTIVE user.
    -- Cooperative-scoped grants never satisfy this predicate.
    IF NOT EXISTS (
        SELECT 1
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id
            AND r.status = 'active'
            AND r.cooperative_id IS NULL
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
            AND p.key = 'audit.read'
          JOIN public.users uu ON uu.id = ura.user_id
            AND uu.status = 'active'
         WHERE ura.user_id = v_user
           AND ura.cooperative_id IS NULL) THEN
        RAISE EXCEPTION 'insufficient_privilege: platform audit.read required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    RETURN QUERY
        SELECT e.id, e.occurred_at, e.event_type,
               e.user_id, u.username, e.cooperative_id, e.metadata
          FROM public.security_events e
          LEFT JOIN public.users u ON u.id = e.user_id
         WHERE (p_cooperative_id IS NULL
                OR e.cooperative_id IS NOT DISTINCT FROM p_cooperative_id)
           AND (p_event_type IS NULL OR e.event_type = p_event_type)
           AND (p_actor_user_id IS NULL OR e.user_id = p_actor_user_id)
           AND (p_from IS NULL OR e.occurred_at >= p_from)
           AND (p_to IS NULL OR e.occurred_at <= p_to)
           AND (p_before_at IS NULL
                OR e.occurred_at < p_before_at
                OR (e.occurred_at = p_before_at AND e.id < p_before_id))
         ORDER BY e.occurred_at DESC, e.id DESC
         LIMIT least(greatest(p_limit, 1), 100);
END
$fn$;

REVOKE ALL ON FUNCTION public.app_audit_read_platform(
    bytea, timestamptz, uuid, text, uuid,
    timestamptz, timestamptz, timestamptz, uuid, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_audit_read_platform(
    bytea, timestamptz, uuid, text, uuid,
    timestamptz, timestamptz, timestamptz, uuid, integer)
    TO kooperatif_app;

-- ── Postchecks ──────────────────────────────────────────────────────
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM public.permissions WHERE key = 'audit.read') THEN
        RAISE EXCEPTION 'audit.read permission seed missing';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_index i
         JOIN pg_class c ON c.oid = i.indrelid
         WHERE c.relname = 'security_events'
           AND i.indrelid = 'public.security_events'::regclass
           AND (SELECT a.attname FROM pg_attribute a
                 WHERE a.attrelid = i.indrelid
                   AND a.attnum = i.indkey[0]) = 'cooperative_id'
           AND i.indexrelid = 'security_events_coop_occurred_id_idx'::regclass) THEN
        RAISE EXCEPTION 'security_events_coop_occurred_id_idx missing';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.proname IN ('app_audit_read_scoped',
                             'app_audit_read_platform')
           AND p.pronamespace = 'public'::regnamespace
           AND (NOT p.prosecdef
                OR p.proacl IS NULL
                OR EXISTS (SELECT 1 FROM aclexplode(p.proacl) x
                            WHERE x.grantee = 0))) THEN
        RAISE EXCEPTION 'audit readers must be SECURITY DEFINER and not PUBLIC';
    END IF;
    IF NOT has_function_privilege(
        'kooperatif_app',
        'public.app_audit_read_scoped(bytea,uuid,timestamptz,text,uuid,timestamptz,timestamptz,timestamptz,uuid,integer)',
        'EXECUTE')
       OR NOT has_function_privilege(
        'kooperatif_app',
        'public.app_audit_read_platform(bytea,timestamptz,uuid,text,uuid,timestamptz,timestamptz,timestamptz,uuid,integer)',
        'EXECUTE') THEN
        RAISE EXCEPTION 'audit readers must be executable by kooperatif_app';
    END IF;
END $$;
