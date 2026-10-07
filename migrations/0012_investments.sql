-- =====================================================================
-- 0012  INVESTMENT & INVESTMENT CASH-FLOW  (STEP-012)
-- docs/08-INVESTMENTS-ASSETS-REAL-ESTATE-BUSINESS-HOLDINGS.md,
-- docs/02-DOMAIN-MODEL.md, docs/03-FINANCIAL-CORE.md,
-- docs/15-INVARIANTS.md, docs/16-STATE-MACHINES.md,
-- docs/17-CALCULATION-RULES.md, docs/19-AUDIT-REVERSAL-CORRECTION.md,
-- ADR-003/004/006.
--
-- Scope decisions (implementation record):
--   * The four concepts stay separate (docs/08 core principle):
--       INVESTMENT IDENTITY: investments — what the cooperative owns
--       ACQUISITION COST:    investment_fundings — real money paid out
--                            (each leg = exactly ONE outflow movement)
--       ESTIMATED VALUE:     investment_valuations — informational
--                            history, moves ZERO money
--       CASH RESULT:         investment_incomes / disposal proceeds —
--                            real money in, one movement per leg
--   * Acquisition is NOT Expense / Payment / Transfer / Credit
--     (docs/03 §asset acquisition, docs/15): a funding posts exactly one
--     outflow Account Movement with source_type 'investment_funding'.
--   * Investment income is NOT STEP-010 operational Income: it posts
--     exactly one inflow movement with source_type
--     'investment_income' and creates no `incomes` row — one real
--     receipt, one authoritative cash effect (no double counting).
--     Income-recognition reporting policy is an open decision
--     (docs/08 §open decisions) — movements stay classified by
--     provenance so a later policy can report on them safely.
--   * Valuation NEVER moves cash and NEVER creates Income (docs/08:
--     "Valuation gain is not automatically realized profit/cash").
--   * Disposal: investment_disposals holds the derecognition event
--     (agreed consideration is informational metadata); proceeds legs
--     record ACTUAL cash received per Financial Account. The
--     realized-gain formula is UNRESOLVED (docs/08, docs/17) — no
--     profit is calculated or stored. Disposal reversal semantics are
--     undefined → a posted disposal is terminal in STEP-012.
--   * Investment expense classification is UNRESOLVED (docs/08 §open
--     decisions: cost vs operational Expense vs capitalized cost) —
--     deferred, no table/column exists for it in STEP-012.
--   * Expected (forecast) investment income is DEFINED AS FORECAST
--     DATA (docs/17) — deferred; never realized automatically.
--   * Lifecycle: investments 'active' -> 'disposed' | 'cancelled';
--     fundings/incomes 'posted' -> 'reversed'; valuations 'recorded'
--     -> 'cancelled'. No DELETE path exists for any of them
--     (docs/19). An Investment may be cancelled only while it carries
--     no financial events.
-- =====================================================================

-- ---------------------------------------------------------------------
-- Movement provenance: investment movements must answer "why did
-- money leave/enter this account?" — Investment Funding / Investment
-- Income / Investment Disposal proceeds.
-- ---------------------------------------------------------------------
ALTER TABLE account_movements
    DROP CONSTRAINT account_movements_source_type_check;
ALTER TABLE account_movements
    ADD CONSTRAINT account_movements_source_type_check
    CHECK (source_type IN
        ('payment','transfer','income','expense','share_return_settlement',
         'investment_funding','investment_income','investment_disposal'));

