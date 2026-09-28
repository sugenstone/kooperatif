-- =====================================================================
-- 0007  PAYMENTS & PAYMENT ALLOCATIONS  (STEP-007)
-- docs/06-PAYMENTS-ALLOCATIONS-COLLECTION-SESSIONS.md,
-- docs/15-INVARIANTS.md, docs/16-STATE-MACHINES.md,
-- docs/19-AUDIT-REVERSAL-CORRECTION.md, ADR-004, ADR-006.
--
-- Scope decisions (implementation record):
--   * POSTED-only lifecycle: `posted -> reversed`. A payment row is
--     created together with its allocations in ONE atomic command —
--     there is no Draft/Pending table because no approval policy is
--     approved yet (docs/16 leaves approval to policy).
--   * Money: NUMERIC(19,2), TRY only (ADR-004, matches assessments).
--   * Payer is a Person reference — never implicitly the debtor.
--   * Unallocated remainder is allowed: the operator decision
--     "unallocated balance" (docs/06 disposition choice). It stays ON
--     the Payment (allocated + unallocated = amount, CHECK-free but
--     enforced by command invariants), never attributed to a member.
--   * No Cashbox / Bank / Ledger / Receipt / Session tables —
--     destination accounts, receipt identity, ledger entries and
--     collection sessions are deferred domains (docs/03 §18–§20,
--     docs/06 §88–§92). The Payment records money RECEIVED; where it
--     physically entered is a deferred movement domain.
-- =====================================================================

CREATE TABLE payments (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Business number: gapless-enough identity sequence (identity
    -- column) — stable, human-citable "Tahsilat No".
    payment_number      BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    -- Payer: the Person who handed over the value. Distinct from every
    -- debtor: no FK to shareholders — a shareholder pays via their
    -- PERSON identity and third parties pay identically (§12–§14).
    payer_person_id     UUID        NOT NULL REFERENCES persons(id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    -- Recording method (how the value was handed over). This does NOT
    -- imply a destination financial account (§10).
    method              TEXT        NOT NULL
                        CHECK (method IN ('cash','bank_transfer','card','other')),
    received_at         TIMESTAMPTZ NOT NULL,
    note                TEXT,
    status              TEXT        NOT NULL DEFAULT 'posted'
                        CHECK (status IN ('posted','reversed')),
    -- ADR-006: durable idempotency — one receipt = one Payment row even
    -- under retries. Replays with the same fingerprint return the
    -- existing row; a reused key with a different payload conflicts.
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    -- Reversal bookkeeping (docs/19: reversal keeps the original row,
    -- marks it reversed, records actor/time/reason). Terminal.
    reversed_at         TIMESTAMPTZ,
    reversed_by         UUID        REFERENCES users(id),
    reversal_reason     TEXT,
    created_by          UUID        NOT NULL REFERENCES users(id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT payments_idempotency_key_key UNIQUE (idempotency_key),
    CONSTRAINT payments_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL)
    )
);

CREATE UNIQUE INDEX payments_payment_number_key ON payments (payment_number);
CREATE INDEX payments_payer_idx ON payments (payer_person_id);
CREATE INDEX payments_received_at_idx ON payments (received_at DESC, id);
CREATE INDEX payments_created_at_idx ON payments (created_at DESC, id);

-- ---------------------------------------------------------------------
-- Payment Allocations: application of received value to obligations.
-- One Payment -> many Assessments; one Assessment <- many Payments.
-- Reversal is status-based (never DELETE); the reversed row remains as
-- history linked to its reversal metadata.
-- ---------------------------------------------------------------------
CREATE TABLE payment_allocations (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    payment_id          UUID        NOT NULL REFERENCES payments(id),
    assessment_id       UUID        NOT NULL REFERENCES assessments(id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    status              TEXT        NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active','reversed')),
    reversed_at         TIMESTAMPTZ,
    reversed_by         UUID        REFERENCES users(id),
    reversal_reason     TEXT,
    created_by          UUID        NOT NULL REFERENCES users(id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- At most one allocation line per (payment, assessment): a second
    -- contribution to the same obligation is a distinct line ONLY via a
    -- different Payment — keeps "which receipt covered what" unambiguous.
    CONSTRAINT payment_allocations_payment_assessment_key
        UNIQUE (payment_id, assessment_id),
    CONSTRAINT payment_allocations_reversal_consistent CHECK (
        (status = 'active'
            AND reversed_at IS NULL AND reversed_by IS NULL AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL)
    )
);

CREATE INDEX payment_allocations_assessment_idx
    ON payment_allocations (assessment_id) WHERE status = 'active';
CREATE INDEX payment_allocations_payment_idx
    ON payment_allocations (payment_id);

-- ---------------------------------------------------------------------
-- STEP-007 permission catalog additions (application-owned keys).
-- Two keys: read surface vs collection/reversal commands. No wildcard
-- inheritance; custom roles untouched — admins grant explicitly.
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('payments.read',
     'Tahsilatları Görüntüle',
     'Tahsilat kayıtlarını, dağılımları ve tahsilat özetlerini görüntüleyebilir.',
     'Tahsilat Yönetimi'),
    ('payments.manage',
     'Tahsilatları Yönet',
     'Tahsilat kaydedebilir, dağılım ekleyebilir ve ters kayıt (reversal) yapabilir.',
     'Tahsilat Yönetimi');

-- Explicit migration-time grant to the system-seeded administrative
-- role (fixed id).
INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('payments.read', 'payments.manage')
ON CONFLICT DO NOTHING;
