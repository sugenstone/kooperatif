-- ============================================================================
-- M1-P2-C4 verification queries — READ-ONLY.
-- Data-integrity validation and C5 readiness audit for cooperative-scoped
-- business data. Run on a bootstrapped database after migrations 0021–0023.
-- Every section is a pass/fail gate; expected values are noted per query.
-- The same checks are executed programmatically by
-- apps/server/tests/c4_integrity.rs against a populated disposable
-- two-cooperative database.
-- ============================================================================

-- ---------------------------------------------------------------------------
-- C4-01  Schema inventory: all 37 business tables carry cooperative_id.
-- Expected: exactly 37 rows.
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
-- C4-02  NULL cooperative_id inventory per business table.
-- Expected (bootstrapped installation): every *_null = 0.
-- *_total is informational (row inventory for the C5 readiness report).
-- ---------------------------------------------------------------------------
SELECT 'persons'                          AS t, count(*) AS total, count(*) FILTER (WHERE cooperative_id IS NULL) AS nulls FROM persons
UNION ALL SELECT 'families',                          count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM families
UNION ALL SELECT 'shareholders',                      count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM shareholders
UNION ALL SELECT 'shareholder_family_memberships',    count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM shareholder_family_memberships
UNION ALL SELECT 'shares',                            count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM shares
UNION ALL SELECT 'share_events',                      count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_events
UNION ALL SELECT 'share_ownerships',                  count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_ownerships
UNION ALL SELECT 'periods',                           count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM periods
UNION ALL SELECT 'assessment_rules',                  count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM assessment_rules
UNION ALL SELECT 'assessments',                       count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM assessments
UNION ALL SELECT 'assessment_share_sources',          count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM assessment_share_sources
UNION ALL SELECT 'payments',                          count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM payments
UNION ALL SELECT 'payment_allocations',               count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM payment_allocations
UNION ALL SELECT 'financial_accounts',                count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM financial_accounts
UNION ALL SELECT 'account_movements',                 count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM account_movements
UNION ALL SELECT 'account_transfers',                 count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM account_transfers
UNION ALL SELECT 'shareholder_credits',               count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM shareholder_credits
UNION ALL SELECT 'credit_applications',               count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM credit_applications
UNION ALL SELECT 'financial_categories',              count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM financial_categories
UNION ALL SELECT 'income_entries',                    count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM income_entries
UNION ALL SELECT 'expense_entries',                   count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM expense_entries
UNION ALL SELECT 'share_returns',                     count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_returns
UNION ALL SELECT 'share_return_entitlements',         count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_return_entitlements
UNION ALL SELECT 'share_return_settlements',          count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM share_return_settlements
UNION ALL SELECT 'investments',                       count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM investments
UNION ALL SELECT 'investment_fundings',               count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_fundings
UNION ALL SELECT 'investment_valuations',             count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_valuations
UNION ALL SELECT 'investment_incomes',                count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_incomes
UNION ALL SELECT 'investment_disposals',              count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_disposals
UNION ALL SELECT 'investment_disposal_proceeds',      count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM investment_disposal_proceeds
UNION ALL SELECT 'social_aid_funds',                  count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM social_aid_funds
UNION ALL SELECT 'social_aid_donations',              count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM social_aid_donations
UNION ALL SELECT 'social_aid_disbursements',          count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM social_aid_disbursements
UNION ALL SELECT 'governance_bodies',                 count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM governance_bodies
UNION ALL SELECT 'governance_memberships',            count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM governance_memberships
UNION ALL SELECT 'governance_decisions',              count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM governance_decisions
UNION ALL SELECT 'governance_votes',                  count(*), count(*) FILTER (WHERE cooperative_id IS NULL) FROM governance_votes
ORDER BY t;

