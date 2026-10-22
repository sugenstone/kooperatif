-- M1-P2-C2a — cooperative-consistent composite foreign keys (NOT VALID).
--
-- For every business→business foreign key where both sides carry
-- cooperative_id (M1-P2-C1), this adds a SECOND, cooperative-scoped
-- constraint: (cooperative_id, <fk_col>) REFERENCES <parent>
-- (cooperative_id, id). The original single-column foreign keys are
-- preserved unchanged — the composite constraints ADD same-cooperative
-- consistency; they replace nothing.
--
-- Prerequisites (all created by 0021): every parent table carries a
-- UNIQUE index on (cooperative_id, id) — *_coop_id_uq.
--
-- Explicitly OUT OF SCOPE / documented exceptions:
--   * FKs to users, cooperatives, permissions — those tables are global
--     or already the ownership root; no composite form exists.
--   * role_permissions→roles and other children without cooperative_id.
--   * user_role_assignments — already covered by P1's ura_coop_role_fk
--     and ura_coop_user_fk.
--   * account_movements.source_id — POLYMORPHIC (source_type ∈ 10
--     domain values, no FK possible). Same-cooperative consistency for
--     polymorphic references must be enforced at the application layer
--     or a later milestone; no constraint is invented here.
--   * expense_entries/income_entries category (category_id, entry_kind)
--     — the existing composite kind FK is preserved; C2 adds only the
--     cooperative dimension on category_id.
--
-- NULL semantics (documented, NOT a loophole closure):
--   cooperative_id is nullable until C5; a NULL cooperative_id (or NULL
--   fk column) makes every FK trivially pass — including the composite
--   ones below. C3 (write-path guards) and C5 (NOT NULL enforcement)
--   therefore remain mandatory; C2 only hardens rows that already carry
--   a cooperative_id.
--
-- Locking: ADD CONSTRAINT ... NOT VALID takes SHARE ROW EXCLUSIVE on
-- the child and ROW SHARE on the parent for milliseconds — no table
-- scan. Validation is C2b (0023).
--
-- All FK actions default to NO ACTION / non-deferrable, matching every
-- existing foreign key in the schema.

