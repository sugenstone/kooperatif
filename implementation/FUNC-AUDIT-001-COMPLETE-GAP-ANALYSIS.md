# FUNC-AUDIT-001 — COMPLETE BUSINESS REQUIREMENTS, FEATURE COVERAGE & FUNCTIONAL GAP ANALYSIS

Audit type: independent, read-only. No source code, schema, business
rule, or production data was modified. All write probes ran against a
disposable database (`kooperatif_funcaudit_001`) on the dev Docker
PostgreSQL. Backend probe server ran on `127.0.0.1:8098` and was
terminated after probing.

---

## A. Audited Git baseline

| Check                       | Result                                                                                                                                                              |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Branch                      | `main`                                                                                                                                                              |
| `git status --short`        | clean (no uncommitted work)                                                                                                                                         |
| `git rev-parse HEAD`        | `a0fa441efdccd6da1fec0f63d22fd95a57211052`                                                                                                                          |
| `git rev-parse origin/main` | identical — no drift                                                                                                                                                |
| Latest commits              | `a0fa441` UIUX-019, `3d61987` UIUX-018, `efd0425` UI-TR-001, `dbac231` PILOT-FIX-001 F11                                                                            |
| Stray artifact              | `.kilo/worktrees/versed-stallion/` — a leftover untracked worktree directory containing a partial copy of `migrations/`; not referenced by code. Recommend removal. |

## B. Requirement sources

Authoritative set read: `docs/00`–`docs/30`, `docs/adr/ADR-001`–`013`,
`implementation/STEP-001`–`004` + `STEP-010`–`017` reports, UI-TR-001,
UIUX-018/019 reports, PILOT-001 final report
(`Temp/pilot001-audit/PILOT-001-FINAL-REPORT.md`), PILOT-FIX-001 commit
series, `scripts/e2e-step004…017` harnesses, `scripts/backup/*`,
`.github/workflows/ci.yml`, `README.md`.

**Notable:** `implementation/` contains no STEP-005…009 reports
(shares, periods, payments, financial accounts, credits). The steps
exist in git history, migrations 0005–0009 and per-step E2E scripts —
functionality is verifiable, but the implementation-report trail for
those five steps is absent from the repository.

## C. Original requirements inventory

Reconstructed from charter + domain docs + roadmap phases:

| Domain                                                     | Source                        | Status summary                                          |
| ---------------------------------------------------------- | ----------------------------- | ------------------------------------------------------- |
| Repo/monorepo/SvelteKit/shadcn/Rust/Axum/PG/Docker/CI/i18n | docs/00, ADR-001              | Delivered (STEP-001)                                    |
| Identity/auth/sessions                                     | docs/02, ADR-002              | Delivered (STEP-002)                                    |
| RBAC                                                       | docs/18, ADR-002              | Delivered (STEP-003)                                    |
| Shareholder/guardian/family                                | docs/04                       | Delivered (STEP-004)                                    |
| Shares/ownership                                           | docs/04, docs/16              | Delivered (STEP-005)                                    |
| Periods/assessments                                        | docs/05                       | Delivered (STEP-006)                                    |
| Payments/allocations                                       | docs/06                       | Delivered (STEP-007); Collection Session deferred       |
| Financial accounts/movements/transfers                     | docs/03, docs/07, ADR-003/004 | Delivered TRY-only (STEP-008)                           |
| Shareholder credits/excess                                 | docs/06                       | Delivered (STEP-009)                                    |
| Income/expense + categories                                | docs/07                       | Delivered (STEP-010)                                    |
| Share return/exit/entitlement                              | docs/10                       | Delivered (STEP-011); value-protection POLICY UNDEFINED |
| Investments                                                | docs/08                       | Delivered (STEP-012); several deferrals documented      |
| Social Aid restricted fund                                 | docs/11                       | Delivered (STEP-013) + hard reservation (PILOT-FIX-001) |
| Governance                                                 | docs/09                       | Delivered recording-level (STEP-014); quorum POLICY     |
| Reporting                                                  | docs/14                       | Delivered API+UI (STEP-015); PDF/XLSX deferred          |
| Realtime/TV                                                | ADR-005                       | Delivered (STEP-016)                                    |
| UI/UX hardening                                            | docs/12/13                    | Delivered (STEP-017, UI-TR-001, UIUX-018/019)           |
| Notifications/tasks                                        | docs/24                       | Not implemented — roadmap Phase 12                      |
| Files/attachments                                          | docs/25                       | Not implemented — roadmap Phase 12                      |
| Documents/PDF/XLSX/receipts                                | docs/14, ADR-008              | Not implemented — roadmap Phase 8                       |
| Policy engine (versioned policies)                         | docs/00/15/17                 | Not implemented — free-text `policy_reference` only     |
| Meetings/agenda                                            | docs/09                       | Not implemented — decisions on bodies only              |
| Tenancy (cooperative entity)                               | docs/15, roadmap Phase 1      | Not implemented — single-cooperative                    |

