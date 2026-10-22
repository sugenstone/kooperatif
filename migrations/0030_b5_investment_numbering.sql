-- M1-P3-B5 — cooperative-local numbering adoption: the five investment
-- series (design: implementation/M1-P3-NUMBERING-DESIGN.md §3.4, batch
-- pattern of 0027/0028/0029).
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
-- investment_disposal_proceeds carries NO numbering column (a proceeds
-- leg bound to its disposal) — not a series. families.sequence_number
-- stays manual.
--
-- Historical numbers are never rewritten; the IDENTITY sequences are
-- left in place (inert) for a later cleanup phase so rollback stays
-- trivial.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- Pre-check: scoped replacement uniques must exist (0024) and the
-- allocator registry rows for all five series must exist (0026).
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'investments_coop_investment_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'investments_coop_investment_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'investment_fundings_coop_funding_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'investment_fundings_coop_funding_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'investment_valuations_coop_valuation_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'investment_valuations_coop_valuation_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'investment_incomes_coop_income_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'investment_incomes_coop_income_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'investment_disposals_coop_disposal_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'investment_disposals_coop_disposal_number_uq missing/invalid';
    END IF;
    IF (SELECT count(*) FROM document_number_series
         WHERE series_key IN ('investments.investment_number',
                              'investment_fundings.funding_number',
                              'investment_valuations.valuation_number',
                              'investment_incomes.income_number',
                              'investment_disposals.disposal_number')) <> 5 THEN
        RAISE EXCEPTION 'numbering series registry incomplete';
    END IF;
END $$;

-- ------------------------------------------------------------------
-- investments.investment_number
-- ------------------------------------------------------------------
LOCK TABLE investments IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN investments t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'investments.investment_number'
           AND c.next_value <= t.investment_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for investments.investment_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'investments.investment_number', MAX(investment_number) + 1
  FROM investments
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE investments ALTER COLUMN investment_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- investment_fundings.funding_number
-- ------------------------------------------------------------------
LOCK TABLE investment_fundings IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN investment_fundings t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'investment_fundings.funding_number'
           AND c.next_value <= t.funding_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for investment_fundings.funding_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'investment_fundings.funding_number', MAX(funding_number) + 1
  FROM investment_fundings
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE investment_fundings ALTER COLUMN funding_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- investment_valuations.valuation_number
-- ------------------------------------------------------------------
LOCK TABLE investment_valuations IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN investment_valuations t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'investment_valuations.valuation_number'
           AND c.next_value <= t.valuation_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for investment_valuations.valuation_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'investment_valuations.valuation_number', MAX(valuation_number) + 1
  FROM investment_valuations
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE investment_valuations ALTER COLUMN valuation_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- investment_incomes.income_number
-- ------------------------------------------------------------------
LOCK TABLE investment_incomes IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN investment_incomes t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'investment_incomes.income_number'
           AND c.next_value <= t.income_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for investment_incomes.income_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'investment_incomes.income_number', MAX(income_number) + 1
  FROM investment_incomes
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE investment_incomes ALTER COLUMN income_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- investment_disposals.disposal_number
-- ------------------------------------------------------------------
LOCK TABLE investment_disposals IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN investment_disposals t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'investment_disposals.disposal_number'
           AND c.next_value <= t.disposal_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for investment_disposals.disposal_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'investment_disposals.disposal_number', MAX(disposal_number) + 1
  FROM investment_disposals
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE investment_disposals ALTER COLUMN disposal_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- Post-checks (fail-closed, whole migration rolls back on violation)
-- ------------------------------------------------------------------
DO $$
DECLARE
    under_seeded integer;
BEGIN
    -- Every counter must sit strictly above its cooperative's
    -- existing maximum — a counter at or below it would reissue a
    -- live document number.
    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN investments t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'investments.investment_number'
       AND c.next_value <= t.investment_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'investments counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN investment_fundings t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'investment_fundings.funding_number'
       AND c.next_value <= t.funding_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'investment_fundings counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN investment_valuations t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'investment_valuations.valuation_number'
       AND c.next_value <= t.valuation_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'investment_valuations counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN investment_incomes t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'investment_incomes.income_number'
       AND c.next_value <= t.income_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'investment_incomes counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN investment_disposals t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'investment_disposals.disposal_number'
       AND c.next_value <= t.disposal_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'investment_disposals counter seeded below existing max (% rows)', under_seeded;
    END IF;

    -- IDENTITY must be fully gone on all five columns — a leftover
    -- default would let legacy writes bypass the cooperative counter.
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE (table_name, column_name) IN
               (('investments','investment_number'),
                ('investment_fundings','funding_number'),
                ('investment_valuations','valuation_number'),
                ('investment_incomes','income_number'),
                ('investment_disposals','disposal_number'))
           AND is_identity = 'YES'
    ) THEN
        RAISE EXCEPTION 'IDENTITY still present on adopted number columns';
    END IF;
END $$;
