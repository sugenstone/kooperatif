-- =====================================================================
-- 0011  SHARE RETURN & DEFERRED ENTITLEMENT  (STEP-011)
-- docs/10-SHARE-RETURN-PROFIT-RIGHTS-VALUE-PROTECTION.md,
-- docs/04 (Share return), docs/15-INVARIANTS.md, docs/16-STATE-
-- MACHINES.md, docs/19-AUDIT-REVERSAL-CORRECTION.md, ADR-003/004/006.
--
-- Scope decisions (implementation record):
--   * THREE distinct layers stay separate (docs/10 core principle):
--       SHARE LIFECYCLE:   active -> return_pending -> closed
--       ENTITLEMENT:       share_return_entitlements — what the
--                          cooperative owes (principal / profit)
--       CASH SETTLEMENT:   share_return_settlements — real money
--                          leaving a Financial Account via exactly ONE
--                          Account Movement each
--     No column ever collapses these into "paid = true/false".
--   * Amounts are NEVER calculated by the system: the Refundable
--     Invested Amount and Profit Right formulas are UNRESOLVED /
--     POLICY_DRIVEN (docs/10, docs/17). The operator enters the
--     cooperative-approved amount; the row snapshots amount, due date,
--     policy_reference and recognized_at so a later policy change can
--     never silently move a crystallized right.
--   * amount NULL = "right exists, not yet determined" — a first-class
--     state, never rendered as 0.00. due_date NULL = undetermined
--     timing. A settlement requires a determined positive amount.
--   * Settlement is NOT Expense / Payment / Transfer and creates NO
--     Shareholder Credit: it posts exactly one outflow Account
--     Movement with source_type 'share_return_settlement'. Recognizing
--     an entitlement moves ZERO money.
--   * Lifecycle: share_returns 'pending' -> 'finalized' | 'cancelled';
--     entitlements 'open' -> 'partially_settled' -> 'settled' |
--     'cancelled'; settlements 'posted' -> 'reversed'. No DELETE path
--     exists for any of them (docs/19).
-- =====================================================================

-- ---------------------------------------------------------------------
-- Share lifecycle extension: return_pending and closed join the
-- candidate matrix (docs/16: Active -> Return Pending -> Closed).
-- Voided stays terminal; closed is terminal — no resurrection path.
-- ---------------------------------------------------------------------
ALTER TABLE shares
    DROP CONSTRAINT shares_status_check;
ALTER TABLE shares
    ADD CONSTRAINT shares_status_check
    CHECK (status IN ('active','suspended','voided','return_pending','closed'));

-- Share return milestones are part of the share's business history.
ALTER TABLE share_events
    DROP CONSTRAINT share_events_type_check;
ALTER TABLE share_events
    ADD CONSTRAINT share_events_type_check CHECK (
        event_type IN (
            'initial_acquisition',
            'transfer',
            'sale',
            'status_change',
            'voided',
            'return_requested',
            'return_cancelled',
            'return_finalized'));

-- Movement provenance: a settlement movement must answer "why did
-- money leave this account?" — Share Return Settlement #N.
ALTER TABLE account_movements
    DROP CONSTRAINT account_movements_source_type_check;
ALTER TABLE account_movements
    ADD CONSTRAINT account_movements_source_type_check
    CHECK (source_type IN
        ('payment','transfer','income','expense','share_return_settlement'));

-- ---------------------------------------------------------------------
-- Share return case: the controlled workflow object. Identity
-- SNAPSHOTS (share_number, owner identity label, ownership start) keep
-- the record self-explanatory even after later name/family changes
-- (docs/10 §statements).
-- ---------------------------------------------------------------------
CREATE TABLE share_returns (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    return_number       BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    share_id            UUID        NOT NULL REFERENCES shares (id),
    -- Owner AT INITIATION + immutable display snapshot. The FK keeps
    -- relational resolution; the label keeps history unambiguous.
    shareholder_id      UUID        NOT NULL REFERENCES shareholders (id),
    owner_display_name  TEXT        NOT NULL,
    share_number        BIGINT      NOT NULL,
    ownership_started_at TIMESTAMPTZ NOT NULL,
    requested_at        TIMESTAMPTZ NOT NULL,
    -- Authoritative economic cutoff (docs/10): the business DATE on
    -- which ownership/economic participation ends. Ownership closes at
    -- effective_return_date 00:00 Europe/Istanbul — the share
    -- participates in assessments with effective dates BEFORE it and
    -- never at/after it.
    effective_return_date DATE      NOT NULL,
    reason              TEXT,
    status              TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending','finalized','cancelled')),
    finalized_at        TIMESTAMPTZ,
    finalized_by        UUID        REFERENCES users (id),
    cancelled_at        TIMESTAMPTZ,
    cancelled_by        UUID        REFERENCES users (id),
    cancellation_reason TEXT,
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT share_returns_idempotency_key_key UNIQUE (idempotency_key),
    CONSTRAINT share_returns_finalized_consistent CHECK (
        (status = 'finalized'
            AND finalized_at IS NOT NULL AND finalized_by IS NOT NULL)
        OR
        (status <> 'finalized'
            AND finalized_at IS NULL AND finalized_by IS NULL)),
    CONSTRAINT share_returns_cancelled_consistent CHECK (
        (status = 'cancelled'
            AND cancelled_at IS NOT NULL AND cancelled_by IS NOT NULL
            AND cancellation_reason IS NOT NULL)
        OR
        (status <> 'cancelled'
            AND cancelled_at IS NULL AND cancelled_by IS NULL
            AND cancellation_reason IS NULL))
);

