# FUNC-FIX-002 — SHAREHOLDER-LEVEL DEFAULT COLLECTION ACCOUNT

## Implementation Plan & Migration-Safety Assessment (pre-authorization)

Approved rule (owner): a default collection account belongs to the
**shareholder**; it is a collection _preference_, never a posting
restriction. Family/multi-shareholder payments require an explicit
receiving-account selection — no default inference. TRY-only; no FX,
gold, or conversion work.

Baseline: `main` @ `1470594` (audit docs committed; `origin/main` at
`a0fa441` — audit commit pending push authorization).

---

## 1. Current-state evidence

- `shareholders` columns: `id, person_id, guardian_person_id, status,
created_at, updated_at` — no account reference
  (`migrations/0004`:44-58).
- `payments.destination_account_id` is **required** for new payments
  and is stamped onto the inflow `account_movements` row permanently —
  historical receiving-account truth is already immutable
  (`migrations/0008`:154-156).
- `POST /api/payments` contract: `destinationAccountId` + `allocations`;
  debtor is _derived from allocation targets_, never a payer field —
  payer is `personId XOR first/last name` (`payments/routes.rs`:423-441).
- `GET /api/financial-accounts/options` returns **active accounts
  only** (`list_active_account_options`), gated by
  `financial_accounts.read` OR `payments.manage`
  (`financial_accounts/routes.rs`:299-317).
- Debtor search `GET /api/payments/payer-persons` returns
  `PayerCandidateDto { personId, fullName, shareholderId,
shareholderStatus }` — `payments.read`-gated (`routes.rs`:582-598).
- `tahsilatlar/yeni` already supports multi-shareholder payments
  (family context → per-member assessment rows; `rows` carry
  `debtorShareholderId`) and currently preselects the destination
  account **only when exactly one active account exists**
  (`+page.svelte`:207-224). Submit is blocked without an explicit
  account.
- Shareholder PATCH uses `expectedUpdatedAt` optimistic concurrency
  (`parties/routes.rs`:143-151); status/family changes are separate
  POST endpoints with audit events.
- `security_events` records every domain command; a new event type is
  the established audit channel.

## 2. Design decisions

### 2.1 Storage — nullable FK on `shareholders`

```sql
ALTER TABLE shareholders
    ADD COLUMN default_collection_account_id UUID
        REFERENCES financial_accounts(id);
```

