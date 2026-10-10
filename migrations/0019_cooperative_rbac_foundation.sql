-- 0019 — M1-P1-a Cooperative-scoped RBAC schema foundation (P1-K1..P1-K5).
--
-- Expand + backfill stage only. Columns stay NULLABLE in P1-a so every
-- existing INSERT (rbac.rs create_role, grant-role CLI, test fixtures) keeps
-- working identically while cooperative enforcement is still being built.
-- The NOT NULL / fully contracted state is a separate, later migration
-- (P1-c) that lands only after all write paths carry cooperative_id.
--
-- Semantics of cooperative_id:
--   NULL     = legacy row predating tenant ownership. Under P1-K3 all
--              pre-existing business data logically belongs to the
--              bootstrap cooperative; NULL rows are interpreted that way
--              until contracted.
--   non-NULL = fully constrained: composite FKs below make a
--              cross-cooperative role assignment IMPOSSIBLE at the
--              database level (assignment cooperative must equal both the
--              user's membership cooperative and the role's cooperative).
--
-- permissions stays a GLOBAL, application-owned catalog — the permission
-- key set is code-defined, not tenant data. role_permissions stays keyed
-- by role_id only; a role already belongs to exactly one cooperative.
--
-- P0-003 business gate is untouched; no authorization behavior changes.

ALTER TABLE roles
    ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE user_role_assignments
    ADD cooperative_id UUID REFERENCES cooperatives (id);

-- FK target for composite (cooperative_id, id) references. id is already
-- the PK, so uniqueness is inherent; the index exists to anchor the FK.
CREATE UNIQUE INDEX roles_coop_id_uq ON roles (cooperative_id, id);
CREATE INDEX user_role_assignments_coop_idx
    ON user_role_assignments (cooperative_id, user_id);

-- P1-K4 — independent system role templates. Templates are NOT
-- cooperative-owned roles: they cannot be assigned, appear in no role
-- list, and copying one into a cooperative is an explicit provisioning
-- action (P1-d). Updating a template must never mutate existing roles,
-- which is guaranteed structurally because real roles live in `roles`.
CREATE TABLE system_role_templates (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key         TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active', 'disabled')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX system_role_templates_key_uq
    ON system_role_templates (lower(key));

CREATE TABLE system_role_template_permissions (
    template_id   UUID NOT NULL REFERENCES system_role_templates (id),
    permission_id UUID NOT NULL REFERENCES permissions (id),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (template_id, permission_id)
);

-- Seed the administrative template with the full current permission
-- catalog, mirroring what the seeded 'Sistem Yöneticisi' role accumulated
-- across migrations. Future migrations that grant new permissions to the
-- admin role must also extend this template (P1-d consumes it).
INSERT INTO system_role_templates (key, name, description) VALUES (
    'system-administrator',
    'Sistem Yöneticisi',
    'Kooperatif içi tam yönetim şablonu: provisioning sırasında kooperatife kopyalanır; hiçbir kullanıcıya otomatik atanmaz.'
);
INSERT INTO system_role_template_permissions (template_id, permission_id)
SELECT t.id, p.id
FROM system_role_templates t, permissions p
WHERE t.key = 'system-administrator';

-- Backfill + administrator-continuity assertion.
-- Runs only when exactly one bootstrap cooperative exists (the
-- is_bootstrap partial-unique index guarantees at most one). A
-- pre-bootstrap database has no ownership to migrate and is left
-- untouched; the same statements are replayed by the P1-b/c contract
-- step if bootstrap happens later.
DO $$
DECLARE
    v_bootstrap uuid;
    v_admin_before bigint;
    v_admin_after bigint;
    v_skipped bigint;
BEGIN
    SELECT id INTO v_bootstrap FROM cooperatives WHERE is_bootstrap;
    IF v_bootstrap IS NULL THEN
        RETURN;
    END IF;

    -- Administrative paths that work today (global model): active user +
    -- active role carrying roles.manage.
    SELECT count(DISTINCT ura.user_id) INTO v_admin_before
      FROM user_role_assignments ura
      JOIN roles r ON r.id = ura.role_id AND r.status = 'active'
      JOIN role_permissions rp ON rp.role_id = r.id
      JOIN permissions p ON p.id = rp.permission_id AND p.key = 'roles.manage'
      JOIN users u ON u.id = ura.user_id AND u.status = 'active';

    -- Every existing role predates tenancy, so all of them are
    -- bootstrap-owned (P1-K3). Unconditional backfill.
    UPDATE roles SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;

    -- An assignment may only carry a cooperative when the user actually
    -- holds a membership row there (any status — the composite FK checks
    -- existence, authorization checks status). Rows whose user was never
    -- enrolled stay NULL (legacy), never silently re-homed.
    UPDATE user_role_assignments ura
       SET cooperative_id = v_bootstrap
     WHERE ura.cooperative_id IS NULL
       AND EXISTS (SELECT 1 FROM roles r
                    WHERE r.id = ura.role_id AND r.cooperative_id = v_bootstrap)
       AND EXISTS (SELECT 1 FROM cooperative_memberships m
                    WHERE m.user_id = ura.user_id
                      AND m.cooperative_id = v_bootstrap);

    SELECT count(*) INTO v_skipped
      FROM user_role_assignments WHERE cooperative_id IS NULL;
    IF v_skipped > 0 THEN
        RAISE NOTICE 'P1-a: % assignment row(s) left unscoped (user has no bootstrap membership)', v_skipped;
    END IF;

    -- Usable bootstrap administrator paths after migration: scoped
    -- assignment + active membership + active role + roles.manage.
    SELECT count(DISTINCT ura.user_id) INTO v_admin_after
      FROM user_role_assignments ura
      JOIN roles r ON r.id = ura.role_id AND r.status = 'active'
                   AND r.cooperative_id = v_bootstrap
      JOIN role_permissions rp ON rp.role_id = r.id
      JOIN permissions p ON p.id = rp.permission_id AND p.key = 'roles.manage'
      JOIN users u ON u.id = ura.user_id AND u.status = 'active'
      JOIN cooperative_memberships m
        ON m.user_id = ura.user_id AND m.cooperative_id = v_bootstrap
       AND m.status = 'active'
     WHERE ura.cooperative_id = v_bootstrap;

    -- Invariant 3: if a working administrator path existed before, at
    -- least one must remain scoped to bootstrap after backfill. Failing
    -- loudly rolls the whole migration back; recovery = grant the
    -- missing membership via `add-member`, then re-run migrations.
    IF v_admin_before >= 1 AND v_admin_after < 1 THEN
        RAISE EXCEPTION
            'P1-a migration would leave the bootstrap cooperative without an administrator (% -> %). Fix memberships and re-run.',
            v_admin_before, v_admin_after;
    END IF;
END $$;

-- Composite foreign keys: a scoped assignment can only pair a
-- cooperative with a role owned by that same cooperative and a user
-- holding a membership row in that same cooperative. NULL cooperative_id
-- rows skip the check (MATCH SIMPLE) — that is the temporary legacy lane
-- contracted away in P1-c.
ALTER TABLE user_role_assignments
    ADD CONSTRAINT user_role_assignments_coop_user_fk
        FOREIGN KEY (cooperative_id, user_id)
        REFERENCES cooperative_memberships (cooperative_id, user_id),
    ADD CONSTRAINT user_role_assignments_coop_role_fk
        FOREIGN KEY (cooperative_id, role_id)
        REFERENCES roles (cooperative_id, id);
