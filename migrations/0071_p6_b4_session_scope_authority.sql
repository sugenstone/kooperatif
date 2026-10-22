-- 0071_p6_b4_session_scope_authority.sql
-- M1-P6 B4 — multi-cooperative business authorization.
--
--   1. SESSION-AUTHORITATIVE COOPERATIVE STAMPING. Until now
--      app_stamp_cooperative accepted ANY cooperative the session
--      owner was an active member of — the application chose the
--      scope (bootstrap pin or x-cooperative-id). From B4 the scope of
--      a business transaction is the authenticated session's
--      `active_cooperative_id`, changed only through the audited
--      switch workflow (app_session_set_cooperative). A session that
--      never selected a cooperative keeps the pre-B4 bootstrap scope
--      (compatibility lane — exactly the former headerless behaviour).
--      The database now enforces this itself: stamping any other
--      cooperative returns no row, so a request header, body, query
--      parameter or an application bug can never open a sealed
--      context in a cooperative the session did not switch into.
--
--   2. MEMBERSHIP PERMISSION SEPARATION (approved). `memberships.read`
--      joins the catalog as a distinct cooperative-scoped permission
--      (list memberships + cooperative-scoped role assignments).
--      `memberships.manage` keeps the write lane only. `users.read`
--      keeps its existing responsibilities untouched.
--
--      Grant audit (least privilege): `memberships.read` is granted
--      ONLY to roles/templates that already hold `users.manage` —
--      the principals that can already change membership status
--      through the B3 compatibility lane and therefore must be able
--      to see the memberships they act on. No role receives
--      `memberships.manage`, and no platform permission is touched.
--
-- Append-only; no historical migration is edited.

-- ── 1. Permission catalog ───────────────────────────────────────────

INSERT INTO public.permissions (key, name, description, category) VALUES
    ('memberships.read',
     'Kooperatif Üyeliklerini Görüntüle',
     'Kendi kooperatifindeki üyelikleri ve kooperatif kapsamlı rol atamalarını görüntüleyebilir; kooperatif kapsamında geçerlidir.',
     'Kullanıcı Yönetimi');

-- The B1 description of memberships.manage still advertised reading;
-- the separation makes it a write-only permission.
UPDATE public.permissions
   SET description = 'Kendi kooperatifine kullanıcı ekleyebilir, üyelikleri askıya alabilir, yeniden etkinleştirebilir ve sonlandırabilir; kooperatif kapsamında geçerlidir.'
 WHERE key = 'memberships.manage';

-- ── 2. Justified grants (users.manage holders only) ─────────────────

INSERT INTO public.role_permissions (role_id, permission_id)
SELECT DISTINCT rp.role_id, mr.id
  FROM public.role_permissions rp
  JOIN public.permissions um ON um.id = rp.permission_id
                            AND um.key = 'users.manage'
  CROSS JOIN public.permissions mr
 WHERE mr.key = 'memberships.read'
ON CONFLICT DO NOTHING;

INSERT INTO public.system_role_template_permissions (template_id, permission_id)
SELECT DISTINCT tp.template_id, mr.id
  FROM public.system_role_template_permissions tp
  JOIN public.permissions um ON um.id = tp.permission_id
                            AND um.key = 'users.manage'
  CROSS JOIN public.permissions mr
 WHERE mr.key = 'memberships.read'
ON CONFLICT DO NOTHING;

-- ── 3. Session-authoritative cooperative stamping ───────────────────
-- Body of 0038 carried verbatim; the only addition is the scope guard
-- between the seal check and the membership verification.
CREATE OR REPLACE FUNCTION public.app_stamp_cooperative(p_coop uuid)
RETURNS TABLE(membership_status text, cooperative_name text,
              cooperative_status text, business_enabled boolean)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $fn$
DECLARE
    v_user     uuid;
    v_session  uuid;
    v_txid     text;
    v_seal     text;
    v_expected uuid;
