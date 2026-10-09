# REQUIREMENTS REGISTRY — Kooperatif (PROJECT-CONTROL-001)

Durable traceability registry. Created by PROJECT-CONTROL-001 at
baseline `1a23024`. **Owner decision (PROJECT-CONTROL-001): all 30
requirements are mandatory.** None may be removed, permanently deferred
or downgraded to optional. This file is the single source of truth for
requirement status and must be updated by every future STEP/FIX
closure report.

## Status vocabulary

| Status        | Meaning                                                                                 |
| ------------- | --------------------------------------------------------------------------------------- |
| `VERIFIED`    | Completion rules (below) satisfied with recorded evidence                               |
| `PARTIAL`     | Some required behavior exists and works; the complete workflow does not                 |
| `MISSING`     | Capability absent                                                                       |
| `DEFECT`      | Existing behavior is wrong and must be corrected                                        |
| `NOT STARTED` | Infrastructure/operational duty with no in-repo implementation                          |
| `BLOCKED-PD`  | Implementation sequence blocked on a policy decision (PD-xx); capability still required |

`DEFERRED` is **not** a permitted terminal status. Every item carries a
target milestone (`M0`–`M14`, see `PROJECT-CONTROL-001-RECOVERY-ROADMAP.md`).

## Completion rules (binding)

A requirement or sub-requirement may be marked `VERIFIED` only when:

1. its approved business behavior is implemented;
2. required backend **and** UI workflows are operational;
3. database integrity is preserved (constraints, migrations clean + idempotent);
4. authorization and audit controls pass (backend-enforced);
5. correction/reversal scenarios are covered where relevant;
6. realistic end-to-end acceptance tests pass against the real stack
   (PostgreSQL + Rust API + SvelteKit + browser) and assert the database result.

Unit tests alone never establish business completeness. A parent
requirement is `VERIFIED` only when every sub-requirement is `VERIFIED`.

## Source abbreviations

`docs/NN:L` = specification file + line. `AUD` = `implementation/FUNC-AUDIT-001-COMPLETE-GAP-ANALYSIS.md`.
`FF2` = FUNC-FIX-002 (plan `implementation/FUNC-FIX-002-DEFAULT-COLLECTION-ACCOUNT-PLAN.md`, commit `1a23024`).
`OWN` = explicit owner instruction in session. `S-nnn` = `implementation/STEP-nnn-*.md`.

---

## 1. Master matrix (30 parent requirements)

| ID      | Requirement                                   | Original source                                                        | Prio | Depends on                 | Status          | Milestone     |
| ------- | --------------------------------------------- | ---------------------------------------------------------------------- | ---- | -------------------------- | --------------- | ------------- |
| REQ-001 | Multi-currency accounts (TRY/USD/EUR)         | docs/00:240-243, docs/07:22, ADR-004, OWN (USD/EUR)                    | P1   | REQ-024, REQ-006           | MISSING         | M6            |
| REQ-002 | Gold accounts, gram accounting                | docs/00:242, docs/07:13-14, docs/07:170-176, ADR-004:12-18             | P1   | REQ-001                    | MISSING         | M6            |
| REQ-003 | Structured account location                   | docs/07:5, docs/07:22, docs/06:117                                     | P2   | REQ-024                    | MISSING         | M5            |
| REQ-004 | Opening balances                              | docs/07:176 ("opening-balance migration rules")                        | P2   | REQ-003, (REQ-001 for FX)  | MISSING         | M5 (+M6)      |
| REQ-005 | Account reconciliation                        | docs/07:150-156, docs/13:161-165, docs/29 Phase 4                      | P2   | REQ-004                    | MISSING         | M5            |
| REQ-006 | Strict API write validation                   | AUD §F (silent coercion defect), docs/21                               | P1   | —                          | DEFECT          | M0            |
| REQ-007 | User creation/administration (UI/API)         | S-002 §U (deferred "admin user management"), AUD §P                    | P1   | REQ-024 (coop membership)  | PARTIAL         | M2            |
| REQ-008 | Password change and reset                     | S-002 §U (deferred "password recovery"), docs/22:12                    | P1   | REQ-007                    | MISSING         | M2            |
| REQ-009 | Audit history viewer                          | docs/19:80-84, docs/13 §16 (:190), AUD §P                              | P2   | —                          | PARTIAL         | M2            |
| REQ-010 | Standalone person editing                     | docs/02:62, AUD §G (probe P1=404)                                      | P2   | —                          | MISSING         | M3            |
| REQ-011 | Family sequence number correction             | docs/04 §Family (Family Sequence Number), AUD §D (GET-only)            | P2   | —                          | MISSING         | M3            |
| REQ-012 | Guardian change history                       | docs/04:36-45 ("relationship history/effective dates")                 | P2   | —                          | PARTIAL         | M3            |
| REQ-013 | Shareholder default collection account        | docs/07:33, docs/07:176, FF2                                           | P2   | —                          | **VERIFIED**    | (done)        |
| REQ-014 | Person contact and notes                      | AUD §D, OWN                                                            | P3   | REQ-010                    | MISSING         | M3            |
| REQ-015 | Assessment correction and void                | docs/05:146-152, docs/05:164-168, docs/19:35                           | P1   | —                          | MISSING         | M4            |
| REQ-016 | Collection Session workspace                  | docs/00:163, docs/06:114-140, docs/12:226, docs/13 §10-11, docs/16:161 | P1   | REQ-003, REQ-017           | MISSING         | M9            |
| REQ-017 | PDF/XLSX/print/collection receipts            | docs/14:61-118, docs/01:162, ADR-008, docs/29 Phase 6+8                | P1   | REQ-024, REQ-026 (partial) | MISSING         | M8            |
| REQ-018 | Shareholder/family statement exports          | docs/14:119-135, docs/13 §19, docs/04:225, docs/10:153                 | P2   | REQ-017                    | MISSING         | M8            |
| REQ-019 | Historical as-of reporting                    | S-015 §5, docs/14, docs/19                                             | P2   | REQ-015, PD-28             | MISSING         | M8            |
| REQ-020 | Versioned policy engine + value protection    | docs/00:255-270, docs/09:35-108, docs/10, docs/17:139, docs/03:233     | P1   | REQ-023                    | PARTIAL         | M11           |
| REQ-021 | Advanced investment accounting (7 subs)       | docs/08:45-60, docs/08:132-141, S-012 §5, docs/29 Phase 9              | P2   | REQ-020, REQ-026           | PARTIAL         | M12           |
| REQ-022 | Meeting and agenda management                 | docs/09, docs/29 Phase 7 ("meetings")                                  | P2   | REQ-026 (minutes)          | MISSING         | M10           |
| REQ-023 | Quorum, voting rules, financial authorization | docs/09:121-127, docs/18, S-014 §2                                     | P1   | REQ-022                    | MISSING         | M10           |
| REQ-024 | Multi-cooperative organization model          | docs/00:88-90, docs/15:13-15, docs/22:23-27, docs/29 Phase 1           | P1   | —                          | MISSING         | M1            |
| REQ-025 | Notifications and tasks                       | docs/24, ADR-009, docs/29 Phase 12                                     | P2   | REQ-024                    | MISSING         | M13           |
| REQ-026 | File and attachment management                | docs/25, ADR-007, docs/29 Phase 12                                     | P2   | REQ-024                    | MISSING         | M7            |
| REQ-027 | Social Aid confirmation dialog consistency    | AUD §E, docs/12                                                        | P3   | —                          | DEFECT          | M0            |
| REQ-028 | README accuracy                               | AUD §Q                                                                 | P3   | —                          | DEFECT          | M0 (+each)    |
| REQ-029 | Legacy worktree cleanup (`.kilo/`)            | AUD §A                                                                 | P3   | owner authorization        | NOT STARTED     | M0            |
| REQ-030 | Production provisioning and recovery          | ADR-013, docs/23, docs/26:127, docs/30                                 | P1   | owner authorization (VDS)  | PARTIAL (local) | M14 (plan M0) |

