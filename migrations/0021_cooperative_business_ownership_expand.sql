-- ============================================================================
-- M1-P2-C1 — Cooperative ownership EXPAND on the 37 business tables
-- ============================================================================
--
-- P1 proved cooperative-scoped authorization in the application layer.
-- C1 introduces the schema half of row ownership — additively:
--
--   - `cooperative_id UUID NULL REFERENCES cooperatives(id)` on every
--     business table. NULL means "not yet owned" and stays legal until
--     C5, so the existing application (explicit INSERT column lists)
--     keeps writing unchanged and the NeverBootstrapped legacy lane
--     keeps working.
--   - `UNIQUE (cooperative_id, id)` support indexes, the composite-FK
--     targets C2 will reference. id alone stays the PRIMARY KEY.
--     Exception: assessment_share_sources has no surrogate id — its
--     support index prefixes its real composite PRIMARY KEY,
--     (cooperative_id, assessment_id, share_id).
--
-- Deliberately NOT in C1 (guard rails agreed in the design review):
--
--   - No existing UNIQUE constraint/index is dropped or rescoped
--     (idempotency keys, numbers, (period_id, shareholder_id), …).
--     Dropping them while cooperative_id is still NULL would let
--     duplicate client-supplied keys through — replay protection must
--     never weaken mid-flight. The global→cooperative unique swap is
--     C5, after every write stamps ownership.
--   - No NOT NULL — legacy writes must not break before repositories
--     stamp cooperative_id (C3).
--   - No composite foreign keys — C2 (NOT VALID → VALIDATE).
--   - No standalone INDEX(cooperative_id): the (cooperative_id, id)
--     btree already serves cooperative-prefix lookups.
--
-- Backfill: every existing business row predates tenancy, therefore
-- belongs to the single bootstrap cooperative. A database that has
-- business rows but NO bootstrap cooperative is ambiguous — ownership
-- is never invented — so the migration aborts loudly (whole tx rolls
-- back) instead of silently assigning.
--
-- WHAT C1 ALONE IS NOT: this checkpoint adds ownership plumbing only.
-- It does NOT make business operations tenant-safe (repositories do
-- not stamp or filter by cooperative_id until C3), and it does NOT
-- authorize enabling a second cooperative. business_enabled stays
-- bootstrap-only until the full C3 + test-matrix gate is approved.
--
-- DEPLOYMENT SEQUENCE (the transition window is real even in a
-- single release — "migrate" always runs before the new binary):
--   1. Establish write quiescence (maintenance mode / stop writers).
--   2. Apply this migration (tx-atomic).
--   3. Deploy the C3-compatible application version.
--   4. Verify: V1..V7 in implementation/M1-P2-C1-VERIFICATION.sql —
--      all 37 columns present, zero unowned rows, zero unexpected
--      ownership, legacy uniques intact.
--   5. Resume writes only after verification passes.
--   If quiescence is impossible, legacy writers may insert NULL-owned
--   rows between steps 2 and 4; those are caught by V4 and closed by
--   the C4 gap-backfill step — financial dedup never weakens because
--   the global UNIQUEs remain in place.
--
-- ROLLBACK (dependency-aware — never CASCADE):
--   - Pre-C2: the only objects are these columns + *_coop_id_uq
--     indexes. `ALTER TABLE t DROP COLUMN cooperative_id` x37 removes
--     both column and its index atomically; run V7 first to confirm
--     nothing else depends on them.
--   - Post-C2: composite FKs in OTHER tables reference the
--     (cooperative_id, id) targets. Enumerate them with V7, DROP each
--     child constraint explicitly, then drop indexes/columns.
--     `DROP COLUMN ... CASCADE` is FORBIDDEN — it silently deletes
--     the composite-FK web.
--   - Post-C5 (NOT NULL + unique swap): restore from verified backup
--     per ADR-013.
-- ============================================================================

-- ---------------------------------------------------------------
-- 1. Nullable ownership columns (metadata-only ALTER, instant)
-- ---------------------------------------------------------------