-- ---------------------------------------------------------------------------
-- C4-03  Orphan ownership: stamped cooperative_id must resolve to a live
--        cooperatives row. Expected: every count = 0.
-- ---------------------------------------------------------------------------
SELECT 'persons' AS t, count(*) AS orphans FROM persons x
  LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'families', count(*) FROM families x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'shareholders', count(*) FROM shareholders x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'shareholder_family_memberships', count(*) FROM shareholder_family_memberships x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'shares', count(*) FROM shares x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'share_events', count(*) FROM share_events x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'share_ownerships', count(*) FROM share_ownerships x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'periods', count(*) FROM periods x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'assessment_rules', count(*) FROM assessment_rules x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'assessments', count(*) FROM assessments x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'assessment_share_sources', count(*) FROM assessment_share_sources x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'payments', count(*) FROM payments x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'payment_allocations', count(*) FROM payment_allocations x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'financial_accounts', count(*) FROM financial_accounts x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'account_movements', count(*) FROM account_movements x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'account_transfers', count(*) FROM account_transfers x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'shareholder_credits', count(*) FROM shareholder_credits x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'credit_applications', count(*) FROM credit_applications x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'financial_categories', count(*) FROM financial_categories x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'income_entries', count(*) FROM income_entries x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'expense_entries', count(*) FROM expense_entries x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'share_returns', count(*) FROM share_returns x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'share_return_entitlements', count(*) FROM share_return_entitlements x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'share_return_settlements', count(*) FROM share_return_settlements x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'investments', count(*) FROM investments x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'investment_fundings', count(*) FROM investment_fundings x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'investment_valuations', count(*) FROM investment_valuations x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'investment_incomes', count(*) FROM investment_incomes x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'investment_disposals', count(*) FROM investment_disposals x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'investment_disposal_proceeds', count(*) FROM investment_disposal_proceeds x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'social_aid_funds', count(*) FROM social_aid_funds x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'social_aid_donations', count(*) FROM social_aid_donations x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'social_aid_disbursements', count(*) FROM social_aid_disbursements x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'governance_bodies', count(*) FROM governance_bodies x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'governance_memberships', count(*) FROM governance_memberships x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'governance_decisions', count(*) FROM governance_decisions x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
UNION ALL
SELECT 'governance_votes', count(*) FROM governance_votes x LEFT JOIN cooperatives c ON c.id = x.cooperative_id
 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL
 ORDER BY t;

-- ---------------------------------------------------------------------------
-- C4-04  Composite cooperative foreign keys (migration 0022):
--        66 *_coop_fk constraints, every one VALIDATED by 0023.
-- Expected: 66 rows, convalidated = true for all.
-- ---------------------------------------------------------------------------
SELECT con.conname, cl.relname AS table_name, con.convalidated
  FROM pg_constraint con
  JOIN pg_class cl ON cl.oid = con.conrelid
 WHERE con.contype = 'f'
   AND con.conname LIKE '%\_coop\_fk' ESCAPE '\'
 ORDER BY cl.relname, con.conname;

-- Expected companion: NOT VALID leftovers must be empty.
SELECT con.conname, cl.relname AS table_name
  FROM pg_constraint con
  JOIN pg_class cl ON cl.oid = con.conrelid
 WHERE con.contype = 'f'
   AND con.conname LIKE '%\_coop\_fk' ESCAPE '\'
   AND NOT con.convalidated;

-- ---------------------------------------------------------------------------
-- C4-05  Cross-cooperative relationship probes. The composite FKs of C2
--        already reject these at write time; these read-only counts are the
--        verification evidence. Expected: every count = 0.
-- ---------------------------------------------------------------------------
SELECT 'payment_allocations↔payments' AS rel, count(*) AS violations
  FROM payment_allocations pa
  JOIN payments p ON p.id = pa.payment_id
 WHERE p.cooperative_id IS DISTINCT FROM pa.cooperative_id
UNION ALL
SELECT 'payment_allocations↔assessments', count(*)
  FROM payment_allocations pa
  JOIN assessments a ON a.id = pa.assessment_id
 WHERE a.cooperative_id IS DISTINCT FROM pa.cooperative_id
UNION ALL
SELECT 'assessments↔periods', count(*)
  FROM assessments a JOIN periods pe ON pe.id = a.period_id
 WHERE pe.cooperative_id IS DISTINCT FROM a.cooperative_id
UNION ALL
SELECT 'assessments↔shareholders', count(*)
  FROM assessments a JOIN shareholders s ON s.id = a.shareholder_id
 WHERE s.cooperative_id IS DISTINCT FROM a.cooperative_id
UNION ALL
SELECT 'payments↔payer_persons', count(*)
  FROM payments p JOIN persons ps ON ps.id = p.payer_person_id
 WHERE p.payer_person_id IS NOT NULL
   AND ps.cooperative_id IS DISTINCT FROM p.cooperative_id
UNION ALL
SELECT 'payments↔destination_accounts', count(*)
  FROM payments p JOIN financial_accounts fa ON fa.id = p.destination_account_id
 WHERE p.destination_account_id IS NOT NULL
   AND fa.cooperative_id IS DISTINCT FROM p.cooperative_id
