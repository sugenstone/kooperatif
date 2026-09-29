-- BACKUP-READINESS-001 recovery-drill fixtures.
--
-- Deterministic synthetic data covering every domain the recovery proof
-- must demonstrate (docs/23, ADR-013 restore verification). All UUIDs are
-- fixed; names are obviously synthetic. No real personal data.
--
-- Coverage:
--   user + role assignment + session + security_events
--   persons (shareholder, guardian, second shareholder)
--   families + TEMPORAL membership history (closed + open interval)
--   shares + ownership history (transfer!) + share events
--   periods + rules + assessments (per_shareholder AND per_share with
--   provenance) + exact decimal values incl. 0.01 / 10000.00 / 1234.56
--   payments + allocations (incl. a fully reversed receipt)
--   financial accounts + payment/transfer-sourced movements +
--   a posted account transfer (derived balances must survive restore)

BEGIN;

-- Operator user (fake Argon2 PHC string — never a real credential).
INSERT INTO users (id, username, display_name, status, password_hash) VALUES
    ('11111111-0000-4000-8000-0000000000aa', 'yedek.test.operator',
     'Yedek Test Operatörü', 'active',
     '$argon2id$v=19$m=8192,t=1,p=1$eWVkZWtzYWx0$c2lnbg');

INSERT INTO user_role_assignments (user_id, role_id) VALUES
    ('11111111-0000-4000-8000-0000000000aa', '00000000-0000-4000-8000-000000000001');

INSERT INTO user_sessions
    (user_id, token_hash, csrf_token, expires_at, idle_expires_at, client_label)
VALUES
    ('11111111-0000-4000-8000-0000000000aa',
     decode('00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff', 'hex'),
     'drill-csrf-token', now() + interval '1 hour', now() + interval '30 minutes',
     'drill/terminal');

INSERT INTO security_events (event_type, user_id, metadata) VALUES
    ('login_succeeded', '11111111-0000-4000-8000-0000000000aa', '{"origin":"drill"}'),
    ('share_created', '11111111-0000-4000-8000-0000000000aa',
     '{"share_number":9001}');

-- Persons: shareholder A, shareholder B, guardian person, third-party payer.
INSERT INTO persons (id, first_name, last_name, search_name) VALUES
    ('22222222-0000-4000-8000-000000000001', 'Yedek', 'Hissedarı', 'yedek hissedarı'),
    ('22222222-0000-4000-8000-000000000002', 'Yedek', 'Devralan', 'yedek devralan'),
    ('22222222-0000-4000-8000-000000000003', 'Vasi', 'Test', 'vasi test'),
    ('22222222-0000-4000-8000-000000000004', 'Ödeyen', 'Üçüncü', 'ödeyen üçüncü');

INSERT INTO families (id, sequence_number) VALUES
    ('33333333-0000-4000-8000-000000000001', 900001),
    ('33333333-0000-4000-8000-000000000002', 900002);

INSERT INTO shareholders (id, person_id, guardian_person_id, status) VALUES
    ('44444444-0000-4000-8000-000000000001',
     '22222222-0000-4000-8000-000000000001',
     '22222222-0000-4000-8000-000000000003', 'active'),
    ('44444444-0000-4000-8000-000000000002',
     '22222222-0000-4000-8000-000000000002', NULL, 'active');

-- Temporal family membership: A moved from family 900001 to 900002.
INSERT INTO shareholder_family_memberships
    (id, shareholder_id, family_id, started_at, ended_at, reason) VALUES
    ('55555555-0000-4000-8000-000000000001',
     '44444444-0000-4000-8000-000000000001',
     '33333333-0000-4000-8000-000000000001',
     '2025-01-01T00:00:00Z', '2025-06-30T00:00:00Z', 'drill: aile değişikliği'),
    ('55555555-0000-4000-8000-000000000002',
     '44444444-0000-4000-8000-000000000001',
     '33333333-0000-4000-8000-000000000002',
     '2025-07-01T00:00:00Z', NULL, NULL),
    ('55555555-0000-4000-8000-000000000003',
     '44444444-0000-4000-8000-000000000002',
     '33333333-0000-4000-8000-000000000001',
     '2025-01-01T00:00:00Z', NULL, NULL);