BEGIN
    v_user    := nullif(current_setting('app.user_id', true), '')::uuid;
    v_session := nullif(current_setting('app.session_id', true), '')::uuid;
    v_txid    := nullif(current_setting('app.ctx_txid', true), '');
    v_seal    := nullif(current_setting('app.user_seal', true), '');

    -- Forgery/replay guard: the stamped user seal must recompute from
    -- the presented GUCs, and the seal must belong to THIS tx.
    IF v_user IS NULL OR v_session IS NULL OR v_seal IS NULL
       OR v_txid IS NULL OR v_txid <> txid_current()::text
       OR v_seal <> public.app_seal_of(v_user, v_session, NULL, v_txid::bigint)
    THEN
        RETURN;
    END IF;

    -- B4 scope guard: the only stampable cooperative is the session's
    -- authoritative scope — its active_cooperative_id, or the
    -- bootstrap cooperative while the session never switched. The
    -- session row is the sealed one (v_session came from the verified
    -- user seal), so the caller cannot substitute another session.
    SELECT coalesce(
               us.active_cooperative_id,
               (SELECT b.cooperative_id FROM public.app_resolve_bootstrap() b))
      INTO v_expected
      FROM public.user_sessions us
     WHERE us.id = v_session
       AND us.user_id = v_user;
    IF v_expected IS NULL OR p_coop IS DISTINCT FROM v_expected THEN
        RETURN;
    END IF;

    RETURN QUERY
        SELECT m.membership_status, m.cooperative_name,
               m.cooperative_status, m.business_enabled
          FROM public.app_verify_membership(v_user, p_coop) m;
    IF NOT FOUND THEN
        RETURN;
    END IF;

    PERFORM set_config('app.cooperative_id', p_coop::text, true);
    PERFORM set_config(
        'app.ctx_seal',
        public.app_seal_of(v_user, v_session, p_coop, v_txid::bigint),
        true);
END
$fn$;

REVOKE ALL ON FUNCTION public.app_stamp_cooperative(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_stamp_cooperative(uuid)
    TO kooperatif_app;

-- ── 4. Postchecks ───────────────────────────────────────────────────

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM public.permissions WHERE key = 'memberships.read') THEN
        RAISE EXCEPTION 'B4 postcheck: memberships.read missing';
    END IF;

    -- memberships.read only where users.manage already is.
    IF EXISTS (
        SELECT 1
          FROM public.role_permissions rp
          JOIN public.permissions p ON p.id = rp.permission_id
         WHERE p.key = 'memberships.read'
           AND NOT EXISTS (
               SELECT 1 FROM public.role_permissions rp2
                 JOIN public.permissions p2 ON p2.id = rp2.permission_id
                WHERE rp2.role_id = rp.role_id AND p2.key = 'users.manage'))
    OR EXISTS (
        SELECT 1
          FROM public.system_role_template_permissions tp
          JOIN public.permissions p ON p.id = tp.permission_id
         WHERE p.key = 'memberships.read'
           AND NOT EXISTS (
               SELECT 1 FROM public.system_role_template_permissions tp2
                 JOIN public.permissions p2 ON p2.id = tp2.permission_id
                WHERE tp2.template_id = tp.template_id
                  AND p2.key = 'users.manage'))
    THEN
        RAISE EXCEPTION 'B4 postcheck: memberships.read granted beyond users.manage holders';
    END IF;

    -- No migration-time grant of memberships.manage or cooperatives.manage
    -- outside the B2 platform-lifecycle seed.
    IF EXISTS (
        SELECT 1
          FROM public.role_permissions rp
          JOIN public.permissions p ON p.id = rp.permission_id
         WHERE p.key = 'memberships.manage')
    OR EXISTS (
        SELECT 1
          FROM public.system_role_template_permissions tp
          JOIN public.permissions p ON p.id = tp.permission_id
         WHERE p.key IN ('memberships.manage', 'cooperatives.manage'))
    THEN
        RAISE EXCEPTION 'B4 postcheck: unexpected manage grant';
    END IF;

    IF EXISTS (
        SELECT 1
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public'
           AND p.proname = 'app_stamp_cooperative'
           AND (NOT p.prosecdef OR p.proacl::text LIKE '%PUBLIC%'))
    THEN
        RAISE EXCEPTION 'B4 postcheck: app_stamp_cooperative must stay sealed';
    END IF;

    IF pg_get_functiondef('public.app_stamp_cooperative(uuid)'::regprocedure)
           NOT LIKE '%active_cooperative_id%'
    THEN
        RAISE EXCEPTION 'B4 postcheck: session scope guard missing';
    END IF;
END
$$;
