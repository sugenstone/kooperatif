# STEP-012 — INVESTMENT & INVESTMENT CASH-FLOW FOUNDATION

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
main == origin/main == 1ed4be1  (STEP-011)
```

Authoritative inputs:

```text
docs/08-INVESTMENTS-ASSETS-REAL-ESTATE-BUSINESS-HOLDINGS.md
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

Approved scope: first-class cooperative investments — identity,
acquisition funding, valuation history, actual investment income,
disposal with actual proceeds.

---

# 1. DOMAIN PRINCIPLE — FOUR CONCEPTS STAY SEPARATE (docs/08)

```text
INVESTMENT IDENTITY   investments — what the cooperative owns.
                      Creating one moves ZERO money.

ACQUISITION COST      investment_fundings — REAL money paid out.
                      Each leg = exactly ONE outflow Account Movement
                      (source_type 'investment_funding').

ESTIMATED VALUE       investment_valuations — dated informational
                      history. ZERO movements, ZERO Income, never
                      rewrites cost.

CASH RESULT           investment_incomes ('investment_income' inflow)
                      + investment_disposal_proceeds
                      ('investment_disposal' inflow). One leg = one
                      movement — never double-counted.
```

No column ever collapses these into "cost" or "profit" on the
Investment row. `totalFunded`, `latestValuation`, `totalIncome`,
`totalProceeds` are always DERIVED — never stored.

# 2. SCHEMA (migration 0012)

- `account_movements.source_type` extended:
  `investment_funding | investment_income | investment_disposal`
  join `{payment, transfer, income, expense,
  share_return_settlement}`; `UNIQUE (account_id, source_type,
  source_id)` still prevents a source posting the same leg twice.
- `investments` — `investment_number` (GENERATED IDENTITY), `name`,
  `investment_type ∈ {real_estate, business}` (the minimal
  authoritative distinction; the full taxonomy is an open decision
  in docs/08), bounded descriptive metadata (`description`,
  `location`, `reference`, `counterparty_name`), business `acquired_at`
  DATE, lifecycle `active → disposed | cancelled` with DB-enforced
  consistency CHECKs. No money columns.
- `investment_fundings` — `funding_number`, investment + financial
  account FKs, exact positive `NUMERIC(19,2)`, `occurred_at`, hard 1:1
  `account_movement_id` UNIQUE, `posted → reversed` with a consistency
  CHECK, durable `idempotency_key` UNIQUE + fingerprint.
- `investment_valuations` — `valuation_number`, `valuation_date`,
  exact positive amount, `method` + `source` preserved (docs/08,
  docs/17 POLICY_DRIVEN), `recorded → cancelled` consistency CHECK.
  No movement binding exists by construction.
- `investment_incomes` — `income_number`, investment + financial
  account FKs, positive amount, `description` + `counterparty` +
  `reference_no`, hard 1:1 `account_movement_id` UNIQUE,
  `posted → reversed` consistency CHECK.
- `investment_disposals` — `disposal_number`, `disposed_at` business
  DATE, `consideration_amount` (the AGREED price — informational
  metadata only), `counterparty_name`, `reference`, `status = 'posted'`
  (terminal: disposal reversal semantics are UNRESOLVED in docs/08
  and were not invented). `UNIQUE (investment_id)` — derecognition is
  a single lifecycle event in STEP-012.
- `investment_disposal_proceeds` — one row per ACTUAL cash leg per
  Financial Account, hard 1:1 `account_movement_id` UNIQUE.
- Permissions seeded: `investments.read`, `investments.manage` →
  `Sistem Yöneticisi`.

# 3. BEHAVIORAL CONTRACT

- `POST /api/investments` creates identity only — no account, no
  amount, no movement. Cancellation requires zero financial events.
- `POST /api/investments/{id}/fundings` — locks the account under the
  deterministic ascending-UUID lock order, derives the balance,
  rejects insufficient funds `409`, inserts the funding + its ONE
  outflow movement atomically. It is NOT Expense and creates no
  `expenses`/`incomes` row; the operational summary is untouched.
- `POST /api/investments/{id}/valuations` — informational record;
  zero movements. `POST /api/investment-valuations/{id}/cancel`
  marks a wrong valuation `cancelled` with actor + reason; the row
  stays (docs/19).
- `POST /api/investments/{id}/incomes` — exactly one inflow movement;
  creates NO `incomes` row (operational income/expense reporting is
  untouched — income recognition reporting policy is an open
  decision in docs/08).
- `POST /api/investments/{id}/dispose` — records the derecognition
  event and at least one proceeds leg (422 without proceeds);
  each leg = one `investment_disposal` inflow on its own account;
  the investment transitions `active → disposed`. The agreed
  `consideration_amount` is stored as metadata — it is NEVER
  treated as received cash. No realized-gain formula exists
  (UNRESOLVED in docs/08 + docs/17).
- Reversal of a funding/income flips entry `posted → reversed` and
  movement `active → reversed` in the same transaction; history is
  preserved, nothing is deleted.
- All commands carry durable idempotency keys + payload fingerprints;
  replay returns the stored result, a reused key with a different
  payload answers `409 conflict`.
- Investment commands create NO Payment, Transfer, Shareholder
  Credit or Share Return entitlement — ever.

# 4. API SURFACE

```text
GET|POST /api/investments
GET      /api/investments/{id}
POST     /api/investments/{id}/cancel
POST     /api/investments/{id}/fundings
POST     /api/investment-fundings/{id}/reverse
POST     /api/investments/{id}/valuations
POST     /api/investment-valuations/{id}/cancel
POST     /api/investments/{id}/incomes
POST     /api/investment-incomes/{id}/reverse
POST     /api/investments/{id}/dispose
```

Lists are server-paginated (search / type / status filters). Detail
returns the full aggregate: fundings + valuations + incomes +
disposal with derived totals. Account movement listing resolves
investment source numbers.

# 5. DEFERRED SCOPE (never invented)

- Investment expense classification — UNRESOLVED (docs/08 open
  decisions: cost vs operational Expense vs capitalized cost). No
  table/column exists.
- Expected/forecast investment income — defined as FORECAST data
  (docs/17); deferred.
- Realized gain/loss formula — UNRESOLVED; not computed or stored.
- Disposal reversal — semantics undefined in docs/08; a posted
  disposal is terminal.
- Partial disposal — not defined by the specs.
- Net-asset-value inclusion / profit distribution to shareholders —
  POLICY_DRIVEN, deferred.
- Documents, responsible parties, decision references, approval
  thresholds (docs/18 configurable) — deferred.

# 6. VERIFICATION

- 15 backend tests (`apps/server/tests/investments.rs`): creation
  moves no money, funding = one outflow not expense, multi-account
  legs, insufficient-funds atomicity, concurrent fundings cannot
  overdraw, idempotency replay + conflict, funding/income reversal
  preserving history, valuation moves zero money, disposal requires
  proceeds + closes the investment, income is not operational
  income, permissions/CSRF/no-store, cancel requires no events,
  list filters/validation.
- 10 frontend spec tests (`apps/web/src/specs/investments-pages.spec.ts`).
- Real-stack E2E `scripts/e2e-step012.mjs` — every boundary asserted
  through the UI (create → fund → value → income → dispose).
- Backup drill extended: investment fixtures + provenance + derived
  cost/valuation/income/proceeds + constraint-behavior checks on the
  restored copy.
