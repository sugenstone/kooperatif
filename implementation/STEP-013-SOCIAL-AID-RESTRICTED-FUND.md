# STEP-013 — SOCIAL AID, DONATION & RESTRICTED FUND FOUNDATION

## Repository

```text
C:\Users\sugen\Documents\PROJE\kooperatif
```

Remote:

```text
https://github.com/sugenstone/kooperatif.git
```

Baseline before this step:

```text
main == origin/main == b70a96d  (STEP-012)
```

Authoritative inputs:

```text
docs/11-SOCIAL-AID.md
docs/02-DOMAIN-MODEL.md
docs/03-FINANCIAL-CORE.md
docs/15-INVARIANTS.md
docs/16-STATE-MACHINES.md
docs/17-CALCULATION-RULES.md
docs/18-AUTHORIZATION-APPROVALS.md
docs/19-AUDIT-REVERSAL-CORRECTION.md
docs/20-DATA-MODEL-DATABASE.md
docs/21-API-CONTRACTS.md
docs/22-SECURITY.md
docs/23-BACKUP-RECOVERY.md
docs/26-TESTING-QUALITY-GATES.md
ADR-003 (movement/ledger architecture)
ADR-004 (money representation)
ADR-006 (idempotency/concurrency)
ADR-013 (backup/restore)
```

Approved scope: first-class Social Aid domain — funds (restriction
identity), donors/beneficiaries, donation receipts, aid disbursements,
restricted-purpose balances, full financial-account provenance.

---

# 1. DOMAIN PRINCIPLE — SEPARATE FINANCIAL CONTEXT (docs/11, docs/15)

```text
SOCIAL AID FUND      social_aid_funds — WHAT PURPOSE money is
                     restricted to. The row carries NO balance;
                     creating one moves ZERO money.
                     "Financial Account != Fund" (docs/01).

DONATION             social_aid_donations — REAL money received FOR
                     a restricted purpose. Each = exactly ONE inflow
                     Account Movement (source_type
                     'social_aid_donation'). NOT a Payment,
                     Allocation, Credit, Income, Transfer or
                     Investment event.

AID DISBURSEMENT     social_aid_disbursements — REAL restricted
                     money paid TO a beneficiary. Each = exactly ONE
                     outflow Account Movement (source_type
                     'social_aid_disbursement'). NOT an Expense,
                     Payment, Transfer, Credit or Share Return.

RESTRICTED           derived per (fund, financial_account):
AVAILABILITY         SUM(posted donations) − SUM(posted
                     disbursements). Never stored, never edited.
```

Cooperative Finance and Social Aid Finance never merge: the
operational income/expense summary is untouched by every social-aid
event, and restricted money can only be spent from the account it was
deposited into (the conservative reading of docs/11 + docs/17 —
restricted and unrestricted resources must remain distinguishable).

# 2. SCHEMA (migration 0013)

- `account_movements.source_type` extended:
  `social_aid_donation | social_aid_disbursement` join the existing
  provenance set; `UNIQUE (account_id, source_type, source_id)` still
  prevents a source posting the same leg twice.
- `social_aid_funds` — `fund_number` (GENERATED IDENTITY), `name`,
  bounded `description`, optional `starts_on`/`ends_on` program window
  (`ends_on >= starts_on` CHECK; never auto-expiring), lifecycle
  `active → closed | cancelled` with consistency CHECKs (`closed_at`/
  `closed_by`, `cancelled_at`/`cancelled_by`/`cancellation_reason`),
  durable `idempotency_key` UNIQUE + fingerprint. No money columns.
- `social_aid_donations` — `donation_number`, fund + financial
  account FKs, exact positive `NUMERIC(19,2)`, `occurred_at`,
  `reference`, `note`, hard 1:1 `account_movement_id` UNIQUE,
  `posted → reversed` consistency CHECK, durable idempotency pair.
  Donor identity: `donor_person_id` FK → `persons` (canonical link —
  shareholder, guardian or ANY registered person; membership never
  required) OR bounded `donor_display_name`; a CHECK requires at
  least one (anonymous-donation policy is an open decision in
  docs/11 — deferred).
- `social_aid_disbursements` — mirror structure plus REQUIRED
  `reason` (the disbursement must answer why the aid was granted;
  the Aid Request/Decision approval pipeline is a deferred open
  decision). `beneficiary_person_id`/`beneficiary_display_name`
  follow the same identity CHECK.