-- ---------------------------------------------------------------------
-- Investment aggregate: cooperative-owned asset identity (docs/02,
-- docs/08). Type taxonomy is an open decision (docs/08) — the minimal
-- authoritative distinction is Real Estate vs Business Holding.
-- Cost, value and cash are NEVER stored on this row: cost derives from
-- posted fundings, estimated value from the valuation history, cash
-- from movements.
-- ---------------------------------------------------------------------
CREATE TABLE investments (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    investment_number   BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    name                TEXT        NOT NULL,
    investment_type     TEXT        NOT NULL
                        CHECK (investment_type IN ('real_estate','business')),
    description         TEXT,
    -- Bounded descriptive metadata only (docs/08): a location/address
    -- label for real estate, a counterparty/business label, and a
    -- document reference (title deed / contract / agreement number).
    location            TEXT,
    reference           TEXT,
    counterparty_name   TEXT,
    -- Business date the asset was acquired (identity metadata; cash
    -- timing lives on funding legs).
    acquired_at         DATE,
    status              TEXT        NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active','disposed','cancelled')),
    disposed_at         TIMESTAMPTZ,
    disposed_by         UUID        REFERENCES users (id),
    cancelled_at        TIMESTAMPTZ,
    cancelled_by        UUID        REFERENCES users (id),
    cancellation_reason TEXT,
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT investments_idempotency_key_key UNIQUE (idempotency_key),
    CONSTRAINT investments_disposed_consistent CHECK (
        (status = 'disposed'
            AND disposed_at IS NOT NULL AND disposed_by IS NOT NULL)
        OR
        (status <> 'disposed'
            AND disposed_at IS NULL AND disposed_by IS NULL)),
    CONSTRAINT investments_cancelled_consistent CHECK (
        (status = 'cancelled'
            AND cancelled_at IS NOT NULL AND cancelled_by IS NOT NULL
            AND cancellation_reason IS NOT NULL)
        OR
        (status <> 'cancelled'
            AND cancelled_at IS NULL AND cancelled_by IS NULL
            AND cancellation_reason IS NULL))
);

CREATE UNIQUE INDEX investments_investment_number_key
    ON investments (investment_number);
CREATE INDEX investments_status_idx
    ON investments (status, investment_number DESC);
CREATE INDEX investments_type_idx
    ON investments (investment_type, investment_number DESC);

-- ---------------------------------------------------------------------
-- Acquisition funding legs: real money leaving a Financial Account to
-- acquire/fund the investment. Each posted leg binds 1:1 to exactly
-- ONE outflow Account Movement ('investment_funding') — the movement
-- stays the authoritative money record (ADR-003). "Additional
-- capital" is simply a further funding leg (docs/08). Acquisition is
-- not operational Expense (docs/03, docs/15) and creates no
-- incomes/expenses row.
-- ---------------------------------------------------------------------
CREATE TABLE investment_fundings (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    funding_number      BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    investment_id       UUID        NOT NULL REFERENCES investments (id),
    financial_account_id UUID       NOT NULL REFERENCES financial_accounts (id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    occurred_at         TIMESTAMPTZ NOT NULL,
    reference           TEXT,
    note                TEXT,
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

    CONSTRAINT investment_fundings_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT investment_fundings_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL
            AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL))
);

CREATE UNIQUE INDEX investment_fundings_funding_number_key
    ON investment_fundings (funding_number);
CREATE UNIQUE INDEX investment_fundings_movement_key
    ON investment_fundings (account_movement_id);
CREATE INDEX investment_fundings_investment_idx
    ON investment_fundings (investment_id, occurred_at DESC, id);
CREATE INDEX investment_fundings_account_idx
    ON investment_fundings (financial_account_id, occurred_at DESC, id);

-- ---------------------------------------------------------------------
-- Valuation history (docs/08, docs/17): dated value assessments with
-- recorded method/source. Informational only — a valuation creates
-- ZERO Account Movements and never rewrites acquisition cost.
-- Cancellation marks a wrongly-recorded valuation; the row stays.
-- ---------------------------------------------------------------------
CREATE TABLE investment_valuations (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    valuation_number    BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    investment_id       UUID        NOT NULL REFERENCES investments (id),
    valuation_date      DATE        NOT NULL,
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    method              TEXT,
    source              TEXT,
    note                TEXT,
    status              TEXT        NOT NULL DEFAULT 'recorded'
                        CHECK (status IN ('recorded','cancelled')),
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    cancelled_at        TIMESTAMPTZ,
    cancelled_by        UUID        REFERENCES users (id),
    cancellation_reason TEXT,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT investment_valuations_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT investment_valuations_cancelled_consistent CHECK (
        (status = 'cancelled'
            AND cancelled_at IS NOT NULL AND cancelled_by IS NOT NULL
            AND cancellation_reason IS NOT NULL)
        OR
        (status <> 'cancelled'
            AND cancelled_at IS NULL AND cancelled_by IS NULL
            AND cancellation_reason IS NULL))
);

CREATE UNIQUE INDEX investment_valuations_valuation_number_key
    ON investment_valuations (valuation_number);
CREATE INDEX investment_valuations_investment_idx
    ON investment_valuations
    (investment_id, valuation_date DESC, valuation_number DESC);