ALTER TABLE persons                        ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE families                       ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE shareholders                   ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE shareholder_family_memberships ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE shares                         ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE share_events                   ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE share_ownerships               ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE periods                        ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE assessment_rules               ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE assessments                    ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE assessment_share_sources       ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE payments                       ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE payment_allocations            ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE financial_accounts             ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE account_movements              ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE account_transfers              ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE shareholder_credits            ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE credit_applications            ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE financial_categories           ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE income_entries                 ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE expense_entries                ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE share_returns                  ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE share_return_entitlements      ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE share_return_settlements       ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE investments                    ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE investment_fundings            ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE investment_valuations          ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE investment_incomes             ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE investment_disposals           ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE investment_disposal_proceeds   ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE social_aid_funds               ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE social_aid_donations           ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE social_aid_disbursements       ADD cooperative_id UUID REFERENCES cooperatives (id);

ALTER TABLE governance_bodies              ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE governance_memberships         ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE governance_decisions           ADD cooperative_id UUID REFERENCES cooperatives (id);
ALTER TABLE governance_votes               ADD cooperative_id UUID REFERENCES cooperatives (id);

-- ---------------------------------------------------------------
-- 2. Composite-FK target indexes: UNIQUE (cooperative_id, id)
-- ---------------------------------------------------------------

CREATE UNIQUE INDEX persons_coop_id_uq                        ON persons                        (cooperative_id, id);
CREATE UNIQUE INDEX families_coop_id_uq                       ON families                       (cooperative_id, id);
CREATE UNIQUE INDEX shareholders_coop_id_uq                   ON shareholders                   (cooperative_id, id);
CREATE UNIQUE INDEX shareholder_family_memberships_coop_id_uq ON shareholder_family_memberships (cooperative_id, id);

CREATE UNIQUE INDEX shares_coop_id_uq                         ON shares                         (cooperative_id, id);
CREATE UNIQUE INDEX share_events_coop_id_uq                   ON share_events                   (cooperative_id, id);
CREATE UNIQUE INDEX share_ownerships_coop_id_uq               ON share_ownerships               (cooperative_id, id);

CREATE UNIQUE INDEX periods_coop_id_uq                        ON periods                        (cooperative_id, id);
CREATE UNIQUE INDEX assessment_rules_coop_id_uq               ON assessment_rules               (cooperative_id, id);
CREATE UNIQUE INDEX assessments_coop_id_uq                    ON assessments                    (cooperative_id, id);
-- assessment_share_sources has no surrogate id: its key is the
-- composite PRIMARY KEY (assessment_id, share_id). The cooperative-
-- prefixed unique mirrors that key inside a cooperative — the C2
-- composite FKs will point INTO assessments/shares/share_ownerships,
-- while this index keeps the table's own key per-cooperative.
CREATE UNIQUE INDEX assessment_share_sources_coop_pk_uq
    ON assessment_share_sources (cooperative_id, assessment_id, share_id);

CREATE UNIQUE INDEX payments_coop_id_uq                       ON payments                       (cooperative_id, id);
CREATE UNIQUE INDEX payment_allocations_coop_id_uq            ON payment_allocations            (cooperative_id, id);

CREATE UNIQUE INDEX financial_accounts_coop_id_uq             ON financial_accounts             (cooperative_id, id);
CREATE UNIQUE INDEX account_movements_coop_id_uq              ON account_movements              (cooperative_id, id);
CREATE UNIQUE INDEX account_transfers_coop_id_uq              ON account_transfers              (cooperative_id, id);

CREATE UNIQUE INDEX shareholder_credits_coop_id_uq            ON shareholder_credits            (cooperative_id, id);
CREATE UNIQUE INDEX credit_applications_coop_id_uq            ON credit_applications            (cooperative_id, id);

CREATE UNIQUE INDEX financial_categories_coop_id_uq           ON financial_categories           (cooperative_id, id);
CREATE UNIQUE INDEX income_entries_coop_id_uq                 ON income_entries                 (cooperative_id, id);
CREATE UNIQUE INDEX expense_entries_coop_id_uq                ON expense_entries                (cooperative_id, id);