UNION ALL
SELECT 'account_movements↔accounts', count(*)
  FROM account_movements m JOIN financial_accounts fa ON fa.id = m.account_id
 WHERE fa.cooperative_id IS DISTINCT FROM m.cooperative_id
UNION ALL
SELECT 'transfers↔source_accounts', count(*)
  FROM account_transfers t JOIN financial_accounts fa ON fa.id = t.source_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM t.cooperative_id
UNION ALL
SELECT 'transfers↔destination_accounts', count(*)
  FROM account_transfers t JOIN financial_accounts fa ON fa.id = t.destination_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM t.cooperative_id
UNION ALL
SELECT 'credits↔shareholders', count(*)
  FROM shareholder_credits c JOIN shareholders s ON s.id = c.shareholder_id
 WHERE s.cooperative_id IS DISTINCT FROM c.cooperative_id
UNION ALL
SELECT 'credits↔source_payments', count(*)
  FROM shareholder_credits c JOIN payments p ON p.id = c.source_payment_id
 WHERE c.source_payment_id IS NOT NULL
   AND p.cooperative_id IS DISTINCT FROM c.cooperative_id
UNION ALL
SELECT 'credit_applications↔credits', count(*)
  FROM credit_applications ca JOIN shareholder_credits c ON c.id = ca.credit_id
 WHERE c.cooperative_id IS DISTINCT FROM ca.cooperative_id
UNION ALL
SELECT 'credit_applications↔assessments', count(*)
  FROM credit_applications ca JOIN assessments a ON a.id = ca.assessment_id
 WHERE a.cooperative_id IS DISTINCT FROM ca.cooperative_id
UNION ALL
SELECT 'income_entries↔accounts', count(*)
  FROM income_entries e JOIN financial_accounts fa ON fa.id = e.financial_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM e.cooperative_id
UNION ALL
SELECT 'income_entries↔categories', count(*)
  FROM income_entries e JOIN financial_categories fc ON fc.id = e.category_id
 WHERE fc.cooperative_id IS DISTINCT FROM e.cooperative_id
UNION ALL
SELECT 'expense_entries↔accounts', count(*)
  FROM expense_entries e JOIN financial_accounts fa ON fa.id = e.financial_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM e.cooperative_id
UNION ALL
SELECT 'expense_entries↔categories', count(*)
  FROM expense_entries e JOIN financial_categories fc ON fc.id = e.category_id
 WHERE fc.cooperative_id IS DISTINCT FROM e.cooperative_id
UNION ALL
SELECT 'movements_income↔entries', count(*)
  FROM account_movements m JOIN income_entries e ON e.account_movement_id = m.id
 WHERE e.cooperative_id IS DISTINCT FROM m.cooperative_id
UNION ALL
SELECT 'movements_expense↔entries', count(*)
  FROM account_movements m JOIN expense_entries e ON e.account_movement_id = m.id
 WHERE e.cooperative_id IS DISTINCT FROM m.cooperative_id
UNION ALL
SELECT 'share_events↔shares', count(*)
  FROM share_events ev JOIN shares s ON s.id = ev.share_id
 WHERE s.cooperative_id IS DISTINCT FROM ev.cooperative_id
UNION ALL
SELECT 'share_ownerships↔shareholders', count(*)
  FROM share_ownerships o JOIN shareholders s ON s.id = o.shareholder_id
 WHERE s.cooperative_id IS DISTINCT FROM o.cooperative_id
UNION ALL
SELECT 'share_ownerships↔shares', count(*)
  FROM share_ownerships o JOIN shares s ON s.id = o.share_id
 WHERE s.cooperative_id IS DISTINCT FROM o.cooperative_id
UNION ALL
SELECT 'family_memberships↔shareholders', count(*)
  FROM shareholder_family_memberships fm JOIN shareholders s ON s.id = fm.shareholder_id
 WHERE s.cooperative_id IS DISTINCT FROM fm.cooperative_id
UNION ALL
SELECT 'family_memberships↔families', count(*)
  FROM shareholder_family_memberships fm JOIN families f ON f.id = fm.family_id
 WHERE f.cooperative_id IS DISTINCT FROM fm.cooperative_id
UNION ALL
SELECT 'shareholders↔persons', count(*)
  FROM shareholders s JOIN persons p ON p.id = s.person_id
 WHERE p.cooperative_id IS DISTINCT FROM s.cooperative_id