-- ---------------------------------------------------------------------
-- Investment cash income: real money received BECAUSE of the
-- investment (rent, business distribution, other proceeds). Exactly
-- ONE inflow Account Movement per row ('investment_income'); it is
-- NOT a STEP-010 `incomes` row and never creates Payment / Credit /
-- Transfer / Entitlement side effects.
-- ---------------------------------------------------------------------
CREATE TABLE investment_incomes (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    income_number       BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    investment_id       UUID        NOT NULL REFERENCES investments (id),
    financial_account_id UUID       NOT NULL REFERENCES financial_accounts (id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    occurred_at         TIMESTAMPTZ NOT NULL,
    description         TEXT        NOT NULL,
    counterparty        TEXT,
    reference_no        TEXT,
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

    CONSTRAINT investment_incomes_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT investment_incomes_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL
            AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL))
);

CREATE UNIQUE INDEX investment_incomes_income_number_key
    ON investment_incomes (income_number);
CREATE UNIQUE INDEX investment_incomes_movement_key
    ON investment_incomes (account_movement_id);
CREATE INDEX investment_incomes_investment_idx
    ON investment_incomes (investment_id, occurred_at DESC, id);
CREATE INDEX investment_incomes_account_idx
    ON investment_incomes (financial_account_id, occurred_at DESC, id);

-- ---------------------------------------------------------------------
-- Disposal: asset derecognition event (docs/08 §sale/disposal). One
-- per investment (partial disposal is NOT modelled in STEP-012 —
-- specs do not define it). `consideration_amount` is the AGREED sale
-- price — informational metadata; actual cash received lives on
-- proceeds legs. No realized gain/loss is calculated (formula
-- UNRESOLVED). A posted disposal is terminal in STEP-012: reversal
-- semantics are undefined (docs/08 §open decisions) and must not be
-- invented.
-- ---------------------------------------------------------------------
CREATE TABLE investment_disposals (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    disposal_number     BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    investment_id       UUID        NOT NULL REFERENCES investments (id),
    -- Business date of the sale/disposal.
    disposed_at         DATE        NOT NULL,
    consideration_amount NUMERIC(19,2)
                        CHECK (consideration_amount IS NULL
                               OR consideration_amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    counterparty_name   TEXT,
    reference           TEXT,
    note                TEXT,
    status              TEXT        NOT NULL DEFAULT 'posted'
                        CHECK (status IN ('posted')),
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT investment_disposals_idempotency_key_key
        UNIQUE (idempotency_key)
);

CREATE UNIQUE INDEX investment_disposals_disposal_number_key
    ON investment_disposals (disposal_number);
-- One disposal per Investment — derecognition is a single lifecycle
-- event in STEP-012.
CREATE UNIQUE INDEX investment_disposals_one_per_investment
    ON investment_disposals (investment_id);

-- ---------------------------------------------------------------------
-- Disposal proceeds legs: ACTUAL cash received per Financial Account.
-- Each leg binds 1:1 to exactly ONE inflow Account Movement
-- ('investment_disposal'). Sale proceeds are NOT operational Income.
-- ---------------------------------------------------------------------
CREATE TABLE investment_disposal_proceeds (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    disposal_id         UUID        NOT NULL
                        REFERENCES investment_disposals (id),
    financial_account_id UUID       NOT NULL REFERENCES financial_accounts (id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    occurred_at         TIMESTAMPTZ NOT NULL,
    reference           TEXT,
    account_movement_id UUID        NOT NULL REFERENCES account_movements (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX investment_disposal_proceeds_movement_key
    ON investment_disposal_proceeds (account_movement_id);
CREATE INDEX investment_disposal_proceeds_disposal_idx
    ON investment_disposal_proceeds (disposal_id);
CREATE INDEX investment_disposal_proceeds_account_idx
    ON investment_disposal_proceeds (financial_account_id, occurred_at DESC, id);

-- ---------------------------------------------------------------------
-- STEP-012 permission catalog additions.
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('investments.read',
     'Yatırımları Görüntüle',
     'Yatırımları, finansmanı, değerlemeleri, yatırım gelirlerini ve tasfiye kayıtlarını görüntüleyebilir.',
     'Yatırım Yönetimi'),
    ('investments.manage',
     'Yatırımları Yönet',
     'Yatırım oluşturabilir/iptal edebilir, edinim finansmanı kaydedebilir/tersine çevirebilir, değerleme ve yatırım geliri kaydedebilir ve yatırımı tasfiye edebilir.',
     'Yatırım Yönetimi');

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('investments.read', 'investments.manage')
ON CONFLICT DO NOTHING;