## D. Feature traceability matrix (condensed)

| Requirement                                                    | Backend                                                            | Frontend                                                   | Tests                     | Result                                   |
| -------------------------------------------------------------- | ------------------------------------------------------------------ | ---------------------------------------------------------- | ------------------------- | ---------------------------------------- |
| Create/edit shareholder                                        | POST/PATCH `/api/shareholders`                                     | `hissedarlar`, `hissedarlar/yeni`, `[id]`                  | parties spec + e2e        | COMPLETE — VERIFIED                      |
| Disable/void shareholder                                       | `/status-change`                                                   | `[id]` action                                              | spec                      | COMPLETE — VERIFIED                      |
| Duplicate-name detection                                       | `/api/shareholders/duplicates`                                     | used in create flow                                        | spec + probe S1=200       | COMPLETE — VERIFIED                      |
| Guardian assignment/edit                                       | guardian field in PATCH                                            | shareholder forms                                          | spec                      | COMPLETE — VERIFIED                      |
| Guardian change **history**                                    | single column, no history table                                    | —                                                          | —                         | PARTIAL                                  |
| Standalone Person edit (guardian/payer who is not shareholder) | **no `/api/persons/{id}`** (probe P1=404)                          | —                                                          | —                         | MISSING                                  |
| Person contact/notes fields                                    | `persons` = first/last name only                                   | —                                                          | —                         | MISSING (minor)                          |
| Create family                                                  | POST `/api/families`                                               | `aileler`                                                  | spec + e2e                | COMPLETE — VERIFIED                      |
| Family sequence uniqueness                                     | unique index, 409 under race                                       | —                                                          | e2e                       | COMPLETE — VERIFIED                      |
| Member move / new family from member                           | `/family-change` temporal                                          | `hissedarlar/[id]`                                         | spec + e2e                | COMPLETE — VERIFIED                      |
| Family membership history                                      | `shareholder_family_memberships` + exclusion constraint            | membership history on detail                               | spec                      | COMPLETE — VERIFIED                      |
| Family edit (sequence number correction)                       | **no PATCH `/api/families/{id}`** (route GET-only)                 | —                                                          | —                         | MISSING                                  |
| Family collection context                                      | `/api/families/{id}/collection-context`                            | `aileler/[id]`                                             | probe S3=200              | COMPLETE — VERIFIED                      |
| Create share (founder/later acquisition)                       | POST `/api/shares`                                                 | `hisseler`, `hisseler/yeni`                                | spec + e2e                | COMPLETE — VERIFIED                      |
| Share transfer ≠ sale                                          | `/transfer`, `/sale` + `AcquisitionType`                           | `hisseler/[id]` modes                                      | spec                      | COMPLETE — VERIFIED                      |
| Share suspend/void                                             | `/status-change`                                                   | `hisseler/[id]`                                            | spec                      | COMPLETE — VERIFIED                      |
| Ownership history                                              | `share_ownership` temporal                                         | detail timeline                                            | e2e                       | COMPLETE — VERIFIED                      |
| Create period + rule (per shareholder / per share)             | POST `/api/periods`, `AssessmentRuleType`                          | `donemler/yeni`                                            | spec + e2e                | COMPLETE — VERIFIED                      |
| Assessment preview + generate                                  | `/assessment-preview`, `/generate-assessments`                     | `donemler/[id]`                                            | e2e                       | COMPLETE — VERIFIED                      |
| Period close                                                   | `/periods/{id}/close`                                              | `donemler/[id]`                                            | e2e                       | COMPLETE — VERIFIED                      |
| Assessment **correction/void**                                 | `voided` status reserved; **no endpoint** (probe P7=404)           | —                                                          | —                         | MISSING                                  |
| Record payment (payer ≠ debtor)                                | payer person XOR name; debtor via allocations                      | `tahsilatlar/yeni`                                         | spec + e2e + probe F=201  | COMPLETE — VERIFIED                      |
| Partial / full / overpayment                                   | allocations; unallocated remainder → credit                        | `yeni` + `[id]`                                            | e2e B/C                   | COMPLETE — VERIFIED                      |
| Payment + allocation reversal                                  | `/payments/{id}/reverse`, `/allocations/{aid}/reverse`             | `tahsilatlar/[id]`                                         | spec + e2e                | COMPLETE — VERIFIED                      |
| Idempotent submission                                          | key + fingerprint; double-submit disabled                          | `yeni`                                                     | e2e                       | COMPLETE — VERIFIED                      |
| Credit assign → apply → reverse                                | `/payments/{id}/credits`, `/credit-applications`                   | `tahsilatlar/[id]`, `tahakkuklar/[id]`, `hissedarlar/[id]` | e2e C                     | COMPLETE — VERIFIED                      |
| Collection Session module                                      | **404** (probe P5)                                                 | —                                                          | —                         | DEFERRED (PILOT F8)                      |
| Create financial account                                       | POST; type cash\|bank                                              | `finansal-hesaplar/yeni`                                   | probe A/B=201             | PARTIAL (see §F)                         |
| Currency selection                                             | **not in request contract; DB CHECK='TRY'**                        | —                                                          | probe C/D                 | MISSING + POLICY                         |
| Gold/gram accounts                                             | **accountType parse rejects 'gold'** (probe E1=400)                | —                                                          | —                         | MISSING + POLICY                         |
| Account location/context dimension                             | none (name-only)                                                   | —                                                          | probe E2                  | MISSING                                  |
| Opening balance                                                | **field ignored** (probe P9 → balance 0.00)                        | —                                                          | —                         | MISSING + POLICY                         |
| Account status lifecycle                                       | active↔inactive `/status-change`                                   | `[id]`                                                     | spec                      | COMPLETE — VERIFIED                      |
| Balance = Σ movements                                          | derived, no balance column                                         | `[id]`, summary                                            | e2e + SQL parity (PILOT)  | COMPLETE — VERIFIED                      |
| Transfers + reversal                                           | POST `/api/account-transfers`, `/reverse`                          | `transferler`, `[id]`                                      | probe H/L + e2e           | COMPLETE — VERIFIED                      |
| Cross-currency transfer                                        | impossible (all legs TRY)                                          | —                                                          | probe I                   | N/A + POLICY                             |
| Negative balance prevention                                    | row-lock check → 409                                               | —                                                          | e2e + PILOT race          | COMPLETE — VERIFIED                      |
| Reconciliation endpoint                                        | **404** (probe P8)                                                 | —                                                          | —                         | MISSING                                  |
| Income/expense post + reverse                                  | `/incomes`, `/expenses`, `/reverse`                                | `gelirler`, `giderler`, `[id]`                             | spec + e2e                | COMPLETE — VERIFIED                      |
| Aidat NOT auto-income                                          | provenance classes separate                                        | overview/report                                            | e2e J                     | COMPLETE — VERIFIED                      |
| Categories + status                                            | `/api/financial-categories` + status-change                        | `kategoriler`                                              | spec                      | COMPLETE — VERIFIED                      |
| Share return → entitlements → settlement                       | full chain + `/finalize`, `/determine`, `/settlements`, `/reverse` | `hisse-iadeleri` + `[id]`                                  | e2e F                     | COMPLETE — VERIFIED                      |
| Profit entitlement may stay undetermined                       | `NULL` amount preserved                                            | detail + report                                            | PILOT H                   | COMPLETE — VERIFIED                      |
| Value-protection/indexation modes                              | `policy_reference` free-text only                                  | —                                                          | —                         | POLICY UNDEFINED                         |
| Investment create/fund/income/value/dispose + reversals        | full chain; valuation cancels; disposal terminal                   | `yatirimlar` + `[id]` + `yeni`                             | e2e G                     | COMPLETE — VERIFIED                      |
| Noncash valuation never moves money                            | zero movement provenance                                           | detail                                                     | PILOT + tests             | COMPLETE — VERIFIED                      |
| Investment expense classification                              | not modeled                                                        | —                                                          | —                         | POLICY UNDEFINED (docs/08 open decision) |
| Realized gain/loss formula                                     | not computed                                                       | —                                                          | —                         | POLICY UNDEFINED                         |
| Lease contracts ("cooperative as landlord")                    | not modeled                                                        | —                                                          | —                         | DEFERRED (STEP-012 §5)                   |
| Social aid funds/donations/disbursements                       | full chain                                                         | `sosyal-yardim` + `[id]` + `yeni`                          | e2e H                     | COMPLETE — VERIFIED                      |
| Donor ≠ shareholder                                            | `donor_person_id` XOR display name                                 | form                                                       | probe S5=201              | COMPLETE — VERIFIED                      |
| Hard reservation (physical ≥ reserved)                         | PILOT-FIX-001 enforcement                                          | detail shows restricted text                               | e2e H + reservation tests | COMPLETE — VERIFIED                      |
| Governance bodies/members/decisions/votes                      | full recording chain                                               | `yonetim`, `kurullar`, `kararlar`                          | e2e I                     | COMPLETE — VERIFIED                      |
| Governance never moves money                                   | zero movement provenance                                           | —                                                          | e2e I                     | COMPLETE — VERIFIED                      |
| Meetings/agenda entity                                         | not modeled                                                        | —                                                          | —                         | MISSING/POLICY                           |
| Quorum/majority computation                                    | not computed                                                       | —                                                          | —                         | DEFERRED + POLICY                        |
| 15 report endpoints incl. overview + trend                     | `/api/reports/*`                                                   | `raporlar` (10 catalog + social-aid + governance panels)   | reports tests             | COMPLETE — VERIFIED                      |
| Credit vs new-cash distinction                                 | `credit_applied` separate columns                                  | catalog columns                                            | tests                     | COMPLETE — VERIFIED                      |
| PDF/XLSX/print                                                 | none                                                               | —                                                          | —                         | DEFERRED (STEP-015 §5)                   |
| Live TV + WS auth                                              | `/api/realtime`, denial screen                                     | `(tv)/canli-ekran`                                         | e2e K                     | COMPLETE — VERIFIED                      |
| Login/logout/session list/revoke                               | `/api/auth/*`                                                      | `oturumlar`                                                | spec + e2e                | COMPLETE — VERIFIED                      |
| **Create user via UI/API**                                     | **405** (probe P2) — CLI `create-user` only                        | —                                                          | —                         | MISSING                                  |
| Password change / admin reset                                  | **404** (probe P3/P4)                                              | —                                                          | —                         | MISSING                                  |
| Disable user → sessions revoked                                | same-tx revoke (`revocation_reason='user_disabled'`)               | `kullanicilar` AlertDialog                                 | spec                      | COMPLETE — VERIFIED                      |
| Roles CRUD + permission edit                                   | full API                                                           | `roller`, `roller/[id]`                                    | spec                      | COMPLETE — VERIFIED                      |
| Audit trail **write**                                          | `security_events` for every domain command                         | —                                                          | code-verified             | COMPLETE (write)                         |
| Audit trail **query/UI**                                       | **404** (probe P10)                                                | —                                                          | —                         | MISSING                                  |
| Backup/restore tooling                                         | `scripts/backup/*`, verified drill                                 | —                                                          | PILOT-FIX-001 FIX-E       | COMPLETE (local)                         |
| Production deploy (systemd/HTTPS/WAL/off-site)                 | provisioning duty                                                  | —                                                          | —                         | NOT STARTED                              |

