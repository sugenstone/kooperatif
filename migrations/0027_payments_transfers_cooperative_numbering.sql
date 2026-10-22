-- M1-P3-B2 — cooperative-local numbering adoption: payments and
-- account_transfers (design: implementation/M1-P3-NUMBERING-DESIGN.md §3.4).
--
-- For each adopted series this migration, inside ONE transaction:
--
--   * takes AccessExclusive locks on the business table first, so no
--     concurrent legacy writer can slip a row between IDENTITY removal
--     and counter seeding;
--   * DROPs IDENTITY from the number column — after this point any
--     INSERT that does not supply the number fails closed on NOT NULL,
--     which structurally prevents mixed IDENTITY/counter allocation;
--   * seeds cooperative_number_counters at MAX(number)+1 per
--     cooperative — existing numbers are preserved verbatim, nothing
--     is ever renumbered;
--   * verifies every seeded counter is above its cooperative's
--     existing maximum (fail-closed).
--
-- account_movements is NOT a numbering series (no number column —
-- identified in B2 discovery, reported, not improvised).
--
-- Historical numbers are never rewritten; the IDENTITY sequences are
-- left in place (inert) for a later cleanup phase so rollback stays
-- trivial.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- Pre-check: scoped replacement uniques must exist (0024) and the
-- allocator registry rows for both series must exist (0026).
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'payments_coop_payment_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'payments_coop_payment_number_uq missing/invalid';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = 'account_transfers_coop_transfer_number_uq' AND i.indisvalid
    ) THEN
        RAISE EXCEPTION 'account_transfers_coop_transfer_number_uq missing/invalid';
    END IF;
    IF (SELECT count(*) FROM document_number_series
         WHERE series_key IN ('payments.payment_number',
                              'account_transfers.transfer_number')) <> 2 THEN
        RAISE EXCEPTION 'numbering series registry incomplete';
    END IF;
END $$;

-- ------------------------------------------------------------------
-- payments.payment_number
-- ------------------------------------------------------------------
LOCK TABLE payments IN ACCESS EXCLUSIVE MODE;
ALTER TABLE payments ALTER COLUMN payment_number DROP IDENTITY;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'payments.payment_number', MAX(payment_number) + 1
  FROM payments
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

-- ------------------------------------------------------------------
-- account_transfers.transfer_number
-- ------------------------------------------------------------------
LOCK TABLE account_transfers IN ACCESS EXCLUSIVE MODE;
ALTER TABLE account_transfers ALTER COLUMN transfer_number DROP IDENTITY;

INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value)
SELECT cooperative_id, 'account_transfers.transfer_number', MAX(transfer_number) + 1
  FROM account_transfers
 GROUP BY cooperative_id
ON CONFLICT (cooperative_id, series_key) DO NOTHING;

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
      JOIN payments p ON p.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'payments.payment_number'
       AND c.next_value <= p.payment_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'payments counter seeded below existing max (% rows)', under_seeded;
    END IF;

    SELECT count(*) INTO under_seeded
      FROM cooperative_number_counters c
      JOIN account_transfers t ON t.cooperative_id = c.cooperative_id
     WHERE c.series_key = 'account_transfers.transfer_number'
       AND c.next_value <= t.transfer_number;
    IF under_seeded > 0 THEN
        RAISE EXCEPTION 'account_transfers counter seeded below existing max (% rows)', under_seeded;
    END IF;

    -- IDENTITY must be fully gone on both columns — a leftover
    -- default would let legacy writes bypass the cooperative counter.
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE (table_name, column_name) IN
               (('payments','payment_number'),('account_transfers','transfer_number'))
           AND is_identity = 'YES'
    ) THEN
        RAISE EXCEPTION 'IDENTITY still present on adopted number columns';
    END IF;
END $$;