-- Shares + events + temporal ownership (share 9001 transfers A→B).
-- share_number/period_number are GENERATED ALWAYS AS IDENTITY —
-- OVERRIDING SYSTEM VALUE keeps fixtures deterministic.
INSERT INTO shares (id, share_number, status, created_by)
OVERRIDING SYSTEM VALUE
VALUES
    ('66666666-0000-4000-8000-000000000001', 9001, 'active',
     '11111111-0000-4000-8000-0000000000aa'),
    ('66666666-0000-4000-8000-000000000002', 9002, 'active',
     '11111111-0000-4000-8000-0000000000aa');
INSERT INTO share_events
    (id, share_id, event_type, occurred_at, from_shareholder_id,
     to_shareholder_id, acquisition_type, amount, actor_user_id) VALUES
    ('77777777-0000-4000-8000-000000000001',
     '66666666-0000-4000-8000-000000000001', 'initial_acquisition',
     '2025-01-10T00:00:00Z', NULL, '44444444-0000-4000-8000-000000000001',
     'founder', 100.00, '11111111-0000-4000-8000-0000000000aa'),
    ('77777777-0000-4000-8000-000000000002',
     '66666666-0000-4000-8000-000000000001', 'transfer',
     '2025-08-01T00:00:00Z', '44444444-0000-4000-8000-000000000001',
     '44444444-0000-4000-8000-000000000002', NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa'),
    ('77777777-0000-4000-8000-000000000003',
     '66666666-0000-4000-8000-000000000002', 'initial_acquisition',
     '2025-02-01T00:00:00Z', NULL, '44444444-0000-4000-8000-000000000002',
     'founder', NULL, '11111111-0000-4000-8000-0000000000aa');

INSERT INTO share_ownerships
    (id, share_id, shareholder_id, started_at, ended_at,
     acquisition_type, source_event_id, created_by) VALUES
    ('88888888-0000-4000-8000-000000000001',
     '66666666-0000-4000-8000-000000000001',
     '44444444-0000-4000-8000-000000000001',
     '2025-01-10T00:00:00Z', '2025-08-01T00:00:00Z', 'founder',
     '77777777-0000-4000-8000-000000000001',
     '11111111-0000-4000-8000-0000000000aa'),
    ('88888888-0000-4000-8000-000000000002',
     '66666666-0000-4000-8000-000000000001',
     '44444444-0000-4000-8000-000000000002',
     '2025-08-01T00:00:00Z', NULL, 'transfer',
     '77777777-0000-4000-8000-000000000002',
     '11111111-0000-4000-8000-0000000000aa'),
    ('88888888-0000-4000-8000-000000000003',
     '66666666-0000-4000-8000-000000000002',
     '44444444-0000-4000-8000-000000000002',
     '2025-02-01T00:00:00Z', NULL, 'founder',
     '77777777-0000-4000-8000-000000000003',
     '11111111-0000-4000-8000-0000000000aa');

-- Periods: one per_shareholder, two per_share.
INSERT INTO periods
    (id, period_number, name, status, collection_start_date, due_date, created_by)
OVERRIDING SYSTEM VALUE
VALUES
    ('99999999-0000-4000-8000-000000000001', 9001, 'Yedek Dönem A', 'open',
     '2026-01-01', '2026-01-31', '11111111-0000-4000-8000-0000000000aa'),
    ('99999999-0000-4000-8000-000000000002', 9002, 'Yedek Dönem B', 'open',
     '2026-02-01', '2026-02-28', '11111111-0000-4000-8000-0000000000aa'),
    ('99999999-0000-4000-8000-000000000003', 9003, 'Yedek Dönem C', 'closed',
     '2026-03-01', '2026-03-31', '11111111-0000-4000-8000-0000000000aa');

INSERT INTO assessment_rules
    (id, period_id, rule_type, base_amount, assessment_effective_date) VALUES
    ('abababab-0000-4000-8000-000000000001',
     '99999999-0000-4000-8000-000000000001', 'per_shareholder',
     10000.00, '2026-01-15'),
    ('abababab-0000-4000-8000-000000000002',
     '99999999-0000-4000-8000-000000000002', 'per_share',
     0.01, '2026-02-10'),
    ('abababab-0000-4000-8000-000000000003',
     '99999999-0000-4000-8000-000000000003', 'per_share',
     1234.56, '2026-03-10');

INSERT INTO assessments
    (id, period_id, shareholder_id, rule_type, base_amount, amount,
     currency, assessment_effective_date, generated_by) VALUES
    -- Dönem A: flat obligation per shareholder (10000.00 each).
    ('acacacac-0000-4000-8000-000000000001',
     '99999999-0000-4000-8000-000000000001',
     '44444444-0000-4000-8000-000000000001', 'per_shareholder',
     10000.00, 10000.00, 'TRY', '2026-01-15',
     '11111111-0000-4000-8000-0000000000aa'),
    ('acacacac-0000-4000-8000-000000000002',
     '99999999-0000-4000-8000-000000000001',
     '44444444-0000-4000-8000-000000000002', 'per_shareholder',
     10000.00, 10000.00, 'TRY', '2026-01-15',
     '11111111-0000-4000-8000-0000000000aa'),
    -- Dönem B: 0.01 per share; B owns both shares at the effective date.
    ('acacacac-0000-4000-8000-000000000003',
     '99999999-0000-4000-8000-000000000002',
     '44444444-0000-4000-8000-000000000002', 'per_share',
     0.01, 0.02, 'TRY', '2026-02-10',
     '11111111-0000-4000-8000-0000000000aa'),
    -- Dönem C: 1234.56 per share → 2469.12 with two provenance rows.
    ('acacacac-0000-4000-8000-000000000004',
     '99999999-0000-4000-8000-000000000003',
     '44444444-0000-4000-8000-000000000002', 'per_share',
     1234.56, 2469.12, 'TRY', '2026-03-10',
     '11111111-0000-4000-8000-0000000000aa');

INSERT INTO assessment_share_sources
    (assessment_id, share_id, ownership_id, amount_component) VALUES
    ('acacacac-0000-4000-8000-000000000003',
     '66666666-0000-4000-8000-000000000001',
     '88888888-0000-4000-8000-000000000002', 0.01),
    ('acacacac-0000-4000-8000-000000000003',
     '66666666-0000-4000-8000-000000000002',
     '88888888-0000-4000-8000-000000000003', 0.01),
    ('acacacac-0000-4000-8000-000000000004',
     '66666666-0000-4000-8000-000000000001',
     '88888888-0000-4000-8000-000000000002', 1234.56),
    ('acacacac-0000-4000-8000-000000000004',
     '66666666-0000-4000-8000-000000000002',
     '88888888-0000-4000-8000-000000000003', 1234.56);

-- STEP-007 payments: money RECEIVED, distinct from obligation.
-- Payment 9001: third-party payer covers TWO different debtors'
-- assessments in one receipt (payer ≠ debtor; family-style bulk).
INSERT INTO payments
    (id, payment_number, payer_person_id, amount, currency, method,
     received_at, note, status, idempotency_key, idempotency_fingerprint,
     reversed_at, reversed_by, reversal_reason,
     created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('dddddddd-0000-4000-8000-000000000001', 9001,
     '22222222-0000-4000-8000-000000000004', 150.00, 'TRY', 'cash',
     '2026-01-20T10:00:00Z', 'drill: aile tahsilatı', 'posted',
     'drill-idem-0001', 'drill-fp-0001',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-20T10:00:00Z', '2026-01-20T10:00:00Z'),
    -- Payment 9002: fully REVERSED receipt — history survives restore.
    ('dddddddd-0000-4000-8000-000000000002', 9002,
     '22222222-0000-4000-8000-000000000002', 30.00, 'TRY',
     'bank_transfer',
     '2026-01-22T09:00:00Z', 'drill: ters kayıt', 'reversed',
     'drill-idem-0002', 'drill-fp-0002',
     '2026-01-22T12:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı kayıt',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-22T09:00:00Z', '2026-01-22T12:00:00Z');

INSERT INTO payment_allocations
    (id, payment_id, assessment_id, amount, status,
     reversed_at, reversed_by, reversal_reason, created_by, created_at)
VALUES
    -- Payment 9001 split across two DIFFERENT debtors' obligations.
    ('eeeeeeee-0000-4000-8000-000000000001',
     'dddddddd-0000-4000-8000-000000000001',
     'acacacac-0000-4000-8000-000000000001', 100.00, 'active',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-20T10:00:00Z'),
    ('eeeeeeee-0000-4000-8000-000000000002',
     'dddddddd-0000-4000-8000-000000000001',
     'acacacac-0000-4000-8000-000000000002', 50.00, 'active',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-20T10:00:00Z'),
    -- Payment 9002's allocation is reversed (history preserved).
    ('eeeeeeee-0000-4000-8000-000000000003',
     'dddddddd-0000-4000-8000-000000000002',
     'acacacac-0000-4000-8000-000000000001', 30.00, 'reversed',
     '2026-01-22T12:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı kayıt',
     '11111111-0000-4000-8000-0000000000aa', '2026-01-22T09:00:00Z');

-- STEP-008: financial accounts, account movements, account transfer.
-- One CASH + one BANK account; balances are NEVER stored — the drill
-- verifies the restored movement rows derive them exactly.
INSERT INTO financial_accounts
    (id, name, account_type, currency, status, bank_name, iban, created_by)
VALUES
    ('f1f1f1f1-0000-4000-8000-000000000001', 'Yedek Kasa', 'cash', 'TRY',
     'active', NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa'),
    ('f1f1f1f1-0000-4000-8000-000000000002', 'Yedek Banka', 'bank', 'TRY',
     'active', 'Yedek Bankası A.Ş.', 'TR00 0000 0000 0000 0000 0000 00',
     '11111111-0000-4000-8000-0000000000aa');

-- Where did each posted payment's money physically go?
UPDATE payments SET destination_account_id = 'f1f1f1f1-0000-4000-8000-000000000001'
    WHERE payment_number = 9001;
UPDATE payments SET destination_account_id = 'f1f1f1f1-0000-4000-8000-000000000002'
    WHERE payment_number = 9002;

-- Payment-sourced movements: 9001 active on cash; 9002 reversed on
-- bank (reversal bookkeeping survives the restore).
INSERT INTO account_movements
    (id, account_id, direction, amount, source_type, source_id, occurred_at,
     status, reversed_at, reversed_by, reversal_reason, created_by, created_at)
VALUES
    ('f2f2f2f2-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000001', 'inflow', 150.00, 'payment',
     'dddddddd-0000-4000-8000-000000000001', '2026-01-20T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-20T10:00:00Z'),
    ('f2f2f2f2-0000-4000-8000-000000000002',
     'f1f1f1f1-0000-4000-8000-000000000002', 'inflow', 30.00, 'payment',
     'dddddddd-0000-4000-8000-000000000002', '2026-01-22T09:00:00Z',
     'reversed', '2026-01-22T12:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: hatalı kayıt',
     '11111111-0000-4000-8000-0000000000aa', '2026-01-22T09:00:00Z');

-- One posted transfer cash → bank 40.00 (two linked legs, one row).
INSERT INTO account_transfers
    (id, transfer_number, source_account_id, destination_account_id, amount,
     currency, occurred_at, note, status, idempotency_key,
     idempotency_fingerprint, created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('f3f3f3f3-0000-4000-8000-000000000001', 9001,
     'f1f1f1f1-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000002',
     40.00, 'TRY', '2026-01-25T10:00:00Z', 'drill: kasa→banka', 'posted',
     'drill-tidem-0001', 'drill-tfp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-25T10:00:00Z', '2026-01-25T10:00:00Z');

INSERT INTO account_movements
    (id, account_id, direction, amount, source_type, source_id, occurred_at,
     status, reversed_at, reversed_by, reversal_reason, created_by, created_at)
VALUES
    ('f2f2f2f2-0000-4000-8000-000000000003',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 40.00, 'transfer',
     'f3f3f3f3-0000-4000-8000-000000000001', '2026-01-25T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-25T10:00:00Z'),
    ('f2f2f2f2-0000-4000-8000-000000000004',
     'f1f1f1f1-0000-4000-8000-000000000002', 'inflow', 40.00, 'transfer',
     'f3f3f3f3-0000-4000-8000-000000000001', '2026-01-25T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-25T10:00:00Z');

COMMIT;