CREATE UNIQUE INDEX share_returns_return_number_key
    ON share_returns (return_number);
-- A Share may carry at most ONE live return process at a time.
CREATE UNIQUE INDEX share_returns_one_pending_per_share
    ON share_returns (share_id) WHERE status = 'pending';
CREATE INDEX share_returns_share_idx ON share_returns (share_id);
CREATE INDEX share_returns_shareholder_idx
    ON share_returns (shareholder_id);
CREATE INDEX share_returns_status_idx
    ON share_returns (status, requested_at DESC);

-- ---------------------------------------------------------------------
-- Entitlements: the cooperative's obligations arising from a return.
-- One row per right per return (partial UNIQUE — a cancelled row may
-- be re-recognized). amount NULL is "right exists, quantification
-- pending"; 0.00 is a legitimately determined zero — NULL <> 0.
-- ---------------------------------------------------------------------
CREATE TABLE share_return_entitlements (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    entitlement_number  BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    share_return_id     UUID        NOT NULL REFERENCES share_returns (id),
    entitlement_type    TEXT        NOT NULL
                        CHECK (entitlement_type IN ('principal','profit')),
    -- Historically stable beneficiary: the shareholder recorded on the
    -- return case. Whether they are still an ACTIVE shareholder is
    -- irrelevant to being owed money (docs/10 §statements).
    beneficiary_shareholder_id UUID NOT NULL REFERENCES shareholders (id),
    amount              NUMERIC(19,2)
                        CHECK (amount IS NULL OR amount >= 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    due_date            DATE,
    -- Free-text evidence of the governing rule/decision under which
    -- this right was quantified (policy name, board decision, date).
    policy_reference    TEXT,
    description         TEXT,
    recognized_at       TIMESTAMPTZ NOT NULL,
    -- When the amount was fixed (NULL while undetermined).
    determined_at       TIMESTAMPTZ,
    status              TEXT        NOT NULL DEFAULT 'open'
                        CHECK (status IN
                            ('open','partially_settled','settled','cancelled')),
    cancelled_at        TIMESTAMPTZ,
    cancelled_by        UUID        REFERENCES users (id),
    cancellation_reason TEXT,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT share_return_entitlements_cancelled_consistent CHECK (
        (status = 'cancelled'
            AND cancelled_at IS NOT NULL AND cancelled_by IS NOT NULL
            AND cancellation_reason IS NOT NULL)
        OR
        (status <> 'cancelled'
            AND cancelled_at IS NULL AND cancelled_by IS NULL
            AND cancellation_reason IS NULL)),
    CONSTRAINT share_return_entitlements_determined_consistent CHECK (
        (amount IS NOT NULL AND determined_at IS NOT NULL)
        OR (amount IS NULL AND determined_at IS NULL))
);

CREATE UNIQUE INDEX share_return_entitlements_number_key
    ON share_return_entitlements (entitlement_number);
CREATE UNIQUE INDEX share_return_entitlements_one_per_right
    ON share_return_entitlements (share_return_id, entitlement_type)
    WHERE status <> 'cancelled';
CREATE INDEX share_return_entitlements_beneficiary_idx
    ON share_return_entitlements (beneficiary_shareholder_id);
CREATE INDEX share_return_entitlements_due_idx
    ON share_return_entitlements (status, due_date);

-- ---------------------------------------------------------------------
-- Settlements: real money leaving a Financial Account. Each posted
-- settlement binds 1:1 to exactly ONE outflow Account Movement —
-- the movement stays the authoritative money record (ADR-003).
-- ---------------------------------------------------------------------
CREATE TABLE share_return_settlements (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    settlement_number   BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    entitlement_id      UUID        NOT NULL
                        REFERENCES share_return_entitlements (id),
    financial_account_id UUID       NOT NULL REFERENCES financial_accounts (id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    settled_at          TIMESTAMPTZ NOT NULL,
    account_movement_id UUID        NOT NULL REFERENCES account_movements (id),
    status              TEXT        NOT NULL DEFAULT 'posted'
                        CHECK (status IN ('posted','reversed')),
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    reversed_at         TIMESTAMPTZ,
    reversed_by         UUID        REFERENCES users (id),
    reversal_reason     TEXT,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT share_return_settlements_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT share_return_settlements_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL
            AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL))
);

CREATE UNIQUE INDEX share_return_settlements_number_key
    ON share_return_settlements (settlement_number);
CREATE UNIQUE INDEX share_return_settlements_movement_key
    ON share_return_settlements (account_movement_id);
CREATE INDEX share_return_settlements_entitlement_idx
    ON share_return_settlements (entitlement_id);
CREATE INDEX share_return_settlements_account_idx
    ON share_return_settlements (financial_account_id, settled_at DESC, id);

-- ---------------------------------------------------------------------
-- STEP-011 permission catalog additions.
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('share_returns.read',
     'Hisse İadelerini Görüntüle',
     'Hisse iade taleplerini, hak edişleri ve iade ödemelerini görüntüleyebilir.',
     'Hisse Yönetimi'),
    ('share_returns.manage',
     'Hisse İadelerini Yönet',
     'Hisse iade süreci başlatabilir, kesinleştirebilir/iptal edebilir, hak ediş tanıyabilir ve iade ödemesi yapabilir/tersine çevirebilir.',
     'Hisse Yönetimi');

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('share_returns.read', 'share_returns.manage')
ON CONFLICT DO NOTHING;
