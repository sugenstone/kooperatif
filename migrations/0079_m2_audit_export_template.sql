-- M2 PD-15 follow-up: the system-administrator template must mirror the
-- cooperative-scoped catalog (rbac_cooperative_schema invariant). The
-- audit.export permission (0076) was granted to the fixed admin ROLE but
-- not to the template — closing the gap so the template stays complete.
-- Append-only; permission row already exists (ON CONFLICT safe).

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

INSERT INTO public.template_permissions (template_id, permission_id)
SELECT t.id, p.id
FROM public.system_role_templates t
JOIN public.permissions p ON p.key = 'audit.export'
WHERE t.key = 'system-administrator' AND t.status = 'active'
ON CONFLICT DO NOTHING;
