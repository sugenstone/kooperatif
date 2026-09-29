-- =====================================================================
-- 0008  FINANCIAL ACCOUNTS, ACCOUNT MOVEMENTS & TRANSFERS  (STEP-008)
-- docs/03-FINANCIAL-CORE.md, docs/07-FINANCIAL-ACCOUNTS-INCOME-EXPENSE.md,
-- docs/15-INVARIANTS.md, docs/16-STATE-MACHINES.md,
-- docs/19-AUDIT-REVERSAL-CORRECTION.md, ADR-003, ADR-004, ADR-006.
--
-- Scope decisions (implementation record):
--   * ONE Financial Account abstraction: account_type distinguishes
--     'cash' | 'bank' — a single balance engine (docs/07; the prompt's
--     common-model requirement). No separate cashbox*/bank_* engines.
--   * Balance is DERIVED from account_movements — there is
--     deliberately NO mutable `financial_accounts.balance` column
--     (ADR-003, docs/15 invariants).
--   * Movements are produced ONLY by domain commands (Payment posting,
--     Transfer): source_type + source_id provenance is relational,
--     never free text. No arbitrary POST /account-movements exists.
--   * Movement effect: direction ('inflow'|'outflow') + positive
--     amount — one unambiguous representation (never both sign models).
--   * Reversal is status-based ('active' -> 'reversed') with
--     actor/time/reason — consistent with the STEP-007 correction
--     model; the original row always survives (docs/19).
--   * Negative balances are FORBIDDEN (operator decision): a transfer
--     or reversal whose effect would take an account below zero is
--     rejected under row locks.
--   * Currency is explicit TRY-only at this step; cross-currency,
--     gold/commodity units and exchange semantics are deferred
--     (docs/07 open decisions, docs/20 gold boundary).
--   * payments.destination_account_id is NULLABLE: pre-STEP-008
--     Payments keep NULL — no fabricated destination is invented for
--     historical money. New Payments must supply one.
--   * No Ledger/Income/Expense/Investment/Social Aid/Receipt objects —
--     deferred domains (docs/07; ADR-003 covers movement semantics,
--     not a chart of accounts).
-- =====================================================================

CREATE TABLE financial_accounts (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT        NOT NULL
                    CHECK (char_length(btrim(name)) BETWEEN 1 AND 120),
    -- One engine, two operational shapes (docs/07 §10 minimum).
    account_type    TEXT        NOT NULL
                    CHECK (account_type IN ('cash','bank')),
    currency        TEXT        NOT NULL DEFAULT 'TRY'
                    CHECK (currency = 'TRY'),
    -- active <-> inactive (operator-approved lifecycle): an inactive
    -- account keeps history/balance but accepts no new postings.
    status          TEXT        NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active','inactive')),
    description     TEXT,
    -- Optional bank metadata (only meaningful for type 'bank').
    bank_name       TEXT,
    iban            TEXT
                    CHECK (iban IS NULL OR char_length(btrim(iban)) BETWEEN 5 AND 34),
    created_by      UUID        NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX financial_accounts_status_idx ON financial_accounts (status, account_type);
CREATE INDEX financial_accounts_name_idx ON financial_accounts (name);

-- ---------------------------------------------------------------------
-- Account Movements: the immutable financial history that DERIVES every
-- balance (ADR-003). Created only by domain commands:
--   source_type 'payment'  -> source_id = payments.id
--   source_type 'transfer' -> source_id = account_transfers.id
-- UNIQUE (account_id, source_type, source_id): a source can never post
-- the same leg twice — structural duplicate-posting protection.
-- ---------------------------------------------------------------------
CREATE TABLE account_movements (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL REFERENCES financial_accounts(id),
    direction       TEXT        NOT NULL CHECK (direction IN ('inflow','outflow')),
    amount          NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    source_type     TEXT        NOT NULL CHECK (source_type IN ('payment','transfer')),
    source_id       UUID        NOT NULL,
    occurred_at     TIMESTAMPTZ NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active','reversed')),
    reversed_at     TIMESTAMPTZ,
    reversed_by     UUID        REFERENCES users(id),
    reversal_reason TEXT,
    created_by      UUID        NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT account_movements_source_leg_key
        UNIQUE (account_id, source_type, source_id),
    CONSTRAINT account_movements_reversal_consistent CHECK (
        (status = 'active'
            AND reversed_at IS NULL AND reversed_by IS NULL AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL)
    )
);

CREATE INDEX account_movements_account_idx
    ON account_movements (account_id, occurred_at DESC, id);
CREATE INDEX account_movements_source_idx
    ON account_movements (source_type, source_id);
CREATE INDEX account_movements_active_idx
    ON account_movements (account_id) WHERE status = 'active';

-- ---------------------------------------------------------------------
-- Account Transfers: ONE logical move between two cooperative accounts
-- (docs/07: source -X, destination +X — never two unrelated movements).
-- ---------------------------------------------------------------------
CREATE TABLE account_transfers (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    transfer_number         BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    source_account_id       UUID        NOT NULL REFERENCES financial_accounts(id),
    destination_account_id  UUID        NOT NULL REFERENCES financial_accounts(id),
    amount                  NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency                TEXT        NOT NULL DEFAULT 'TRY'
                            CHECK (currency = 'TRY'),
    occurred_at             TIMESTAMPTZ NOT NULL,
    note                    TEXT,
    status                  TEXT        NOT NULL DEFAULT 'posted'
                            CHECK (status IN ('posted','reversed')),
    -- ADR-006 narrow idempotency, same pattern as payments.
    idempotency_key         TEXT        NOT NULL,
    idempotency_fingerprint TEXT        NOT NULL,
    reversed_at             TIMESTAMPTZ,
    reversed_by             UUID        REFERENCES users(id),
    reversal_reason         TEXT,
    created_by              UUID        NOT NULL REFERENCES users(id),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT account_transfers_distinct_accounts
        CHECK (source_account_id <> destination_account_id),
    CONSTRAINT account_transfers_idempotency_key_key UNIQUE (idempotency_key),
    CONSTRAINT account_transfers_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL)
    )
);

CREATE UNIQUE INDEX account_transfers_transfer_number_key
    ON account_transfers (transfer_number);
CREATE INDEX account_transfers_source_idx ON account_transfers (source_account_id);
CREATE INDEX account_transfers_destination_idx ON account_transfers (destination_account_id);

-- ---------------------------------------------------------------------
-- STEP-008 integration: where did the received money GO?
-- Nullable so pre-STEP-008 Payments stay honest (no fabricated
-- destination). New Payments must supply it (route-level validation).
-- ---------------------------------------------------------------------
ALTER TABLE payments
    ADD COLUMN destination_account_id UUID
        REFERENCES financial_accounts(id);

CREATE INDEX payments_destination_account_idx
    ON payments (destination_account_id)
    WHERE destination_account_id IS NOT NULL;

-- ---------------------------------------------------------------------
-- STEP-008 permission catalog additions.
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('financial_accounts.read',
     'Finansal Hesapları Görüntüle',
     'Finansal hesapları, bakiyeleri, hareket geçmişini ve transferleri görüntüleyebilir.',
     'Finansal Hesap Yönetimi'),
    ('financial_accounts.manage',
     'Finansal Hesapları Yönet',
     'Finansal hesap açabilir/düzenleyebilir/pasifleştirebilir ve hesaplar arası transfer yapabilir.',
     'Finansal Hesap Yönetimi');

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('financial_accounts.read', 'financial_accounts.manage')
ON CONFLICT DO NOTHING;
