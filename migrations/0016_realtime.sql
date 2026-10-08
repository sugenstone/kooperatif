-- STEP-016 — Live operations & real-time foundation (ADR-005, ADR-009,
-- docs/27 §Real-time).
--
-- One generic AFTER-trigger emits pg_notify('kooperatif_domain_changed',
-- <domain>) from inside the mutating transaction. pg_notify is
-- TRANSACTIONAL in PostgreSQL: the notification is delivered only after
-- COMMIT and discarded entirely on ROLLBACK, so no committed-state
-- signal can ever be published for a rolled-back write — and no signal
-- is emitted before commit. Identical (channel, payload) notifications
-- within one transaction are coalesced by PostgreSQL, so a multi-row
-- transaction emits exactly one notification per touched domain.
--
-- The payload carries ONLY the domain tag — never row data, never
-- personal/financial values (docs/22 data minimization). Delivery is a
-- transient signal; durable truth stays in the domain tables and
-- clients resynchronize through the canonical read APIs.
--
-- LISTEN/NOTIFY is process-wide and instance-wide: any backend instance
-- listening on this database receives the notification (ADR-012 single
-- node today; cross-instance delivery works without extra infra).

CREATE OR REPLACE FUNCTION kooperatif_notify_domain()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    -- TG_ARGV[0] is the logical domain tag chosen at trigger creation.
    PERFORM pg_notify('kooperatif_domain_changed', TG_ARGV[0]);
    RETURN NULL;
END;
$$;

-- Identity / relationship domain (shareholders.read / families.read).
CREATE TRIGGER rt_parties_shareholders AFTER INSERT OR UPDATE OR DELETE ON shareholders
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('shareholders');
CREATE TRIGGER rt_parties_persons AFTER INSERT OR UPDATE OR DELETE ON persons
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('shareholders');
CREATE TRIGGER rt_parties_families AFTER INSERT OR UPDATE OR DELETE ON families
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('families');
CREATE TRIGGER rt_parties_family_members AFTER INSERT OR UPDATE OR DELETE ON shareholder_family_memberships
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('families');

-- Share lifecycle (shares.read).
CREATE TRIGGER rt_shares AFTER INSERT OR UPDATE OR DELETE ON shares
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('shares');
CREATE TRIGGER rt_shares_ownerships AFTER INSERT OR UPDATE OR DELETE ON share_ownerships
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('shares');
CREATE TRIGGER rt_shares_events AFTER INSERT OR UPDATE OR DELETE ON share_events
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('shares');

-- Obligation domain (periods.read / assessments.read).
CREATE TRIGGER rt_periods AFTER INSERT OR UPDATE OR DELETE ON periods
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('periods');
CREATE TRIGGER rt_period_rules AFTER INSERT OR UPDATE OR DELETE ON assessment_rules
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('periods');
CREATE TRIGGER rt_assessments AFTER INSERT OR UPDATE OR DELETE ON assessments
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('assessments');
CREATE TRIGGER rt_assessment_sources AFTER INSERT OR UPDATE OR DELETE ON assessment_share_sources
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('assessments');

-- Money received + application (payments.read / credits.read).
CREATE TRIGGER rt_payments AFTER INSERT OR UPDATE OR DELETE ON payments
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('payments');
CREATE TRIGGER rt_payment_allocations AFTER INSERT OR UPDATE OR DELETE ON payment_allocations
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('payments');
CREATE TRIGGER rt_credits AFTER INSERT OR UPDATE OR DELETE ON shareholder_credits
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('credits');
CREATE TRIGGER rt_credit_applications AFTER INSERT OR UPDATE OR DELETE ON credit_applications
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('credits');

-- Financial accounts / movements / transfers (financial_accounts.read).
CREATE TRIGGER rt_accounts AFTER INSERT OR UPDATE OR DELETE ON financial_accounts
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('accounts');
CREATE TRIGGER rt_movements AFTER INSERT OR UPDATE OR DELETE ON account_movements
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('accounts');
CREATE TRIGGER rt_transfers AFTER INSERT OR UPDATE OR DELETE ON account_transfers
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('accounts');

-- Operational income/expense (income_expense.read).
CREATE TRIGGER rt_incomes AFTER INSERT OR UPDATE OR DELETE ON income_entries
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('income_expense');
CREATE TRIGGER rt_expenses AFTER INSERT OR UPDATE OR DELETE ON expense_entries
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('income_expense');
CREATE TRIGGER rt_categories AFTER INSERT OR UPDATE OR DELETE ON financial_categories
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('income_expense');

-- Share return / exit (share_returns.read).
CREATE TRIGGER rt_returns AFTER INSERT OR UPDATE OR DELETE ON share_returns
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('share_returns');
CREATE TRIGGER rt_return_entitlements AFTER INSERT OR UPDATE OR DELETE ON share_return_entitlements
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('share_returns');
CREATE TRIGGER rt_return_settlements AFTER INSERT OR UPDATE OR DELETE ON share_return_settlements
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('share_returns');

-- Investments (investments.read).
CREATE TRIGGER rt_investments AFTER INSERT OR UPDATE OR DELETE ON investments
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('investments');
CREATE TRIGGER rt_investment_fundings AFTER INSERT OR UPDATE OR DELETE ON investment_fundings
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('investments');
CREATE TRIGGER rt_investment_valuations AFTER INSERT OR UPDATE OR DELETE ON investment_valuations
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('investments');
CREATE TRIGGER rt_investment_incomes AFTER INSERT OR UPDATE OR DELETE ON investment_incomes
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('investments');
CREATE TRIGGER rt_investment_disposals AFTER INSERT OR UPDATE OR DELETE ON investment_disposals
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('investments');
CREATE TRIGGER rt_investment_proceeds AFTER INSERT OR UPDATE OR DELETE ON investment_disposal_proceeds
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('investments');

-- Social aid — restricted fund, never merged with cooperative money
-- (social_aid.read).
CREATE TRIGGER rt_aid_funds AFTER INSERT OR UPDATE OR DELETE ON social_aid_funds
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('social_aid');
CREATE TRIGGER rt_aid_donations AFTER INSERT OR UPDATE OR DELETE ON social_aid_donations
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('social_aid');
CREATE TRIGGER rt_aid_disbursements AFTER INSERT OR UPDATE OR DELETE ON social_aid_disbursements
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('social_aid');

-- Governance evidence (governance.read).
CREATE TRIGGER rt_gov_bodies AFTER INSERT OR UPDATE OR DELETE ON governance_bodies
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('governance');
CREATE TRIGGER rt_gov_memberships AFTER INSERT OR UPDATE OR DELETE ON governance_memberships
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('governance');
CREATE TRIGGER rt_gov_decisions AFTER INSERT OR UPDATE OR DELETE ON governance_decisions
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('governance');
CREATE TRIGGER rt_gov_votes AFTER INSERT OR UPDATE OR DELETE ON governance_votes
    FOR EACH ROW EXECUTE FUNCTION kooperatif_notify_domain('governance');