ALTER TABLE account_movements
    ADD CONSTRAINT account_movements_account_id_coop_fk
    FOREIGN KEY (cooperative_id, account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE account_transfers
    ADD CONSTRAINT account_transfers_destination_account_id_coop_fk
    FOREIGN KEY (cooperative_id, destination_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE account_transfers
    ADD CONSTRAINT account_transfers_source_account_id_coop_fk
    FOREIGN KEY (cooperative_id, source_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE assessment_rules
    ADD CONSTRAINT assessment_rules_period_id_coop_fk
    FOREIGN KEY (cooperative_id, period_id)
    REFERENCES periods (cooperative_id, id) NOT VALID;

ALTER TABLE assessment_share_sources
    ADD CONSTRAINT assessment_share_sources_assessment_id_coop_fk
    FOREIGN KEY (cooperative_id, assessment_id)
    REFERENCES assessments (cooperative_id, id) NOT VALID;

ALTER TABLE assessment_share_sources
    ADD CONSTRAINT assessment_share_sources_ownership_id_coop_fk
    FOREIGN KEY (cooperative_id, ownership_id)
    REFERENCES share_ownerships (cooperative_id, id) NOT VALID;

ALTER TABLE assessment_share_sources
    ADD CONSTRAINT assessment_share_sources_share_id_coop_fk
    FOREIGN KEY (cooperative_id, share_id)
    REFERENCES shares (cooperative_id, id) NOT VALID;

ALTER TABLE assessments
    ADD CONSTRAINT assessments_period_id_coop_fk
    FOREIGN KEY (cooperative_id, period_id)
    REFERENCES periods (cooperative_id, id) NOT VALID;

ALTER TABLE assessments
    ADD CONSTRAINT assessments_shareholder_id_coop_fk
    FOREIGN KEY (cooperative_id, shareholder_id)
    REFERENCES shareholders (cooperative_id, id) NOT VALID;

ALTER TABLE credit_applications
    ADD CONSTRAINT credit_applications_assessment_id_coop_fk
    FOREIGN KEY (cooperative_id, assessment_id)
    REFERENCES assessments (cooperative_id, id) NOT VALID;

ALTER TABLE credit_applications
    ADD CONSTRAINT credit_applications_credit_id_coop_fk
    FOREIGN KEY (cooperative_id, credit_id)
    REFERENCES shareholder_credits (cooperative_id, id) NOT VALID;

ALTER TABLE expense_entries
    ADD CONSTRAINT expense_entries_account_movement_id_coop_fk
    FOREIGN KEY (cooperative_id, account_movement_id)
    REFERENCES account_movements (cooperative_id, id) NOT VALID;

ALTER TABLE expense_entries
    ADD CONSTRAINT expense_entries_category_id_coop_fk
    FOREIGN KEY (cooperative_id, category_id)
    REFERENCES financial_categories (cooperative_id, id) NOT VALID;

ALTER TABLE expense_entries
    ADD CONSTRAINT expense_entries_financial_account_id_coop_fk
    FOREIGN KEY (cooperative_id, financial_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE governance_decisions
    ADD CONSTRAINT governance_decisions_body_id_coop_fk
    FOREIGN KEY (cooperative_id, body_id)
    REFERENCES governance_bodies (cooperative_id, id) NOT VALID;

ALTER TABLE governance_memberships
    ADD CONSTRAINT governance_memberships_body_id_coop_fk
    FOREIGN KEY (cooperative_id, body_id)
    REFERENCES governance_bodies (cooperative_id, id) NOT VALID;

ALTER TABLE governance_memberships
    ADD CONSTRAINT governance_memberships_person_id_coop_fk
    FOREIGN KEY (cooperative_id, person_id)
    REFERENCES persons (cooperative_id, id) NOT VALID;

ALTER TABLE governance_votes
    ADD CONSTRAINT governance_votes_decision_id_coop_fk
    FOREIGN KEY (cooperative_id, decision_id)
    REFERENCES governance_decisions (cooperative_id, id) NOT VALID;

ALTER TABLE governance_votes
    ADD CONSTRAINT governance_votes_membership_id_coop_fk
    FOREIGN KEY (cooperative_id, membership_id)
    REFERENCES governance_memberships (cooperative_id, id) NOT VALID;

ALTER TABLE governance_votes
    ADD CONSTRAINT governance_votes_person_id_coop_fk
    FOREIGN KEY (cooperative_id, person_id)
    REFERENCES persons (cooperative_id, id) NOT VALID;

ALTER TABLE income_entries
    ADD CONSTRAINT income_entries_account_movement_id_coop_fk
    FOREIGN KEY (cooperative_id, account_movement_id)
    REFERENCES account_movements (cooperative_id, id) NOT VALID;

ALTER TABLE income_entries
    ADD CONSTRAINT income_entries_category_id_coop_fk
    FOREIGN KEY (cooperative_id, category_id)
    REFERENCES financial_categories (cooperative_id, id) NOT VALID;

ALTER TABLE income_entries
    ADD CONSTRAINT income_entries_financial_account_id_coop_fk
    FOREIGN KEY (cooperative_id, financial_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE investment_disposal_proceeds
    ADD CONSTRAINT investment_disposal_proceeds_account_movement_id_coop_fk
    FOREIGN KEY (cooperative_id, account_movement_id)
    REFERENCES account_movements (cooperative_id, id) NOT VALID;

ALTER TABLE investment_disposal_proceeds
    ADD CONSTRAINT investment_disposal_proceeds_disposal_id_coop_fk
    FOREIGN KEY (cooperative_id, disposal_id)
    REFERENCES investment_disposals (cooperative_id, id) NOT VALID;

ALTER TABLE investment_disposal_proceeds
    ADD CONSTRAINT investment_disposal_proceeds_financial_account_id_coop_fk
    FOREIGN KEY (cooperative_id, financial_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE investment_disposals
    ADD CONSTRAINT investment_disposals_investment_id_coop_fk
    FOREIGN KEY (cooperative_id, investment_id)
    REFERENCES investments (cooperative_id, id) NOT VALID;

ALTER TABLE investment_fundings
    ADD CONSTRAINT investment_fundings_account_movement_id_coop_fk
    FOREIGN KEY (cooperative_id, account_movement_id)
    REFERENCES account_movements (cooperative_id, id) NOT VALID;

ALTER TABLE investment_fundings
    ADD CONSTRAINT investment_fundings_financial_account_id_coop_fk
    FOREIGN KEY (cooperative_id, financial_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE investment_fundings
    ADD CONSTRAINT investment_fundings_investment_id_coop_fk
    FOREIGN KEY (cooperative_id, investment_id)
    REFERENCES investments (cooperative_id, id) NOT VALID;

ALTER TABLE investment_incomes
    ADD CONSTRAINT investment_incomes_account_movement_id_coop_fk
    FOREIGN KEY (cooperative_id, account_movement_id)
    REFERENCES account_movements (cooperative_id, id) NOT VALID;

ALTER TABLE investment_incomes
    ADD CONSTRAINT investment_incomes_financial_account_id_coop_fk
    FOREIGN KEY (cooperative_id, financial_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE investment_incomes
    ADD CONSTRAINT investment_incomes_investment_id_coop_fk
    FOREIGN KEY (cooperative_id, investment_id)
    REFERENCES investments (cooperative_id, id) NOT VALID;

ALTER TABLE investment_valuations
    ADD CONSTRAINT investment_valuations_investment_id_coop_fk
    FOREIGN KEY (cooperative_id, investment_id)
    REFERENCES investments (cooperative_id, id) NOT VALID;

ALTER TABLE payment_allocations
    ADD CONSTRAINT payment_allocations_assessment_id_coop_fk
    FOREIGN KEY (cooperative_id, assessment_id)
    REFERENCES assessments (cooperative_id, id) NOT VALID;

ALTER TABLE payment_allocations
    ADD CONSTRAINT payment_allocations_payment_id_coop_fk
    FOREIGN KEY (cooperative_id, payment_id)
    REFERENCES payments (cooperative_id, id) NOT VALID;

ALTER TABLE payments
    ADD CONSTRAINT payments_destination_account_id_coop_fk
    FOREIGN KEY (cooperative_id, destination_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE payments
    ADD CONSTRAINT payments_payer_person_id_coop_fk
    FOREIGN KEY (cooperative_id, payer_person_id)
    REFERENCES persons (cooperative_id, id) NOT VALID;

ALTER TABLE share_events
    ADD CONSTRAINT share_events_from_shareholder_id_coop_fk
    FOREIGN KEY (cooperative_id, from_shareholder_id)
    REFERENCES shareholders (cooperative_id, id) NOT VALID;

ALTER TABLE share_events
    ADD CONSTRAINT share_events_share_id_coop_fk
    FOREIGN KEY (cooperative_id, share_id)
    REFERENCES shares (cooperative_id, id) NOT VALID;

ALTER TABLE share_events
    ADD CONSTRAINT share_events_to_shareholder_id_coop_fk
    FOREIGN KEY (cooperative_id, to_shareholder_id)
    REFERENCES shareholders (cooperative_id, id) NOT VALID;

ALTER TABLE share_ownerships
    ADD CONSTRAINT share_ownerships_share_id_coop_fk
    FOREIGN KEY (cooperative_id, share_id)
    REFERENCES shares (cooperative_id, id) NOT VALID;

ALTER TABLE share_ownerships
    ADD CONSTRAINT share_ownerships_shareholder_id_coop_fk
    FOREIGN KEY (cooperative_id, shareholder_id)
    REFERENCES shareholders (cooperative_id, id) NOT VALID;

ALTER TABLE share_ownerships
    ADD CONSTRAINT share_ownerships_source_event_id_coop_fk
    FOREIGN KEY (cooperative_id, source_event_id)
    REFERENCES share_events (cooperative_id, id) NOT VALID;

ALTER TABLE share_return_entitlements
    ADD CONSTRAINT share_return_entitlements_beneficiary_shareholder_id_coop_fk
    FOREIGN KEY (cooperative_id, beneficiary_shareholder_id)
    REFERENCES shareholders (cooperative_id, id) NOT VALID;

ALTER TABLE share_return_entitlements
    ADD CONSTRAINT share_return_entitlements_share_return_id_coop_fk
    FOREIGN KEY (cooperative_id, share_return_id)
    REFERENCES share_returns (cooperative_id, id) NOT VALID;

ALTER TABLE share_return_settlements
    ADD CONSTRAINT share_return_settlements_account_movement_id_coop_fk
    FOREIGN KEY (cooperative_id, account_movement_id)
    REFERENCES account_movements (cooperative_id, id) NOT VALID;

ALTER TABLE share_return_settlements
    ADD CONSTRAINT share_return_settlements_entitlement_id_coop_fk
    FOREIGN KEY (cooperative_id, entitlement_id)
    REFERENCES share_return_entitlements (cooperative_id, id) NOT VALID;

ALTER TABLE share_return_settlements
    ADD CONSTRAINT share_return_settlements_financial_account_id_coop_fk
    FOREIGN KEY (cooperative_id, financial_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE share_returns
    ADD CONSTRAINT share_returns_share_id_coop_fk
    FOREIGN KEY (cooperative_id, share_id)
    REFERENCES shares (cooperative_id, id) NOT VALID;

ALTER TABLE share_returns
    ADD CONSTRAINT share_returns_shareholder_id_coop_fk
    FOREIGN KEY (cooperative_id, shareholder_id)
    REFERENCES shareholders (cooperative_id, id) NOT VALID;

ALTER TABLE shareholder_credits
    ADD CONSTRAINT shareholder_credits_shareholder_id_coop_fk
    FOREIGN KEY (cooperative_id, shareholder_id)
    REFERENCES shareholders (cooperative_id, id) NOT VALID;

ALTER TABLE shareholder_credits
    ADD CONSTRAINT shareholder_credits_source_payment_id_coop_fk
    FOREIGN KEY (cooperative_id, source_payment_id)
    REFERENCES payments (cooperative_id, id) NOT VALID;

ALTER TABLE shareholder_family_memberships
    ADD CONSTRAINT shareholder_family_memberships_family_id_coop_fk
    FOREIGN KEY (cooperative_id, family_id)
    REFERENCES families (cooperative_id, id) NOT VALID;

ALTER TABLE shareholder_family_memberships
    ADD CONSTRAINT shareholder_family_memberships_shareholder_id_coop_fk
    FOREIGN KEY (cooperative_id, shareholder_id)
    REFERENCES shareholders (cooperative_id, id) NOT VALID;

ALTER TABLE shareholders
    ADD CONSTRAINT shareholders_default_collection_account_id_coop_fk
    FOREIGN KEY (cooperative_id, default_collection_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE shareholders
    ADD CONSTRAINT shareholders_guardian_person_id_coop_fk
    FOREIGN KEY (cooperative_id, guardian_person_id)
    REFERENCES persons (cooperative_id, id) NOT VALID;

ALTER TABLE shareholders
    ADD CONSTRAINT shareholders_person_id_coop_fk
    FOREIGN KEY (cooperative_id, person_id)
    REFERENCES persons (cooperative_id, id) NOT VALID;

ALTER TABLE social_aid_disbursements
    ADD CONSTRAINT social_aid_disbursements_account_movement_id_coop_fk
    FOREIGN KEY (cooperative_id, account_movement_id)
    REFERENCES account_movements (cooperative_id, id) NOT VALID;

ALTER TABLE social_aid_disbursements
    ADD CONSTRAINT social_aid_disbursements_beneficiary_person_id_coop_fk
    FOREIGN KEY (cooperative_id, beneficiary_person_id)
    REFERENCES persons (cooperative_id, id) NOT VALID;

ALTER TABLE social_aid_disbursements
    ADD CONSTRAINT social_aid_disbursements_financial_account_id_coop_fk
    FOREIGN KEY (cooperative_id, financial_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE social_aid_disbursements
    ADD CONSTRAINT social_aid_disbursements_fund_id_coop_fk
    FOREIGN KEY (cooperative_id, fund_id)
    REFERENCES social_aid_funds (cooperative_id, id) NOT VALID;

ALTER TABLE social_aid_donations
    ADD CONSTRAINT social_aid_donations_account_movement_id_coop_fk
    FOREIGN KEY (cooperative_id, account_movement_id)
    REFERENCES account_movements (cooperative_id, id) NOT VALID;

ALTER TABLE social_aid_donations
    ADD CONSTRAINT social_aid_donations_donor_person_id_coop_fk
    FOREIGN KEY (cooperative_id, donor_person_id)
    REFERENCES persons (cooperative_id, id) NOT VALID;

ALTER TABLE social_aid_donations
    ADD CONSTRAINT social_aid_donations_financial_account_id_coop_fk
    FOREIGN KEY (cooperative_id, financial_account_id)
    REFERENCES financial_accounts (cooperative_id, id) NOT VALID;

ALTER TABLE social_aid_donations
    ADD CONSTRAINT social_aid_donations_fund_id_coop_fk
    FOREIGN KEY (cooperative_id, fund_id)
    REFERENCES social_aid_funds (cooperative_id, id) NOT VALID;
