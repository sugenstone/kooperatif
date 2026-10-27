-- M2 PD-15 — definer-owned mint for the CSV export event.
-- app_audit_emit is EXECUTE-restricted from kooperatif_app by design;
-- the export endpoint mints 'audit_exported' through this thin
-- definer, which re-verifies audit.export inside the sealed
-- cooperative scope before emitting. The runtime lane cannot call it
-- without the grant.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

CREATE OR REPLACE FUNCTION public.app_audit_export_mint(
    p_cooperative_id uuid,
    p_metadata       jsonb
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $function$
DECLARE
    v_user uuid := nullif(current_setting('app.user_id', true), '')::uuid;
BEGIN
    IF v_user IS NULL OR p_cooperative_id IS NULL
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
    PERFORM public.app_audit_emit('audit_exported', coalesce(p_metadata, '{}'::jsonb));
END;
$function$;
REVOKE ALL ON FUNCTION public.app_audit_export_mint(uuid, jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_audit_export_mint(uuid, jsonb)
    TO kooperatif_app;