Totals: VERIFIED 1 · PARTIAL 6 · MISSING 19 · DEFECT 3 · NOT STARTED 1.

Priority is a **sequencing signal only**; every requirement is mandatory.

---

## 2. Per-requirement detail and nested inventory

Field legend: **BE** backend · **FE** frontend · **DB** database ·
**PERM** permissions · **AUDIT** audit events · **CORR** correction/reversal ·
**AUTO** automated tests · **E2E** real-browser scenarios · **AC** acceptance criteria.

### REQ-001 — Multi-Currency Financial Accounts — MISSING — M6

- Current: `financial_accounts.currency CHECK (currency='TRY')` and the same on `account_transfers` (`migrations/0008`:41-44, 115-116); `assessments.currency CHECK='TRY'` (`0006`); `payments.currency DEFAULT 'TRY'`. Contract `CreateFinancialAccountRequest` has no currency field. Amounts `NUMERIC(19,2)`.
- BE: value-type code on account; every movement inherits and is validated against it; transfers require equal value code; summaries grouped by value code.
- FE: currency selector on `finansal-hesaplar/yeni`; currency shown on every balance, movement, transfer, payment.
- DB: relax CHECK to a catalog/enum; backfill `TRY`; consider value code on `account_movements` (denormalized, CHECK-matched to account).
- PERM: `financial_accounts.manage` (existing). AUDIT: account create/status events carry value code.
- CORR: reversal preserves value code (already true structurally).
- AUTO: per-currency balance derivation; cross-currency transfer rejected; reports never sum across codes; reservation invariant per account unchanged.
- E2E: create TRY/USD/EUR accounts; post USD-denominated inflow; attempt TRY→USD transfer → rejected; reports show separate totals.
- AC: no endpoint or report ever adds amounts of different value codes; `currency` cannot be silently coerced (see REQ-006).

| Sub    | Item                                                       | Status              | AC                                                         |
| ------ | ---------------------------------------------------------- | ------------------- | ---------------------------------------------------------- |
| 001.1  | TRY accounts                                               | VERIFIED (existing) | existing suites stay green after migration                 |
| 001.2  | USD accounts                                               | MISSING             | USD account created, balance shown as USD                  |
| 001.3  | EUR accounts                                               | MISSING             | EUR account created, balance shown as EUR                  |
| 001.4  | Explicit currency selection (API + UI)                     | MISSING             | required field, validated against catalog                  |
| 001.5  | Same-currency validation (transfer, payment, expense, aid) | MISSING             | mismatched value codes → 400/409, nothing posted           |
| 001.6  | Storage of value code on account (+movement)               | MISSING             | migration clean + idempotent; existing rows `TRY`          |
| 001.7  | Currency-specific reporting                                | MISSING             | every money report groups by value code; no cross-code sum |
| 001.8  | Foreign-currency payment of assessments                    | BLOCKED-PD (PD-04)  | behavior per approved decision                             |
| 001.9  | Exchange/conversion transaction                            | BLOCKED-PD (PD-05)  | paired legs, explicit rate, audited, reversible            |
| 001.10 | Consolidated TRY valuation view                            | BLOCKED-PD (PD-06)  | clearly labelled valuation, never merged with balances     |

