-- M2 — PD-14 platform password reset + PD-15 audit.export.
-- Append-only. NOT APPLIED anywhere except test DBs.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- 1. audit.export permission (separate from audit.read, PD-15).
INSERT INTO public.permissions (key, name, description, category) VALUES
    ('audit.export',
     'Denetim Kayıtlarını Dışa Aktar',
     'Denetim kayıtlarını CSV olarak dışa aktarabilir; ayrı yetki, audit.read tek başına yetmez.',
     'Denetim')
ON CONFLICT (key) DO NOTHING;

INSERT INTO public.role_permissions (role_id, permission_id)
SELECT '00000000-0000-4000-8000-000000000001', p.id
FROM public.permissions p WHERE p.key = 'audit.export'
ON CONFLICT DO NOTHING;

-- ------------------------------------------------------------------
-- 2. app_platform_password_reset — PD-14: platform-lane admin reset.
-- Requires an active NULL-scope users.manage grant (same verification
-- shape as app_platform_user_set_status, 0060). Rotates the target's
-- credentials to an operator-issued TEMPORARY secret, forces
-- must_change_password, revokes every session, and mints
-- 'password_reset_by_admin'. The plaintext never touches the audit row.
-- ------------------------------------------------------------------
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
       SET password_hash        = p_new_password_hash,
           login_verifier       = crypt(p_new_verifier, gen_salt('bf')),
           must_change_password = true,
           password_changed_at  = now(),
           updated_at           = now()
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

-- ------------------------------------------------------------------
-- 3. app_audit_export_scoped — cooperative CSV export lane (PD-15).
-- Same row contract as app_audit_read_scoped but requires
-- `audit.export` INSIDE the sealed cooperative scope — audit.read
-- alone cannot reach it. Page-clamped like the reader.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_audit_export_scoped(
    p_cooperative_id uuid,
    p_event_type     text,
    p_actor_user_id  uuid,
    p_from           timestamptz,
    p_to             timestamptz,
    p_before_at      timestamptz,
    p_before_id      uuid,
    p_limit          int
)
RETURNS TABLE (
    id uuid, occurred_at timestamptz, event_type text,
    actor_user_id uuid, actor_username text, cooperative_id uuid,
    metadata jsonb
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $function$
DECLARE
    v_user uuid := nullif(current_setting('app.user_id', true), '')::uuid;
BEGIN
    IF v_user IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: sealed session required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF p_cooperative_id IS NULL
       OR NOT EXISTS (
        SELECT 1
          FROM public.user_role_assignments ura
          JOIN public.roles r ON r.id = ura.role_id AND r.status = 'active'
            AND r.cooperative_id = p_cooperative_id
          JOIN public.role_permissions rp ON rp.role_id = r.id
          JOIN public.permissions p ON p.id = rp.permission_id
            AND p.key = 'audit.export'
          JOIN public.cooperative_memberships m
            ON m.user_id = ura.user_id AND m.cooperative_id = p_cooperative_id
           AND m.status = 'active'
          JOIN public.cooperatives c ON c.id = m.cooperative_id
           AND c.status = 'active' AND c.business_enabled
         WHERE ura.user_id = v_user
           AND ura.cooperative_id = p_cooperative_id)
    THEN
        RAISE EXCEPTION 'insufficient_privilege: audit.export required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    RETURN QUERY
    SELECT se.id, se.occurred_at, se.event_type, se.user_id,
           u.username, se.cooperative_id, se.metadata
      FROM public.security_events se
      LEFT JOIN public.users u ON u.id = se.user_id
     WHERE se.cooperative_id = p_cooperative_id
       AND (p_event_type IS NULL OR se.event_type = p_event_type)
       AND (p_actor_user_id IS NULL OR se.user_id = p_actor_user_id)
       AND (p_from IS NULL OR se.occurred_at >= p_from)
       AND (p_to IS NULL OR se.occurred_at <= p_to)
       AND (p_before_at IS NULL
            OR (se.occurred_at, se.id) < (p_before_at, p_before_id))
     ORDER BY se.occurred_at DESC, se.id DESC
     LIMIT LEAST(GREATEST(p_limit, 1), 100);
END;
$function$;
REVOKE ALL ON FUNCTION public.app_audit_export_scoped(uuid, text, uuid, timestamptz, timestamptz, timestamptz, uuid, int) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_audit_export_scoped(uuid, text, uuid, timestamptz, timestamptz, timestamptz, uuid, int) TO kooperatif_app;
