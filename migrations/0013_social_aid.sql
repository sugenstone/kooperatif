-- =====================================================================
-- 0013  SOCIAL AID, DONATION & RESTRICTED FUND  (STEP-013)
-- docs/11-SOCIAL-AID.md, docs/02-DOMAIN-MODEL.md, docs/03-FINANCIAL-CORE.md,
-- docs/15-INVARIANTS.md, docs/16-STATE-MACHINES.md, docs/17-CALCULATION-RULES.md,
-- docs/19-AUDIT-REVERSAL-CORRECTION.md, docs/22-SECURITY.md,
-- ADR-003/004/006.
--
-- Scope decisions (implementation record):
--   * Social Aid is a SEPARATE financial context (docs/11 core rule,
--     docs/15): donations and aid disbursements are their own business
--     events — they create NO Payment / Allocation / Assessment /
--     Shareholder Credit / Income / Expense / Transfer / Share Return /
--     Investment rows. One real cash event = exactly ONE movement.
--   * FINANCIAL ACCOUNT answers WHERE money physically sits; SOCIAL AID
--     FUND answers WHAT PURPOSE it is restricted to. A Fund is not an
--     account (docs/01 canonical distinction "Financial Account != Fund").
--   * Restricted availability is derived per (fund, financial_account)
--     pair — the conservative reading of docs/11 + docs/17
--     ("restricted and unrestricted resources must remain
--     distinguishable"): a donation restricted to Fund F deposited in
--     Account A can only fund aid paid FROM Account A. No fund-level
--     availability can be satisfied by restricted money sitting in an
--     unrelated account. Fund-to-fund reallocation and restricted-money
--     account transfers are NOT defined by the specs → deferred.
--   * Donor / Beneficiary identity reuses `persons` (docs/02 Party
--     reuse) when the real person is already known; a bounded
--     free-text display name covers external persons and organization
--     supporters (docs/02: "controlled free-text fallback"; glossary:
--     Supporter may be an organization). At least one identity must be
--     present — anonymous donation rules are an open decision in
--     docs/11 → deferred, so a fully anonymous donation cannot be
--     recorded in STEP-013.
--   * Aid Request / Aid Decision approval pipeline (docs/02 canonical
--     concepts, docs/16 candidate lifecycle) is NOT implemented:
--     docs/11 conditions payment on an approved request only "where
--     required" and approval authority/thresholds are open decisions
--     (docs/18, docs/11 §open decisions). A disbursement carries an
--     explicit reason + reference instead; the grant/decision pipeline
--     is deferred to the governance step.
--   * Fund lifecycle: 'active' -> 'closed' | 'cancelled'. A fund with
--     any financial events cannot be cancelled; a fund cannot be
--     closed while any (fund, account) restricted availability is
--     non-zero — restricted money must never become ownerless.
--   * Reversal is status-based, never deletion (docs/19):
--     donation/disbursement 'posted' -> 'reversed' flips the bound
--     movement 'active' -> 'reversed' in the same transaction.
--     Donation reversal is refused when it would overdraw the physical
--     account OR the fund's restricted availability (docs/11:
--     restricted money already spent cannot be invalidated).
-- =====================================================================

-- ---------------------------------------------------------------------
-- Movement provenance: social-aid movements must answer "why did money
-- enter/leave this account?" — Donation inflow / Aid outflow.
-- ---------------------------------------------------------------------
ALTER TABLE account_movements
    DROP CONSTRAINT account_movements_source_type_check;
ALTER TABLE account_movements
    ADD CONSTRAINT account_movements_source_type_check
    CHECK (source_type IN
        ('payment','transfer','income','expense','share_return_settlement',
         'investment_funding','investment_income','investment_disposal',
         'social_aid_donation','social_aid_disbursement'));

