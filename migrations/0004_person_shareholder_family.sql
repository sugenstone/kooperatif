-- STEP-004: Shareholder, Guardian & Family identity foundation
-- (docs/04 + STEP-004 amendment: temporal family membership).
--
-- Identity model: a normalized `persons` layer. One real person exists
-- once, may act as a Shareholder (shareholders row) and/or be referenced
-- as Guardian — without duplication. Names are NEVER unique: multiple
-- persons may legitimately share first+last name.
--
-- Family membership is TEMPORAL (shareholder_family_memberships): a
-- shareholder has at most one active membership (ended_at IS NULL) and
-- non-overlapping history, enforced by a PostgreSQL exclusion constraint
-- (btree_gist). No mutable shareholders.family_id exists.

CREATE EXTENSION IF NOT EXISTS btree_gist;

CREATE TABLE persons (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    first_name  TEXT NOT NULL,
    last_name   TEXT NOT NULL,
    -- Application-computed Turkish-aware search fold (see
    -- src/parties/model.rs). Search normalization only: stored display
    -- values always keep the original Turkish characters.
    search_name TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Deliberately NO uniqueness on (first_name, last_name) or search_name:
-- same-name persons are valid and distinct (hard invariant).
CREATE INDEX persons_search_name_idx ON persons (search_name);

CREATE TABLE families (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Business identifier: stable, human-readable, searchable, unique.
    -- Manually assigned (historical registry imports need explicit
    -- numbers); uniqueness is database-enforced, including under
    -- concurrent creation (unique violation surfaces as 409).
    sequence_number BIGINT NOT NULL
                    CONSTRAINT families_sequence_positive CHECK (sequence_number > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX families_sequence_number_key ON families (sequence_number);

CREATE TABLE shareholders (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- One Person -> at most one Shareholder record (STEP-004 §46).
    person_id         UUID NOT NULL UNIQUE REFERENCES persons (id),
    -- Guardian is an optional person reference: may be a Shareholder or
    -- not; never auto-created as a Shareholder; NULL = "Vasi bilgisi
    -- yok" (no fake placeholder persons).
    guardian_person_id UUID REFERENCES persons (id),
    -- active | inactive | voided (voided = mistaken unused record;
    --   destructive deletion is not performed). No financial semantics.
    status            TEXT NOT NULL DEFAULT 'active'
                      CONSTRAINT shareholders_status_check
                      CHECK (status IN ('active', 'inactive', 'voided')),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX shareholders_guardian_idx ON shareholders (guardian_person_id);

-- Temporal family membership (STEP-004 amendment).
-- Current membership = ended_at IS NULL. History is retained forever;
-- family changes close the old interval and open a new one in ONE
-- transaction. The exclusion constraint makes overlapping memberships
-- (including two active ones) impossible at the database level, under
-- any concurrency.
CREATE TABLE shareholder_family_memberships (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    shareholder_id  UUID NOT NULL REFERENCES shareholders (id),
    family_id       UUID NOT NULL REFERENCES families (id),
    started_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at        TIMESTAMPTZ,
    reason          TEXT,
    created_by      UUID REFERENCES users (id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT memberships_interval_check
      CHECK (ended_at IS NULL OR ended_at >= started_at)
);

ALTER TABLE shareholder_family_memberships
    ADD CONSTRAINT shareholder_family_memberships_no_overlap
    EXCLUDE USING gist (
        shareholder_id WITH =,
        tstzrange(started_at, COALESCE(ended_at, 'infinity'::timestamptz)) WITH &&
    );

CREATE INDEX shareholder_family_memberships_family_current_idx
    ON shareholder_family_memberships (family_id)
    WHERE ended_at IS NULL;

-- STEP-004 permission catalog additions (application-owned keys).
INSERT INTO permissions (key, name, description, category) VALUES
    ('shareholders.read',
     'Hissedarları Görüntüle',
     'Hissedar, vasi ve aile kimlik kayıtlarını görüntüleyebilir.',
     'Hissedar Yönetimi'),
    ('shareholders.manage',
     'Hissedarları Yönet',
     'Hissedar, vasi ve aile kayıtları oluşturabilir ve değiştirebilir.',
     'Hissedar Yönetimi'),
    ('families.read',
     'Aileleri Görüntüle',
     'Aile listesini ve üyelerini görüntüleyebilir.',
     'Aile Yönetimi'),
    ('families.manage',
     'Aileleri Yönet',
     'Aile oluşturabilir ve hissedarların aile üyeliğini değiştirebilir.',
     'Aile Yönetimi');

-- STEP-003 established: no wildcards — new permissions are NOT
-- inherited automatically. This migration-time, deterministic seed
-- grants the new STEP-004 permissions explicitly to the system-seeded
-- administrative role (fixed id; custom roles are untouched).
INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('shareholders.read', 'shareholders.manage',
                'families.read', 'families.manage')
ON CONFLICT DO NOTHING;

-- No Share/Hisse, Period, Payment, Ledger, balance or Social Aid
-- objects: future STEPs own them.
