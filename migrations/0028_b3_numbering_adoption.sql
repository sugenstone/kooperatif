-- M1-P3-B3 — cooperative-local numbering adoption: periods, shares and
-- shareholder_credits (design: implementation/M1-P3-NUMBERING-DESIGN.md §3.4,
-- batch pattern proven by 0027).
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
-- credit_applications carries NO numbering column (B3 discovery —
-- reported, not improvised). families.sequence_number stays manual.
--
-- Historical numbers are never rewritten; the IDENTITY sequences are
-- left in place (inert) for a later cleanup phase so rollback stays
-- trivial.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- Pre-check: scoped replacement uniques must exist (0024) and the
-- allocator registry rows for all three series must exist (0026).
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'periods_coop_period_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'periods_coop_period_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'shares_coop_share_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'shares_coop_share_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'shareholder_credits_coop_credit_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'shareholder_credits_coop_credit_number_uq missing/invalid';
    END IF;
    IF (SELECT count(*) FROM document_number_series
         WHERE series_key IN ('periods.period_number',
                              'shares.share_number',
                              'shareholder_credits.credit_number')) <> 3 THEN
        RAISE EXCEPTION 'numbering series registry incomplete';
    END IF;
END $$;

-- ------------------------------------------------------------------
-- periods.period_number
-- ------------------------------------------------------------------
LOCK TABLE periods IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN periods p ON p.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'periods.period_number'
           AND c.next_value <= p.period_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for periods.period_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'periods.period_number', MAX(period_number) + 1
  FROM periods
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE periods ALTER COLUMN period_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- shares.share_number
-- ------------------------------------------------------------------
LOCK TABLE shares IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN shares s ON s.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'shares.share_number'
           AND c.next_value <= s.share_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for shares.share_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'shares.share_number', MAX(share_number) + 1
  FROM shares
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE shares ALTER COLUMN share_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- shareholder_credits.credit_number
-- ------------------------------------------------------------------
LOCK TABLE shareholder_credits IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN shareholder_credits sc ON sc.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'shareholder_credits.credit_number'
           AND c.next_value <= sc.credit_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for shareholder_credits.credit_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'shareholder_credits.credit_number', MAX(credit_number) + 1
  FROM shareholder_credits
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE shareholder_credits ALTER COLUMN credit_number DROP IDENTITY;

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
      JOIN periods p ON p.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'periods.period_number'
       AND c.next_value <= p.period_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'periods counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN shares s ON s.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'shares.share_number'
       AND c.next_value <= s.share_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'shares counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN shareholder_credits sc ON sc.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'shareholder_credits.credit_number'
       AND c.next_value <= sc.credit_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'shareholder_credits counter seeded below existing max (% rows)', under_seeded;
    END IF;

    -- IDENTITY must be fully gone on all three columns — a leftover
    -- default would let legacy writes bypass the cooperative counter.
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE (table_name, column_name) IN
               (('periods','period_number'),('shares','share_number'),
                ('shareholder_credits','credit_number'))
           AND is_identity = 'YES'
    ) THEN
        RAISE EXCEPTION 'IDENTITY still present on adopted number columns';
    END IF;
END $$;