UNION ALL
SELECT 'shareholders↔guardian_persons', count(*)
  FROM shareholders s JOIN persons p ON p.id = s.guardian_person_id
 WHERE s.guardian_person_id IS NOT NULL
   AND p.cooperative_id IS DISTINCT FROM s.cooperative_id
UNION ALL
SELECT 'shareholders↔default_accounts', count(*)
  FROM shareholders s JOIN financial_accounts fa ON fa.id = s.default_collection_account_id
 WHERE s.default_collection_account_id IS NOT NULL
   AND fa.cooperative_id IS DISTINCT FROM s.cooperative_id
UNION ALL
SELECT 'share_returns↔shareholders', count(*)
  FROM share_returns r JOIN shareholders s ON s.id = r.shareholder_id
 WHERE s.cooperative_id IS DISTINCT FROM r.cooperative_id
UNION ALL
SELECT 'entitlements↔returns', count(*)
  FROM share_return_entitlements e JOIN share_returns r ON r.id = e.share_return_id
 WHERE r.cooperative_id IS DISTINCT FROM e.cooperative_id
UNION ALL
SELECT 'settlements↔entitlements', count(*)
  FROM share_return_settlements st JOIN share_return_entitlements e ON e.id = st.entitlement_id
 WHERE e.cooperative_id IS DISTINCT FROM st.cooperative_id
UNION ALL
SELECT 'settlements↔accounts', count(*)
  FROM share_return_settlements st JOIN financial_accounts fa ON fa.id = st.financial_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM st.cooperative_id
UNION ALL
SELECT 'fundings↔investments', count(*)
  FROM investment_fundings f JOIN investments i ON i.id = f.investment_id
 WHERE i.cooperative_id IS DISTINCT FROM f.cooperative_id
UNION ALL
SELECT 'fundings↔accounts', count(*)
  FROM investment_fundings f JOIN financial_accounts fa ON fa.id = f.financial_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM f.cooperative_id
UNION ALL
SELECT 'valuations↔investments', count(*)
  FROM investment_valuations v JOIN investments i ON i.id = v.investment_id
 WHERE i.cooperative_id IS DISTINCT FROM v.cooperative_id
UNION ALL
SELECT 'inv_incomes↔investments', count(*)
  FROM investment_incomes n JOIN investments i ON i.id = n.investment_id
 WHERE i.cooperative_id IS DISTINCT FROM n.cooperative_id
UNION ALL
SELECT 'disposals↔investments', count(*)
  FROM investment_disposals d JOIN investments i ON i.id = d.investment_id
 WHERE i.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'proceeds↔disposals', count(*)
  FROM investment_disposal_proceeds pr JOIN investment_disposals d ON d.id = pr.disposal_id
 WHERE d.cooperative_id IS DISTINCT FROM pr.cooperative_id
UNION ALL
SELECT 'proceeds↔accounts', count(*)
  FROM investment_disposal_proceeds pr JOIN financial_accounts fa ON fa.id = pr.financial_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM pr.cooperative_id
UNION ALL
SELECT 'donations↔funds', count(*)
  FROM social_aid_donations d JOIN social_aid_funds f ON f.id = d.fund_id
 WHERE f.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'donations↔donor_persons', count(*)
  FROM social_aid_donations d JOIN persons p ON p.id = d.donor_person_id
 WHERE d.donor_person_id IS NOT NULL
   AND p.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'donations↔accounts', count(*)
  FROM social_aid_donations d JOIN financial_accounts fa ON fa.id = d.financial_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'disbursements↔funds', count(*)
  FROM social_aid_disbursements d JOIN social_aid_funds f ON f.id = d.fund_id
 WHERE f.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'disbursements↔beneficiary_persons', count(*)
  FROM social_aid_disbursements d JOIN persons p ON p.id = d.beneficiary_person_id
 WHERE d.beneficiary_person_id IS NOT NULL
   AND p.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'disbursements↔accounts', count(*)
  FROM social_aid_disbursements d JOIN financial_accounts fa ON fa.id = d.financial_account_id
 WHERE fa.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'memberships↔bodies', count(*)
  FROM governance_memberships m JOIN governance_bodies b ON b.id = m.body_id
 WHERE b.cooperative_id IS DISTINCT FROM m.cooperative_id
