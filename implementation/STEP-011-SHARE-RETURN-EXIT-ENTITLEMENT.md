# STEP-011 — SHARE RETURN, EXIT & DEFERRED ENTITLEMENT FOUNDATION

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
main == origin/main == e45403c  (STEP-010)
```

Authoritative inputs:

```text
docs/10-SHARE-RETURN-PROFIT-RIGHTS-VALUE-PROTECTION.md
docs/04-SHAREHOLDERS-FAMILIES-SHARES.md
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

Approved scope: share return request → exit/closure → recognized
entitlements (principal + profit) → deferred determination → cash
settlement + reversal.

---

# 1. DOMAIN PRINCIPLE — THREE SEPARATE LAYERS (docs/10)

```text
SHARE LIFECYCLE   active → return_pending → closed
                  (initiation / finalization / cancellation)

ENTITLEMENT       share_return_entitlements — WHAT the cooperative
                  owes: principal ("Refundable Invested Amount Right")
                  and profit ("Profit Right"). Recognition moves
                  ZERO money.

SETTLEMENT        share_return_settlements — REAL money leaving a
                  Financial Account through exactly ONE outflow
                  Account Movement each.
```

No column ever collapses these into "paid = true/false". Closing a
share is not recognizing a debt; recognizing a debt is not paying it.

# 2. SCHEMA (migration 0011)

- `shares.status` extended: `active | suspended | voided |
return_pending | closed`. `closed` is terminal; no resurrection.
- `share_events.event_type` extended: `return_requested |
return_cancelled | return_finalized` — return milestones are part of
  the share's business history.
- `account_movements.source_type` extended:
  `share_return_settlement` joins `{payment, transfer, income,
expense}`; `UNIQUE (account_id, source_type, source_id)` still
  prevents a source posting the same leg twice.
- `share_returns` — `return_number` (GENERATED IDENTITY), share +
  shareholder FKs, immutable identity snapshots (`owner_display_name`,
  `share_number`, `ownership_started_at`), `requested_at`, the
  authoritative `effective_return_date` (DATE — ownership closes at
  that day 00:00 Europe/Istanbul), `reason`,
  `status ∈ {pending, finalized, cancelled}` with lifecycle
  consistency CHECKs, durable idempotency key + fingerprint, partial
  unique index `one pending return per share`.
- `share_return_entitlements` — `entitlement_number`,
  `entitlement_type ∈ {principal, profit}`, explicit
  `beneficiary_shareholder_id`, `amount NUMERIC(19,2)` — **NULL is a
  first-class "not yet determined" state, never `0.00`** — `due_date`,
  `policy_reference` (the governing decision, free-text evidence),
  `recognized_at`, `determined_at`, `status ∈ {open,
partially_settled, settled, cancelled}` + consistency CHECKs,
  `CHECK (amount ↔ determined_at pairing)`, partial unique index: one
  non-cancelled entitlement per (return, type).
- `share_return_settlements` — `settlement_number`, `entitlement_id`,
  `financial_account_id`, `amount > 0`, `settled_at`, hard 1:1
  `account_movement_id` FK + UNIQUE, `status ∈ {posted, reversed}` +
  reversal consistency CHECK, durable idempotency key + fingerprint.
- Permissions `share_returns.read` / `share_returns.manage` seeded,
  granted to `Sistem Yöneticisi` only.

# 3. POLICY-DRIVEN AMOUNTS — NOTHING INVENTED (docs/10, docs/17)

- Refundable Invested Amount and Profit Right formulas are
  `UNRESOLVED / POLICY_DRIVEN`. The system never computes them.
- The operator enters the cooperative-approved amount; the row
  snapshots `amount + due_date + policy_reference + recognized_at`
  so a later policy change cannot silently move a crystallized right.
- A profit right may be recognized with `amount = NULL` at finalize
  and quantified later via `POST …/determine` (optimistic
  `expectedUpdatedAt`). `NULL ≠ "0.00"`: an explicit zero is a valid
  determined amount, NULL is "not yet determined".
- Early settlement is allowed; `dueState`
  (`undetermined|not_due|due|overdue|settled|cancelled`) is derived at
  the API boundary — a full due-date rules engine is deferred.

# 4. SEMANTICS

- **Initiation** (`POST /api/share-returns`): share must be `active`
  with an open ownership interval; `effective_return_date` must be a
  valid business date — not in the future (Istanbul day) and not
  before ownership started. One pending return per share. Share flips
  to `return_pending`; `return_requested` event recorded. Zero money
  moves.
- **Cancellation** (`POST …/cancel`): pending only; share returns to
  `active`; `return_cancelled` event; reason required.
- **Finalization** (`POST …/finalize`): pending only; optimistic
  `expectedUpdatedAt`; accepts zero-or-more entitlement specs (a
  no-right exit is legal). Ownership interval closes at the
  effective-date cutoff; share flips to `closed`; `return_finalized`
  event; entitlement rows are recognized in the same transaction.
  Still zero money moves.
- **Entitlement commands**: `POST /api/share-returns/{id}/entitlements`
  (recognize a right on a finalized return — duplicate open right per
  type rejected `409`), `POST …/{id}/determine` (fix an undetermined
  amount — settles the NULL→value transition with optimistic
  concurrency), `POST …/{id}/cancel` (only while the right carries
  no settlements).