### REQ-002 — Gold Accounts and Gram-Based Accounting — MISSING — M6

- Current: `account_type IN ('cash','bank')`; `AccountType::parse("gold") → None` (400). An "Altın"-named account is TRY.
- BE/DB: value code for gold (ADR-004 example `XAU_GRAM`); quantity column precision > 2 decimals (scale per PD-08); movement validation as REQ-001.
- FE: gold account creation; quantities formatted as grams (tr-TR), never as ₺.
- AC: gram quantities never summed with money; a gold account cannot pay a TRY expense.

| Sub   | Item                             | Status             | AC                                                 |
| ----- | -------------------------------- | ------------------ | -------------------------------------------------- |
| 002.1 | Gold account type / value code   | MISSING            | account created with gold value code               |
| 002.2 | Gram quantity storage            | MISSING            | quantity stored exactly, unit explicit             |
| 002.3 | Precision (scale)                | BLOCKED-PD (PD-08) | scale per decision; no rounding loss in round trip |
| 002.4 | Unit and purity model            | BLOCKED-PD (PD-07) | unit/purity recorded per account                   |
| 002.5 | Valuation rules                  | BLOCKED-PD (PD-06) | valuation separate from quantity; dated rate       |
| 002.6 | Gold buy/sell / conversion rules | BLOCKED-PD (PD-09) | modeled as approved (conversion vs investment)     |
| 002.7 | Reporting without unit mixing    | MISSING            | gram totals separate from currency totals          |

### REQ-003 — Structured Account Location — MISSING — M5

- Current: location only encoded in account name (free text).
- BE/DB: `locations` catalog (per cooperative) + `financial_accounts.location_id`; list/report filters.
- FE: location select on account create/edit; location column + filter on accounts list and account reports.
- AUDIT: location change recorded before/after. CORR: reassignment is metadata only; historical movements keep account identity.
- AC: "İstanbul" and "Köy" are records, not name conventions; reports filter by location.

| Sub   | Item                                      | Status  | AC                                     |
| ----- | ----------------------------------------- | ------- | -------------------------------------- |
| 003.1 | Location catalog (configurable, per coop) | MISSING | CRUD with status lifecycle, audited    |
| 003.2 | Account ↔ location association            | MISSING | required/optional per PD-11; validated |
| 003.3 | Location filter in lists and reports      | MISSING | balances report filterable by location |
| 003.4 | Location context for Collection Sessions  | MISSING | session carries location (docs/06:117) |

### REQ-004 — Opening Balances — MISSING — M5 (TRY), M6 (FX/gold)

- Current: an `openingBalance` field is silently ignored (AUD probe P9 → 0.00).
- BE/DB: dedicated movement `source_type='opening_balance'` (never income); one per account unless corrected; dated.
- PERM: separate permission (e.g. `financial_accounts.opening_balance`). AUDIT: create + reversal.
- CORR: correction by reversal + re-entry; never edit in place.
- AC: opening balance appears in balance and movement history, excluded from income reports, reversible.

| Sub   | Item                          | Status             | AC                                               |
| ----- | ----------------------------- | ------------------ | ------------------------------------------------ |
| 004.1 | Opening-balance movement type | MISSING            | balance = Σ movements incl. opening entry        |
| 004.2 | Permission + audit            | MISSING            | unauthorized → 403; audit event with amount/date |
| 004.3 | Correction via reversal       | MISSING            | reversed entry visible; re-entry allowed         |
| 004.4 | Single-active / dating guard  | BLOCKED-PD (PD-12) | rule per decision                                |
| 004.5 | Not classified as income      | MISSING            | income/expense reports unaffected                |
| 004.6 | FX/gold opening balances      | MISSING            | after M6, per value code                         |

### REQ-005 — Financial Account Reconciliation — MISSING — M5

- Spec: expected system balance, counted/bank statement balance, difference, reconciliation record; adjustments explicit and audited, not hidden (docs/07:150-156).
- BE/DB: `account_reconciliations` (account, as-of, expected, counted, difference, status, notes, actor); optional adjustment movement `source_type='reconciliation_adjustment'` per PD-13.
- FE: reconciliation action + history on `finansal-hesaplar/[id]`; differences report.
- AC: a reconciliation never changes balance unless an explicit, permissioned, audited adjustment is posted.

| Sub   | Item                                        | Status             | AC                                            |
| ----- | ------------------------------------------- | ------------------ | --------------------------------------------- |
| 005.1 | Reconciliation record (expected vs counted) | MISSING            | expected computed server-side as-of timestamp |
| 005.2 | Difference investigation view               | MISSING            | movements since last reconciliation listed    |
| 005.3 | Explicit audited adjustment                 | BLOCKED-PD (PD-13) | adjustment movement, separate permission      |
| 005.4 | Reconciliation history + report             | MISSING            | history on account detail; report card        |

### REQ-006 — Strict API Write Validation — DEFECT — M0

