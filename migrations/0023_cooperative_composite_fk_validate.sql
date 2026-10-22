-- M1-P2-C2b — validate the cooperative composite foreign keys added
-- NOT VALID by 0022 (66 constraints).
--
-- VALIDATE CONSTRAINT takes SHARE UPDATE EXCLUSIVE on the child
-- table only: concurrent reads and writes are NOT blocked. A
-- validation failure aborts the transaction and signals real
-- cross-cooperative data that must be investigated — never
-- silently rewritten.

ALTER TABLE account_movements     VALIDATE CONSTRAINT account_movements_account_id_coop_fk;
ALTER TABLE account_transfers     VALIDATE CONSTRAINT account_transfers_destination_account_id_coop_fk;
ALTER TABLE account_transfers     VALIDATE CONSTRAINT account_transfers_source_account_id_coop_fk;
ALTER TABLE assessment_rules     VALIDATE CONSTRAINT assessment_rules_period_id_coop_fk;
ALTER TABLE assessment_share_sources     VALIDATE CONSTRAINT assessment_share_sources_assessment_id_coop_fk;
ALTER TABLE assessment_share_sources     VALIDATE CONSTRAINT assessment_share_sources_ownership_id_coop_fk;
ALTER TABLE assessment_share_sources     VALIDATE CONSTRAINT assessment_share_sources_share_id_coop_fk;
ALTER TABLE assessments     VALIDATE CONSTRAINT assessments_period_id_coop_fk;
ALTER TABLE assessments     VALIDATE CONSTRAINT assessments_shareholder_id_coop_fk;
ALTER TABLE credit_applications     VALIDATE CONSTRAINT credit_applications_assessment_id_coop_fk;
ALTER TABLE credit_applications     VALIDATE CONSTRAINT credit_applications_credit_id_coop_fk;
ALTER TABLE expense_entries     VALIDATE CONSTRAINT expense_entries_account_movement_id_coop_fk;
ALTER TABLE expense_entries     VALIDATE CONSTRAINT expense_entries_category_id_coop_fk;
ALTER TABLE expense_entries     VALIDATE CONSTRAINT expense_entries_financial_account_id_coop_fk;
ALTER TABLE governance_decisions     VALIDATE CONSTRAINT governance_decisions_body_id_coop_fk;
ALTER TABLE governance_memberships     VALIDATE CONSTRAINT governance_memberships_body_id_coop_fk;
ALTER TABLE governance_memberships     VALIDATE CONSTRAINT governance_memberships_person_id_coop_fk;
ALTER TABLE governance_votes     VALIDATE CONSTRAINT governance_votes_decision_id_coop_fk;
ALTER TABLE governance_votes     VALIDATE CONSTRAINT governance_votes_membership_id_coop_fk;
ALTER TABLE governance_votes     VALIDATE CONSTRAINT governance_votes_person_id_coop_fk;
ALTER TABLE income_entries     VALIDATE CONSTRAINT income_entries_account_movement_id_coop_fk;
ALTER TABLE income_entries     VALIDATE CONSTRAINT income_entries_category_id_coop_fk;
ALTER TABLE income_entries     VALIDATE CONSTRAINT income_entries_financial_account_id_coop_fk;
ALTER TABLE investment_disposal_proceeds     VALIDATE CONSTRAINT investment_disposal_proceeds_account_movement_id_coop_fk;
ALTER TABLE investment_disposal_proceeds     VALIDATE CONSTRAINT investment_disposal_proceeds_disposal_id_coop_fk;
ALTER TABLE investment_disposal_proceeds     VALIDATE CONSTRAINT investment_disposal_proceeds_financial_account_id_coop_fk;
ALTER TABLE investment_disposals     VALIDATE CONSTRAINT investment_disposals_investment_id_coop_fk;
ALTER TABLE investment_fundings     VALIDATE CONSTRAINT investment_fundings_account_movement_id_coop_fk;
ALTER TABLE investment_fundings     VALIDATE CONSTRAINT investment_fundings_financial_account_id_coop_fk;
ALTER TABLE investment_fundings     VALIDATE CONSTRAINT investment_fundings_investment_id_coop_fk;
ALTER TABLE investment_incomes     VALIDATE CONSTRAINT investment_incomes_account_movement_id_coop_fk;
ALTER TABLE investment_incomes     VALIDATE CONSTRAINT investment_incomes_financial_account_id_coop_fk;
ALTER TABLE investment_incomes     VALIDATE CONSTRAINT investment_incomes_investment_id_coop_fk;
ALTER TABLE investment_valuations     VALIDATE CONSTRAINT investment_valuations_investment_id_coop_fk;
ALTER TABLE payment_allocations     VALIDATE CONSTRAINT payment_allocations_assessment_id_coop_fk;
ALTER TABLE payment_allocations     VALIDATE CONSTRAINT payment_allocations_payment_id_coop_fk;
ALTER TABLE payments     VALIDATE CONSTRAINT payments_destination_account_id_coop_fk;
ALTER TABLE payments     VALIDATE CONSTRAINT payments_payer_person_id_coop_fk;
ALTER TABLE share_events     VALIDATE CONSTRAINT share_events_from_shareholder_id_coop_fk;
ALTER TABLE share_events     VALIDATE CONSTRAINT share_events_share_id_coop_fk;
ALTER TABLE share_events     VALIDATE CONSTRAINT share_events_to_shareholder_id_coop_fk;
ALTER TABLE share_ownerships     VALIDATE CONSTRAINT share_ownerships_share_id_coop_fk;
ALTER TABLE share_ownerships     VALIDATE CONSTRAINT share_ownerships_shareholder_id_coop_fk;
ALTER TABLE share_ownerships     VALIDATE CONSTRAINT share_ownerships_source_event_id_coop_fk;
ALTER TABLE share_return_entitlements     VALIDATE CONSTRAINT share_return_entitlements_beneficiary_shareholder_id_coop_fk;
ALTER TABLE share_return_entitlements     VALIDATE CONSTRAINT share_return_entitlements_share_return_id_coop_fk;
ALTER TABLE share_return_settlements     VALIDATE CONSTRAINT share_return_settlements_account_movement_id_coop_fk;
ALTER TABLE share_return_settlements     VALIDATE CONSTRAINT share_return_settlements_entitlement_id_coop_fk;
ALTER TABLE share_return_settlements     VALIDATE CONSTRAINT share_return_settlements_financial_account_id_coop_fk;
ALTER TABLE share_returns     VALIDATE CONSTRAINT share_returns_share_id_coop_fk;
ALTER TABLE share_returns     VALIDATE CONSTRAINT share_returns_shareholder_id_coop_fk;
ALTER TABLE shareholder_credits     VALIDATE CONSTRAINT shareholder_credits_shareholder_id_coop_fk;
ALTER TABLE shareholder_credits     VALIDATE CONSTRAINT shareholder_credits_source_payment_id_coop_fk;
ALTER TABLE shareholder_family_memberships     VALIDATE CONSTRAINT shareholder_family_memberships_family_id_coop_fk;
ALTER TABLE shareholder_family_memberships     VALIDATE CONSTRAINT shareholder_family_memberships_shareholder_id_coop_fk;
ALTER TABLE shareholders     VALIDATE CONSTRAINT shareholders_default_collection_account_id_coop_fk;
ALTER TABLE shareholders     VALIDATE CONSTRAINT shareholders_guardian_person_id_coop_fk;
ALTER TABLE shareholders     VALIDATE CONSTRAINT shareholders_person_id_coop_fk;
ALTER TABLE social_aid_disbursements     VALIDATE CONSTRAINT social_aid_disbursements_account_movement_id_coop_fk;
ALTER TABLE social_aid_disbursements     VALIDATE CONSTRAINT social_aid_disbursements_beneficiary_person_id_coop_fk;
ALTER TABLE social_aid_disbursements     VALIDATE CONSTRAINT social_aid_disbursements_financial_account_id_coop_fk;
ALTER TABLE social_aid_disbursements     VALIDATE CONSTRAINT social_aid_disbursements_fund_id_coop_fk;
ALTER TABLE social_aid_donations     VALIDATE CONSTRAINT social_aid_donations_account_movement_id_coop_fk;
ALTER TABLE social_aid_donations     VALIDATE CONSTRAINT social_aid_donations_donor_person_id_coop_fk;
ALTER TABLE social_aid_donations     VALIDATE CONSTRAINT social_aid_donations_financial_account_id_coop_fk;
ALTER TABLE social_aid_donations     VALIDATE CONSTRAINT social_aid_donations_fund_id_coop_fk;
