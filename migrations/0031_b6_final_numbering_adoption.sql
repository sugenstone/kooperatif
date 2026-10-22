-- M1-P3-B6 — cooperative-local numbering adoption: the final five
-- series — social_aid_funds, social_aid_donations,
-- social_aid_disbursements, governance_bodies, governance_decisions
-- (design: implementation/M1-P3-NUMBERING-DESIGN.md §3.4, batch pattern
-- of 0027..0030). After this migration every registered automatic
-- document series is cooperative-local; only the manual
-- families.sequence_number lane remains outside the allocator.
--
-- For each adopted series this migration, inside ONE transaction:
--
--   * takes an AccessExclusive lock on the business table first, so no
--     concurrent legacy writer can slip a row between seeding and
--     IDENTITY removal;
--   * REJECTS stale or inconsistent pre-existing counters — a counter
--     row at or below the table's live maximum would reissue a live
--     document number, so the migration fails closed BEFORE IDENTITY
--     is touched;
--   * seeds cooperative_number_counters at MAX(number)+1 per
--     cooperative (ON CONFLICT DO NOTHING keeps a consistent existing
--     counter untouched) — existing numbers are preserved verbatim,
--     nothing is ever renumbered;
--   * DROPs IDENTITY from the number column — after this point any
--     INSERT that does not supply the number fails closed on NOT NULL,
--     which structurally prevents mixed IDENTITY/counter allocation.
--
-- families.sequence_number stays manual. Historical numbers are never
-- rewritten; the IDENTITY sequences are left in place (inert) for a
-- later cleanup phase so rollback stays trivial.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- Pre-check: scoped replacement uniques must exist (0024) and the
-- allocator registry rows for all five series must exist (0026).
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'social_aid_funds_coop_fund_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'social_aid_funds_coop_fund_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'social_aid_donations_coop_donation_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'social_aid_donations_coop_donation_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'social_aid_disbursements_coop_disbursement_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'social_aid_disbursements_coop_disbursement_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'governance_bodies_coop_body_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'governance_bodies_coop_body_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'governance_decisions_coop_decision_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'governance_decisions_coop_decision_number_uq missing/invalid';
    END IF;
    IF (SELECT count(*) FROM document_number_series
         WHERE series_key IN ('social_aid_funds.fund_number',
                              'social_aid_donations.donation_number',
                              'social_aid_disbursements.disbursement_number',
                              'governance_bodies.body_number',
                              'governance_decisions.decision_number')) <> 5 THEN
        RAISE EXCEPTION 'numbering series registry incomplete';
    END IF;
END $$;

-- ------------------------------------------------------------------
-- social_aid_funds.fund_number
-- ------------------------------------------------------------------
LOCK TABLE social_aid_funds IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN social_aid_funds t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'social_aid_funds.fund_number'
           AND c.next_value <= t.fund_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for social_aid_funds.fund_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'social_aid_funds.fund_number', MAX(fund_number) + 1
  FROM social_aid_funds
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE social_aid_funds ALTER COLUMN fund_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- social_aid_donations.donation_number
-- ------------------------------------------------------------------
LOCK TABLE social_aid_donations IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN social_aid_donations t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'social_aid_donations.donation_number'
           AND c.next_value <= t.donation_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for social_aid_donations.donation_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'social_aid_donations.donation_number', MAX(donation_number) + 1
  FROM social_aid_donations
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE social_aid_donations ALTER COLUMN donation_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- social_aid_disbursements.disbursement_number
-- ------------------------------------------------------------------
LOCK TABLE social_aid_disbursements IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN social_aid_disbursements t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'social_aid_disbursements.disbursement_number'
           AND c.next_value <= t.disbursement_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for social_aid_disbursements.disbursement_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'social_aid_disbursements.disbursement_number', MAX(disbursement_number) + 1
  FROM social_aid_disbursements
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE social_aid_disbursements ALTER COLUMN disbursement_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- governance_bodies.body_number
-- ------------------------------------------------------------------
LOCK TABLE governance_bodies IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN governance_bodies t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'governance_bodies.body_number'
           AND c.next_value <= t.body_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for governance_bodies.body_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'governance_bodies.body_number', MAX(body_number) + 1
  FROM governance_bodies
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE governance_bodies ALTER COLUMN body_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- governance_decisions.decision_number
-- ------------------------------------------------------------------
LOCK TABLE governance_decisions IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN governance_decisions t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'governance_decisions.decision_number'
           AND c.next_value <= t.decision_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for governance_decisions.decision_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'governance_decisions.decision_number', MAX(decision_number) + 1
  FROM governance_decisions
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE governance_decisions ALTER COLUMN decision_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- Post-checks (fail-closed, whole migration rolls back on violation)
-- ------------------------------------------------------------------
DO $$
DECLARE
    under_seeded integer;
BEGIN
    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN social_aid_funds t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'social_aid_funds.fund_number'
       AND c.next_value <= t.fund_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'social_aid_funds counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN social_aid_donations t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'social_aid_donations.donation_number'
       AND c.next_value <= t.donation_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'social_aid_donations counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN social_aid_disbursements t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'social_aid_disbursements.disbursement_number'
       AND c.next_value <= t.disbursement_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'social_aid_disbursements counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN governance_bodies t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'governance_bodies.body_number'
       AND c.next_value <= t.body_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'governance_bodies counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN governance_decisions t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'governance_decisions.decision_number'
       AND c.next_value <= t.decision_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'governance_decisions counter seeded below existing max (% rows)', under_seeded;
    END IF;

    -- IDENTITY must be fully gone on all five columns — a leftover
    -- default would let legacy writes bypass the cooperative counter.
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE (table_name, column_name) IN
               (('social_aid_funds','fund_number'),
                ('social_aid_donations','donation_number'),
                ('social_aid_disbursements','disbursement_number'),
                ('governance_bodies','body_number'),
                ('governance_decisions','decision_number'))
           AND is_identity = 'YES'
    ) THEN
        RAISE EXCEPTION 'IDENTITY still present on adopted number columns';
    END IF;

    -- Terminal invariant: NO registered automatic series may still
    -- rely on IDENTITY anywhere in the schema.
    IF EXISTS (
        SELECT 1 FROM information_schema.columns c
          JOIN document_number_series s
            ON s.series_key = c.table_name || '.' || c.column_name
         WHERE c.is_identity = 'YES'
    ) THEN
        RAISE EXCEPTION 'a registered series still uses IDENTITY';
    END IF;
END $$;