- Current: no `deny_unknown_fields` on any write DTO (repo-wide grep: 0); `currency:'USD'` accepted, stored `TRY` (AUD §F).
- BE: reject unknown fields on every mutating request DTO (`#[serde(deny_unknown_fields)]` or equivalent); stable `validation_failed` error.
- FE: none (contracts already exact); verify no client sends extra fields.
- AUTO: one negative test per mutating endpoint family (unknown field → 400, nothing persisted).
- AC: the AUD §F probe (`currency:'USD'`) returns 400 and creates nothing.

| Sub   | Item                                      | Status  | AC                                       |
| ----- | ----------------------------------------- | ------- | ---------------------------------------- |
| 006.1 | Unknown-field rejection on all write DTOs | DEFECT  | 400 for unknown field, zero rows written |
| 006.2 | Client compatibility check                | MISSING | full Vitest + E2E green after change     |
| 006.3 | Regression tests per module               | MISSING | negative test per router                 |

### REQ-007 — User Creation and Administration — PARTIAL — M2

- Current: list users, enable/disable (same-tx session revocation), role assignment: VERIFIED (`kullanicilar`, rbac tests). Create: CLI `create-user` / `grant-role` only (`apps/server/src/main.rs`); `POST /api/users` → 405.
- Historical: STEP-002 §U explicitly deferred "admin user management".
- BE: `POST /api/users` (username, display name, initial credential per PD-14), `PATCH` display name; `users.manage`; last-admin lockout protections preserved.
- FE: create-user dialog on `kullanicilar`; edit display name.
- AC: an administrator creates an operator from the UI, the operator logs in, no SSH required.

| Sub   | Item                                | Status   | AC                                   |
| ----- | ----------------------------------- | -------- | ------------------------------------ |
| 007.1 | List users                          | VERIFIED | existing                             |
| 007.2 | Enable/disable + session revocation | VERIFIED | existing                             |
| 007.3 | Role assignment                     | VERIFIED | existing                             |
| 007.4 | Create user API                     | MISSING  | 201; duplicate username 409; audited |
| 007.5 | Create user UI                      | MISSING  | E2E: create → login as new user      |
| 007.6 | Edit user profile (display name)    | MISSING  | optimistic concurrency + audit       |
| 007.7 | Cooperative membership of users     | MISSING  | after M1 (REQ-024.3)                 |

### REQ-008 — Password Change and Reset — MISSING — M2

- Current: none (AUD probes P3/P4 = 404). Argon2 hashing exists; no email channel exists.
- BE: self change (current password required; other sessions revoked); admin reset (per PD-14: temporary credential + forced change, or one-time token); rate limiting; never log secrets.
- AC: changed password works, old fails, other sessions revoked; reset requires `users.manage`; audit event contains no secret.

| Sub   | Item                                      | Status             | AC                                          |
| ----- | ----------------------------------------- | ------------------ | ------------------------------------------- |
| 008.1 | Self password change                      | MISSING            | current password verified; sessions revoked |
| 008.2 | Administrative reset                      | BLOCKED-PD (PD-14) | flow per decision                           |
| 008.3 | Forced change at next login (if chosen)   | BLOCKED-PD (PD-14) | cannot use app until changed                |
| 008.4 | Policy, rate limit, audit without secrets | MISSING            | tests assert no secret in logs/events       |

### REQ-009 — Audit History Viewer — PARTIAL — M2

- Current: `security_events` written by every domain command (write side COMPLETE); no read API (AUD probe P10 = 404).
- Spec: audit UI is read-only; export separately permissioned; no destructive cleanup without approved retention (docs/19:80-90).
- BE: `GET /api/audit-events` (filters: actor, type, entity id, date range; cursor pagination ADR-010); new `audit.read`, `audit.export`.
- FE: `denetim` screen; entity history panels on shareholder/account/payment detail.
- AC: an auditor finds the FF2 default-account change by shareholder id; non-auditor → 403.

| Sub   | Item                               | Status             | AC                                          |
| ----- | ---------------------------------- | ------------------ | ------------------------------------------- |
| 009.1 | Audit write coverage               | VERIFIED           | existing                                    |
| 009.2 | Audit read API (search/filter)     | MISSING            | filters + pagination, `audit.read` enforced |
| 009.3 | Audit UI (read-only)               | MISSING            | E2E search by entity                        |
| 009.4 | Entity history panels              | MISSING            | shown on key detail pages                   |
| 009.5 | Audit export (separate permission) | MISSING            | depends REQ-017 XLSX                        |
| 009.6 | Retention policy                   | BLOCKED-PD (PD-15) | no destructive cleanup until decided        |

### REQ-010 — Standalone Person Editing — MISSING — M3

- Current: persons editable only indirectly via shareholder PATCH; guardians/payers/donors who are not shareholders cannot be corrected (`/api/persons/{id}` PATCH = 404).
- BE: `GET/PATCH /api/persons/{id}` with `expectedUpdatedAt`; search_name recomputed; audit before/after.
- FE: person detail page (`kisiler/[id]`) reachable from guardian/payer/donor links.
- CORR: name correction must not rewrite immutable document snapshots (receipts after M8).
- AC: a misspelt guardian name is corrected once and reflected wherever referenced; history in audit.

| Sub   | Item                           | Status  | AC                                      |
| ----- | ------------------------------ | ------- | --------------------------------------- |
| 010.1 | Person read/update API         | MISSING | stale update → 409                      |
| 010.2 | Person detail UI               | MISSING | E2E correct guardian name               |
| 010.3 | Audit + history                | MISSING | before/after recorded                   |
| 010.4 | Document snapshot preservation | MISSING | issued receipts keep original name (M8) |