## E. UI action coverage matrix

All 89 API endpoints mapped to UI controls. Findings:

- **Every existing mutating endpoint is exposed in the UI** (verified
  by contract-symbol → route grep: transfer reverse, payment/allocation
  reverse, credit apply/reverse, income/expense reverse, share-return
  cancel/finalize/entitlement determine+cancel/settlement reverse,
  investment funding/income reverse + valuation cancel + dispose +
  cancel, fund close/cancel + donation/disbursement reverse, decision
  open/update/finalize/cancel + votes, body close + membership end,
  account/category status, shareholder status + family-change, share
  transfer/sale/status, period generate + close, role CRUD+permissions,
  session revoke, user enable/disable + role assignment).
- **No UI control exists without backend support** (no fake actions).
- Backend-only surfaces (no UI): none found among business actions —
  the only backend-only data is `security_events` (no read API at all).
- Missing actions are missing because the **endpoint does not exist**
  (see §D): person edit, family edit, user create, password ops,
  assessment void, account reconcile, collection session, export.
- Minor: `sosyal-yardim/[id]` still uses `window.confirm` for
  close/cancel/reverse (AlertDialog pattern applied only to users).
- `transferler` list has no server-side `search` param → correctly no
  fake search control (documented gap).

## F. Financial account type / currency / gold findings (§3)

