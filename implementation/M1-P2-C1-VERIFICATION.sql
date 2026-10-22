-- ============================================================================
-- M1-P2-C1 verification queries — READ-ONLY.
-- Run after migration 0021 on any environment. Every result set is a
-- pass/fail gate; expected values are noted per query.
-- ============================================================================

-- ---------------------------------------------------------------------------
-- V1. Schema expansion applied: all 37 business tables carry cooperative_id.
-- Expected: 37 rows.
-- ---------------------------------------------------------------------------
SELECT attrelid::regclass AS table_name
  FROM pg_attribute a
  JOIN pg_class c ON c.oid = a.attrelid AND c.relkind = 'r'
 WHERE a.attname = 'cooperative_id'
   AND NOT a.attisdropped
   AND c.relname IN (
        'persons','families','shareholders','shareholder_family_memberships',
        'shares','share_events','share_ownerships',
        'periods','assessment_rules','assessments','assessment_share_sources',
        'payments','payment_allocations',
        'financial_accounts','account_movements','account_transfers',
        'shareholder_credits','credit_applications',
        'financial_categories','income_entries','expense_entries',
        'share_returns','share_return_entitlements','share_return_settlements',
        'investments','investment_fundings','investment_valuations',
        'investment_incomes','investment_disposals',
        'investment_disposal_proceeds',
        'social_aid_funds','social_aid_donations','social_aid_disbursements',
        'governance_bodies','governance_memberships','governance_decisions',
        'governance_votes')
 ORDER BY 1;