### REQ-011 — Family Sequence Number Correction — MISSING — M3

- Current: unique sequence enforced (concurrency-tested); `/api/families/{id}` is GET-only.
- BE: `PATCH /api/families/{id}` (sequenceNumber, expectedUpdatedAt); 409 on duplicate; audit before/after.
- AC: wrong family number corrected; duplicate rejected; old number visible in audit history.

| Sub   | Item               | Status             | AC                        |
| ----- | ------------------ | ------------------ | ------------------------- |
| 011.1 | Correction API     | MISSING            | uniqueness 409 under race |
| 011.2 | Correction UI      | MISSING            | E2E on `aileler/[id]`     |
| 011.3 | Audit + reuse rule | BLOCKED-PD (PD-18) | reuse per decision        |

### REQ-012 — Guardian Change History — PARTIAL — M3

- Current: single `shareholders.guardian_person_id` column; changes overwrite. Audit events record before/after (partial evidence only).
- DB: `shareholder_guardianships` temporal table (exclusion constraint like family memberships); backfill current guardian as open interval.
- FE: guardian timeline on shareholder detail.
- AC: after two guardian changes, three intervals are visible with effective dates.

| Sub   | Item                         | Status             | AC                              |
| ----- | ---------------------------- | ------------------ | ------------------------------- |
| 012.1 | Temporal guardianship model  | MISSING            | non-overlap enforced in DB      |
| 012.2 | Backfill from current column | MISSING            | migration safe on existing data |
| 012.3 | Effective-date entry rule    | BLOCKED-PD (PD-17) | backdating per decision         |
| 012.4 | Timeline UI                  | MISSING            | E2E                             |

### REQ-013 — Shareholder Default Collection Account — VERIFIED

Evidence: commit `1a23024`; backend `apps/server/tests/parties.rs::shareholder_default_collection_account`; Vitest `payment-new-default-account.spec.ts` (6) + `parties-pages.spec.ts`; real-browser + DB acceptance 35/35 (FUNC-FIX-002 supplementary report); clean sequential full Rust suite pass. Approved rule preserved: a payment covering multiple shareholders never auto-selects a receiving account.

| Sub   | Item                                                    | Status   |
| ----- | ------------------------------------------------------- | -------- |
| 013.1 | Storage (nullable FK, no backfill)                      | VERIFIED |
| 013.2 | Assign at create / change / clear (audited, stale-safe) | VERIFIED |
| 013.3 | Single-debtor proposal, operator override               | VERIFIED |
| 013.4 | Multi-shareholder: no inference                         | VERIFIED |
| 013.5 | Inactive default retained + warned, not used            | VERIFIED |
| 013.6 | List column "Varsayılan Kasa" + filter, "Atanmamış"     | VERIFIED |
| 013.7 | History/reversal unaffected                             | VERIFIED |

Residual hygiene (not a re-implementation): the E2E harness lives in a temp directory; M0 commits it to `scripts/` so the evidence is reproducible.

### REQ-014 — Person Contact and Notes — MISSING — M3

- Current: `persons` = first name, last name, search name.
- DB/BE: contact fields (per PD-16), notes; validation (phone/e-mail formats); PII access control (`persons.contact.read`?).
- AC: contact data visible only to permitted roles; edits audited; notes never leak into TV/realtime payloads.

| Sub   | Item                 | Status             | AC                        |
| ----- | -------------------- | ------------------ | ------------------------- |
| 014.1 | Contact fields       | BLOCKED-PD (PD-16) | field set per decision    |
| 014.2 | Notes                | MISSING            | create/edit audited       |
| 014.3 | Validation           | MISSING            | invalid formats rejected  |
| 014.4 | Access control (PII) | MISSING            | 403 for unpermitted roles |
| 014.5 | Edit workflow UI     | MISSING            | E2E                       |

### REQ-015 — Assessment Correction and Void — MISSING — M4

- Current: `assessments.status` allows `voided` (reserved, `0006`), no endpoint (AUD probe P7 = 404).
- Spec: no arbitrary deletion; explicit cancellation/reversal/correction depending on whether allocations exist (docs/05:146-152).
- BE: void endpoint with reason; behavior when active allocations/credit applications exist per PD-19; amount correction = void + reissue (recommended) with linkage.
- AC: settled formula (allocations + credit applications) never exceeds assessed amount; voided assessments excluded from debt totals, visible in history; payments' cash unaffected.

| Sub   | Item                                    | Status             | AC                                                    |
| ----- | --------------------------------------- | ------------------ | ----------------------------------------------------- |
| 015.1 | Void unpaid assessment                  | MISSING            | status voided, reason, audit; excluded from open debt |
| 015.2 | Void with allocations/credits           | BLOCKED-PD (PD-19) | consequence per decision; no orphan allocation        |
| 015.3 | Amount correction (void + reissue)      | MISSING            | linkage old→new preserved                             |
| 015.4 | Closed-period handling                  | BLOCKED-PD (PD-19) | rule per decision                                     |
| 015.5 | Reports exclude voided, show as history | MISSING            | report tests                                          |
| 015.6 | UI on `tahakkuklar/[id]`                | MISSING            | E2E                                                   |

### REQ-016 — Collection Session Workspace — MISSING — M9