### Model vs contract vs UI

- Schema: `account_type IN ('cash','bank')`; `currency TEXT DEFAULT 'TRY' CHECK (currency='TRY')` on both `financial_accounts` and `account_transfers` (`migrations/0008`:41-44, 115-116).
- Contract: `CreateFinancialAccountRequest` has **no currency field** (`packages/contracts/src/financial_accounts.ts`:97-104); `FinancialAccountType = 'cash'|'bank'`.
- API: `AccountType::parse` rejects anything but cash/bank; unit test asserts `parse("gold") → None`.
- UI: `finansal-hesaplar/yeni` offers cash/bank Select, bank-only metadata, no currency picker; list shows `currency` column (always "TRY").
- Balance engine: single `NUMERIC(19,2)` `account_movements.amount`, no denomination on the movement row itself (currency inherited from account, which is constrained to TRY).

### §3.4 scenario results (executed on disposable DB)

| Scenario                      | Result                                                                         | Classification                                       |
| ----------------------------- | ------------------------------------------------------------------------------ | ---------------------------------------------------- |
| A İstanbul TL cash            | 201, currency=TRY                                                              | SUPPORTED AND VERIFIED                               |
| B Köy TL cash                 | 201, currency=TRY                                                              | SUPPORTED AND VERIFIED                               |
| C İstanbul USD                | `currency:'USD'` **silently ignored**, stored TRY                              | NOT SUPPORTED (+ silent-coercion defect)             |
| D İstanbul EUR                | same, stored TRY                                                               | NOT SUPPORTED                                        |
| E Gold (grams)                | `accountType:'gold'` → 400; `"Altın"`-named cash account → 201 TRY             | NOT SUPPORTED (name ≠ semantics)                     |
| F TRY payment → TRY account   | 201                                                                            | SUPPORTED AND VERIFIED                               |
| G "USD" transaction           | N/A — no USD account can exist                                                 | NOT SUPPORTED                                        |
| H TRY→TRY transfer            | 201                                                                            | SUPPORTED AND VERIFIED                               |
| I TRY→USD transfer            | N/A — cannot construct non-TRY leg                                             | NOT SUPPORTED                                        |
| J Spend gold as TRY           | N/A — no gold unit exists                                                      | NOT SUPPORTED                                        |
| K Report without unit mixing  | summary groups by currency: `{"totals":[{"balance":"0.00","currency":"TRY"}]}` | SUPPORTED AND VERIFIED (trivially — single currency) |
| L Reversal keeps denomination | reversal preserves row; currency field on transfer survives                    | SUPPORTED AND VERIFIED (TRY only)                    |