-- ---------------------------------------------------------------------------
-- V2. Legacy UNIQUEs untouched: every pre-existing unique index is still
--     present and NOT prefaced by cooperative_id (the swap is C5's job).
-- Expected: 0 rows (nothing global got rescoped early).
-- ---------------------------------------------------------------------------
SELECT indexrelid::regclass AS index_name,
       indrelid::regclass   AS table_name
  FROM pg_index i
 WHERE indisunique
   AND indexrelid::regclass::text NOT LIKE '%_pkey'
   AND indexrelid::regclass::text NOT LIKE '%_coop_id_uq'
   AND indexrelid::regclass::text NOT LIKE '%_coop_pk_uq'
   AND indrelid::regclass::text IN (
        'persons','families','shareholders','shareholder_family_memberships',
        'shares','share_events','share_ownerships',
        'periods','assessment_rules','assessments','assessment_share_sources',
        'payments','payment_allocations',
        'financial_accounts','account_movements','account_transfers',
        'shareholder_credits','credit_applications',
        'financial_categories','income_entries','expense_entries',
        'share_returns','share_return_entitlements','share_return_settlements',
        'investments','investment_fundings','investment_valuations',
        'investment_incomes','investment_disposals',
        'investment_disposal_proceeds',
        'social_aid_funds','social_aid_donations','social_aid_disbursements',
        'governance_bodies','governance_memberships','governance_decisions',
        'governance_votes')
   AND (SELECT a.attname FROM pg_attribute a
         WHERE a.attrelid = i.indrelid AND a.attnum = i.indkey[0])
       = 'cooperative_id';

-- ---------------------------------------------------------------------------
-- V3. Support indexes exist: 36 conventional UNIQUE(cooperative_id, id)
--     plus the special assessment_share_sources key index
--     (cooperative_id, assessment_id, share_id) — that table's PK is
--     composite, it has no surrogate id.
-- Expected: 37 rows total (36 *_coop_id_uq + 1 *_coop_pk_uq),
--           all indisunique = true.
-- ---------------------------------------------------------------------------
SELECT indexrelid::regclass AS index_name, indrelid::regclass AS table_name,
       indisunique
  FROM pg_index
 WHERE indexrelid::regclass::text LIKE '%_coop_id_uq'
    OR indexrelid::regclass::text LIKE '%_coop_pk_uq'
 ORDER BY 1;

-- ---------------------------------------------------------------------------
-- V4. Ownership completeness (bootstrapped installations).
-- Expected: every row → unowned = 0. On NeverBootstrapped installs ALL
-- rows legitimately stay NULL — run V5 first to distinguish.
-- ---------------------------------------------------------------------------
SELECT 'persons' t, count(*) FILTER (WHERE cooperative_id IS NULL) unowned FROM persons
UNION ALL SELECT 'families',                        count(*) FILTER (WHERE cooperative_id IS NULL) FROM families
UNION ALL SELECT 'shareholders',                    count(*) FILTER (WHERE cooperative_id IS NULL) FROM shareholders
UNION ALL SELECT 'shareholder_family_memberships',  count(*) FILTER (WHERE cooperative_id IS NULL) FROM shareholder_family_memberships
UNION ALL SELECT 'shares',                          count(*) FILTER (WHERE cooperative_id IS NULL) FROM shares
UNION ALL SELECT 'share_events',                    count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_events
UNION ALL SELECT 'share_ownerships',                count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_ownerships
UNION ALL SELECT 'periods',                         count(*) FILTER (WHERE cooperative_id IS NULL) FROM periods
UNION ALL SELECT 'assessment_rules',                count(*) FILTER (WHERE cooperative_id IS NULL) FROM assessment_rules
UNION ALL SELECT 'assessments',                     count(*) FILTER (WHERE cooperative_id IS NULL) FROM assessments
UNION ALL SELECT 'assessment_share_sources',        count(*) FILTER (WHERE cooperative_id IS NULL) FROM assessment_share_sources
UNION ALL SELECT 'payments',                        count(*) FILTER (WHERE cooperative_id IS NULL) FROM payments
UNION ALL SELECT 'payment_allocations',             count(*) FILTER (WHERE cooperative_id IS NULL) FROM payment_allocations
UNION ALL SELECT 'financial_accounts',              count(*) FILTER (WHERE cooperative_id IS NULL) FROM financial_accounts
UNION ALL SELECT 'account_movements',               count(*) FILTER (WHERE cooperative_id IS NULL) FROM account_movements
UNION ALL SELECT 'account_transfers',               count(*) FILTER (WHERE cooperative_id IS NULL) FROM account_transfers
UNION ALL SELECT 'shareholder_credits',             count(*) FILTER (WHERE cooperative_id IS NULL) FROM shareholder_credits
UNION ALL SELECT 'credit_applications',             count(*) FILTER (WHERE cooperative_id IS NULL) FROM credit_applications
UNION ALL SELECT 'financial_categories',            count(*) FILTER (WHERE cooperative_id IS NULL) FROM financial_categories
UNION ALL SELECT 'income_entries',                  count(*) FILTER (WHERE cooperative_id IS NULL) FROM income_entries
UNION ALL SELECT 'expense_entries',                 count(*) FILTER (WHERE cooperative_id IS NULL) FROM expense_entries
UNION ALL SELECT 'share_returns',                   count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_returns
UNION ALL SELECT 'share_return_entitlements',       count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_return_entitlements
UNION ALL SELECT 'share_return_settlements',        count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_return_settlements
UNION ALL SELECT 'investments',                     count(*) FILTER (WHERE cooperative_id IS NULL) FROM investments
UNION ALL SELECT 'investment_fundings',             count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_fundings
UNION ALL SELECT 'investment_valuations',           count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_valuations
UNION ALL SELECT 'investment_incomes',              count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_incomes
UNION ALL SELECT 'investment_disposals',            count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_disposals
UNION ALL SELECT 'investment_disposal_proceeds',    count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_disposal_proceeds
UNION ALL SELECT 'social_aid_funds',                count(*) FILTER (WHERE cooperative_id IS NULL) FROM social_aid_funds
UNION ALL SELECT 'social_aid_donations',            count(*) FILTER (WHERE cooperative_id IS NULL) FROM social_aid_donations
UNION ALL SELECT 'social_aid_disbursements',        count(*) FILTER (WHERE cooperative_id IS NULL) FROM social_aid_disbursements
UNION ALL SELECT 'governance_bodies',               count(*) FILTER (WHERE cooperative_id IS NULL) FROM governance_bodies
UNION ALL SELECT 'governance_memberships',          count(*) FILTER (WHERE cooperative_id IS NULL) FROM governance_memberships
UNION ALL SELECT 'governance_decisions',            count(*) FILTER (WHERE cooperative_id IS NULL) FROM governance_decisions
UNION ALL SELECT 'governance_votes',                count(*) FILTER (WHERE cooperative_id IS NULL) FROM governance_votes;

-- ---------------------------------------------------------------------------
-- V5. Installation mode disambiguation.
-- bootstrap_cooperative_id NULL → NeverBootstrapped: NULL cooperative_id
-- everywhere is LEGAL until the operator claims via bootstrap-cooperative.
-- ---------------------------------------------------------------------------
SELECT bootstrap_cooperative_id, tenant_bootstrapped_at
  FROM installation_state;

-- ---------------------------------------------------------------------------
-- V6. FK reachability: every stamped cooperative_id resolves.
-- Expected: 0 rows.
-- ---------------------------------------------------------------------------
SELECT 'bad_coop_fk' check_, t.table_name, t.bad
  FROM (SELECT 'persons' table_name, count(*) bad
          FROM persons p
         WHERE p.cooperative_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM cooperatives c WHERE c.id = p.cooperative_id)) t
 WHERE t.bad > 0;