- Spec: session around Period/date/location/account context; candidate attributes (docs/06:114-121); collection-day flow (docs/06:124-140); special UI mode (docs/12:226); state machine (docs/16:161).
- BE/DB: `collection_sessions` (coop, period, title, planned/actual date, location, state, allowed/default accounts, operators, notes, open/close timestamps); `payments.collection_session_id`.
- AC: an operator opens a session, collects many payments quickly, totals update in real time, session closes with a summary (and reconciliation per PD-20).

| Sub   | Item                            | Status             | AC                                            |
| ----- | ------------------------------- | ------------------ | --------------------------------------------- |
| 016.1 | Session entity + lifecycle      | MISSING            | open → closed state machine enforced          |
| 016.2 | High-throughput workspace UI    | MISSING            | keyboard-first flow, identity verification    |
| 016.3 | Payments linked to session      | MISSING            | posting semantics unchanged (FF2 rules apply) |
| 016.4 | Allowed/default accounts        | BLOCKED-PD (PD-20) | restriction per decision                      |
| 016.5 | Session totals + report         | MISSING            | totals = Σ linked active payments             |
| 016.6 | Close-out reconciliation        | BLOCKED-PD (PD-20) | per decision                                  |
| 016.7 | Real-time updates               | MISSING            | uses existing realtime channel                |
| 016.8 | Receipt printing from workspace | MISSING            | depends REQ-017.2                             |

### REQ-017 — PDF, XLSX, Printing, Collection Receipts — MISSING — M8

- Spec: receipt semantics and minimum content (docs/14:61-118); family receipt (docs/14:111); ADR-008 central typed/versioned rendering; donation receipt (docs/11:39).
- AC: every posted payment can produce a numbered receipt; reprint yields an identical snapshot; reversal reflected on the receipt record, not by editing it.

| Sub    | Item                                 | Status             | AC                                   |
| ------ | ------------------------------------ | ------------------ | ------------------------------------ |
| 017.1  | Rendering infrastructure (ADR-008)   | MISSING            | typed templates, versioned, tr-TR    |
| 017.2  | Collection receipt                   | MISSING            | minimum content per docs/14:101      |
| 017.3  | Family receipt                       | MISSING            | allocation lines per member          |
| 017.4  | Numbering scheme                     | BLOCKED-PD (PD-21) | gap/duplicate-free under concurrency |
| 017.5  | Immutable snapshot + reprint history | BLOCKED-PD (PD-21) | reprint identical                    |
| 017.6  | PDF export of reports                | MISSING            | report → PDF                         |
| 017.7  | XLSX export of reports               | MISSING            | exact decimals preserved             |
| 017.8  | Print layouts                        | MISSING            | printable receipt/report             |
| 017.9  | Social Aid donation receipt          | MISSING            | per docs/11:39                       |
| 017.10 | QR/verification, signatures          | BLOCKED-PD (PD-21) | per decision                         |

### REQ-018 — Shareholder and Family Statement Exports — MISSING — M8

| Sub   | Item                          | Status  | AC                                                    |
| ----- | ----------------------------- | ------- | ----------------------------------------------------- |
| 018.1 | Shareholder statement         | MISSING | assessments, allocations, credits, reversals, balance |
| 018.2 | Family statement              | MISSING | per-member attribution; family never owns debt        |
| 018.3 | PDF/XLSX export of statements | MISSING | totals match canonical report formulas                |

### REQ-019 — Historical As-Of Reporting — MISSING — M8

- Historical: STEP-015 §5 declined because the past effect of later reversals was undefined in docs/19.

| Sub   | Item                                   | Status             | AC                                           |
| ----- | -------------------------------------- | ------------------ | -------------------------------------------- |
| 019.1 | As-of semantics (reversal time effect) | BLOCKED-PD (PD-28) | documented rule                              |
| 019.2 | As-of account balances                 | MISSING            | equals Σ movements effective ≤ date per rule |
| 019.3 | As-of debt/settlement                  | MISSING            | uses canonical settled formula               |
| 019.4 | As-of membership/ownership context     | MISSING            | temporal tables queried at date              |

### REQ-020 — Versioned Policy Engine and Value Protection — PARTIAL — M11

- Current: share-return entitlements implemented with NULL-preserved undetermined profit (VERIFIED); policy is a free-text `policy_reference`. No Policy/PolicyVersion entities.
- Rule: never invent formulas; never overwrite original base right (docs/00:264-270).

| Sub    | Item                                                   | Status             | AC                                                    |
| ------ | ------------------------------------------------------ | ------------------ | ----------------------------------------------------- |
| 020.1  | Policy + PolicyVersion entities                        | MISSING            | versions immutable once effective                     |
| 020.2  | Effective-date resolution                              | MISSING            | historical calculations use version in force          |
| 020.3  | Approval linkage (governance)                          | MISSING            | depends REQ-023.4                                     |
| 020.4  | Principal (refundable amount) policy                   | BLOCKED-PD (PD-22) | formula per decision                                  |
| 020.5  | Profit-right policy                                    | BLOCKED-PD (PD-22) | formula per decision                                  |
| 020.6  | Value protection modes (none/index/gold/fixed/formula) | BLOCKED-PD (PD-22) | base preserved; payable equivalent derived            |
| 020.7  | Index/rate data sources                                | BLOCKED-PD (PD-22) | dated, audited inputs                                 |
| 020.8  | Crystallization snapshot                               | PARTIAL            | entitlements snapshot exists; link to version missing |
| 020.9  | Policy simulation (docs/09:102)                        | MISSING            | simulation never posts                                |
| 020.10 | Allocation-priority policy (docs/09:40)                | BLOCKED-PD (PD-22) | per decision                                          |