-- ---------------------------------------------------------------------
-- Social Aid Fund: purpose/restriction identity (docs/08 of the aid
-- domain — "Genel Sosyal Yardım", "Eğitim Yardımı"…). The row carries
-- NO balance: restricted availability is always derived from posted
-- donation/disbursement history (§11/§34).
-- ---------------------------------------------------------------------
CREATE TABLE social_aid_funds (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    fund_number         BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    name                TEXT        NOT NULL,
    description         TEXT,
    -- Optional program window (bounded metadata; the fund does not
    -- auto-expire — enforcement is an operator action).
    starts_on           DATE,
    ends_on             DATE,
    status              TEXT        NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active','closed','cancelled')),
    closed_at           TIMESTAMPTZ,
    closed_by           UUID        REFERENCES users (id),
    cancelled_at        TIMESTAMPTZ,
    cancelled_by        UUID        REFERENCES users (id),
    cancellation_reason TEXT,
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT social_aid_funds_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT social_aid_funds_window_check CHECK (
        starts_on IS NULL OR ends_on IS NULL OR ends_on >= starts_on),
    CONSTRAINT social_aid_funds_closed_consistent CHECK (
        (status = 'closed'
            AND closed_at IS NOT NULL AND closed_by IS NOT NULL)
        OR
        (status <> 'closed'
            AND closed_at IS NULL AND closed_by IS NULL)),
    CONSTRAINT social_aid_funds_cancelled_consistent CHECK (
        (status = 'cancelled'
            AND cancelled_at IS NOT NULL AND cancelled_by IS NOT NULL
            AND cancellation_reason IS NOT NULL)
        OR
        (status <> 'cancelled'
            AND cancelled_at IS NULL AND cancelled_by IS NULL
            AND cancellation_reason IS NULL))
);

CREATE UNIQUE INDEX social_aid_funds_fund_number_key
    ON social_aid_funds (fund_number);
CREATE INDEX social_aid_funds_status_idx
    ON social_aid_funds (status, fund_number DESC);

-- ---------------------------------------------------------------------
-- Donation / support receipt: real money received FOR a restricted
-- purpose. Exactly ONE inflow Account Movement per posted donation
-- ('social_aid_donation'). A donation is NOT a Payment, Allocation,
-- Shareholder Credit, operational Income, Transfer or Investment
-- income (docs/11, docs/15).
--
-- Donor identity (docs/02, docs/11): `donor_person_id` references the
-- canonical Person when the supporter is already known — shareholder,
-- guardian or any registered person, membership NEVER required. The
-- bounded `donor_display_name` covers external persons and
-- organization supporters. At least one form of identity is
-- mandatory: anonymous donation policy is an open decision (docs/11)
-- and is NOT silently implemented.
-- ---------------------------------------------------------------------
CREATE TABLE social_aid_donations (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    donation_number     BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    fund_id             UUID        NOT NULL
                        REFERENCES social_aid_funds (id),
    donor_person_id     UUID        REFERENCES persons (id),
    donor_display_name  TEXT,
    financial_account_id UUID       NOT NULL
                        REFERENCES financial_accounts (id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    occurred_at         TIMESTAMPTZ NOT NULL,
    reference           TEXT,
    note                TEXT,
    account_movement_id UUID        NOT NULL
                        REFERENCES account_movements (id),
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

    CONSTRAINT social_aid_donations_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT social_aid_donations_donor_identity CHECK (
        donor_person_id IS NOT NULL
        OR nullif(btrim(donor_display_name), '') IS NOT NULL),
    CONSTRAINT social_aid_donations_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL
            AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL))
);

CREATE UNIQUE INDEX social_aid_donations_donation_number_key
    ON social_aid_donations (donation_number);
CREATE UNIQUE INDEX social_aid_donations_movement_key
    ON social_aid_donations (account_movement_id);
CREATE INDEX social_aid_donations_fund_idx
    ON social_aid_donations (fund_id, occurred_at DESC, id);