### Verdict

- **Owner's report is accurate for currency**: no currency selection exists anywhere; the system is structurally TRY-only.
- **Owner's report is partially inaccurate for account type**: type IS selectable, but limited to `cash`/`bank`; there is no `gold` type and no İstanbul/Köy **location/context** dimension (must be encoded into the account name — free text, no reporting axis).
- **Silent coercion defect (new finding, P2)**: the API accepts an unknown `currency` field and silently stores `TRY`. A hand-built request or a future half-wired UI could create an account _named_ "USD" that is TRY. Recommendation: `deny_unknown_fields` or explicit rejection on write DTOs.
- Original spec (docs/00:243, docs/07:13-14) **did** name İstanbul TL / Köy TL / İstanbul Altın as account examples — so multi-value-type is an **approved-but-deferred** requirement, not a new request. docs/07 §"Open decisions" + docs/20 §"Gold/commodity values" + ADR-004 deliberately left unit/scale/conversion undefined → implementation correctly declined to invent it. **POLICY UNDEFINED** applies to the semantic details; the missing dimension itself is a MISSING requirement.

## G. Shareholder/family/share findings

- Person↔Shareholder↔Guardian↔Family layering verified; temporal
  membership enforced by exclusion constraint; family sequence unique
  under concurrency.
- Gaps: standalone Person edit, Family sequence edit, guardian-change
  history, person notes/contact fields.