### REQ-021 — Advanced Investment Accounting and Lifecycle — PARTIAL — M12

- Current VERIFIED base: create/fund/income/valuation/disposal + reversals; valuation never moves money (STEP-012).

| Sub   | Item                                       | Status                  | AC                                                                    |
| ----- | ------------------------------------------ | ----------------------- | --------------------------------------------------------------------- |
| 021.a | Investment expense classification          | BLOCKED-PD (PD-23a)     | expense recorded per approved class; never silent operational expense |
| 021.b | Realized gain/loss calculation policy      | BLOCKED-PD (PD-23b)     | computed from approved cost basis; reproducible                       |
| 021.c | Investment disposal reversal               | MISSING (rule PD-23c)   | reversal restores status + reverses proceeds movement, history kept   |
| 021.d | Partial investment disposal                | BLOCKED-PD (PD-23d)     | remaining cost basis per rule; multiple disposals                     |
| 021.e | NAV and profit-distribution policy         | BLOCKED-PD (PD-23e)     | NAV labelled valuation; distribution posts via approved policy        |
| 021.f | Expected/forecast investment income        | MISSING                 | forecast stored separately, never counted as income (docs/00:246)     |
| 021.g | Lease contracts + investment documentation | MISSING (needs REQ-026) | lease entity, schedule, receivable linkage, attachments               |

### REQ-022 — Meeting and Agenda Management — MISSING — M10

| Sub   | Item                               | Status  | AC                                    |
| ----- | ---------------------------------- | ------- | ------------------------------------- |
| 022.1 | Meeting entity (body, date, state) | MISSING | lifecycle audited                     |
| 022.2 | Agenda items                       | MISSING | ordered, editable until meeting opens |
| 022.3 | Decision ↔ agenda linkage          | MISSING | existing decisions optionally linked  |
| 022.4 | Attendance                         | MISSING | input to quorum (REQ-023)             |
| 022.5 | Minutes (attachment)               | MISSING | depends REQ-026                       |

### REQ-023 — Quorum, Voting and Financial Authorization — MISSING — M10

- Historical: STEP-014 §2 deferred quorum, majority, weighted/proxy voting, approval→finance linkage ("AUTHORITATIVE POLICY NOT DEFINED").

| Sub   | Item                                         | Status             | AC                                               |
| ----- | -------------------------------------------- | ------------------ | ------------------------------------------------ |
| 023.1 | Quorum rule                                  | BLOCKED-PD (PD-24) | computed from attendance                         |
| 023.2 | Majority/tie-breaking                        | BLOCKED-PD (PD-24) | computed outcome shown beside recorded outcome   |
| 023.3 | Computed vs recorded outcome                 | MISSING            | mismatch flagged, never silently overridden      |
| 023.4 | Decision → financial-operation authorization | BLOCKED-PD (PD-24) | listed operations require linked passed decision |
| 023.5 | Approval thresholds                          | BLOCKED-PD (PD-24) | configurable per policy version                  |

### REQ-024 — Multi-Cooperative Organization Model — MISSING — M1

- Current: **no tenant/cooperative column in any of the 17 migrations**; single cooperative by construction. Charter: "Design for multiple cooperatives even if the first deployment uses one" (docs/00:88-90); invariant: every tenant-owned record belongs to exactly one tenant, backend-enforced (docs/15:13-15). Roadmap Phase 1 listed "Cooperatives/Organizations; tenant context" — no STEP implemented it and no recorded decision deferring it was found.
- Strategy: see roadmap §M1 and PD-01/PD-02.

| Sub   | Item                                           | Status             | AC                                             |
| ----- | ---------------------------------------------- | ------------------ | ---------------------------------------------- |
| 024.1 | Cooperative entity                             | MISSING            | CRUD by system admin, audited                  |
| 024.2 | `cooperative_id` on every owned record         | MISSING            | NOT NULL + FK; backfilled to default coop      |
| 024.3 | User ↔ cooperative membership + roles per coop | BLOCKED-PD (PD-02) | per decision                                   |
| 024.4 | Server-side isolation on every read/write      | MISSING            | cross-coop id → 404 semantics (docs/21:86)     |
| 024.5 | Tenant-isolation test suite                    | MISSING            | per module IDOR tests (docs/26:36,103)         |
| 024.6 | Per-coop uniqueness/numbering                  | MISSING            | family seq, payment no, receipt no scoped      |
| 024.7 | Cooperative context UI (switcher)              | MISSING            | E2E two coops, no leakage                      |
| 024.8 | Realtime/TV scoped per coop                    | MISSING            | events never cross coop                        |
| 024.9 | Existing-data migration                        | MISSING            | all current rows in default coop; suites green |

### REQ-025 — Notifications and Tasks — MISSING — M13

| Sub   | Item                                   | Status             | AC                                         |
| ----- | -------------------------------------- | ------------------ | ------------------------------------------ |
| 025.1 | Notification entity + read state       | MISSING            | read ≠ domain completion                   |
| 025.2 | Task entity (owner/state/due)          | MISSING            | lifecycle audited                          |
| 025.3 | Triggers (docs/24 list)                | BLOCKED-PD (PD-25) | v1 trigger set per decision                |
| 025.4 | Assignment                             | MISSING            | permission/tenant-scoped                   |
| 025.5 | In-app delivery                        | MISSING            | decided channel (docs/24 "Initial in-app") |
| 025.6 | Server-side scheduler (ADR-009)        | MISSING            | runs without an open browser               |
| 025.7 | Duplicate control                      | MISSING            | idempotent generation                      |
| 025.8 | Task completion never executes finance | MISSING            | test asserts zero movements                |

