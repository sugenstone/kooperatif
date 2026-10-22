-- M1-P3-B4 — cooperative-local numbering adoption: income_entries,
-- expense_entries, share_returns, share_return_entitlements and
-- share_return_settlements (design:
-- implementation/M1-P3-NUMBERING-DESIGN.md §3.4, batch pattern of
-- 0027/0028).
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
-- share_returns.share_number is NOT a generated series — it is the
-- returned share's own business number copied for denormalized
-- display; only return_number is adopted here.
-- families.sequence_number stays manual.
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
         WHERE c.relname = 'income_entries_coop_income_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'income_entries_coop_income_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'expense_entries_coop_expense_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'expense_entries_coop_expense_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'share_returns_coop_return_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'share_returns_coop_return_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'share_return_entitlements_coop_entitlement_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'share_return_entitlements_coop_entitlement_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'share_return_settlements_coop_settlement_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'share_return_settlements_coop_settlement_number_uq missing/invalid';
    END IF;
    IF (SELECT count(*) FROM document_number_series
         WHERE series_key IN ('income_entries.income_number',
                              'expense_entries.expense_number',
                              'share_returns.return_number',
                              'share_return_entitlements.entitlement_number',
                              'share_return_settlements.settlement_number')) <> 5 THEN
        RAISE EXCEPTION 'numbering series registry incomplete';
    END IF;
END $$;

-- ------------------------------------------------------------------
-- income_entries.income_number
-- ------------------------------------------------------------------
LOCK TABLE income_entries IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN income_entries t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'income_entries.income_number'
           AND c.next_value <= t.income_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for income_entries.income_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'income_entries.income_number', MAX(income_number) + 1
  FROM income_entries
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE income_entries ALTER COLUMN income_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- expense_entries.expense_number
-- ------------------------------------------------------------------
LOCK TABLE expense_entries IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN expense_entries t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'expense_entries.expense_number'
           AND c.next_value <= t.expense_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for expense_entries.expense_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'expense_entries.expense_number', MAX(expense_number) + 1
  FROM expense_entries
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE expense_entries ALTER COLUMN expense_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- share_returns.return_number
-- ------------------------------------------------------------------
LOCK TABLE share_returns IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN share_returns t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'share_returns.return_number'
           AND c.next_value <= t.return_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for share_returns.return_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'share_returns.return_number', MAX(return_number) + 1
  FROM share_returns
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE share_returns ALTER COLUMN return_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- share_return_entitlements.entitlement_number
-- ------------------------------------------------------------------
LOCK TABLE share_return_entitlements IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN share_return_entitlements t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'share_return_entitlements.entitlement_number'
           AND c.next_value <= t.entitlement_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for share_return_entitlements.entitlement_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'share_return_entitlements.entitlement_number', MAX(entitlement_number) + 1
  FROM share_return_entitlements
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE share_return_entitlements ALTER COLUMN entitlement_number DROP IDENTITY;

-- ------------------------------------------------------------------
-- share_return_settlements.settlement_number
-- ------------------------------------------------------------------
LOCK TABLE share_return_settlements IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM cooperative_number_counters c
          JOIN share_return_settlements t ON t.cooperative_id = c.cooperative_id
         WHERE c.series_key = 'share_return_settlements.settlement_number'
           AND c.next_value <= t.settlement_number
    ) THEN
        RAISE EXCEPTION 'stale cooperative counter for share_return_settlements.settlement_number';
    END IF;
END $$;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'share_return_settlements.settlement_number', MAX(settlement_number) + 1
  FROM share_return_settlements
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

ALTER TABLE share_return_settlements ALTER COLUMN settlement_number DROP IDENTITY;

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
      JOIN income_entries t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'income_entries.income_number'
       AND c.next_value <= t.income_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'income_entries counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN expense_entries t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'expense_entries.expense_number'
       AND c.next_value <= t.expense_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'expense_entries counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN share_returns t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'share_returns.return_number'
       AND c.next_value <= t.return_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'share_returns counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN share_return_entitlements t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'share_return_entitlements.entitlement_number'
       AND c.next_value <= t.entitlement_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'share_return_entitlements counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN share_return_settlements t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'share_return_settlements.settlement_number'
       AND c.next_value <= t.settlement_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'share_return_settlements counter seeded below existing max (% rows)', under_seeded;
    END IF;

    -- IDENTITY must be fully gone on all five columns — a leftover
    -- default would let legacy writes bypass the cooperative counter.
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE (table_name, column_name) IN
               (('income_entries','income_number'),
                ('expense_entries','expense_number'),
                ('share_returns','return_number'),
                ('share_return_entitlements','entitlement_number'),
                ('share_return_settlements','settlement_number'))
           AND is_identity = 'YES'
    ) THEN
        RAISE EXCEPTION 'IDENTITY still present on adopted number columns';
    END IF;
END $$;