- Shares: complete lifecycle incl. distinct transfer/sale with
  acquisition typing; `return_pending`/`closed` reachable only via the
  return workflow (state-machine integrity).

## H. Assessment/payment findings

- Full lifecycle verified (preview → generate → pay → allocate →
  credit → reverse). `SUM(active allocations) ≤ assessed` enforced
  under concurrency (PILOT race evidence).
- Gap: **no assessment void/correct path**. If a period generates
  wrong obligations (wrong rule/amount), money is unaffected but the
  debt rows cannot be corrected — only workaround is reversal of any
  payments plus leaving the bad rows, or a new period. The `voided`
  enum value exists but is unwired (probe P7=404).
- Late/overdue: no status mutation; reports expose `dueState` =
  overdue derived from dueDate (presentation-level, acceptable).

## I. Financial account & transfer findings

- Core engine verified (derived balance, atomic paired legs, no
  negative balance, reversal-preserving history).
- Gaps: no currency/denomination, no location dimension, no opening
  balance, no reconciliation endpoint (docs/07 lists reconciliation in
  reporting scope — deferred with export), `transferler` lacks server
  search param.

## J. Income/expense findings

- Complete incl. categories, account binding, reversals; aidat never
  auto-income (provenance separation verified).

## K. Share-return findings

- Full chain verified incl. NULL-undetermined profit preserved in
  reports. Gap: value-protection/indexation is a free-text
  `policy_reference` — no versioned policy engine (charter-level
  requirement, POLICY UNDEFINED per docs/10/17 open decisions).

## L. Investment findings

- Funding/income/valuation/disposal verified; valuation never creates
  cash. Deferrals documented in STEP-012 §5: investment expense
  classification, expected income, realized gain formula, disposal
  reversal, partial disposal, NAV/distribution, lease contracts.

## M. Social aid findings

- Funds/donations/disbursements + reversals verified; donor and
  beneficiary support person-ref XOR free display name (non-shareholder
  donors/beneficiaries). Hard reservation enforced (PILOT-FIX-001):
  `physical_balance >= reserved` invariant verified by test + e2e H.

## N. Governance findings

- Bodies/memberships/decisions/votes/finalize/cancel recorded; zero
  money movement verified. Gaps: no Meeting/Agenda entity; no quorum/
  majority computation (deferred; recorded outcome ≠ legal validity —
  already documented).

## O. Reporting findings

- 15 endpoints, 10-card catalog + social-aid + governance + overview +
  trend; credit-vs-cash distinction preserved; no unit mixing (single
  currency). Gaps: no PDF/XLSX/print (docs/14 deferred), no scheduled/
  as-of reports, no Collection Receipt document (docs/06 glossary
  defines it; blocked on document subsystem).

## P. Security findings

- Session revocation on disable verified; CSRF + cookie hardening +
  CSP present (PILOT-FIX-001); RBAC catalog explicit (no wildcards).
- Gaps: no user-creation API/UI (CLI-only onboarding — operationally
  significant on a VDS: creating an operator requires SSH+cargo),
  no password change/reset (self or admin), no audit-trail read
  surface (`security_events` write-only → audit evidence exists but
  is inaccessible to the product).

## Q. Backup/deployment readiness

- `scripts/backup/`: create/restore/drill/retention/status/failure-test
  - fixtures; PILOT-FIX-001 FIX-E shipped restore-verified local
    packaging; runbook docs/30 exists. CI: format/lint/typecheck/tests/
    build + clean-DB migration idempotency + fmt/clippy/tests.
- Gaps for VDS: WAL/PITR, encrypted off-site copy, scheduling, systemd
  units, HTTPS/reverse-proxy config are documented as environment
  provisioning duties — none implemented in-repo (correct per ADR-013,
  but unverified until provisioning).
- `README.md` is **stale**: it still claims "BACKUP-READINESS-001
  stage … No Payments, Ledger, Investments, Governance or Social Aid
  exist yet" — contradicts STEP-017 reality. Fix before release (P3).