UNION ALL
SELECT 'memberships↔persons', count(*)
  FROM governance_memberships m JOIN persons p ON p.id = m.person_id
 WHERE p.cooperative_id IS DISTINCT FROM m.cooperative_id
UNION ALL
SELECT 'decisions↔bodies', count(*)
  FROM governance_decisions d JOIN governance_bodies b ON b.id = d.body_id
 WHERE b.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'votes↔decisions', count(*)
  FROM governance_votes v JOIN governance_decisions d ON d.id = v.decision_id
 WHERE d.cooperative_id IS DISTINCT FROM v.cooperative_id
UNION ALL
SELECT 'votes↔memberships', count(*)
  FROM governance_votes v JOIN governance_memberships m ON m.id = v.membership_id
 WHERE v.membership_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM v.cooperative_id
UNION ALL
SELECT 'votes↔persons', count(*)
  FROM governance_votes v JOIN persons p ON p.id = v.person_id
 WHERE p.cooperative_id IS DISTINCT FROM v.cooperative_id
 ORDER BY rel;

-- ---------------------------------------------------------------------------
-- C4-06  Polymorphic account_movements source ownership: every movement's
--        source row must exist AND live in the same cooperative.
--        Expected: 0 violations per source type, 0 dangling sources.
-- ---------------------------------------------------------------------------
SELECT 'payment' AS src, count(*) FILTER (WHERE p.id IS NULL) AS dangling,
       count(*) FILTER (WHERE p.id IS NOT NULL
                        AND p.cooperative_id IS DISTINCT FROM m.cooperative_id) AS cross_coop
  FROM account_movements m
  LEFT JOIN payments p ON p.id = m.source_id
 WHERE m.source_type = 'payment'