- Permissions seeded: `social_aid.read`, `social_aid.manage` →
  `Sistem Yöneticisi`.
- Data minimization (docs/11 §privacy, docs/22): no medical,
  religious, political or case-management attributes exist anywhere
  in the schema.

# 3. BEHAVIORAL CONTRACT

- `POST /api/social-aid/funds` creates the restriction identity only —
  no account, no amount, no movement.
- `POST /api/social-aid/donations` — locks the fund, then the
  financial account; validates `active` fund + account; inserts the
  donation + its ONE inflow movement atomically.
- `POST /api/social-aid/disbursements` — locks fund + account in
  deterministic order, derives BOTH balances under the account lock,
  and rejects unless `physical_balance >= amount` AND the fund's
  restricted availability IN THAT ACCOUNT `>= amount` (`409`).
  Restricted money in another account can never fund an aid payment.
- Fund close requires every per-(fund, account) restricted
  availability to be zero — restricted money must never become
  ownerless (`409` otherwise). Cancel requires zero financial events
  and a reason. `closed`/`cancelled` funds reject all postings.
- Reversal (docs/19): `posted → reversed` flips the bound movement
  `active → reversed` in the same transaction — originals and
  bookkeeping survive. A disbursement reversal restores both the
  physical balance and the restricted availability; a DONATION
  reversal is refused when it would overdraw the physical account or
  restricted availability already consumed (`409`).
- All create/post commands carry durable `idempotency_key` + payload
  fingerprints; replay returns the stored result, a conflicting reuse
  answers `409 conflict`, a first-writer race resolves via replay
  lookup (unique-violation → re-read).
- Social-aid commands create NO Payment, Allocation, Assessment,
  Shareholder Credit, Income, Expense, Transfer, Share Return or
  Investment row — ever.

# 4. API SURFACE

```text
GET|POST /api/social-aid/funds
GET      /api/social-aid/funds/{id}
POST     /api/social-aid/funds/{id}/close
POST     /api/social-aid/funds/{id}/cancel
GET|POST /api/social-aid/donations        (?fundId=…)
GET      /api/social-aid/donations/{id}
POST     /api/social-aid/donations/{id}/reverse
GET|POST /api/social-aid/disbursements    (?fundId=…)
GET      /api/social-aid/disbursements/{id}
POST     /api/social-aid/disbursements/{id}/reverse
```

Lists are server-paginated. Fund detail returns derived totals +
per-account restricted availability next to the physical account
balance + full donation/disbursement history (each row exposing its
`accountMovementId` + `movementStatus`). All mutations are
CSRF-protected and require `social_aid.manage`; reads require
`social_aid.read`.

# 5. DEFERRED SCOPE (never invented)

- Aid Request / Aid Decision approval pipeline — authority and
  thresholds are open decisions (docs/11, docs/18); a disbursement
  carries a required `reason` + optional `reference` instead.
- Anonymous donations — open decision (docs/11); at least one donor
  identity is mandatory.
- Fund-to-fund reallocation and restricted-money account transfers —
  not defined by the specs.
- Donor/beneficiary deduplication or matching policy.
- Campaign targets, pledge tracking, periodic aid programs —
  `starts_on`/`ends_on` are informational window metadata only.
- Fund reopen — `active → closed | cancelled` is terminal in
  STEP-013 (reopen semantics undefined).

# 6. VERIFICATION

- 14 backend tests (`apps/server/tests/social_aid.rs`): fund create/
  lifecycle, donor/beneficiary identity validation, donation = ONE
  inflow movement + restricted availability, disbursement = ONE
  outflow + both balance bounds, no contamination of
  Payment/Income/Expense/Credit/Transfer tables, idempotent replay +
  conflict, reversal restores/preserves, consumed-restriction
  donation reversal refused, closed/cancelled fund rejections,
  inactive account rejection, concurrent disbursements cannot
  overdraw restricted availability, permission denials.
- 10 frontend spec tests (`apps/web/src/specs/social-aid-pages.spec.ts`).
- Real-stack E2E `scripts/e2e-step013.mjs` — every boundary asserted
  through the UI (create → donate → disburse → reject → reverse →
  close).
- Backup drill extended: fund/donation/disbursement fixtures (incl.
  reversed rows), provenance joins, derived restricted availability
  (300.00 − 120.00 = 180.00), social-aid ≠ income/expense/payment/
  transfer isolation check, and 14 constraint-behavior checks on the
  restored copy.