## R. End-to-end test evidence

| Evidence                                                                                             | Result                                                          |
| ---------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `scripts/e2e-step017-rc.mjs` (fresh DB, real stack, journeys A–K + UI drill + a11y + Firefox/WebKit) | PASS this session                                               |
| `scripts/e2e-step017-uiux.mjs` (6 viewports × 18 routes)                                             | PASS                                                            |
| Vitest                                                                                               | 24 files / 168 tests                                            |
| Backend integration tests                                                                            | 19 files / 225 tests (+ module unit tests)                      |
| FUNC-AUDIT-001 probe (this audit, disposable DB `kooperatif_funcaudit_001`)                          | §3.4 A–L + P1–P10 + S1–S8 executed; log at `Temp/funcaudit001/` |
| PILOT-001 independent probes                                                                         | all findings verified/fixed                                     |

Spot executions this audit: TRY account create (201), USD/EUR coercion
(201→TRY), gold type rejection (400), payment w/ allocations (201),
unallocated payment (201), transfer (201) + reverse (200), transfer to
"USD"-named TRY account (201), duplicate check (200), collection-
context (200), family detail w/ members (200), financial-summary (200),
period summary (200), donation w/o person (201), disable→revoke
(code-verified).

## S. Complete missing-action list (ranked)

1. **Currency/denomination selection** (USD/EUR) — no field anywhere (P1 + POLICY)
2. **Gold/gram account type + unit model** — 400 today (P1 + POLICY)
3. **User creation via UI/API** — CLI-only (P1 operational)
4. **Password change/reset** (self + admin) — none (P1 operational)
5. **Assessment void/correction** — enum reserved, no endpoint (P1)
6. **Standalone person edit** — 404 (P2)
7. **Audit-trail read surface** — write-only (P2)
8. **Account location/context dimension** (İstanbul/Köy structured) — name-only (P2)
9. **Opening balance entry** — silently ignored (P2 + POLICY)
10. **Family sequence-number correction** — GET-only route (P2)
11. **Account reconciliation endpoint** — none (P2)
12. **Guardian-change history** — overwritten (P2)
13. **Collection Session workspace** — 404 (DEFERRED, P2 operational)
14. **PDF/XLSX/print + Collection Receipt** — none (DEFERRED)
15. **Silent unknown-field acceptance on write DTOs** — `currency:'USD'` accepted→TRY (P2 defect)
16. **Meeting/Agenda entity** — decisions only (P3 + POLICY)
17. **Versioned Policy engine + value protection modes** — free-text ref (P1 for exit-rights correctness + POLICY)
18. **Shareholder default account association** — docs/07 optional (P3)
19. **Person contact/notes fields** — name-only model (P3)
20. **`sosyal-yardim/[id]` window.confirm → AlertDialog parity** (P3)
21. **README staleness** (P3)
22. **`.kilo/` leftover worktree** cleanup (P3)
23. Notifications/tasks (docs/24) — not scheduled (DEFERRED)
24. Files/attachments (docs/25) — not scheduled (DEFERRED)
25. Tenant/cooperative entity — single-coop by construction (POLICY)
26. Investment expense classification / realized gain formula / disposal reversal / partial disposal / NAV-distribution / expected income / lease contracts (all POLICY UNDEFINED / DEFERRED per STEP-012 §5)
27. Quorum/majority + approval→finance linkage (POLICY)
28. Shareholder/family **statement export** (DEFERRED, PILOT-listed)
29. As-of point-in-time reports (DEFERRED, STEP-015 §5)
30. Production provisioning: WAL/PITR, off-site encrypted copy, systemd, HTTPS (NOT STARTED — environment duty)

## T. Policy decisions required from owner

Blocking multi-value accounts (before any implementation):

1. Gold unit: grams / pieces / other; scale (>2 decimals likely needed — NUMERIC(19,2) insufficient for gram precision)?
2. Gold purity standard (24k/22k/has)?
3. Foreign-currency payments allowed (donations, aidat)?
4. In-system currency conversion (exchange) — if yes, manual rate entry or external source?
5. Consolidated TRY valuation required in reports?
6. Gold purchases/sales = transfers, conversions, or investments?
7. Social Aid funds may be denominated in FX/gold?
8. Location/context (İstanbul/Köy) = structured dimension or name convention?
9. Opening-balance migration rule for existing physical cash.