### REQ-026 — File and Attachment Management — MISSING — M7

| Sub   | Item                                       | Status             | AC                                           |
| ----- | ------------------------------------------ | ------------------ | -------------------------------------------- |
| 026.1 | Storage backend (ADR-007 S3-compatible)    | BLOCKED-PD (PD-26) | provider per decision                        |
| 026.2 | Metadata table (docs/25:22-25 fields)      | MISSING            | checksum, size, type, uploader, entity links |
| 026.3 | Backend-authorized upload/download         | MISSING            | no public predictable URLs                   |
| 026.4 | Entity linking (payment, share, decision…) | MISSING            | per-entity permission inheritance            |
| 026.5 | Versioning                                 | MISSING            | replacement preserves prior version          |
| 026.6 | Archive instead of delete; retention       | BLOCKED-PD (PD-26) | no destructive delete of evidence            |
| 026.7 | Limits/content validation                  | MISSING            | size/type enforced server-side               |
| 026.8 | Backup coverage of file store              | MISSING            | ADR-013 restore drill includes files         |

### REQ-027 — Social Aid Confirmation Dialog Consistency — DEFECT — M0

- Current: `apps/web/src/routes/(app)/sosyal-yardim/[id]/+page.svelte` uses `window.confirm` for close/cancel/reverse.
- AC: AlertDialog pattern (as on `kullanicilar`) for every destructive action; Vitest updated; no `window.confirm` left in that route.

### REQ-028 — README Accuracy — DEFECT — M0 (+ every milestone)

- Current: README still states an early stage ("No Payments, Ledger … exist yet").
- AC: README lists implemented modules, known limitations (links this registry), configuration, test/verify commands, backup procedure; updated at every milestone closure.

### REQ-029 — Legacy Worktree Cleanup — NOT STARTED — M0 (authorization required)

- Evidence (read-only, PROJECT-CONTROL-001): `.kilo/worktrees/versed-stallion` is a **registered git worktree** (`git worktree list`) at detached `c86a197` ("docs: adopt backup and disaster recovery ADR"), which **is an ancestor of `main`**; `git status --short` in the worktree is empty. `.kilo/.gitignore` ignores `node_modules`, lockfiles and `agent-manager.json`, so ignored files may exist and have not been inventoried.
- Plan: (1) inventory ignored files (`git status --ignored`) and report; (2) on owner authorization run `git worktree remove` (then `git worktree prune`), and remove `.kilo/` only after the inventory is approved.
- AC: no unique commits or files lost; `git worktree list` shows only the main tree.

### REQ-030 — Production Provisioning and Recovery — PARTIAL (local only) — M14

- Current: local backup/restore/drill/retention/failure tooling (`scripts/backup/*`), runbook docs/30, restore-verified locally (PILOT-FIX-001). Nothing provisioned on the VDS. **Production changes require separate explicit owner authorization.**

| Sub    | Item                                 | Status      | AC                                              |
| ------ | ------------------------------------ | ----------- | ----------------------------------------------- |
| 030.1  | PostgreSQL operational configuration | NOT STARTED | native PG on VDS, tuned, least-privilege roles  |
| 030.2  | WAL archiving / PITR                 | NOT STARTED | PITR restore to timestamp demonstrated          |
| 030.3  | Encrypted off-site backups           | NOT STARTED | copy outside VDS failure domain (ADR-013:36)    |
| 030.4  | Restore verification                 | PARTIAL     | local drill verified; production drill required |
| 030.5  | Rust/SvelteKit service management    | NOT STARTED | release build, env/secret handling              |
| 030.6  | systemd units                        | NOT STARTED | restart policy, hardening directives            |
| 030.7  | HTTPS + reverse proxy                | NOT STARTED | TLS, HSTS, origin config matches CSRF allowlist |
| 030.8  | Deployment + rollback procedure      | NOT STARTED | migration-aware rollback rehearsed              |
| 030.9  | Monitoring + alerting                | NOT STARTED | health, backup age, disk, error rate            |
| 030.10 | Operational security                 | NOT STARTED | firewall, SSH policy, secret rotation, patching |

---

## 3. Accounting check

- Parent requirements: **30 / 30** present (REQ-001 … REQ-030).
- Nested items tracked individually: **170** sub-requirements —
  167 table rows (REQ-001: 10, 002: 7, 003: 4, 004: 6, 005: 4, 006: 3,
  007: 7, 008: 4, 009: 6, 010: 4, 011: 3, 012: 4, 013: 7, 014: 5, 015: 6,
  016: 8, 017: 10, 018: 3, 019: 4, 020: 10, 021: 7, 022: 5, 023: 5,
  024: 9, 025: 8, 026: 8, 030: 10) + 3 single-item requirements
  (REQ-027, REQ-028, REQ-029), each with its own AC.
- Sub-items VERIFIED: **12** (001.1, 007.1–007.3, 009.1, 013.1–013.7).
  Every `BLOCKED-PD` row names its policy decision id.
- Every item has a target milestone. No item is marked deferred.
