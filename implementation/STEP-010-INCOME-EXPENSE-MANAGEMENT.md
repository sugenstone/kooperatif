# STEP-010 — INCOME & EXPENSE MANAGEMENT FOUNDATION

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
main == origin/main == bd7ff84  (STEP-009 + HOTFIX-001)
```

Authoritative inputs:

```text
docs/07-FINANCIAL-ACCOUNTS-INCOME-EXPENSE.md
docs/15-INVARIANTS.md
docs/16-STATE-MACHINES.md
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
ADR-011 (audit/observability)
ADR-013 (backup/restore)
```

Approved scope: operational Income and Expense management on top of the
STEP-008 Financial Account / Account Movement infrastructure.

---

# 1. DOMAIN PRINCIPLE

Income and Expense are **business events** — they explain WHY money
moved. The Account Movement remains the ONLY authoritative record of
money entering or leaving a financial account.

- Posted Income ⇒ exactly ONE `inflow` Account Movement.
- Posted Expense ⇒ exactly ONE `outflow` Account Movement.
- No `income.balance`, no `expense.balance`, no second ledger exists.
- Account balance stays `SUM(active movements)` — unchanged semantics.

# 2. SCHEMA (migration 0010)

- `financial_categories` — typed reference data
  (`category_type ∈ {income, expense}`), `status ∈ {active, inactive}`,
  `UNIQUE (category_type, lower(btrim(name)))`, `UNIQUE (id,
  category_type)` supporting the composite entry FK.
- `income_entries` / `expense_entries` — posted events with
  `GENERATED ALWAYS AS IDENTITY` entry numbers, `NUMERIC(19,2)` TRY
  amounts (`amount > 0`), `occurred_at`, `description` (required),
  `counterparty` + `reference_no` (descriptive text only — never Person
  semantics), `account_movement_id` (hard FK, `UNIQUE` ⇒ strict 1:1),
  `status ∈ {posted, reversed}` + reversal-consistency CHECK,
  durable `idempotency_key` (UNIQUE) + `idempotency_fingerprint`.
- Composite FK `(category_id, entry_kind) → financial_categories(id,
  category_type)` — a wrong-typed category is impossible at the
  database level; `entry_kind` is a constant column existing solely
  for that binding.
- `account_movements.source_type` extended to `{payment, transfer,
  income, expense}`; `UNIQUE (account_id, source_type, source_id)`
  still prevents a source posting the same leg twice.
- Permissions `income_expense.read` / `income_expense.manage` seeded
  and granted to `Sistem Yöneticisi` only.
- Ten operational seed categories (migration-owned, `created_by NULL`).

# 3. SEMANTICS

- **Posting is atomic**: account row lock (`FOR UPDATE`) → validate →
  insert entry → insert the one movement → commit. Partial states are
  impossible.
- **Negative balances stay forbidden** (STEP-008 operator decision):
  an unfundable Expense posts nothing (`409`); an Income reversal that
  would take the account below zero is rejected identically.
- **Reversal**: entry flips `posted → reversed` with actor/time/reason
  and the bound movement flips `active → reversed` in the same
  transaction. Originals always survive; no DELETE endpoint exists.
- **Idempotency** (ADR-006): posting replays on same key + same
  payload fingerprint; a reused key with a different payload answers
  `409 conflict`. First-writer races resolve through replay lookup.
- **Optimistic concurrency**: category rename requires
  `expectedUpdatedAt` (`409 stale_state`); double-reversal races
  serialize on the entry row lock — one effective reversal.
- **Provenance**: movement listing joins resolve `income`/`expense`
  source rows to their entry numbers, so an account's history answers
  "Gelir #7 / Gider #12" next to "Tahsilat #42 / Transfer #3".

# 4. BOUNDARIES PRESERVED

- Payment ≠ Income (separate `source_type`, separate totals).
- Transfer is neither Income nor Expense (paired `transfer` legs).
- Shareholder Credit assignment/application creates ZERO movements —
  untouched by this step.
- The operational summary (`incomeTotal − expenseTotal = net`) counts
  posted income/expense entries ONLY and is never the account balance.
- `counterparty` is descriptive text — no Person/Shareholder/Guardian
  coupling.
- TRY-only; all API money is canonical decimal strings (ADR-004).
- HOTFIX-001 boundary intact: editable tr-TR inputs go through
  `parseTryInput`/`canonicalToTryInput`, never raw canonical strings.

# 5. SURFACES

API: `GET|POST /api/incomes`, `GET /api/incomes/{id}`,
`POST /api/incomes/{id}/reverse`, same for `/api/expenses`;
`GET|POST /api/financial-categories`, `GET
/api/financial-categories/options`, `PATCH /api/financial-categories/{id}`,
`POST /api/financial-categories/{id}/status-change`, `GET
/api/income-expense/summary`. All mutations are Origin+CSRF-checked and
permission-gated server-side; lists are server-paginated with
date/account/category/status/search filters. No unbounded lists, no
DELETE.

Frontend (tr-TR, permission-gated nav): `/gelirler`, `/gelirler/yeni`,
`/gelirler/{id}`, `/giderler`, `/giderler/yeni`, `/giderler/{id}`,
`/gelir-gider` (operational summary — NOT a balance sheet),
`/kategoriler` (category management: list/create/rename/activate/
deactivate).

Audit events: `income_created`, `income_reversed`, `expense_created`,
`expense_reversed`, `financial_category_created`,
`financial_category_updated`, `financial_category_status_changed`.

# 6. INCIDENTAL FIX (in-scope regression)

`#[serde(flatten)]` query structs silently broke numeric query
deserialization (`?pageSize=20` → 400) on `/api/financial-accounts`
and the new list endpoints. Flatten was replaced by inlined fields in
`AccountListQuery`, `CategoryListQuery`, `EntryListQuery`; regression
covered by tests.

# 7. DEFERRED SCOPE (per STEP-010 §53)

General Ledger / double-entry accounting, invoice file storage, vendor
management, Investment domain, Social Aid finance, tax/VAT engine,
payroll, budgeting, purchase orders, approval workflows, profit
distribution, premium dashboard/UI redesign.

# 8. EVIDENCE

- `apps/server/tests/income_expense.rs` — 14 integration tests covering
  posting, exactness, category type binding, insufficient funds,
  idempotent replay/payload-conflict, reversal, concurrent reversal,
  concurrent expenses at the balance boundary, permissions, CSRF,
  filtering/pagination, movement provenance, and the
  payment/transfer/credit double-count barrier.
- `apps/web/src/specs/income-expense-pages.spec.ts` — 12 component tests
  (lists, create with canonical money submit, detail + reversal,
  permission gating, summary, categories, canonical↔tr-TR round-trip).
- `scripts/e2e-step010.mjs` — real-stack UI flow with derived-balance
  checks at every step and the payment-not-income barrier.
- `scripts/backup/fixtures.sql` + `drill.mjs` — categories, entries,
  movements, a reversed expense and STEP-010 constraint-firing checks
  now participate in the recovery drill.