-- (repeat the EXISTS pattern per table as needed; schema guarantees it via
--  REFERENCES cooperatives(id) — this is a belt-and-suspenders read check)

-- ---------------------------------------------------------------------------
-- V7. C1 rollback blast radius — constraints/indexes depending on the
--     C1-created objects. Run BEFORE any rollback.
--
-- 7a. Constraints that depend on the 37 *_coop_id_uq support indexes
--     (composite FKs added by C2+ land here).
--     Expected: 0 rows pre-C2 → columns may be dropped directly.
--               >0 rows post-C2 → drop each listed constraint FIRST,
--               explicitly; never use DROP COLUMN ... CASCADE.
-- ---------------------------------------------------------------------------
SELECT DISTINCT
       con.conname                     AS dependent_constraint,
       con.conrelid::regclass          AS on_table,
       con.contype                     AS type_,
       idx.indexrelid::regclass        AS depends_on_index
  FROM pg_constraint con
  JOIN pg_depend     d   ON d.objid      = con.oid
                        AND d.classid    = 'pg_constraint'::regclass
  JOIN pg_index      idx ON idx.indexrelid = d.refobjid
                        AND d.refclassid = 'pg_class'::regclass
 WHERE idx.indexrelid::regclass::text LIKE '%_coop_id_uq'
    OR idx.indexrelid::regclass::text LIKE '%_coop_pk_uq'
 ORDER BY 1;

-- ---------------------------------------------------------------------------
-- 7b. FKs from business tables into cooperatives carried by the new columns
--     (informational — these are removed automatically with the column).
--     Expected: 37 rows (one per C1 column).
-- ---------------------------------------------------------------------------
SELECT con.conname, con.conrelid::regclass AS on_table
  FROM pg_constraint con
 WHERE con.contype = 'f'
   AND con.confrelid = 'cooperatives'::regclass
   AND con.conrelid::regclass::text IN (
        'persons','families','shareholders','shareholder_family_memberships',
        'shares','share_events','share_ownerships',
        'periods','assessment_rules','assessments','assessment_share_sources',
        'payments','payment_allocations',
        'financial_accounts','account_movements','account_transfers',
        'shareholder_credits','credit_applications',
        'financial_categories','income_entries','expense_entries',
        'share_returns','share_return_entitlements','share_return_settlements',
        'investments','investment_fundings','investment_valuations',
        'investment_incomes','investment_disposals',
        'investment_disposal_proceeds',
        'social_aid_funds','social_aid_donations','social_aid_disbursements',
        'governance_bodies','governance_memberships','governance_decisions',
        'governance_votes')
 ORDER BY 2;
