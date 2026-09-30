-- =====================================================================
-- 0010  INCOME & EXPENSE MANAGEMENT  (STEP-010)
-- docs/07-FINANCIAL-ACCOUNTS-INCOME-EXPENSE.md, docs/15-INVARIANTS.md,
-- docs/16-STATE-MACHINES.md, docs/19-AUDIT-REVERSAL-CORRECTION.md,
-- ADR-003, ADR-004, ADR-006.
--
-- Scope decisions (implementation record):
--   * Income / Expense are BUSINESS EVENTS posted atomically with
--     exactly ONE Account Movement each (approved STEP-010 scope).
--     The movement stays the authoritative money record (ADR-003);
--     the entry only explains WHY it exists. No income/expense
--     balance columns — nothing becomes a second source of truth.
--   * ONE financial_categories table, typed by category_type
--     ('income' | 'expense'). A composite FK
--     (category_id, entry_kind) -> categories(id, category_type)
--     makes a wrong-typed category IMPOSSIBLE at the database level,
--     not just at application validation.
--   * entry_kind ('income'/'expense') is a constant column on each
--     entry table whose only purpose is that composite FK.
--   * Status lifecycle: 'posted' -> 'reversed' — terminal, original
--     row survives (docs/19). No hard delete exists.
--   * account_movements.source_type gains 'income' | 'expense' —
--     movement provenance stays relational, never free text. The
--     entry ALSO stores account_movement_id for a hard 1:1 link.
--   * counterparty is descriptive text only (income source / expense
--     payee). It deliberately does NOT create Person/Shareholder
--     semantics (docs/01, STEP-010 scope).
--   * Categories are seeded with a minimal operational set and are
--     manageable (create/rename/activate/deactivate); they are NEVER
--     hard-deleted — history keeps a stable relation, so a rename
--     stays coherent and deactivation only blocks NEW postings.
--   * Negative balances stay forbidden (STEP-008 operator decision):
--     an Expense posts only when the locked account can fund it, and
--     an Income reversal that would take the account below zero is
--     rejected under the same row locks.
--   * Payments, Transfers and Shareholder Credits are NOT Income or
--     Expense — totals count only income_entries / expense_entries,
--     so double counting is impossible by construction.
-- =====================================================================

-- ---------------------------------------------------------------------
-- Categories: typed, manageable reference data. Historical entries
-- reference the row directly; deactivation blocks NEW use only.
-- UNIQUE (id, category_type) exists solely to support the composite
-- entry FK that hard-binds category type to entry kind.
-- ---------------------------------------------------------------------
CREATE TABLE financial_categories (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    category_type   TEXT        NOT NULL
                    CHECK (category_type IN ('income','expense')),
    name            TEXT        NOT NULL
                    CHECK (char_length(btrim(name)) BETWEEN 1 AND 120),
    description     TEXT,
    status          TEXT        NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active','inactive')),
    -- NULL on system-seeded rows: no bootstrap user exists at
    -- migration time, and seed provenance is the migration itself.
    created_by      UUID        REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT financial_categories_type_key UNIQUE (id, category_type)
);

CREATE UNIQUE INDEX financial_categories_name_key
    ON financial_categories (category_type, lower(btrim(name)));
CREATE INDEX financial_categories_type_status_idx
    ON financial_categories (category_type, status);

-- Initial operational category set (STEP-010 approved examples). All
-- are editable/deactivatable; nothing here is load-bearing code.
INSERT INTO financial_categories (category_type, name) VALUES
    ('income',  'Diğer Gelir'),
    ('income',  'Kira Geliri'),
    ('income',  'Yatırım Geliri'),
    ('expense', 'Genel Gider'),
    ('expense', 'Kira'),
    ('expense', 'Ulaşım'),
    ('expense', 'Bakım / Onarım'),
    ('expense', 'Elektrik / Su / İletişim'),
    ('expense', 'Organizasyon'),
    ('expense', 'Diğer Gider');

