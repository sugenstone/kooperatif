-- =====================================================================
-- 0017  SHAREHOLDER DEFAULT COLLECTION ACCOUNT  (FUNC-FIX-002)
-- docs/07 "Shareholder default account association" + owner-approved
-- FUNC-FIX-002 rules.
--
-- Semantics:
--   * The default account belongs to the SHAREHOLDER — never to the
--     family or an individual share.
--   * It is a collection PREFERENCE, not a posting restriction:
--     payments still carry an explicit destination_account_id chosen
--     at confirmation time.
--   * Assignment requires the account to exist and be 'active'
--     (route-level check under a row lock); the column itself stays a
--     plain nullable FK so an account that later becomes 'inactive'
--     keeps the saved preference — it is shown with a warning and is
--     never silently used or silently cleared (owner decision 1).
--   * Historical reconstruction is already guaranteed by
--     payments.destination_account_id + account_movements; a temporal
--     association table is deliberately not added (docs/07: effective
--     dating is required only if reconstruction needs it — it does
--     not). Changes are audited via security_events
--     ('shareholder_default_account_changed').
--   * NULL default = no preference → explicit account selection is
--     required for every payment (no backfill by design).
--   * Rollback: DROP COLUMN — nothing references it.
-- =====================================================================

ALTER TABLE shareholders
    ADD COLUMN default_collection_account_id UUID
        REFERENCES financial_accounts(id);

COMMENT ON COLUMN shareholders.default_collection_account_id IS
    'FUNC-FIX-002: optional default COLLECTION account — a payment-screen
     preference only; never a posting restriction. Assignment validated
     to an active account at write time; may point at a later-inactive
     account (warned, never auto-used).';