CREATE INDEX social_aid_donations_account_idx
    ON social_aid_donations (financial_account_id, occurred_at DESC, id);
CREATE INDEX social_aid_donations_donor_idx
    ON social_aid_donations (donor_person_id, occurred_at DESC)
    WHERE donor_person_id IS NOT NULL;

-- ---------------------------------------------------------------------
-- Aid disbursement: real restricted money paid TO a beneficiary.
-- Exactly ONE outflow Account Movement per posted disbursement
-- ('social_aid_disbursement'). It is NOT operational Expense, Payment,
-- Transfer, Shareholder Credit or Share Return settlement (docs/11,
-- docs/15). `reason` is required: the disbursement must answer why the
-- aid was granted (decision pipeline deferred, see header).
--
-- Beneficiary identity mirrors the donor rule: canonical Person when
-- known (shareholder membership never required), bounded display name
-- otherwise. Data minimization (docs/11 §privacy, docs/22): no
-- medical, religious, political or case-management attributes exist.
-- ---------------------------------------------------------------------
CREATE TABLE social_aid_disbursements (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    disbursement_number BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    fund_id             UUID        NOT NULL
                        REFERENCES social_aid_funds (id),
    beneficiary_person_id UUID      REFERENCES persons (id),
    beneficiary_display_name TEXT,
    financial_account_id UUID       NOT NULL
                        REFERENCES financial_accounts (id),
    amount              NUMERIC(19,2) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL DEFAULT 'TRY'
                        CHECK (currency = 'TRY'),
    occurred_at         TIMESTAMPTZ NOT NULL,
    reason              TEXT        NOT NULL,
    reference           TEXT,
    account_movement_id UUID        NOT NULL
                        REFERENCES account_movements (id),
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

    CONSTRAINT social_aid_disbursements_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT social_aid_disbursements_beneficiary_identity CHECK (
        beneficiary_person_id IS NOT NULL
        OR nullif(btrim(beneficiary_display_name), '') IS NOT NULL),
    CONSTRAINT social_aid_disbursements_reversal_consistent CHECK (
        (status = 'posted'
            AND reversed_at IS NULL AND reversed_by IS NULL
            AND reversal_reason IS NULL)
        OR
        (status = 'reversed'
            AND reversed_at IS NOT NULL AND reversed_by IS NOT NULL
            AND reversal_reason IS NOT NULL))
);

CREATE UNIQUE INDEX social_aid_disbursements_disbursement_number_key
    ON social_aid_disbursements (disbursement_number);
CREATE UNIQUE INDEX social_aid_disbursements_movement_key
    ON social_aid_disbursements (account_movement_id);
CREATE INDEX social_aid_disbursements_fund_idx
    ON social_aid_disbursements (fund_id, occurred_at DESC, id);
CREATE INDEX social_aid_disbursements_account_idx
    ON social_aid_disbursements (financial_account_id, occurred_at DESC, id);
CREATE INDEX social_aid_disbursements_beneficiary_idx
    ON social_aid_disbursements (beneficiary_person_id, occurred_at DESC)
    WHERE beneficiary_person_id IS NOT NULL;

-- ---------------------------------------------------------------------
-- STEP-013 permission catalog additions. Social Aid data is sensitive
-- (docs/11 §privacy, docs/22): its own read/manage pair, never folded
-- into broader financial permissions.
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('social_aid.read',
     'Sosyal Yardımı Görüntüle',
     'Sosyal yardım fonlarını, bağışları ve yardım ödemelerini görüntüleyebilir.',
     'Sosyal Yardım'),
    ('social_aid.manage',
     'Sosyal Yardımı Yönet',
     'Sosyal yardım fonu oluşturabilir/kapatabilir, bağış ve yardım ödemesi kaydedebilir ve tersine çevirebilir.',
     'Sosyal Yardım');

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('social_aid.read', 'social_aid.manage')
ON CONFLICT DO NOTHING;