UNION ALL
SELECT 'transfer', count(*) FILTER (WHERE t.id IS NULL),
       count(*) FILTER (WHERE t.id IS NOT NULL
                        AND t.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN account_transfers t ON t.id = m.source_id
 WHERE m.source_type = 'transfer'
UNION ALL
SELECT 'income', count(*) FILTER (WHERE e.id IS NULL),
       count(*) FILTER (WHERE e.id IS NOT NULL
                        AND e.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN income_entries e ON e.id = m.source_id
 WHERE m.source_type = 'income'
UNION ALL
SELECT 'expense', count(*) FILTER (WHERE e.id IS NULL),
       count(*) FILTER (WHERE e.id IS NOT NULL
                        AND e.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN expense_entries e ON e.id = m.source_id
 WHERE m.source_type = 'expense'
UNION ALL
SELECT 'share_return_settlement', count(*) FILTER (WHERE s.id IS NULL),
       count(*) FILTER (WHERE s.id IS NOT NULL
                        AND s.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN share_return_settlements s ON s.id = m.source_id
 WHERE m.source_type = 'share_return_settlement'
UNION ALL
SELECT 'investment_funding', count(*) FILTER (WHERE f.id IS NULL),
       count(*) FILTER (WHERE f.id IS NOT NULL
                        AND f.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN investment_fundings f ON f.id = m.source_id
 WHERE m.source_type = 'investment_funding'
UNION ALL
SELECT 'investment_income', count(*) FILTER (WHERE n.id IS NULL),
       count(*) FILTER (WHERE n.id IS NOT NULL
                        AND n.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN investment_incomes n ON n.id = m.source_id
 WHERE m.source_type = 'investment_income'
UNION ALL
SELECT 'investment_disposal', count(*) FILTER (WHERE pr.id IS NULL),
       count(*) FILTER (WHERE pr.id IS NOT NULL
                        AND pr.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN investment_disposal_proceeds pr ON pr.id = m.source_id
 WHERE m.source_type = 'investment_disposal'
UNION ALL
SELECT 'social_aid_donation', count(*) FILTER (WHERE d.id IS NULL),
       count(*) FILTER (WHERE d.id IS NOT NULL
                        AND d.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN social_aid_donations d ON d.id = m.source_id
 WHERE m.source_type = 'social_aid_donation'
UNION ALL
SELECT 'social_aid_disbursement', count(*) FILTER (WHERE d.id IS NULL),
       count(*) FILTER (WHERE d.id IS NOT NULL
                        AND d.cooperative_id IS DISTINCT FROM m.cooperative_id)
  FROM account_movements m
  LEFT JOIN social_aid_disbursements d ON d.id = m.source_id
 WHERE m.source_type = 'social_aid_disbursement';

-- ---------------------------------------------------------------------------
-- C4-07  Duplicate identifier scan: human-facing numbers and idempotency
--        keys must stay globally unique until C5 rescopes them.
--        Expected: every duplicates = 0.
--        NULLs are excluded: auto-apply stamps the key on the first row
--        only; uniqueness applies to non-NULL values.
-- ---------------------------------------------------------------------------
SELECT 'families.sequence_number' AS identifier, count(*) FILTER (WHERE sequence_number IS NOT NULL) - count(DISTINCT sequence_number) AS duplicates
  FROM families
UNION ALL SELECT 'periods.period_number',            count(*) - count(DISTINCT period_number)            FROM periods
UNION ALL SELECT 'payments.payment_number',          count(*) - count(DISTINCT payment_number)           FROM payments
UNION ALL SELECT 'payments.idempotency_key',         count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM payments
UNION ALL SELECT 'account_transfers.transfer_number',count(*) - count(DISTINCT transfer_number)          FROM account_transfers
UNION ALL SELECT 'account_transfers.idem_key',       count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM account_transfers
UNION ALL SELECT 'shareholder_credits.credit_number',count(*) - count(DISTINCT credit_number)            FROM shareholder_credits
UNION ALL SELECT 'shareholder_credits.idem_key',     count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM shareholder_credits
UNION ALL SELECT 'credit_applications.idem_key',     count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM credit_applications
UNION ALL SELECT 'shares.share_number',              count(*) - count(DISTINCT share_number)             FROM shares
UNION ALL SELECT 'income_entries.income_number',     count(*) - count(DISTINCT income_number)            FROM income_entries
UNION ALL SELECT 'income_entries.idem_key',          count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM income_entries
UNION ALL SELECT 'expense_entries.expense_number',   count(*) - count(DISTINCT expense_number)           FROM expense_entries
UNION ALL SELECT 'expense_entries.idem_key',         count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM expense_entries
UNION ALL SELECT 'share_returns.return_number',      count(*) - count(DISTINCT return_number)            FROM share_returns
UNION ALL SELECT 'share_returns.idem_key',           count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM share_returns
UNION ALL SELECT 'share_return_entitlements.entitlement_number', count(*) - count(DISTINCT entitlement_number) FROM share_return_entitlements
UNION ALL SELECT 'share_return_settlements.number',  count(*) - count(DISTINCT settlement_number)        FROM share_return_settlements
UNION ALL SELECT 'share_return_settlements.idem_key',count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM share_return_settlements
UNION ALL SELECT 'investments.investment_number',    count(*) - count(DISTINCT investment_number)        FROM investments
UNION ALL SELECT 'investments.idem_key',             count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM investments
UNION ALL SELECT 'investment_fundings.number',       count(*) - count(DISTINCT funding_number)           FROM investment_fundings
UNION ALL SELECT 'investment_fundings.idem_key',     count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM investment_fundings
UNION ALL SELECT 'investment_valuations.number',     count(*) - count(DISTINCT valuation_number)         FROM investment_valuations
UNION ALL SELECT 'investment_valuations.idem_key',   count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM investment_valuations
UNION ALL SELECT 'investment_incomes.number',        count(*) - count(DISTINCT income_number)            FROM investment_incomes
UNION ALL SELECT 'investment_incomes.idem_key',      count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM investment_incomes
UNION ALL SELECT 'investment_disposals.number',      count(*) - count(DISTINCT disposal_number)          FROM investment_disposals
UNION ALL SELECT 'investment_disposals.idem_key',    count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM investment_disposals
UNION ALL SELECT 'social_aid_funds.fund_number',     count(*) - count(DISTINCT fund_number)              FROM social_aid_funds
UNION ALL SELECT 'social_aid_funds.idem_key',        count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM social_aid_funds
UNION ALL SELECT 'social_aid_donations.number',      count(*) - count(DISTINCT donation_number)          FROM social_aid_donations
UNION ALL SELECT 'social_aid_donations.idem_key',    count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM social_aid_donations
UNION ALL SELECT 'social_aid_disbursements.number',  count(*) - count(DISTINCT disbursement_number)      FROM social_aid_disbursements
UNION ALL SELECT 'social_aid_disbursements.idem_key',count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM social_aid_disbursements
UNION ALL SELECT 'governance_bodies.body_number',    count(*) - count(DISTINCT body_number)              FROM governance_bodies
UNION ALL SELECT 'governance_bodies.idem_key',       count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM governance_bodies
UNION ALL SELECT 'governance_memberships.idem_key',  count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM governance_memberships
UNION ALL SELECT 'governance_decisions.decision_number', count(*) - count(DISTINCT decision_number)      FROM governance_decisions
UNION ALL SELECT 'governance_decisions.idem_key',    count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM governance_decisions
UNION ALL SELECT 'governance_votes.idem_key',        count(*) FILTER (WHERE idempotency_key IS NOT NULL) - count(DISTINCT idempotency_key)          FROM governance_votes
 ORDER BY identifier;

-- ---------------------------------------------------------------------------
-- C4-08  C5 replacement inventory: every UNIQUE index on a business table
--        that does NOT contain cooperative_id is a C5 rescoping candidate.
--        Informational — the listed constraints must NOT be dropped by C4.
--        Expected: rows = the exact C5 work list.
-- ---------------------------------------------------------------------------
SELECT idx.indexrelid::regclass AS index_name,
       idx.indrelid::regclass   AS table_name,
       pg_get_indexdef(idx.indexrelid) AS definition
  FROM pg_index idx
  JOIN pg_class t ON t.oid = idx.indrelid
 WHERE t.relkind = 'r'
   AND idx.indisunique
   AND t.relname IN (
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
   AND NOT EXISTS (
        SELECT 1
          FROM pg_attribute a
         WHERE a.attrelid = idx.indrelid
           AND a.attname = 'cooperative_id'
           AND a.attnum = ANY(idx.indkey))
 ORDER BY 2, 1;

-- ---------------------------------------------------------------------------
-- C4-09  Movement↔entry binding (the OTHER direction of C4-06): an
--        entry's bound movement must live in the entry's cooperative.
--        Expected: 0 violations.
-- ---------------------------------------------------------------------------
SELECT 'income_entries.account_movement_id' AS binding, count(*) AS violations
  FROM income_entries e
  JOIN account_movements m ON m.id = e.account_movement_id
 WHERE e.account_movement_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM e.cooperative_id
UNION ALL
SELECT 'expense_entries.account_movement_id', count(*)
  FROM expense_entries e
  JOIN account_movements m ON m.id = e.account_movement_id
 WHERE e.account_movement_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM e.cooperative_id
UNION ALL
SELECT 'share_return_settlements.account_movement_id', count(*)
  FROM share_return_settlements s
  JOIN account_movements m ON m.id = s.account_movement_id
 WHERE s.account_movement_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM s.cooperative_id
UNION ALL
SELECT 'investment_fundings.account_movement_id', count(*)
  FROM investment_fundings f
  JOIN account_movements m ON m.id = f.account_movement_id
 WHERE f.account_movement_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM f.cooperative_id
UNION ALL
SELECT 'investment_incomes.account_movement_id', count(*)
  FROM investment_incomes n
  JOIN account_movements m ON m.id = n.account_movement_id
 WHERE n.account_movement_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM n.cooperative_id
UNION ALL
SELECT 'investment_disposal_proceeds.account_movement_id', count(*)
  FROM investment_disposal_proceeds p
  JOIN account_movements m ON m.id = p.account_movement_id
 WHERE p.account_movement_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM p.cooperative_id
UNION ALL
SELECT 'social_aid_donations.account_movement_id', count(*)
  FROM social_aid_donations d
  JOIN account_movements m ON m.id = d.account_movement_id
 WHERE d.account_movement_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM d.cooperative_id
UNION ALL
SELECT 'social_aid_disbursements.account_movement_id', count(*)
  FROM social_aid_disbursements d
  JOIN account_movements m ON m.id = d.account_movement_id
 WHERE d.account_movement_id IS NOT NULL
   AND m.cooperative_id IS DISTINCT FROM d.cooperative_id;

-- ---------------------------------------------------------------------------
-- C4-10  Bootstrap seed ownership: global financial_categories seeds
--        (created_by IS NULL) must be claimed in a bootstrapped install.
--        Expected: unclaimed_seeds = 0 when a bootstrap cooperative exists.
-- ---------------------------------------------------------------------------
SELECT count(*) FILTER (WHERE is_bootstrap) AS bootstrap_cooperatives,
       (SELECT count(*) FROM financial_categories
         WHERE created_by IS NULL AND cooperative_id IS NULL) AS unclaimed_seeds
  FROM cooperatives;
