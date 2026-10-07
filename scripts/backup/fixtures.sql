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
--   shareholder credits + credit applications (automatic + a REVERSED
--   manual application) — entitlement history, never new movements
--   financial categories + income/expense entries (incl. a REVERSED
--   expense) bound 1:1 to income/expense-sourced movements
--   share return + entitlements + settlements (incl. a REVERSED
--   settlement); investments + fundings + valuations + incomes +
--   disposal proceeds; social aid funds + donations + disbursements
--   (incl. REVERSED rows) bound 1:1 to social_aid-sourced movements —
--   restricted availability is DERIVED, never stored
--   governance bodies (active + closed) + temporal memberships +
--   decisions (approved w/ frozen snapshot, cancelled, rejected) +
--   votes — pure evidence, ZERO account movements

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

-- STEP-009: Shareholder Credit (Excess Payment) + applications.
-- Payment 9003 leaves an 80.00 remainder explicitly assigned to
-- shareholder A; 30.00 auto-offsets assessment ...0001 and a 25.00
-- manual application is reversed. A credit is an ENTITLEMENT over
-- already-received cash — exactly one payment inflow exists, credit
-- operations never create movements.
INSERT INTO payments
    (id, payment_number, payer_person_id, amount, currency, method,
     received_at, note, status, idempotency_key, idempotency_fingerprint,
     destination_account_id, created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('dddddddd-0000-4000-8000-000000000003', 9003,
     '22222222-0000-4000-8000-000000000001', 80.00, 'TRY', 'cash',
     '2026-01-28T10:00:00Z', 'drill: avans kaynağı', 'posted',
     'drill-idem-0003', 'drill-fp-0003',
     'f1f1f1f1-0000-4000-8000-000000000001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-28T10:00:00Z', '2026-01-28T10:00:00Z');

INSERT INTO account_movements
    (id, account_id, direction, amount, source_type, source_id, occurred_at,
     status, created_by, created_at)
VALUES
    ('f2f2f2f2-0000-4000-8000-000000000005',
     'f1f1f1f1-0000-4000-8000-000000000001', 'inflow', 80.00, 'payment',
     'dddddddd-0000-4000-8000-000000000003', '2026-01-28T10:00:00Z',
     'active',
     '11111111-0000-4000-8000-0000000000aa', '2026-01-28T10:00:00Z');

INSERT INTO shareholder_credits
    (id, credit_number, source_payment_id, shareholder_id, amount,
     currency, status, note, idempotency_key, idempotency_fingerprint,
     created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('a9a9a9a9-0000-4000-8000-000000000001', 9001,
     'dddddddd-0000-4000-8000-000000000003',
     '44444444-0000-4000-8000-000000000001', 80.00, 'TRY', 'active',
     'drill: tahsilat kalanı', 'drill-cidem-0001', 'drill-cfp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-28T10:05:00Z', '2026-01-28T10:05:00Z');

INSERT INTO credit_applications
    (id, credit_id, assessment_id, amount, currency, mode, status,
     idempotency_key, idempotency_fingerprint,
     reversed_at, reversed_by, reversal_reason, created_by, created_at)
VALUES
    -- Automatic offset during assessment generation (no client key).
    ('b1b1b1b1-0000-4000-8000-000000000001',
     'a9a9a9a9-0000-4000-8000-000000000001',
     'acacacac-0000-4000-8000-000000000001', 30.00, 'TRY', 'automatic',
     'active', NULL, NULL, NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-28T10:06:00Z'),
    -- A REVERSED manual application — reversal bookkeeping survives.
    ('b1b1b1b1-0000-4000-8000-000000000002',
     'a9a9a9a9-0000-4000-8000-000000000001',
     'acacacac-0000-4000-8000-000000000001', 25.00, 'TRY', 'manual',
     'reversed', 'drill-capp-0001', 'drill-capfp-0001',
     '2026-01-29T09:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı mahsup',
     '11111111-0000-4000-8000-0000000000aa', '2026-01-28T10:07:00Z');

-- STEP-010: financial categories + income/expense entries.
-- Each posted entry owns exactly ONE movement; provenance is relational
-- (source_type 'income'/'expense' + account_movement_id). The reversed
-- expense keeps both entry AND movement reversal bookkeeping.
INSERT INTO financial_categories
    (id, category_type, name, description, status, created_by) VALUES
    ('c0c0c0c0-0000-4000-8000-000000000001', 'income', 'Yedek Gelir Türü',
     'drill: gelir kategorisi', 'active',
     '11111111-0000-4000-8000-0000000000aa'),
    ('c0c0c0c0-0000-4000-8000-000000000002', 'expense', 'Yedek Gider Türü',
     'drill: gider kategorisi', 'active',
     '11111111-0000-4000-8000-0000000000aa');

-- Movements first: the entry's account_movement_id is a hard FK.
INSERT INTO account_movements
    (id, account_id, direction, amount, source_type, source_id, occurred_at,
     status, reversed_at, reversed_by, reversal_reason, created_by, created_at)
VALUES
    -- Income 9001: +500.00 inflow on cash.
    ('f2f2f2f2-0000-4000-8000-000000000006',
     'f1f1f1f1-0000-4000-8000-000000000001', 'inflow', 500.00, 'income',
     'e1e1e1e1-0000-4000-8000-000000000001', '2026-01-30T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-30T10:00:00Z'),
    -- Expense 9001: -60.00 outflow on cash, still active.
    ('f2f2f2f2-0000-4000-8000-000000000007',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 60.00, 'expense',
     'e2e2e2e2-0000-4000-8000-000000000001', '2026-01-30T11:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-30T11:00:00Z'),
    -- Expense 9002: -25.00 outflow on cash, REVERSED (excluded from
    -- derived balance; the row survives with reversal bookkeeping).
    ('f2f2f2f2-0000-4000-8000-000000000008',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 25.00, 'expense',
     'e2e2e2e2-0000-4000-8000-000000000002', '2026-01-30T12:00:00Z',
     'reversed', '2026-01-30T14:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: hatalı gider',
     '11111111-0000-4000-8000-0000000000aa', '2026-01-30T12:00:00Z');

INSERT INTO income_entries
    (id, income_number, financial_account_id, category_id, amount,
     currency, occurred_at, description, counterparty, reference_no,
     account_movement_id, status, idempotency_key, idempotency_fingerprint,
     created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('e1e1e1e1-0000-4000-8000-000000000001', 9001,
     'f1f1f1f1-0000-4000-8000-000000000001',
     'c0c0c0c0-0000-4000-8000-000000000001', 500.00, 'TRY',
     '2026-01-30T10:00:00Z', 'drill: stant kirası geliri',
     'Yedek Pazar Yeri', 'DRILL-G-001',
     'f2f2f2f2-0000-4000-8000-000000000006', 'posted',
     'drill-iidem-0001', 'drill-iifp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-30T10:00:00Z', '2026-01-30T10:00:00Z');

INSERT INTO expense_entries
    (id, expense_number, financial_account_id, category_id, amount,
     currency, occurred_at, description, counterparty, reference_no,
     account_movement_id, status, idempotency_key, idempotency_fingerprint,
     reversed_at, reversed_by, reversal_reason,
     created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('e2e2e2e2-0000-4000-8000-000000000001', 9001,
     'f1f1f1f1-0000-4000-8000-000000000001',
     'c0c0c0c0-0000-4000-8000-000000000002', 60.00, 'TRY',
     '2026-01-30T11:00:00Z', 'drill: kırtasiye alımı',
     'Yedek Kırtasiye', 'DRILL-X-001',
     'f2f2f2f2-0000-4000-8000-000000000007', 'posted',
     'drill-eidem-0001', 'drill-eifp-0001',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-30T11:00:00Z', '2026-01-30T11:00:00Z'),
    -- Reversed expense — entry and movement reversal bookkeeping
    -- must survive the restore.
    ('e2e2e2e2-0000-4000-8000-000000000002', 9002,
     'f1f1f1f1-0000-4000-8000-000000000001',
     'c0c0c0c0-0000-4000-8000-000000000002', 25.00, 'TRY',
     '2026-01-30T12:00:00Z', 'drill: hatalı gider kaydı',
     NULL, NULL,
     'f2f2f2f2-0000-4000-8000-000000000008', 'reversed',
     'drill-eidem-0002', 'drill-eifp-0002',
     '2026-01-30T14:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı gider',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-30T12:00:00Z', '2026-01-30T14:00:00Z');

-- STEP-011: Share Return + Entitlement + Settlement.
-- Share 9003 (owner A) went through the full lifecycle: requested →
-- finalized → CLOSED, ownership interval ended at the effective cutoff.
-- The finalized case crystallized TWO rights: a determined principal
-- (partially settled) and an undetermined profit right (NULL ≠ 0.00).
-- One posted settlement + one REVERSED settlement — each bound 1:1 to
-- its own share_return_settlement-sourced movement.
INSERT INTO shares (id, share_number, status, created_by)
OVERRIDING SYSTEM VALUE
VALUES
    ('67676767-0000-4000-8000-000000000001', 9003, 'closed',
     '11111111-0000-4000-8000-0000000000aa');

INSERT INTO share_events
    (id, share_id, event_type, occurred_at, from_shareholder_id,
     to_shareholder_id, status_from, status_to, actor_user_id) VALUES
    ('77777777-0000-4000-8000-000000000004',
     '67676767-0000-4000-8000-000000000001', 'initial_acquisition',
     '2025-03-01T00:00:00Z', NULL, '44444444-0000-4000-8000-000000000001',
     NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa'),
    ('77777777-0000-4000-8000-000000000005',
     '67676767-0000-4000-8000-000000000001', 'return_requested',
     '2026-02-05T10:00:00Z', '44444444-0000-4000-8000-000000000001', NULL,
     'active', 'return_pending',
     '11111111-0000-4000-8000-0000000000aa'),
    ('77777777-0000-4000-8000-000000000006',
     '67676767-0000-4000-8000-000000000001', 'return_finalized',
     '2026-02-10T10:00:00Z', '44444444-0000-4000-8000-000000000001', NULL,
     'return_pending', 'closed',
     '11111111-0000-4000-8000-0000000000aa');

INSERT INTO share_ownerships
    (id, share_id, shareholder_id, started_at, ended_at,
     acquisition_type, source_event_id, created_by) VALUES
    ('88888888-0000-4000-8000-000000000004',
     '67676767-0000-4000-8000-000000000001',
     '44444444-0000-4000-8000-000000000001',
     '2025-03-01T00:00:00Z', '2026-02-10T00:00:00+03', 'founder',
     '77777777-0000-4000-8000-000000000004',
     '11111111-0000-4000-8000-0000000000aa');

INSERT INTO share_returns
    (id, return_number, share_id, shareholder_id, owner_display_name,
     share_number, ownership_started_at, requested_at,
     effective_return_date, reason, status, finalized_at, finalized_by,
     idempotency_key, idempotency_fingerprint, created_by,
     created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('d5d5d5d5-0000-4000-8000-000000000001', 9001,
     '67676767-0000-4000-8000-000000000001',
     '44444444-0000-4000-8000-000000000001',
     'Yedek Hissedarı · Vasi: Vasi Test · Aile No 900002',
     9003, '2025-03-01T00:00:00Z', '2026-02-05T10:00:00Z',
     '2026-02-10', 'drill: çıkış', 'finalized',
     '2026-02-10T10:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill-ridem-0001', 'drill-rfp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-05T10:00:00Z', '2026-02-10T10:00:00Z');

INSERT INTO share_return_entitlements
    (id, entitlement_number, share_return_id, entitlement_type,
     beneficiary_shareholder_id, amount, currency, due_date,
     policy_reference, description, recognized_at, determined_at,
     status, created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    -- Principal: determined 1.000,00; 300 posted + 100 reversed →
    -- remaining 800.00 (derived, never stored).
    ('e7e7e7e7-0000-4000-8000-000000000001', 9001,
     'd5d5d5d5-0000-4000-8000-000000000001', 'principal',
     '44444444-0000-4000-8000-000000000001', 1000.00, 'TRY',
     '2026-03-10', 'drill: YK-2026-01', 'drill: ana para iadesi',
     '2026-02-10T10:00:00Z', '2026-02-10T10:00:00Z',
     'partially_settled',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-10T10:00:00Z', '2026-02-12T10:00:00Z'),
    -- Profit right: recognized but the formula is policy-driven —
    -- amount NULL means "not yet determined", never zero.
    ('e7e7e7e7-0000-4000-8000-000000000002', 9002,
     'd5d5d5d5-0000-4000-8000-000000000001', 'profit',
     '44444444-0000-4000-8000-000000000001', NULL, 'TRY',
     NULL, 'drill: GK beklenecek', NULL,
     '2026-02-10T10:00:00Z', NULL,
     'open',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-10T10:00:00Z', '2026-02-10T10:00:00Z');

-- Movements first: each settlement's account_movement_id is a hard FK.
INSERT INTO account_movements
    (id, account_id, direction, amount, source_type, source_id, occurred_at,
     status, reversed_at, reversed_by, reversal_reason, created_by, created_at)
VALUES
    -- Settlement 9001: −300.00 outflow on cash, posted.
    ('f2f2f2f2-0000-4000-8000-000000000009',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 300.00,
     'share_return_settlement',
     'f4f4f4f4-0000-4000-8000-000000000001', '2026-02-11T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-02-11T10:00:00Z'),
    -- Settlement 9002: −100.00 outflow on cash, REVERSED.
    ('f2f2f2f2-0000-4000-8000-000000000010',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 100.00,
     'share_return_settlement',
     'f4f4f4f4-0000-4000-8000-000000000002', '2026-02-12T10:00:00Z',
     'reversed', '2026-02-12T14:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: hatalı ödeme',
     '11111111-0000-4000-8000-0000000000aa', '2026-02-12T10:00:00Z');

INSERT INTO share_return_settlements
    (id, settlement_number, entitlement_id, financial_account_id, amount,
     currency, settled_at, account_movement_id, status, idempotency_key,
     idempotency_fingerprint, reversed_at, reversed_by, reversal_reason,
     created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('f4f4f4f4-0000-4000-8000-000000000001', 9001,
     'e7e7e7e7-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000001', 300.00, 'TRY',
     '2026-02-11T10:00:00Z',
     'f2f2f2f2-0000-4000-8000-000000000009', 'posted',
     'drill-sidem-0001', 'drill-sfp-0001',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-11T10:00:00Z', '2026-02-11T10:00:00Z'),
    ('f4f4f4f4-0000-4000-8000-000000000002', 9002,
     'e7e7e7e7-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000001', 100.00, 'TRY',
     '2026-02-12T10:00:00Z',
     'f2f2f2f2-0000-4000-8000-000000000010', 'reversed',
     'drill-sidem-0002', 'drill-sfp-0002',
     '2026-02-12T14:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı ödeme',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-12T10:00:00Z', '2026-02-12T14:00:00Z');

-- =====================================================================
-- STEP-012: Investments — identity, acquisition funding, valuation
-- history, investment income, disposal + actual proceeds.
-- =====================================================================

INSERT INTO investments
    (id, investment_number, name, investment_type, description,
     location, reference, counterparty_name, acquired_at, status,
     disposed_at, disposed_by, cancelled_at, cancelled_by,
     cancellation_reason, idempotency_key, idempotency_fingerprint,
     created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    -- 9001: disposed real estate — funded, valued, earned, sold.
    ('10101010-0000-4000-8000-000000000001', 9001, 'Drill Deposu',
     'real_estate', 'drill: depo', 'Organize Sanayi Bölgesi',
     'TAPU-9001', NULL, '2026-01-10', 'disposed',
     '2026-04-01T10:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     NULL, NULL, NULL,
     'drill-iidem-0001', 'drill-ifp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-10T10:00:00Z', '2026-04-01T10:00:00Z'),
    -- 9002: active business holding — identity only, no events.
    ('10101010-0000-4000-8000-000000000002', 9002, 'Drill Ortaklığı',
     'business', NULL, NULL, NULL, 'Drill İşletmesi AŞ', '2026-01-15',
     'active', NULL, NULL, NULL, NULL, NULL,
     'drill-iidem-0002', 'drill-ifp-0002',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-15T10:00:00Z', '2026-01-15T10:00:00Z');

-- Movements first: every funding/income/proceeds row hard-FKs its leg.
INSERT INTO account_movements
    (id, account_id, direction, amount, source_type, source_id, occurred_at,
     status, reversed_at, reversed_by, reversal_reason, created_by, created_at)
VALUES
    -- Funding 9001: −200.00 outflow on cash, posted.
    ('f2f2f2f2-0000-4000-8000-000000000011',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 200.00,
     'investment_funding',
     '12121212-0000-4000-8000-000000000001', '2026-01-10T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-01-10T10:00:00Z'),
    -- Funding 9002: −50.00 outflow on cash, REVERSED.
    ('f2f2f2f2-0000-4000-8000-000000000012',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 50.00,
     'investment_funding',
     '12121212-0000-4000-8000-000000000002', '2026-01-11T10:00:00Z',
     'reversed', '2026-01-11T15:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: hatalı finansman',
     '11111111-0000-4000-8000-0000000000aa', '2026-01-11T10:00:00Z'),
    -- Investment income 9001: +25.00 inflow on cash, posted.
    ('f2f2f2f2-0000-4000-8000-000000000013',
     'f1f1f1f1-0000-4000-8000-000000000001', 'inflow', 25.00,
     'investment_income',
     '16161616-0000-4000-8000-000000000001', '2026-02-01T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-02-01T10:00:00Z'),
    -- Investment income 9002: +10.00 inflow on cash, REVERSED.
    ('f2f2f2f2-0000-4000-8000-000000000014',
     'f1f1f1f1-0000-4000-8000-000000000001', 'inflow', 10.00,
     'investment_income',
     '16161616-0000-4000-8000-000000000002', '2026-02-02T10:00:00Z',
     'reversed', '2026-02-02T14:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: hatalı gelir',
     '11111111-0000-4000-8000-0000000000aa', '2026-02-02T10:00:00Z'),
    -- Disposal proceeds leg: +320.00 inflow on BANK, posted.
    ('f2f2f2f2-0000-4000-8000-000000000015',
     'f1f1f1f1-0000-4000-8000-000000000002', 'inflow', 320.00,
     'investment_disposal',
     '20202020-0000-4000-8000-000000000001', '2026-04-01T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-04-01T10:00:00Z');

INSERT INTO investment_fundings
    (id, funding_number, investment_id, financial_account_id, amount,
     currency, occurred_at, reference, note, account_movement_id, status,
     idempotency_key, idempotency_fingerprint, reversed_at, reversed_by,
     reversal_reason, created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('12121212-0000-4000-8000-000000000001', 9001,
     '10101010-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000001', 200.00, 'TRY',
     '2026-01-10T10:00:00Z', 'SÖZ-9001', NULL,
     'f2f2f2f2-0000-4000-8000-000000000011', 'posted',
     'drill-fidem-0001', 'drill-ffp-0001',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-10T10:00:00Z', '2026-01-10T10:00:00Z'),
    ('12121212-0000-4000-8000-000000000002', 9002,
     '10101010-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000001', 50.00, 'TRY',
     '2026-01-11T10:00:00Z', NULL, NULL,
     'f2f2f2f2-0000-4000-8000-000000000012', 'reversed',
     'drill-fidem-0002', 'drill-ffp-0002',
     '2026-01-11T15:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı finansman',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-11T10:00:00Z', '2026-01-11T15:00:00Z');

INSERT INTO investment_valuations
    (id, valuation_number, investment_id, valuation_date, amount,
     currency, method, source, note, status, idempotency_key,
     idempotency_fingerprint, cancelled_at, cancelled_by,
     cancellation_reason, created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('14141414-0000-4000-8000-000000000001', 9001,
     '10101010-0000-4000-8000-000000000001', '2026-02-15', 300.00, 'TRY',
     'Emsal karşılaştırma', 'Eksper raporu ER-9', NULL, 'recorded',
     'drill-videm-0001', 'drill-vfp-0001',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-15T10:00:00Z', '2026-02-15T10:00:00Z'),
    -- Cancelled valuation keeps the row + reason; never deleted.
    ('14141414-0000-4000-8000-000000000002', 9002,
     '10101010-0000-4000-8000-000000000001', '2026-02-20', 310.00, 'TRY',
     'Emsal karşılaştırma', NULL, NULL, 'cancelled',
     'drill-videm-0002', 'drill-vfp-0002',
     '2026-02-20T15:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı değerleme',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-20T10:00:00Z', '2026-02-20T15:00:00Z');

INSERT INTO investment_incomes
    (id, income_number, investment_id, financial_account_id, amount,
     currency, occurred_at, description, counterparty, reference_no,
     account_movement_id, status, idempotency_key,
     idempotency_fingerprint, reversed_at, reversed_by, reversal_reason,
     created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('16161616-0000-4000-8000-000000000001', 9001,
     '10101010-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000001', 25.00, 'TRY',
     '2026-02-01T10:00:00Z', 'drill: Şubat kirası', 'Kiracı AŞ', NULL,
     'f2f2f2f2-0000-4000-8000-000000000013', 'posted',
     'drill-gidem-0001', 'drill-gfp-0001',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-01T10:00:00Z', '2026-02-01T10:00:00Z'),
    ('16161616-0000-4000-8000-000000000002', 9002,
     '10101010-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000001', 10.00, 'TRY',
     '2026-02-02T10:00:00Z', 'drill: hatalı gelir', NULL, NULL,
     'f2f2f2f2-0000-4000-8000-000000000014', 'reversed',
     'drill-gidem-0002', 'drill-gfp-0002',
     '2026-02-02T14:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı gelir',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-02T10:00:00Z', '2026-02-02T14:00:00Z');

INSERT INTO investment_disposals
    (id, disposal_number, investment_id, disposed_at,
     consideration_amount, currency, counterparty_name, reference, note,
     status, idempotency_key, idempotency_fingerprint, created_by,
     created_at)
OVERRIDING SYSTEM VALUE
VALUES
    -- Agreed price 350.00 is informational; actual cash received is
    -- the 320.00 proceeds leg — the difference is NEVER a computed
    -- gain/loss (formula UNRESOLVED, docs/08).
    ('18181818-0000-4000-8000-000000000001', 9001,
     '10101010-0000-4000-8000-000000000001', '2026-04-01',
     350.00, 'TRY', 'Drill Alıcı AŞ', 'SAT-9001', NULL, 'posted',
     'drill-didem-0001', 'drill-dfp-0001',
     '11111111-0000-4000-8000-0000000000aa', '2026-04-01T10:00:00Z');

INSERT INTO investment_disposal_proceeds
    (id, disposal_id, financial_account_id, amount, currency,
     occurred_at, reference, account_movement_id, created_at)
VALUES
    ('20202020-0000-4000-8000-000000000001',
     '18181818-0000-4000-8000-000000000001',
     'f1f1f1f1-0000-4000-8000-000000000002', 320.00, 'TRY',
     '2026-04-01T10:00:00Z', 'EFT-9001',
     'f2f2f2f2-0000-4000-8000-000000000015', '2026-04-01T10:00:00Z');

-- =====================================================================
-- STEP-013: Social Aid — restricted fund, donations, aid disbursements.
-- Fund ≠ Financial Account: the fund row carries NO balance; restricted
-- availability is always derived (posted 300.00 donation − posted
-- 120.00 disbursement = 180.00). Donor 9001 links the canonical Person
-- (a third-party payer — donor ≠ shareholder proof); donor 9002 is an
-- external organization by display name only. One reversed donation and
-- one reversed disbursement keep entry + movement bookkeeping.
-- =====================================================================

INSERT INTO social_aid_funds
    (id, fund_number, name, description, status, cancelled_at,
     cancelled_by, cancellation_reason, idempotency_key,
     idempotency_fingerprint, created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    ('30303030-0000-4000-8000-000000000001', 9001, 'Drill Eğitim Fonu',
     'drill: burs ve eğitim destekleri', 'active', NULL, NULL, NULL,
     'drill-aidem-0001', 'drill-afp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-01T10:00:00Z', '2026-02-01T10:00:00Z'),
    -- Cancelled fund keeps reason + actor; a fund never disappears.
    ('30303030-0000-4000-8000-000000000002', 9002, 'Drill Gıda Fonu',
     NULL, 'cancelled', '2026-03-01T10:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: program iptal',
     'drill-aidem-0002', 'drill-afp-0002',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-05T10:00:00Z', '2026-03-01T10:00:00Z');

-- Movements first: every donation/disbursement hard-FKs its leg.
INSERT INTO account_movements
    (id, account_id, direction, amount, source_type, source_id, occurred_at,
     status, reversed_at, reversed_by, reversal_reason, created_by, created_at)
VALUES
    -- Donation 9001: +300.00 inflow on cash, posted.
    ('f2f2f2f2-0000-4000-8000-000000000016',
     'f1f1f1f1-0000-4000-8000-000000000001', 'inflow', 300.00,
     'social_aid_donation',
     '31313131-0000-4000-8000-000000000001', '2026-02-10T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-02-10T10:00:00Z'),
    -- Donation 9002: +40.00 inflow on cash, REVERSED.
    ('f2f2f2f2-0000-4000-8000-000000000017',
     'f1f1f1f1-0000-4000-8000-000000000001', 'inflow', 40.00,
     'social_aid_donation',
     '31313131-0000-4000-8000-000000000002', '2026-02-11T10:00:00Z',
     'reversed', '2026-02-11T15:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: hatalı bağış',
     '11111111-0000-4000-8000-0000000000aa', '2026-02-11T10:00:00Z'),
    -- Disbursement 9001: −120.00 outflow on cash, posted.
    ('f2f2f2f2-0000-4000-8000-000000000018',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 120.00,
     'social_aid_disbursement',
     '32323232-0000-4000-8000-000000000001', '2026-02-15T10:00:00Z',
     'active', NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa', '2026-02-15T10:00:00Z'),
    -- Disbursement 9002: −50.00 outflow on cash, REVERSED.
    ('f2f2f2f2-0000-4000-8000-000000000019',
     'f1f1f1f1-0000-4000-8000-000000000001', 'outflow', 50.00,
     'social_aid_disbursement',
     '32323232-0000-4000-8000-000000000002', '2026-02-16T10:00:00Z',
     'reversed', '2026-02-16T15:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: hatalı yardım',
     '11111111-0000-4000-8000-0000000000aa', '2026-02-16T10:00:00Z');

INSERT INTO social_aid_donations
    (id, donation_number, fund_id, donor_person_id, donor_display_name,
     financial_account_id, amount, currency, occurred_at, reference, note,
     account_movement_id, status, idempotency_key, idempotency_fingerprint,
     reversed_at, reversed_by, reversal_reason, created_by, created_at,
     updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    -- Donor = canonical Person (the third-party payer — membership is
    -- NEVER required for a donor).
    ('31313131-0000-4000-8000-000000000001', 9001,
     '30303030-0000-4000-8000-000000000001',
     '22222222-0000-4000-8000-000000000004', NULL,
     'f1f1f1f1-0000-4000-8000-000000000001', 300.00, 'TRY',
     '2026-02-10T10:00:00Z', 'MKB-9001', 'drill: burs bağışı',
     'f2f2f2f2-0000-4000-8000-000000000016', 'posted',
     'drill-didem-0001', 'drill-dfp-0001',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-10T10:00:00Z', '2026-02-10T10:00:00Z'),
    -- Donor = external organization (display name only), REVERSED.
    ('31313131-0000-4000-8000-000000000002', 9002,
     '30303030-0000-4000-8000-000000000001',
     NULL, 'Drill Hayırsever A.Ş.',
     'f1f1f1f1-0000-4000-8000-000000000001', 40.00, 'TRY',
     '2026-02-11T10:00:00Z', NULL, NULL,
     'f2f2f2f2-0000-4000-8000-000000000017', 'reversed',
     'drill-didem-0002', 'drill-dfp-0002',
     '2026-02-11T15:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı bağış',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-11T10:00:00Z', '2026-02-11T15:00:00Z');

INSERT INTO social_aid_disbursements
    (id, disbursement_number, fund_id, beneficiary_person_id,
     beneficiary_display_name, financial_account_id, amount, currency,
     occurred_at, reason, reference, account_movement_id, status,
     idempotency_key, idempotency_fingerprint, reversed_at, reversed_by,
     reversal_reason, created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    -- Beneficiary = canonical Person (a shareholder may also be an aid
    -- beneficiary — the role is independent).
    ('32323232-0000-4000-8000-000000000001', 9001,
     '30303030-0000-4000-8000-000000000001',
     '22222222-0000-4000-8000-000000000002', NULL,
     'f1f1f1f1-0000-4000-8000-000000000001', 120.00, 'TRY',
     '2026-02-15T10:00:00Z', 'drill: burs ödemesi', NULL,
     'f2f2f2f2-0000-4000-8000-000000000018', 'posted',
     'drill-yidem-0001', 'drill-yfp-0001',
     NULL, NULL, NULL,
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-15T10:00:00Z', '2026-02-15T10:00:00Z'),
    -- Beneficiary = external family (display name only), REVERSED.
    ('32323232-0000-4000-8000-000000000002', 9002,
     '30303030-0000-4000-8000-000000000001',
     NULL, 'Drill Ailesi',
     'f1f1f1f1-0000-4000-8000-000000000001', 50.00, 'TRY',
     '2026-02-16T10:00:00Z', 'drill: gıda desteği', NULL,
     'f2f2f2f2-0000-4000-8000-000000000019', 'reversed',
     'drill-yidem-0002', 'drill-yfp-0002',
     '2026-02-16T15:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: hatalı yardım',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-16T10:00:00Z', '2026-02-16T15:00:00Z');

-- ---------------------------------------------------------------------
-- STEP-014: Governance — bodies, temporal memberships, decisions, votes.
-- Governance is EVIDENCE, never money: zero account_movements legs.
-- ---------------------------------------------------------------------

INSERT INTO governance_bodies
    (id, body_number, name, body_type, description, status,
     closed_at, closed_by, idempotency_key, idempotency_fingerprint,
     created_by, created_at, updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    -- Active organ.
    ('40404040-0000-4000-8000-000000000001', 9001,
     'Drill Yönetim Kurulu', 'Yönetim Kurulu', 'drill: ana organ',
     'active', NULL, NULL,
     'drill-gbidem-0001', 'drill-gbfp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-05T10:00:00Z', '2026-01-05T10:00:00Z'),
    -- Closed organ keeps close evidence + ALL history.
    ('40404040-0000-4000-8000-000000000002', 9002,
     'Drill Denetim Kurulu', 'Denetim Kurulu', NULL,
     'closed', '2026-03-01T10:00:00Z',
     '11111111-0000-4000-8000-0000000000aa',
     'drill-gbidem-0002', 'drill-gbfp-0002',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-05T11:00:00Z', '2026-03-01T10:00:00Z');

INSERT INTO governance_memberships
    (id, body_id, person_id, title, started_at, ended_at, ended_by,
     end_reason, idempotency_key, idempotency_fingerprint,
     created_by, created_at, updated_at)
VALUES
    -- Active seat on the open board.
    ('41414141-0000-4000-8000-000000000001',
     '40404040-0000-4000-8000-000000000001',
     '22222222-0000-4000-8000-000000000001', 'Üye',
     '2026-01-06T10:00:00Z', NULL, NULL, NULL,
     'drill-gmidem-0001', 'drill-gmfp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-06T10:00:00Z', '2026-01-06T10:00:00Z'),
    -- Seat that was ACTIVE when its vote was cast (2026-02-06) and
    -- ended later — ending a term never rewrites who voted.
    ('41414141-0000-4000-8000-000000000002',
     '40404040-0000-4000-8000-000000000001',
     '22222222-0000-4000-8000-000000000002', 'Üye',
     '2026-01-06T11:00:00Z', '2026-02-28T10:00:00Z',
     '11111111-0000-4000-8000-0000000000aa', 'drill: görev süresi doldu',
     'drill-gmidem-0002', 'drill-gmfp-0002',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-06T11:00:00Z', '2026-02-28T10:00:00Z'),
    -- Historical seat on the closed board.
    ('41414141-0000-4000-8000-000000000003',
     '40404040-0000-4000-8000-000000000002',
     '22222222-0000-4000-8000-000000000003', NULL,
     '2026-01-06T12:00:00Z', '2026-02-20T10:00:00Z',
     '11111111-0000-4000-8000-0000000000aa',
     'drill: kurul kapanışı öncesi',
     'drill-gmidem-0003', 'drill-gmfp-0003',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-06T12:00:00Z', '2026-02-20T10:00:00Z');

INSERT INTO governance_decisions
    (id, decision_number, body_id, title, decision_text, decision_on,
     effective_on, status, opened_at, opened_by, finalized_at,
     finalized_by, eligible_count, approve_count, reject_count,
     abstain_count, cancelled_at, cancelled_by, cancellation_reason,
     idempotency_key, idempotency_fingerprint, created_by, created_at,
     updated_at)
OVERRIDING SYSTEM VALUE
VALUES
    -- APPROVED with the frozen finalization snapshot: electorate 2,
    -- tally 1/0/1. The snapshot is evidence — no pass rule exists.
    ('42424242-0000-4000-8000-000000000001', 9001,
     '40404040-0000-4000-8000-000000000001',
     'Drill Bütçe Kararı', 'drill: 2026 bütçesi onaylansın',
     '2026-02-05', '2026-03-01', 'approved',
     '2026-02-05T10:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     '2026-02-05T12:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     2, 1, 0, 1,
     NULL, NULL, NULL,
     'drill-gdidem-0001', 'drill-gdfp-0001',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-04T10:00:00Z', '2026-02-05T12:00:00Z'),
    -- CANCELLED draft — cancellation keeps actor + reason, no delete.
    ('42424242-0000-4000-8000-000000000002', 9002,
     '40404040-0000-4000-8000-000000000001',
     'Drill Vazgeçilen Karar', 'drill: gündemden düşürülen taslak',
     '2026-02-08', NULL, 'cancelled',
     NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
     '2026-02-09T10:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     'drill: gündemden düştü',
     'drill-gdidem-0002', 'drill-gdfp-0002',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-02-08T10:00:00Z', '2026-02-09T10:00:00Z'),
    -- REJECTED decision on the closed body — history survives close.
    ('42424242-0000-4000-8000-000000000003', 9003,
     '40404040-0000-4000-8000-000000000002',
     'Drill Denetim Kararı', 'drill: rapor reddedilsin',
     '2026-01-20', NULL, 'rejected',
     '2026-01-20T10:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     '2026-01-20T11:00:00Z', '11111111-0000-4000-8000-0000000000aa',
     1, 0, 1, 0,
     NULL, NULL, NULL,
     'drill-gdidem-0003', 'drill-gdfp-0003',
     '11111111-0000-4000-8000-0000000000aa',
     '2026-01-19T10:00:00Z', '2026-01-20T11:00:00Z');

INSERT INTO governance_votes
    (id, decision_id, membership_id, person_id, choice, cast_at, note,
     recorded_by, idempotency_key, idempotency_fingerprint, created_at)
VALUES
    ('43434343-0000-4000-8000-000000000001',
     '42424242-0000-4000-8000-000000000001',
     '41414141-0000-4000-8000-000000000001',
     '22222222-0000-4000-8000-000000000001', 'approve',
     '2026-02-05T10:30:00Z', 'drill: lehte',
     '11111111-0000-4000-8000-0000000000aa',
     'drill-gvidem-0001', 'drill-gvfp-0001', '2026-02-05T10:30:00Z'),
    -- membership_id snapshots eligibility: this seat ended AFTER the
    -- vote — restoring must keep the voter historically eligible.
    ('43434343-0000-4000-8000-000000000002',
     '42424242-0000-4000-8000-000000000001',
     '41414141-0000-4000-8000-000000000002',
     '22222222-0000-4000-8000-000000000002', 'abstain',
     '2026-02-06T10:00:00Z', NULL,
     '11111111-0000-4000-8000-0000000000aa',
     'drill-gvidem-0002', 'drill-gvfp-0002', '2026-02-06T10:00:00Z'),
    ('43434343-0000-4000-8000-000000000003',
     '42424242-0000-4000-8000-000000000003',
     '41414141-0000-4000-8000-000000000003',
     '22222222-0000-4000-8000-000000000003', 'reject',
     '2026-01-20T10:30:00Z', NULL,
     '11111111-0000-4000-8000-0000000000aa',
     'drill-gvidem-0003', 'drill-gvfp-0003', '2026-01-20T10:30:00Z');

COMMIT;