- **Settlement** (`POST …/{id}/settlements`): entitlement must be
  `open|partially_settled` with a determined amount; `0 < amount ≤
remaining`; the entitlement row lock serializes concurrent postings;
  the account row lock + `FOR UPDATE` balance check rejects
  insufficient funds `409`. Exactly ONE `outflow` movement with
  `source_type = 'share_return_settlement'` is inserted atomically —
  the entry's `account_movement_id` is a hard FK.
- **Reversal** (`POST …/{id}/reverse`): posted only; flips settlement
  `posted → reversed` and the bound movement `active → reversed` in
  one transaction (docs/19 bookkeeping); the derived remaining amount
  is restored automatically. Reversal that would take the account
  below zero is rejected `409`.
- **Idempotency** (ADR-006): initiation, finalize, determine, settle
  and reversal replay on same key + same fingerprint; a reused key
  with a different payload answers `409 conflict`. First-writer races
  resolve through the unique key.
- **Effective-date eligibility fix** (in-scope regression): period /
  assessment share-source eligibility previously filtered
  `shares.status = 'active'` — a share closed today would have wrongly
  dropped yesterday's obligations. It now consults temporal ownership
  intervals and statuses `active | return_pending | closed`, so
  economic responsibility follows the effective dates, not the current
  flag.

# 5. BOUNDARIES PRESERVED

- Settlement ≠ Expense ≠ Payment ≠ Transfer ≠ Shareholder Credit —
  separate `source_type`, separate tables, separate totals; income/
  expense summaries, payments and credits are untouched by returns.
- Entitlement recognition moves ZERO money; account balance stays
  `SUM(active movements)` — never stored, never manually editable.
- Existing assessments, payments, allocations and credits survive a
  share closure unchanged (proven by tests).
- `return_pending`/`closed` are lifecycle statuses only — manual share
  status changes remain limited to `active|suspended|voided`.
- TRY-only; all API money is canonical decimal strings (ADR-004);
  business dates are `YYYY-MM-DD`, timestamps RFC3339.
- Turkish-first UI; NULL amounts render as "Henüz belirlenmedi", never
  as `0,00 ₺`.

# 6. SURFACES

API: `GET|POST /api/share-returns`, `GET /api/share-returns/{id}`,
`POST /api/share-returns/{id}/finalize`, `POST …/cancel`,
`POST /api/share-returns/{id}/entitlements`,
`GET /api/share-return-entitlements` (beneficiary / status / due-state
filters, server-paginated),
`POST /api/share-return-entitlements/{id}/determine` + `/cancel` +
`/settlements`, `POST /api/share-return-settlements/{id}/reverse`.
All mutations Origin+CSRF-checked and permission-gated server-side;
no unbounded lists; no DELETE.

Frontend (tr-TR, permission-gated nav): `/hisse-iadeleri` (list +
filters + outstanding totals), `/hisse-iadeleri/yeni` (active-share
picker or `?share=` prefill + effective date + preview),
`/hisse-iadeleri/{id}` (lifecycle card, finalize panel with explicit
principal/profit specs, determine / cancel entitlement, settle with
remaining-aware client guard, reasoned reversal). Share detail shows
`return_pending`/`closed` badges and a return-request action;
shareholder detail lists the member's return cases.

Audit events: `share_return_requested`, `share_return_finalized`,
`share_return_cancelled`, `share_return_entitlement_recognized`,
`share_return_entitlement_determined`,
`share_return_entitlement_cancelled`,
`share_return_settlement_posted`, `share_return_settlement_reversed`.

# 7. DEFERRED SCOPE

Formula engines for refundable amount / profit right, exit valuation,
indexation, profit distribution, approval workflows (docs/10 pending
decisions), automatic payment plans, GL posting, reports/documents.

# 8. EVIDENCE

- `apps/server/tests/share_returns.rs` — 15 integration tests:
  initiation + idempotent replay + payload conflict, duplicate pending
  rejection, invalid/future/early effective dates, ineligible share,
  cancellation restoring `active`, finalization + closure, duplicate
  entitlement types, no-right finalization, principal + profit
  entitlements, deferred profit determination, entitlement cancel
  constraints, settlement boundaries (undetermined / over-remaining /
  insufficient funds), settlement idempotency, concurrent settlement
  serialization, exactly-one provenance-checked movement, reversal
  restoring remaining, debts/credits untouched, effective-date
  eligibility after closure, authn/authz/CSRF/IDOR/no-store,
  list filtering/pagination.
- `apps/web/src/specs/share-returns-pages.spec.ts` — 8 component
  tests (list render, prefill + canonical submit, finalize payload
  with parsed TRY amounts, NULL-amount rendering, settle payload,
  client-side over-settlement guard, reversal, permission gating).
- `scripts/e2e-step011.mjs` — real-stack UI flow: initiate → finalize
  (determined principal + undetermined profit) → closure → partial
  settlement (single `share_return_settlement` outflow;
  income/expense/payment/transfer untouched) → reversal.
- `scripts/backup/fixtures.sql` + `drill.mjs` — a finalized return,
  determined + undetermined entitlements, a posted and a reversed
  settlement plus their movements, derived-remaining check and
  STEP-011 constraint-firing checks all participate in the recovery
  drill (restore verified, not just backup).