Other pending decisions: value-protection modes for exit rights;
investment expense classification + realized-gain formula + disposal
reversal + partial disposal; quorum/majority rules; approval→finance
linkage; tenant model confirmation (single cooperative OK?).

## U. Prioritized remediation roadmap

| #   | Task                                                                  | Scope                       | Reason                                            | Depends          | Owner decisions                       | Complexity |
| --- | --------------------------------------------------------------------- | --------------------------- | ------------------------------------------------- | ---------------- | ------------------------------------- | ---------- |
| 1   | User admin API+UI (create, password reset, self change)               | rbac.ts + kullanicilar      | Operator onboarding on VDS impossible without SSH | —                | password policy                       | M          |
| 2   | Assessment void/correction endpoint                                   | periods                     | Wrong generation is uncorrectable today           | reversal model   | void rules (paid/allocation handling) | M          |
| 3   | Person edit endpoint + minimal person UI                              | parties                     | Guardian/payer name typos uncorrectable           | —                | —                                     | S          |
| 4   | Family edit endpoint                                                  | parties                     | Sequence-number typo uncorrectable                | —                | —                                     | S          |
| 5   | Write-DTO strictness (`deny_unknown_fields` or explicit rejects)      | all routers                 | Silent currency/field coercion (defect)           | —                | —                                     | S          |
| 6   | Audit-trail read API + admin view                                     | auth/reports                | Recorded evidence inaccessible                    | —                | scope/permissions                     | M          |
| 7   | Account location/context dimension                                    | financial_accounts          | İstanbul/Köy reporting axis                       | —                | structured vs name                    | S–M        |
| 8   | Multi-value-type foundation (ADR-004 value codes)                     | schema+money core           | USD/EUR/gram prerequisite                         | owner §T.1–9     | ALL currency decisions                | XL         |
| 9   | FX/gold account types + denomination-aware engine                     | movements/transfers/reports | full §3.4 support                                 | #8               | conversion rules                      | XL         |
| 10  | Opening-balance workflow                                              | financial_accounts          | legacy cash onboarding                            | #9 or standalone | migration rule                        | M          |
| 11  | Reconciliation endpoint + report                                      | financial_accounts          | docs/07 scope                                     | —                | recon frequency                       | M          |
| 12  | Guardian-change history                                               | parties                     | temporal relationship evidence                    | —                | effective dating                      | M          |
| 13  | Versioned Policy engine + value protection                            | policies + share_returns    | exit-rights correctness                           | policy spec      | modes/formulas                        | L–XL       |
| 14  | Collection Session workspace                                          | payments                    | high-throughput collection                        | owner UX spec    | session semantics                     | L          |
| 15  | Document subsystem (receipt/PDF/XLSX/numbering)                       | docs/14, ADR-008            | receipts/statements/exports                       | —                | templates                             | L–XL       |
| 16  | Meeting/Agenda entity + quorum                                        | governance                  | legal-form completeness                           | quorum rules     | POLICY                                | M          |
| 17  | Notifications/tasks                                                   | docs/24                     | operational reminders                             | —                | channels                              | L          |
| 18  | Files/attachments                                                     | docs/25, ADR-007            | document evidence                                 | storage backend  | retention                             | L          |
| 19  | Social-aid confirm→AlertDialog parity + README fix + `.kilo/` cleanup | web/docs                    | consistency                                       | —                | —                                     | S          |
| 20  | Production provisioning (WAL/PITR, off-site, systemd, HTTPS)          | infra                       | ADR-013 gate                                      | VDS access       | hosting decisions                     | M          |

## V. Final audit decision

**AUDIT COMPLETE — GAPS IDENTIFIED**

The implemented surface is real and verified: every API endpoint maps
to a working UI action; financial semantics (allocation, credit,
reservation, reversal, idempotency, derived balances) survive
concurrency and independent audit. The gaps are concentrated in four
areas: **multi-value-type accounts (TRY-only, blocks İstanbul Altın /
USD / EUR)**, **operational user administration (no UI/API user
creation or password management)**, **correction paths (assessment,
person, family)**, and **documented-but-unbuilt subsystems**
(Collection Session, documents/exports, policy engine, notifications,
files) — most of which were explicitly deferred with owner decisions
still required rather than falsely claimed complete.