CREATE UNIQUE INDEX share_returns_coop_id_uq                  ON share_returns                  (cooperative_id, id);
CREATE UNIQUE INDEX share_return_entitlements_coop_id_uq      ON share_return_entitlements      (cooperative_id, id);
CREATE UNIQUE INDEX share_return_settlements_coop_id_uq       ON share_return_settlements       (cooperative_id, id);

CREATE UNIQUE INDEX investments_coop_id_uq                    ON investments                    (cooperative_id, id);
CREATE UNIQUE INDEX investment_fundings_coop_id_uq            ON investment_fundings            (cooperative_id, id);
CREATE UNIQUE INDEX investment_valuations_coop_id_uq          ON investment_valuations          (cooperative_id, id);
CREATE UNIQUE INDEX investment_incomes_coop_id_uq             ON investment_incomes             (cooperative_id, id);
CREATE UNIQUE INDEX investment_disposals_coop_id_uq           ON investment_disposals           (cooperative_id, id);
CREATE UNIQUE INDEX investment_disposal_proceeds_coop_id_uq   ON investment_disposal_proceeds   (cooperative_id, id);

CREATE UNIQUE INDEX social_aid_funds_coop_id_uq               ON social_aid_funds               (cooperative_id, id);
CREATE UNIQUE INDEX social_aid_donations_coop_id_uq           ON social_aid_donations           (cooperative_id, id);
CREATE UNIQUE INDEX social_aid_disbursements_coop_id_uq       ON social_aid_disbursements       (cooperative_id, id);

CREATE UNIQUE INDEX governance_bodies_coop_id_uq              ON governance_bodies              (cooperative_id, id);
CREATE UNIQUE INDEX governance_memberships_coop_id_uq         ON governance_memberships         (cooperative_id, id);
CREATE UNIQUE INDEX governance_decisions_coop_id_uq           ON governance_decisions           (cooperative_id, id);
CREATE UNIQUE INDEX governance_votes_coop_id_uq               ON governance_votes               (cooperative_id, id);

-- ---------------------------------------------------------------
-- 3. Deterministic bootstrap backfill with strict preflight
-- ---------------------------------------------------------------
--
--   bootstrap present             → all NULL rows become bootstrap-owned
--                                   (they all predate tenancy)
--   no bootstrap + zero rows      → vacuous, nothing to own (later
--                                   NeverBootstrapped writes stay NULL)
--   no bootstrap + business rows  → ABORT. Ownership must never be
--                                   invented for legacy data; the
--                                   operator claims it deliberately
--                                   (`bootstrap-cooperative` CLI) first.

DO $$
DECLARE
    v_bootstrap   uuid;
    v_inst        uuid;
    v_residue     bigint;
    v_unexpected  bigint;