-- ---------------------------------------------------------------------
-- Income entries: a posted Income = ONE inflow Account Movement.
-- ---------------------------------------------------------------------
CREATE TABLE income_entries (
    id                  UUID        PRIMARY KEY,
    income_number       BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    financial_account_id UUID       NOT NULL REFERENCES financial_accounts(id),
    category_id         UUID        NOT NULL,
    entry_kind          TEXT        NOT NULL DEFAULT 'income'
                        CHECK (entry_kind = 'income'),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    occurred_at         TIMESTAMPTZ NOT NULL,
    description         TEXT        NOT NULL
                        CHECK (char_length(btrim(description)) BETWEEN 1 AND 500),
    counterparty        TEXT,
    reference_no        TEXT,
    account_movement_id UUID        NOT NULL REFERENCES account_movements(id),
    status              TEXT        NOT NULL DEFAULT 'posted'
                        CHECK (status IN ('posted','reversed')),
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    reversed_at         TIMESTAMPTZ,
    reversed_by         UUID        REFERENCES users(id),
    reversal_reason     TEXT,
    created_by          UUID        NOT NULL REFERENCES users(id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT income_entries_category_kind_fkey
        FOREIGN KEY (category_id, entry_kind)
        REFERENCES financial_categories (id, category_type),
    CONSTRAINT income_entries_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT income_entries_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL)
    )
);

CREATE UNIQUE INDEX income_entries_income_number_key
    ON income_entries (income_number);
CREATE UNIQUE INDEX income_entries_movement_key
    ON income_entries (account_movement_id);
CREATE INDEX income_entries_account_idx
    ON income_entries (financial_account_id, occurred_at DESC, id);
CREATE INDEX income_entries_category_idx
    ON income_entries (category_id);

-- ---------------------------------------------------------------------
-- Expense entries: a posted Expense = ONE outflow Account Movement.
-- ---------------------------------------------------------------------
CREATE TABLE expense_entries (
    id                  UUID        PRIMARY KEY,
    expense_number      BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    financial_account_id UUID       NOT NULL REFERENCES financial_accounts(id),
    category_id         UUID        NOT NULL,
    entry_kind          TEXT        NOT NULL DEFAULT 'expense'
                        CHECK (entry_kind = 'expense'),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    occurred_at         TIMESTAMPTZ NOT NULL,
    description         TEXT        NOT NULL
                        CHECK (char_length(btrim(description)) BETWEEN 1 AND 500),
    counterparty        TEXT,
    reference_no        TEXT,
    account_movement_id UUID        NOT NULL REFERENCES account_movements(id),
    status              TEXT        NOT NULL DEFAULT 'posted'
                        CHECK (status IN ('posted','reversed')),
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    reversed_at         TIMESTAMPTZ,
    reversed_by         UUID        REFERENCES users(id),
    reversal_reason     TEXT,
    created_by          UUID        NOT NULL REFERENCES users(id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT expense_entries_category_kind_fkey
        FOREIGN KEY (category_id, entry_kind)
        REFERENCES financial_categories (id, category_type),
    CONSTRAINT expense_entries_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT expense_entries_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL)
    )
);

CREATE UNIQUE INDEX expense_entries_expense_number_key
    ON expense_entries (expense_number);
CREATE UNIQUE INDEX expense_entries_movement_key
    ON expense_entries (account_movement_id);
CREATE INDEX expense_entries_account_idx
    ON expense_entries (financial_account_id, occurred_at DESC, id);
CREATE INDEX expense_entries_category_idx
    ON expense_entries (category_id);

-- ---------------------------------------------------------------------
-- Movement provenance extension: 'income'/'expense' join the existing
-- 'payment'/'transfer' source types. No arbitrary movement endpoint
-- exists — only domain commands produce them (docs/07).
-- ---------------------------------------------------------------------
ALTER TABLE account_movements
    DROP CONSTRAINT account_movements_source_type_check;
ALTER TABLE account_movements
    ADD CONSTRAINT account_movements_source_type_check
    CHECK (source_type IN ('payment','transfer','income','expense'));

-- ---------------------------------------------------------------------
-- STEP-010 permission catalog additions.
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('income_expense.read',
     'Gelir/Gider Kayıtlarını Görüntüle',
     'Gelir ve gider kayıtlarını, kategorileri ve dönemsel özetleri görüntüleyebilir.',
     'Gelir/Gider Yönetimi'),
    ('income_expense.manage',
     'Gelir/Gider Kayıtlarını Yönet',
     'Gelir ve gider kaydedebilir, kayıtları tersine çevirebilir ve kategorileri yönetebilir.',
     'Gelir/Gider Yönetimi');

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('income_expense.read', 'income_expense.manage')
ON CONFLICT DO NOTHING;
