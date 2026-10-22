-- M1-P6 BATCH B1 — cooperative authorization foundation.
--
-- Two changes, both prerequisites for multi-cooperative administration
-- (B2+). NO APIs ship in this batch.
--
-- 1. Role-name uniqueness rescope. `roles_name_key` enforced a GLOBAL
--    unique lower(name): two cooperatives could never own a role with
--    the same name, which makes per-cooperative template cloning
--    (M1-K3) impossible. Uniqueness is now per cooperative:
--      - `UNIQUE NULLS NOT DISTINCT (cooperative_id, lower(name))`.
--    NULLS NOT DISTINCT is the critical part: a plain multi-column
--    UNIQUE treats NULL cooperative_id values as DISTINCT and would
--    silently allow duplicate PLATFORM (NULL-scope) role names. With
--    NULLS NOT DISTINCT the NULL-scope group is one uniqueness domain
--    — platform role names stay globally unique, cooperative names
--    unique per cooperative, and the same name may coexist across
--    different cooperatives.
--    Existing rows, assignments, templates and history are untouched —
--    index swap only. The swap fails loudly if hidden duplicates
--    already exist (GROUP BY treats NULLs as one group, matching the
--    new index semantics).
--
-- 2. Permission catalog seeds for the future management surfaces:
--      - `cooperatives.manage` — PLATFORM-scope lifecycle authority
--        (create/activate/suspend/archive). Never cooperative-scoped.
--      - `memberships.manage`  — cooperative-scoped membership
--        administration (add/list members of the OWN cooperative).
--    Per the approved B1 boundary neither permission is granted to any
--    existing role or template in this migration: assignments land with
--    the APIs that consume them (B2/B3). Existing `Sistem Yöneticisi`
--    (NULL-scope) and cooperative administrator roles are deliberately
--    left without these keys until the product decision ships.
--
-- Append-only. No historical migration is modified.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ── 0. Fail-loud precondition: no hidden duplicates ────────────────
-- Under the CURRENT global index duplicates are impossible, so this
-- guard can only fire on manually repaired databases — but the rescope
-- must never silently pass over divergent state.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM (
            SELECT cooperative_id, lower(name) AS lname
              FROM public.roles
             GROUP BY cooperative_id, lower(name)
            HAVING count(*) > 1
          ) dup
    ) THEN
        RAISE EXCEPTION 'duplicate (cooperative_id, name) roles exist — resolve before rescope';
    END IF;
END
$$;

-- ── 1. Role-name uniqueness rescope ────────────────────────────────

DROP INDEX public.roles_name_key;

CREATE UNIQUE INDEX roles_coop_name_uq
    ON public.roles (cooperative_id, lower(name)) NULLS NOT DISTINCT;

-- ── 2. Permission catalog seeds ────────────────────────────────────

INSERT INTO public.permissions (key, name, description, category) VALUES
    ('cooperatives.manage',
     'Kooperatifleri Yönet',
     'Kooperatif oluşturabilir, aktive edebilir, askıya alabilir ve arşivleyebilir; yalnızca platform kapsamında (kooperatifsiz) geçerlidir.',
     'Yönetim'),
    ('memberships.manage',
     'Kooperatif Üyeliklerini Yönet',
     'Kendi kooperatifine kullanıcı ekleyebilir ve üyelikleri görüntüleyebilir; kooperatif kapsamında geçerlidir.',
     'Kullanıcı Yönetimi');

-- Deliberately NO grants here: B1 is the catalog foundation only.
-- `cooperatives.manage` must never appear on a cooperative-scoped role
-- or the `system-administrator` template (it is platform authority);
-- `memberships.manage` reaches cooperative roles only when the B3 API
-- lands with its own approved grant decision.

-- ── 3. Postchecks ───────────────────────────────────────────────────

DO $$
BEGIN
    -- The rescoped index exists with exactly the intended shape.
    IF NOT EXISTS (
        SELECT 1
          FROM pg_indexes
         WHERE schemaname = 'public'
           AND indexname = 'roles_coop_name_uq'
           AND indexdef LIKE '%NULLS NOT DISTINCT%'
           AND indexdef LIKE '%cooperative_id%'
           AND indexdef LIKE '%lower(name)%'
    ) THEN
        RAISE EXCEPTION 'roles_coop_name_uq missing or wrong shape';
    END IF;

    -- The old global index is gone (a stale second unique index would
    -- silently keep the global constraint alive).
    IF EXISTS (
        SELECT 1
          FROM pg_indexes
         WHERE schemaname = 'public'
           AND indexname = 'roles_name_key'
    ) THEN
        RAISE EXCEPTION 'roles_name_key still present';
    END IF;

    -- Both permission keys exist.
    IF (SELECT count(*) FROM public.permissions
         WHERE key IN ('cooperatives.manage', 'memberships.manage')) <> 2 THEN
        RAISE EXCEPTION 'permission seeds missing';
    END IF;

    -- No automatic grant happened anywhere: no role and no template
    -- may carry either key after this migration.
    IF EXISTS (
        SELECT 1
          FROM public.role_permissions rp
          JOIN public.permissions p ON p.id = rp.permission_id
         WHERE p.key IN ('cooperatives.manage', 'memberships.manage')
    ) OR EXISTS (
        SELECT 1
          FROM public.system_role_template_permissions tp
          JOIN public.permissions p ON p.id = tp.permission_id
         WHERE p.key IN ('cooperatives.manage', 'memberships.manage')
    ) THEN
        RAISE EXCEPTION 'B1 must not auto-grant the new permissions';
    END IF;
END
$$;