- NULL = no preference → explicit selection required (spec default).
- Historical reconstruction is guaranteed by
  `payments.destination_account_id` + `account_movements`; docs/07
  makes effective-dating conditional ("if historical reconstruction
  requires it") — a temporal table is therefore **not** needed. Audit
  trail comes from a `security_events` entry per change.

### 2.2 Assignment rules (backend, authoritative)

- Set/clear via existing `PATCH /api/shareholders/{id}` (extends
  `UpdateShareholderRequest` with `defaultCollectionAccountId?:
string | null`; `null` clears). Reuses `shareholders.manage` +
  `expectedUpdatedAt` — no new permission (collection-preference is
  identity metadata, not finance).
- Optional `defaultCollectionAccountId` on `CreateShareholderRequest`.
- Validation inside the same tx: account must exist **and be
  `active`** — lock the account row (`FOR UPDATE`) to serialize against
  a concurrent status-change. Inactive/unknown → 400.
- Setting `null` always allowed (clearing is never invalid).
- Audit: `SecurityEventType::ShareholderDefaultAccountChanged` with
  `{ shareholder_id, old_account_id, new_account_id, actor }`.
- **Payment posting path is untouched**: `destinationAccountId` stays a
  required client-supplied field; the backend never infers a default.
  This preserves the contract, the idempotency fingerprint, and keeps
  the "explicit selection at confirmation" rule server-enforced.

### 2.3 Surfacing the preference (read paths)

Add `defaultCollectionAccount: { id, name, status } | null` to:

- `ShareholderDetailDto` / `ShareholderListItemDto` (detail + list can
  show a badge; detail drives the edit control).
- `PayerCandidateDto` (debtor search) — `payments.read`-gated, so the
  collection desk does **not** need `shareholders.read`.
- `FamilyCollectionContext` member entries (family flow).

All are additive fields — no contract breakage.

### 2.4 Frontend behavior (`tahsilatlar/yeni`)

- On adding debtor assessments, collect distinct
  `debtorShareholderId`s from `rows`.
- **Exactly one distinct debtor** and its default is `active` →
  preselect once, but never override an operator-chosen value (track
  `autoSelectedAccountId`; only replace if untouched or equal to a
  previous auto-selection).
- **Distinct debtors > 1** → never auto-select (approved family rule);
  show an info hint "birden çok hissedar — hesabı açıkça seçin".
- Saved default is `inactive` → visible warning + require explicit
  selection (inactive account won't appear in `options` anyway).
- No default → unchanged behavior (required selection).
- Payer fields never influence the account (payer ≠ debtor).

### 2.5 Shareholder screens

- `hissedarlar/yeni`: optional "Varsayılan Tahsilat Hesabı" Select
  (options endpoint, active only).
- `hissedarlar/[id]`: display default (name + status badge); authorized
  (`shareholders.manage`) inline change control — Select + save with
  `expectedUpdatedAt`; clear action. Inactive default shown with a
  warning badge.
- `hissedarlar` list: optional column (name only) — cheap join already
  planned in DTO.

### 2.6 i18n / a11y

New TR + EN keys: `shareholders.defaultAccount*`,
`payments.defaultAccountHint`, `payments.defaultAccountInactive`,
`payments.multiDebtorAccountHint`, `accounts.inactive`. Select
triggers get `aria-labelledby` + stable ids per UIUX-019 standard.

## 3. Change surface

| Layer                                                                          | Change                                                                 |
| ------------------------------------------------------------------------------ | ---------------------------------------------------------------------- |
| `migrations/0017_shareholder_default_account.sql`                              | nullable FK + comment                                                  |
| `parties/model.rs`                                                             | validation fn; error variant                                           |
| `parties/repo.rs`                                                              | SELECT/INSERT/UPDATE joins + column                                    |
| `parties/routes.rs`                                                            | DTO fields; create/patch wiring; audit event                           |
| `payments/routes.rs`, `payments/repo.rs`                                       | `PayerCandidateDto` + family-context member field (join)               |
| `auth/audit.rs`                                                                | new `SecurityEventType` variant                                        |
| `packages/contracts`                                                           | request/response field additions                                       |
| web: `hissedarlar/yeni`, `hissedarlar/[id]`, `hissedarlar`, `tahsilatlar/yeni` | selects, display, preselect logic                                      |
| i18n `tr-TR.ts`, `en.ts`                                                       | new keys                                                               |
| tests                                                                          | backend integration + parties/payments specs + existing suites updated |

**Explicitly untouched:** payment posting transaction, allocation,
credit, reversal, movement derivation, reservation logic, all money
semantics, API payloads of every other endpoint.

## 4. Migration-safety assessment

- `ADD COLUMN … UUID NULL REFERENCES` = metadata-only on PostgreSQL ≥11;
  **no table rewrite, no backfill, no lock amplification** beyond the
  FK check on `financial_accounts` (AccessShareLock — non-blocking).
- Backward compatibility: old code ignores the column; new code
  requires it → standard order is migrate-then-deploy (the existing
  `cargo run -- migrate` step already precedes `serve`).
- Rollback: `DROP COLUMN` is safe; **nothing references the column**
  (payments keep their own `destination_account_id`). No data loss of
  financial records — only the preference itself disappears.
- Zero financial-semantics risk: no posting path changes; a wrong
  default can only mislabel a _proposal_, and the backend still demands
  the explicit account id at posting.
- Concurrency: assignment serializes with account status-change via a
  row lock; PATCH keeps `expectedUpdatedAt`.
- Deactivation interplay: account `inactive` does **not** clear stored
  defaults (no silent mutation); the payment screen warns and requires
  explicit selection — satisfies "never used silently".

## 5. Acceptance mapping

| Scenario                          | Mechanism                                                                 |
| --------------------------------- | ------------------------------------------------------------------------- |
| Default Köy TL → pays into Köy TL | preselect → post to selected                                              |
| Pays into İstanbul TL instead     | explicit override; saved default untouched                                |
| Default changed Köy→İstanbul      | PATCH + audit event; historical payments keep `destination_account_id`    |
| No default                        | submit blocked until selection (existing required field)                  |
| Default deactivated               | options exclude it; warning + explicit selection                          |
| Unauthorized change               | `shareholders.manage` → 403                                               |
| Third-party payer                 | preselect keyed to **debtor** shareholder, not payer                      |
| Multi-shareholder payment         | no inference; explicit selection enforced                                 |
| Reversal                          | reverses recorded `destination_account_id` movement only (unchanged code) |

## 6. Test plan

Backend (`tests/parties.rs`, `tests/payments.rs`, `tests/financial_accounts.rs`):
assign on create; PATCH set/clear; inactive-account assignment → 400;
unknown account → 400; concurrent assign vs deactivate; audit row
written; candidate + family-context fields populated; payment with
default-vs-override posts to the _selected_ account; reversal history
intact.

Frontend (vitest): preselect on single debtor; no preselect on
multi-debtor; inactive-default warning; no-default requires selection;
detail-page change flow (CSRF + expectedUpdatedAt); permission gating.

E2E: extend `e2e-step017-rc.mjs` journey B with one default-account
assertion (optional, low cost).

## 7. Gates

prettier · eslint · svelte-check · vitest (24 files) · cargo fmt ·
clippy `-D warnings` · cargo test · clean-DB migration (incl. double
migration idempotency) · production build · RC E2E.

## 8. Open questions for owner (non-blocking)

1. Should an _inactive_ default remain visible on the shareholder
   detail (with a warning) — recommended — or auto-clear? (Plan keeps
   it visible; auto-clear would be a silent mutation.)
2. List-page column for the default account: include or keep detail-only?
   (Plan includes it — one join.)

## Status

**PLAN READY — AWAITING EXECUTION AUTHORIZATION.** No implementation
performed; nothing deployed; working tree untouched by this step.
