-- STEP-003: RBAC & authorization foundation.
--
-- Model (docs/02-DOMAIN-MODEL.md Identity & Access; docs/18):
--   User ──< user_role_assignments >── Role ──< role_permissions >── Permission
-- Effective permissions = union over the user's ACTIVE roles; absence of
-- a permission means denial (default DENY, no wildcards, no explicit
-- deny rules).
--
-- Permission keys are an APPLICATION-OWNED security contract: the
-- catalog below is provisioned exclusively through migrations (never
-- through a runtime API), so administrators can never invent arbitrary
-- keys, and unknown database keys can never become trusted.
--
-- No Scope / ApprovalRule tables yet: contextual policies and approval
-- workflows are deliberately reserved for their own STEPs (STEP-003
-- §31/§32). No business-domain tables.

CREATE TABLE permissions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key         TEXT NOT NULL,
    name        TEXT NOT NULL,          -- Turkish display name
    description TEXT NOT NULL DEFAULT '',
    category    TEXT NOT NULL DEFAULT '', -- grouping for admin UI
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX permissions_key_key ON permissions (key);

CREATE TABLE roles (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'active'
                CONSTRAINT roles_status_check
                CHECK (status IN ('active', 'disabled')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Unique under a documented normalization: case-insensitive (database
-- lower()). Renaming a role never changes authorization behavior —
-- authorization derives from permission assignments, never the name.
CREATE UNIQUE INDEX roles_name_key ON roles (lower(name));

CREATE TABLE role_permissions (
    role_id       UUID NOT NULL REFERENCES roles (id),
    permission_id UUID NOT NULL REFERENCES permissions (id),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (role_id, permission_id)
);

CREATE INDEX role_permissions_permission_idx ON role_permissions (permission_id);

CREATE TABLE user_role_assignments (
    user_id    UUID NOT NULL REFERENCES users (id),
    role_id    UUID NOT NULL REFERENCES roles (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, role_id)
);

CREATE INDEX user_role_assignments_role_idx ON user_role_assignments (role_id);

-- Permission catalog: the exact minimal set required by STEP-003
-- functionality (§6). Deterministic, migration-controlled seeds.
INSERT INTO permissions (key, name, description, category) VALUES
    ('users.read',
     'Kullanıcıları Görüntüle',
     'Kullanıcı listesini ve rol atamalarını görebilir.',
     'Kullanıcı Yönetimi'),
    ('users.manage',
     'Kullanıcı Rolleri Yönet',
     'Kullanıcılara rol atayabilir ve kaldırabilir.',
     'Kullanıcı Yönetimi'),
    ('roles.read',
     'Rolleri ve Yetkileri Görüntüle',
     'Rol listesini ve yetki katalogunu görebilir.',
     'Rol ve Yetki Yönetimi'),
    ('roles.manage',
     'Rolleri Yönet',
     'Rol oluşturabilir, adlandırabilir, etkinleştirip devre dışı bırakabilir ve rol yetkilerini değiştirebilir.',
     'Rol ve Yetki Yönetimi');

-- System-seeded administrative role. It is NOT special-cased anywhere in
-- code: its power is exactly its explicitly assigned permission set
-- (§15/§16 — no wildcard). The bootstrap operator receives it through
-- the explicit `grant-role` CLI command, never automatically.
INSERT INTO roles (id, name, description) VALUES (
    '00000000-0000-4000-8000-000000000001',
    'Sistem Yöneticisi',
    'Uygulama yönetimi: kullanıcı ve rol yetkilendirmesini yönetir. Adının değiştirilmesi davranışını değiştirmez; yetkileri açıkça atanmış izinlerle tanımlıdır.'
);

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('users.read', 'users.manage', 'roles.read', 'roles.manage');

-- Deliberately NOT seeded: any wildcard/superuser permission, financial
-- domain permissions (payments.*, ledger.*, shares.*, ...) and any
-- production restore permission (ADR-013 keeps restore as an
-- infrastructure/operator privilege outside application RBAC).
