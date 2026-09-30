-- =====================================================================
-- 0009  SHAREHOLDER CREDIT (EXCESS PAYMENT) & AUTOMATIC OFFSET
-- docs/00 §"Excess payments", docs/02 §"Excess", docs/03-FINANCIAL-CORE,
-- docs/05 §"Automatic use of existing excess", docs/06 §"Existing/New
-- excess", docs/15, docs/19, ADR-003, ADR-006.
--
-- Scope decisions (implementation record):
--   * Canonical domain term: SHAREHOLDER CREDIT = an Excess Payment
--     Balance attributable to exactly ONE Shareholder (glossary:
--     "Excess Payment Balance"; UI language: "Fazla Ödeme").
--   * A Credit is a CLAIM against ALREADY RECEIVED money: it is backed
--     by a posted Payment's unassigned remainder and creates NO
--     additional Account Movement (the cash entered via the Payment —
--     anti-double-counting, docs/03 reconciliation invariant).
--   * credit_applications settle Assessments from held credit; they
--     are NOT Payments and create NO Account Movement.
--   * NO stored authoritative balance anywhere:
--       available credit   = SUM(active credits) - SUM(active apps)
--       assessment settled = SUM(active payment_allocations)
--                          + SUM(active credit_applications)
--   * Beneficiary is ALWAYS an explicit Shareholder — never inferred
--     from payer, guardian or Family (docs/06 §"New excess",
--     docs/17 family overpayment UNRESOLVED → explicit attribution).
--   * Reversal is status-based ('active' -> 'reversed') with
--     actor/time/reason — originals survive (docs/19).
--   * Auto-offset trigger: assessment generation consumes the
--     beneficiary's available credit FIFO (credit_number ASC);
--     a newly assigned credit offsets the beneficiary's EXISTING
--     open Assessments oldest-due-first (operator-approved policy).
--   * NO refund/disbursement, NO credit transfer between
--     shareholders, NO interest/indexation, NO expiry — deferred.
-- =====================================================================

-- ---------------------------------------------------------------------
-- Credit origins: one row per explicitly assigned Payment remainder.
-- UNIQUE source leg protection is not needed per-payment (a Payment
-- remainder may fund credits for SEVERAL beneficiaries — the Payment
-- row lock guarantees SUM(credits) <= unassigned remainder).
-- ---------------------------------------------------------------------
CREATE TABLE shareholder_credits (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Identity column: stable FIFO order + human-readable reference.
    credit_number       BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    source_payment_id   UUID        NOT NULL REFERENCES payments(id),
    -- Beneficiary: exactly one Shareholder. Never a Family, never
    -- implicitly the payer.
    shareholder_id      UUID        NOT NULL REFERENCES shareholders(id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    status              TEXT        NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active','reversed')),
    note                TEXT,
    -- ADR-006 durable idempotency, same pattern as payments/transfers.
    idempotency_key         TEXT    NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    reversed_at         TIMESTAMPTZ,
    reversed_by         UUID        REFERENCES users(id),
    reversal_reason     TEXT,
    created_by          UUID        NOT NULL REFERENCES users(id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT shareholder_credits_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT shareholder_credits_reversal_consistent CHECK (
        (status = 'active'
            AND reversed_at IS NULL AND reversed_by IS NULL AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL)
    )
);

CREATE UNIQUE INDEX shareholder_credits_credit_number_key
    ON shareholder_credits (credit_number);
CREATE INDEX shareholder_credits_shareholder_idx
    ON shareholder_credits (shareholder_id, credit_number);
CREATE INDEX shareholder_credits_payment_idx
    ON shareholder_credits (source_payment_id);

-- ---------------------------------------------------------------------
-- Credit applications: which debt consumed which entitlement.
-- mode: 'automatic' (generation/assignment engine) | 'manual'
-- (operator apply-credit command). Provenance is relational —
-- credit -> Payment source, assessment -> obligation target.
-- ---------------------------------------------------------------------
CREATE TABLE credit_applications (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    credit_id       UUID        NOT NULL REFERENCES shareholder_credits(id),
    assessment_id   UUID        NOT NULL REFERENCES assessments(id),
    amount          NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency        TEXT        NOT NULL DEFAULT 'TRY'
                    CHECK (currency = 'TRY'),
    mode            TEXT        NOT NULL
                    CHECK (mode IN ('automatic','manual')),
    status          TEXT        NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active','reversed')),
    -- Manual commands carry a client idempotency key + payload
    -- fingerprint; a repeated command with the same key replays only
    -- when the payload is identical (ADR-006).
    idempotency_key         TEXT,
    idempotency_fingerprint TEXT,
    reversed_at     TIMESTAMPTZ,
    reversed_by     UUID        REFERENCES users(id),
    reversal_reason TEXT,
    created_by      UUID        NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT credit_applications_reversal_consistent CHECK (
        (status = 'active'
            AND reversed_at IS NULL AND reversed_by IS NULL AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL)
    )
);

CREATE UNIQUE INDEX credit_applications_idempotency_key_key
    ON credit_applications (idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE INDEX credit_applications_credit_idx
    ON credit_applications (credit_id) WHERE status = 'active';
CREATE INDEX credit_applications_assessment_idx
    ON credit_applications (assessment_id) WHERE status = 'active';

-- ---------------------------------------------------------------------
-- STEP-009 permission catalog additions.
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('credits.read',
     'Hissedar Avansını Görüntüle',
     'Hissedar fazla ödeme/avans kayıtlarını, mahsupları ve türetilmiş kalan bakiyeyi görüntüleyebilir.',
     'Hissedar Avansı'),
    ('credits.manage',
     'Hissedar Avansını Yönet',
     'Tahsilat kalanını hissedar avansı olarak atayabilir, avans mahsubu yapabilir ve ters kaydedebilir.',
     'Hissedar Avansı');

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('credits.read', 'credits.manage')
ON CONFLICT DO NOTHING;