BEGIN
    -- Bootstrap identification is the is_bootstrap flag, which
    -- cooperatives_bootstrap_uq (0018) caps at one row. installation_state
    -- (0020) carries the same fact transactionally — the two MUST agree.
    -- Any divergence is tampering/ambiguity: abort, never pick one.
    SELECT id INTO v_bootstrap FROM cooperatives WHERE is_bootstrap;
    SELECT bootstrap_cooperative_id INTO v_inst FROM installation_state;

    IF v_bootstrap IS NULL THEN
        -- Consistent NeverBootstrapped state requires the durable marker
        -- to agree (both NULL). A marker claiming bootstrap without the
        -- flag row is ambiguous — fail loudly.
        IF v_inst IS NOT NULL THEN
            RAISE EXCEPTION
                'P2-C1: installation_state claims bootstrap % but no is_bootstrap cooperative exists — ambiguous state, aborting.',
                v_inst;
        END IF;

        -- Residue check — fail-closed seed classification:
        -- migration 0010 plants exactly 10 built-in categories into every
        -- fresh database long before any cooperative can exist. Those
        -- rows are identifiable: created_by IS NULL (no bootstrap user
        -- exists at migration time) AND (category_type, name) matching
        -- the fixed seed list below. Anything else in the table —
        -- a user-created category (created_by IS NOT NULL) or an
        -- unexpected NULL-created_by row — is genuine unowned business
        -- data and must abort like every other table. Seed rows may
        -- stay NULL on NeverBootstrapped installs until claim +
        -- gap-backfill own them later; they still get backfilled
        -- normally when a bootstrap cooperative exists.
        SELECT (SELECT count(*) FROM persons)
             + (SELECT count(*) FROM families)
             + (SELECT count(*) FROM shareholders)
             + (SELECT count(*) FROM shareholder_family_memberships)
             + (SELECT count(*) FROM shares)
             + (SELECT count(*) FROM share_events)
             + (SELECT count(*) FROM share_ownerships)
             + (SELECT count(*) FROM periods)
             + (SELECT count(*) FROM assessment_rules)
             + (SELECT count(*) FROM assessments)
             + (SELECT count(*) FROM assessment_share_sources)
             + (SELECT count(*) FROM payments)
             + (SELECT count(*) FROM payment_allocations)
             + (SELECT count(*) FROM financial_accounts)
             + (SELECT count(*) FROM account_movements)
             + (SELECT count(*) FROM account_transfers)
             + (SELECT count(*) FROM shareholder_credits)
             + (SELECT count(*) FROM credit_applications)
             + (SELECT count(*) FROM financial_categories
                 WHERE created_by IS NOT NULL
                    OR (category_type, name) NOT IN (
                        ('income',  'Diğer Gelir'),
                        ('income',  'Kira Geliri'),
                        ('income',  'Yatırım Geliri'),
                        ('expense', 'Genel Gider'),
                        ('expense', 'Kira'),
                        ('expense', 'Ulaşım'),
                        ('expense', 'Bakım / Onarım'),
                        ('expense', 'Elektrik / Su / İletişim'),
                        ('expense', 'Organizasyon'),
                        ('expense', 'Diğer Gider')))
             + (SELECT count(*) FROM income_entries)
             + (SELECT count(*) FROM expense_entries)
             + (SELECT count(*) FROM share_returns)
             + (SELECT count(*) FROM share_return_entitlements)
             + (SELECT count(*) FROM share_return_settlements)
             + (SELECT count(*) FROM investments)
             + (SELECT count(*) FROM investment_fundings)
             + (SELECT count(*) FROM investment_valuations)
             + (SELECT count(*) FROM investment_incomes)
             + (SELECT count(*) FROM investment_disposals)
             + (SELECT count(*) FROM investment_disposal_proceeds)
             + (SELECT count(*) FROM social_aid_funds)
             + (SELECT count(*) FROM social_aid_donations)
             + (SELECT count(*) FROM social_aid_disbursements)
             + (SELECT count(*) FROM governance_bodies)
             + (SELECT count(*) FROM governance_memberships)
             + (SELECT count(*) FROM governance_decisions)
             + (SELECT count(*) FROM governance_votes)
          INTO v_residue;

        IF v_residue > 0 THEN
            RAISE EXCEPTION
                'P2-C1: % business row(s) exist but no bootstrap cooperative — refusing to invent ownership. Run bootstrap-cooperative first, then re-run migrations.',
                v_residue;
        END IF;
        RETURN;
    END IF;

    -- Marker/flag consistency: bootstrap exists, so installation_state
    -- must point at exactly it (0020 backfills from is_bootstrap; fresh
    -- bootstrap writes both in one transaction).
    IF v_inst IS DISTINCT FROM v_bootstrap THEN
        RAISE EXCEPTION
            'P2-C1: is_bootstrap cooperative % disagrees with installation_state marker % — ambiguous state, aborting.',
            v_bootstrap, v_inst;
    END IF;

    -- The column is brand new; a pre-existing non-NULL value can only be
    -- bootstrap-owned. Anything else is unexpected ownership — fail
    -- loudly instead of silently accepting it into the scoped model.
    SELECT (SELECT count(*) FROM persons                        WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM families                       WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM shareholders                   WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM shareholder_family_memberships WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM shares                         WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM share_events                   WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM share_ownerships               WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM periods                        WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM assessment_rules               WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM assessments                    WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM assessment_share_sources       WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM payments                       WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM payment_allocations            WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM financial_accounts             WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM account_movements              WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM account_transfers              WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM shareholder_credits            WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM credit_applications            WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM financial_categories           WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM income_entries                 WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM expense_entries                WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM share_returns                  WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM share_return_entitlements      WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM share_return_settlements       WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM investments                    WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM investment_fundings            WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM investment_valuations          WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM investment_incomes             WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM investment_disposals           WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM investment_disposal_proceeds   WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM social_aid_funds               WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM social_aid_donations           WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM social_aid_disbursements       WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM governance_bodies              WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM governance_memberships         WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM governance_decisions           WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
         + (SELECT count(*) FROM governance_votes               WHERE cooperative_id IS NOT NULL AND cooperative_id <> v_bootstrap)
      INTO v_unexpected;
    IF v_unexpected > 0 THEN
        RAISE EXCEPTION
            'P2-C1: % business row(s) carry ownership other than the bootstrap cooperative — refusing silent acceptance. Investigate before re-running.',
            v_unexpected;
    END IF;

    UPDATE persons                        SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE families                       SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE shareholders                   SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE shareholder_family_memberships SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE shares                         SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE share_events                   SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE share_ownerships               SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE periods                        SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE assessment_rules               SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE assessments                    SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE assessment_share_sources       SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE payments                       SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE payment_allocations            SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE financial_accounts             SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE account_movements              SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE account_transfers              SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE shareholder_credits            SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE credit_applications            SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE financial_categories           SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE income_entries                 SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE expense_entries                SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE share_returns                  SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE share_return_entitlements      SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE share_return_settlements       SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE investments                    SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE investment_fundings            SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE investment_valuations          SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE investment_incomes             SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE investment_disposals           SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE investment_disposal_proceeds   SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE social_aid_funds               SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE social_aid_donations           SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE social_aid_disbursements       SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE governance_bodies              SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE governance_memberships         SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE governance_decisions           SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;
    UPDATE governance_votes               SET cooperative_id = v_bootstrap WHERE cooperative_id IS NULL;

    -- Post-backfill invariant: on a bootstrapped installation every
    -- business row must be owned. A leftover NULL means a table was
    -- skipped — fail loudly, roll the whole transaction back.
    SELECT (SELECT count(*) FROM persons                        WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM families                       WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM shareholders                   WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM shareholder_family_memberships WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM shares                         WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM share_events                   WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM share_ownerships               WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM periods                        WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM assessment_rules               WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM assessments                    WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM assessment_share_sources       WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM payments                       WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM payment_allocations            WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM financial_accounts             WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM account_movements              WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM account_transfers              WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM shareholder_credits            WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM credit_applications            WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM financial_categories           WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM income_entries                 WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM expense_entries                WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM share_returns                  WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM share_return_entitlements      WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM share_return_settlements       WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM investments                    WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM investment_fundings            WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM investment_valuations          WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM investment_incomes             WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM investment_disposals           WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM investment_disposal_proceeds   WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM social_aid_funds               WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM social_aid_donations           WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM social_aid_disbursements       WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM governance_bodies              WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM governance_memberships         WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM governance_decisions           WHERE cooperative_id IS NULL)
         + (SELECT count(*) FROM governance_votes               WHERE cooperative_id IS NULL)
      INTO v_residue;
    IF v_residue > 0 THEN
        RAISE EXCEPTION
            'P2-C1: % business row(s) still unowned after bootstrap backfill — a table was skipped. Aborting.',
            v_residue;
    END IF;
END $$;
