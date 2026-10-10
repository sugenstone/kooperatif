# PROJECT-CONTROL-001 — COMPLETE REQUIREMENTS RECOVERY & DELIVERY ROADMAP

Type: planning and documentation only. No feature code, schema,
production system or remote repository was changed.
Companion registry: `implementation/REQUIREMENTS-REGISTRY.md` (30
parent requirements, 170 sub-requirements, status vocabulary,
completion rules).

---

## A. Baseline and authorization boundary

| Item               | Value                                                                       |
| ------------------ | --------------------------------------------------------------------------- |
| Branch             | `main`                                                                      |
| HEAD               | `1a23024` feat: shareholder-level default collection account (FUNC-FIX-002) |
| Local-only commits | `1470594` (FUNC-AUDIT-001), `fb6793c` (FUNC-FIX-002 plan), `1a23024`        |
| `origin/main`      | `a0fa441` — 3 commits ahead, **not pushed**                                 |
| Working tree       | clean before this task                                                      |
| Migrations         | `0001`–`0017`                                                               |

Owner decisions recorded by this task:

- **All 30 FUNC-AUDIT-001 findings are mandatory** (REQ-001…REQ-030). None may be omitted, downgraded, permanently deferred or removed.
- **FUNC-FIX-002 is VERIFIED** (REQ-013) on the recorded evidence: 35/35 real-browser + database checks, clean sequential full Rust suite, PoolTimedOut reproduced only under concurrent `cargo test` processes (test-environment contention, not a connection leak). It is not re-implemented.

Boundary: no new features, no production/VDS access, no push, no
deletion of development artifacts without separate authorization.

---

## B. Historical omission / deferral analysis

Only evidence found in the repository is stated. "No record" means no
deferral decision was located — no explanation is invented.

| REQ | Where the requirement originates                                                                    | Why it is not implemented — evidence                                                                                                                                                                        | Owner decision vs agent assumption                                                                                                         |
| --- | --------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| 001 | docs/00:240-243 (Istanbul TRY, Village TRY, Istanbul Gold); docs/07:22 value type/currency; ADR-004 | docs/07:170-176 "Open decisions": exact account types, gold unit model, exchange/conversion. STEP-008 shipped TRY-only (`CHECK currency='TRY'`). STEP-008 report is absent from `implementation/` (AUD §B). | USD/EUR named by owner (OWN). TRY-only scope was an implementation choice pending open decisions; no explicit owner deferral record found. |
| 002 | docs/00:242, docs/07:13-14, ADR-004:12-18 (`XAU_GRAM` example)                                      | Same open decisions (gold unit model).                                                                                                                                                                      | Gold required by charter; unit/purity/scale never decided.                                                                                 |
| 003 | docs/07:5 ("value locations"), docs/07:22 location/context                                          | No record. Account names carry location.                                                                                                                                                                    | Agent omission in STEP-008 scope — no decision found.                                                                                      |
| 004 | docs/07:176 "opening-balance migration rules"                                                       | Listed as open decision; request field silently ignored (AUD P9).                                                                                                                                           | Policy open; silent ignore is a defect (see REQ-006).                                                                                      |
| 005 | docs/07:150-156, docs/29 Phase 4 "reconciliation"                                                   | docs/07:150 phrased as "Support future reconciliation"; frequency open (docs/07:176).                                                                                                                       | No explicit deferral decision; spec wording "future".                                                                                      |
| 006 | docs/21 contract discipline                                                                         | Not a deferral — defect discovered by AUD §F.                                                                                                                                                               | —                                                                                                                                          |
| 007 | docs/18, docs/29 Phase 1                                                                            | **STEP-002 §U explicitly deferred "admin user management"**; never scheduled afterwards.                                                                                                                    | Documented deferral in STEP-002 instructions.                                                                                              |
| 008 | docs/22:12                                                                                          | **STEP-002 §U explicitly deferred "password recovery"**.                                                                                                                                                    | Documented deferral.                                                                                                                       |
| 009 | docs/19:80-84, docs/13 §16                                                                          | Write side delivered in every STEP; read surface never scheduled. No record.                                                                                                                                | Agent/roadmap omission.                                                                                                                    |
| 010 | docs/02:62 Party model                                                                              | STEP-004 delivered shareholder-centric edits only; "advanced history UI" and "Person merge" listed as deferred (STEP-004 §W); person edit not listed.                                                       | No decision found for person edit.                                                                                                         |
| 011 | docs/04 Family Sequence Number                                                                      | No record.                                                                                                                                                                                                  | Omission.                                                                                                                                  |
| 012 | docs/04:36-45 "relationship history … if operationally required"                                    | Spec made it conditional; docs/04:245 open decision on Guardian meaning.                                                                                                                                    | Conditional wording; owner now confirms required.                                                                                          |
| 013 | docs/07:33, docs/07:176                                                                             | Delivered FUNC-FIX-002.                                                                                                                                                                                     | Owner-approved rules recorded in FF2 plan.                                                                                                 |
| 014 | Owner request (AUD §D)                                                                              | Person model limited to names by STEP-004 scope. No record.                                                                                                                                                 | Owner confirms required now.                                                                                                               |
| 015 | docs/05:146-152                                                                                     | `voided` reserved in `0006` for "future correction/reversal workflow (docs/19)"; STEP-006 report absent.                                                                                                    | Deferred in migration comment; no owner decision record.                                                                                   |
| 016 | docs/00:163, docs/06:114, docs/12:226, docs/29 Phase 6                                              | Deferred during PILOT (finding F8, AUD §D); docs/06:219-223 open decisions on session semantics.                                                                                                            | Documented deferral (PILOT), now mandatory.                                                                                                |
| 017 | docs/14, ADR-008, docs/29 Phase 6+8                                                                 | STEP-015 §5: "PDF/XLSX/print export — docs/14 leaves rendering technology open" (although ADR-008 is Accepted). docs/14:208 numbering open.                                                                 | Documented deferral; rationale partially superseded by ADR-008.                                                                            |
| 018 | docs/14:119-135                                                                                     | STEP-015 §5: "Receivable statements/documents — separate transactional-document class".                                                                                                                     | Documented deferral.                                                                                                                       |
| 019 | docs/14 / reporting                                                                                 | STEP-015 §5: as-of "reversal's historical-past effect is undefined in docs/19".                                                                                                                             | Documented deferral with genuine policy gap (PD-28).                                                                                       |
| 020 | docs/00:255-270, docs/09, docs/10, docs/17:139                                                      | docs/29 Phase 10 "Blocked until calculation policies are finalized"; STEP-014 §2 "Policy / PolicyVersion engine — DEFERRED"; docs/10:159 open formulas.                                                     | Documented deferral, genuinely policy-blocked.                                                                                             |
| 021 | docs/08, docs/29 Phase 9                                                                            | STEP-012 §5 lists each sub-item as UNRESOLVED/deferred (expense classification, forecast income, realized gain, disposal reversal, partial disposal, NAV/distribution, documents).                          | Documented deferral per sub-item.                                                                                                          |
| 022 | docs/29 Phase 7 "meetings"                                                                          | No meeting entity in STEP-014; no explicit deferral line for meetings found.                                                                                                                                | Omission.                                                                                                                                  |
| 023 | docs/09:121-127                                                                                     | STEP-014 §2: quorum, majority, weighted/proxy voting, approval→finance linkage "DEFERRED — AUTHORITATIVE POLICY NOT DEFINED".                                                                               | Documented deferral, policy-blocked.                                                                                                       |
| 024 | docs/00:88-90, docs/15:13-15, docs/22:23-27, docs/29 Phase 1                                        | **No STEP implemented tenant context; no deferral decision found.** STEP-001…003 reports contain no tenant scope. All 17 migrations lack a cooperative column.                                              | Omission against a charter principle and system invariant.                                                                                 |
| 025 | docs/24, ADR-009, docs/29 Phase 12                                                                  | Phase 12 never started.                                                                                                                                                                                     | Roadmap-stage, not a decision.                                                                                                             |
| 026 | docs/25, ADR-007, docs/29 Phase 12                                                                  | Phase 12 never started.                                                                                                                                                                                     | Roadmap-stage.                                                                                                                             |
| 027 | docs/12 dialog pattern                                                                              | UIUX-019 applied AlertDialog to users only (AUD §E).                                                                                                                                                        | Incomplete application.                                                                                                                    |
| 028 | —                                                                                                   | README not updated since early stage.                                                                                                                                                                       | Omission.                                                                                                                                  |
| 029 | —                                                                                                   | Leftover from a tool session (registered worktree at `c86a197`).                                                                                                                                            | Artifact.                                                                                                                                  |
| 030 | ADR-013, docs/23, docs/26:127, docs/30                                                              | Provisioning is an environment duty requiring VDS authorization; local drill delivered by PILOT-FIX-001.                                                                                                    | Authorization-gated by owner rule.                                                                                                         |

Additional evidence gap: `implementation/` has no STEP-005…009 reports
(shares, periods, payments, financial accounts, credits). Functionality
is verifiable through code, migrations and E2E scripts, but their
deferral rationale cannot be quoted.

---

## C. Current implementation status (summary)

Full evidence per item is in the registry. Parent-level:

| Status      | Count | Requirements                                                                                  |
| ----------- | ----- | --------------------------------------------------------------------------------------------- |
| VERIFIED    | 1     | 013                                                                                           |
| PARTIAL     | 6     | 007, 009, 012, 020, 021, 030                                                                  |
| DEFECT      | 3     | 006, 027, 028                                                                                 |
| NOT STARTED | 1     | 029                                                                                           |
| MISSING     | 19    | 001, 002, 003, 004, 005, 008, 010, 011, 014, 015, 016, 017, 018, 019, 022, 023, 024, 025, 026 |

Verified existing behavior that the roadmap must preserve (§F):
collection/allocation (payer ≠ debtor), overpayment credit and credit
application, transfers and reversals, derived balances, negative-balance
prevention, hard Social Aid reservation (`physical ≥ reserved`),
investment cash-flow separation (valuation moves no money), share
lifecycle and return entitlements (NULL-undetermined profit), governance
recording with zero money movement, audit writes, idempotency and
concurrency protections, RBAC, session revocation, CSP/CSRF, realtime
authorization, FUNC-FIX-002 default-account rules.

---

## D. Delivery roadmap (dependency-aware)

Ordering principles:

1. Fix defects that can corrupt data first (M0).
2. Introduce the cooperative (tenant) dimension **before** adding more
   domain tables — each later migration otherwise needs a second
   retrofit (M1).
3. Close operational blockers for a real deployment (users, passwords,
   audit, corrections) before new subsystems (M2–M5).
4. The value-type (currency/gold) engine is XL and policy-blocked; it
   follows the TRY-only account structure work so location, opening
   balance and reconciliation are designed value-type-aware once (M5→M6).
5. Infrastructure subsystems precede their consumers: files (M7) before
   documents/minutes/leases; documents (M8) before collection sessions'
   receipt printing (M9); governance (M10) before policy approval (M11)
   before investment distribution (M12).
6. Production provisioning (M14) is sequenced by **authorization**, not
   dependency: its plan is produced in M0 and it may be executed at any
   point the owner authorizes, subject to the ADR-013 gate.

```text
M0 ─► M1 ─► M2 ─► M3 ─► M4 ─► M5 ─► M6
              │                 │
              └──► M7 ─► M8 ─► M9
                         │
                   M10 ─► M11 ─► M12
                   M13 (after M1, M7; triggers grow with domains)
M14 (plan in M0; execution on owner authorization)
```

Every milestone closes only when: fmt/clippy/lint/typecheck, full Rust
suite (sequential), full Vitest, build, clean-DB migration + idempotent
re-run, **existing RC E2E A–K + FUNC-FIX-002 harness + the milestone's
new real-browser/DB scenarios** all pass, and the registry rows are
updated. Each milestone is one or more STEPs, each ending with STOP.

### M0 — Integrity defects and repository hygiene — S

- REQ: 006, 027, 028, 029, REQ-013 evidence hygiene, REQ-030 planning.
- Outcome: no silent field coercion; consistent destructive dialogs; truthful README; reproducible FF2 evidence; provisioning plan ready.
- Scope: `deny_unknown_fields` on all write DTOs; AlertDialog in `sosyal-yardim/[id]`; README rewrite; commit FF2 E2E harness to `scripts/e2e-funcfix002.mjs`; `.kilo` inventory report (removal only after authorization); `docs`-level production provisioning plan (no VDS access).
- Migrations: none.
- Security/audit: stricter input validation.
- Tests: negative unknown-field test per router; Vitest dialog tests.
- Acceptance: AUD §F probe `currency:'USD'` → 422 `validation_failed`, nothing created; social-aid reverse requires AlertDialog confirm.
- Risks: a client sending an extra field breaks — mitigated by full Vitest + E2E run.
- Decisions: owner authorization for REQ-029 deletion.
- **PROJECT-CONTROL-002 refinement (M0 discovery):** detailed plan in `implementation/M0-FOUNDATION-CLEANUP-PLAN.md`. Added: CI repair (F-M0-02 — remote CI red since run 6; WP-9/WP-10), REQ-027 scope question (F-M0-01 — 20 native confirms in 6 routes; M0-D1), test-client field cleanup (F-M0-04). Owner decisions M0-D1…M0-D4.
- **M0 CLOSURE (2026-10-09):** all owner decisions approved (M0-D1 all 20 dialogs, M0-D2 `.kilo/` removal, M0-D3 CI repair, M0-D4 historical scripts preserved). Implemented: `deny_unknown_fields` on ~57 request DTOs + 15 negative tests (REQ-006 VERIFIED); `ConfirmActionDialog` + all 20 operations converted, `e2e:dialogs` 18/18 (REQ-027 VERIFIED); README corrected (REQ-028 VERIFIED); `.kilo/` removed after re-verified safety checks (REQ-029 VERIFIED); `scripts/e2e-funcfix002.mjs` + `pnpm e2e:ff002` → 35/35; CI workflow repaired locally — `backend` job runs dependency-free tests only, `database` job keeps all DB-gated suites, `backup` job gained diagnostics (remote verification still requires an owner-authorized push); `implementation/TEST-INVENTORY.md` classifies active vs historical scripts (M0-D4); `implementation/M14-PRODUCTION-PROVISIONING-PLAN.md` delivered (REQ-030 planning only). Status: **locally complete — awaiting owner gate for M1**.

### M1 — Multi-cooperative foundation — XL

- REQ: 024 (all subs), 007.7.
- Outcome: the platform hosts several cooperatives with backend-enforced isolation; existing data becomes the default cooperative.
- Scope (recommended strategy, PD-01): shared database, `cooperatives` table, `cooperative_id UUID NOT NULL` FK on every cooperative-owned table, composite uniqueness (e.g. `(cooperative_id, sequence_number)`), request-scoped cooperative context resolved from session + membership, repository-layer mandatory scoping; optional PostgreSQL Row-Level Security as defense in depth.
- Migrations: add nullable column → backfill default cooperative → `SET NOT NULL` → swap unique indexes; one migration per module group, each idempotent and reversible; executed in a maintenance window on production.
- Backend: context extractor; every repo query takes cooperative id; realtime channels scoped; reports scoped.
- Frontend: cooperative switcher for multi-membership users; cooperative name in shell/TV.
- Security/audit: `security_events.cooperative_id`; cross-cooperative ids answer 404 (docs/21:86).
- Tests: tenant-isolation/IDOR suite per module (docs/26:36,103); migration test on a populated snapshot.
- Acceptance: two cooperatives with identically numbered families and accounts; an operator of cooperative A sees, posts and receives realtime events only for A; all prior suites green.
- Risks: largest blast radius of the roadmap (touches every query). Mitigation: mechanical, module-by-module STEPs; contract tests; restore-verified backup before production migration.
- Decisions: PD-01, PD-02.
- **M1-ARCH-001 STATUS (2026-10): discovery + planning complete, implementation NOT started.** Detailed artifacts: `M1-ARCHITECTURE-DISCOVERY.md`, `M1-TENANT-DATA-MODEL.md`, `M1-DATABASE-MIGRATION-PLAN.md`, `M1-TENANT-ISOLATION-TEST-MATRIX.md`, `M1-IMPLEMENTATION-ROADMAP.md` (phases P0–P9, gates G1–G5), `M1-OWNER-DECISIONS.md` (7 open decisions M1-K1…K7). Recommended deviations from the sketch above: per-coop counter-table numbering (identity columns cannot produce per-coop sequences — 024.6), header + session-default hybrid context (multi-tab requirement), persons coop-owned, RLS via dedicated `kooperatif_app` role, coop creation via CLI. **Blocked on owner decision gate G1.**

### M2 — User administration, passwords, audit viewer — M

- REQ: 007, 008, 009 (009.5 export completes in M8).
- Outcome: no SSH needed to onboard operators; credentials manageable; audit evidence accessible.
- Migrations: optional `users.must_change_password`, `password_changed_at`; index on `security_events (cooperative_id, created_at)`, `(event_type)`, metadata entity id.
- Backend: `POST/PATCH /api/users`, `POST /api/auth/password`, admin reset endpoint; `GET /api/audit-events`; permissions `audit.read`, `audit.export`.
- Frontend: create-user dialog; password change page; admin reset dialog; `denetim` screen; history panels.
- Security: argon2 parameters unchanged; rate limits; revoke other sessions on change; no secrets in events/logs; last-admin protection.
- Acceptance: admin creates user → user logs in → changes password → old sessions revoked; auditor finds a default-account change by shareholder; non-auditor 403.
- Decisions: PD-14, PD-15.

### M3 — Party data corrections — M

- REQ: 010, 011, 012, 014.
- Migrations: `shareholder_guardianships` (temporal, exclusion constraint, backfill); person contact/notes columns; none destructive.
- Backend: `PATCH /api/persons/{id}`, `PATCH /api/families/{id}`, guardian change writes interval; contact/notes with PII permission.
- Frontend: person detail page; family edit; guardian timeline; contact/notes form.
- Acceptance: correct guardian name once, visible everywhere; correct family number, duplicate rejected; two guardian changes → three intervals; contact hidden from unpermitted role.
- Decisions: PD-16, PD-17, PD-18.

### M4 — Assessment correction and void — M

- REQ: 015.
- Migrations: `assessments.voided_at/by/reason`, `replaces_assessment_id`; no deletion.
- Backend: void endpoint; reissue linkage; behavior with existing allocations/credits per PD-19; reports exclude voided.
- Frontend: void/correct actions on `tahakkuklar/[id]` with consequence preview.
- Financial integrity: settled ≤ assessed preserved; any allocation release follows the existing reversal model; credit semantics unchanged; no cash movement.
- Acceptance: wrong period amount → void + reissue, debts correct, payments' cash and account balances unchanged, history complete.
- Decisions: PD-19.

### M5 — Account structure (TRY): location, opening balance, reconciliation — M/L

- REQ: 003, 004 (004.1–004.5), 005.
- Migrations: `locations`; `financial_accounts.location_id`; movement source types `opening_balance`, `reconciliation_adjustment`; `account_reconciliations`.
- Backend: location CRUD; opening-balance post/reverse; reconciliation create/list; adjustment per PD-13.
- Frontend: location select/filter; opening-balance action; reconciliation tab on account detail; reconciliation report card.
- Financial integrity: opening balance and adjustments are neither income nor expense; reservation invariant applies to adjustments that reduce balance.
- Acceptance: İstanbul/Köy accounts filtered by location; opening balance 10.000,00 visible and reversible; counted 9.950,00 → difference −50,00 recorded; balance unchanged unless an approved adjustment is posted.
- Decisions: PD-11, PD-12, PD-13.

### M6 — Value-type engine: USD, EUR, gold — XL

- REQ: 001, 002, 004.6.
- Migrations: value-type catalog; relax `CHECK currency='TRY'` on accounts/transfers (and payments/assessments only as PD-04 decides); widen quantity precision where gold requires (PD-08) — type change on movement tables rewrites them, so schedule with a maintenance window; backfill `TRY`.
- Backend: value-type validation on every posting path (payment, transfer, income, expense, settlement, investment, social aid); same-code transfer rule; conversion transaction if approved (PD-05) as paired legs with explicit rate; all summaries grouped by value code.
- Frontend: currency/unit selector; unit-aware formatting (₺, $, €, g); reports with separate totals; no consolidated figure unless PD-06 approves a labelled valuation.
- Acceptance: AUD §F scenarios A–L re-run: C/D/E/G become SUPPORTED; I rejected (or converted only via approved conversion); J rejected; K never mixes units; L preserves value code.
- Risks: highest financial-semantics risk. Mitigation: value code enforced in DB (CHECK/trigger matching account) and in Rust types; golden cross-currency report fixture.
- Decisions: PD-03 … PD-10.

### M7 — Files and attachments — L

- REQ: 026; enables 021.g, 022.5, 017 evidence.
- Migrations: `attachments`, `attachment_links`, versions.
- Backend: upload/download via backend authorization (ADR-007), checksum, limits, archive not delete.
- Frontend: attachment panel component on payment, share, decision, investment, social-aid detail.
- Ops: storage included in ADR-013 backup + restore drill.
- Acceptance: upload dekont to a payment; unpermitted user 403; replace keeps v1; archived file still restorable.
- Decisions: PD-26.

### M8 — Documents, statements, as-of reports — L/XL

- REQ: 017, 018, 019, 009.5.
- Migrations: `documents` (type, number, snapshot, template version), numbering sequences per cooperative.
- Backend: ADR-008 renderer; receipt on payment post (or on demand per PD-21); PDF/XLSX for report catalog; statements; as-of query layer per PD-28.
- Frontend: print/download actions; receipt view; statement pages.
- Acceptance: payment → receipt with unique number under concurrent posting; reprint identical; reversal shown on receipt status; XLSX exact decimals; as-of balance for a past date equals the hand-computed value.
- Decisions: PD-21, PD-28.

### M9 — Collection Session workspace — L

- REQ: 016.
- Migrations: `collection_sessions`, `collection_session_accounts`, `collection_session_operators`, `payments.collection_session_id`.
- Backend: session lifecycle; posting inside session reuses the existing payment command (no new financial semantics); session totals; close-out per PD-20.
- Frontend: dedicated high-throughput workspace (docs/12:226, docs/13 §10-11); receipt print (M8).
- Acceptance: open session for a period at Köy location; 20 payments incl. family and third-party payers; FF2 multi-shareholder rule still enforced; totals live; close with summary.
- Decisions: PD-20.

### M10 — Meetings, agenda, quorum, financial authorization — M/L

- REQ: 022, 023.
- Migrations: `meetings`, `agenda_items`, `meeting_attendance`, decision↔agenda link, policy-configured thresholds.
- Backend: computed quorum/majority beside recorded outcome; authorization guard on listed financial operations (PD-24).
- Frontend: meeting/agenda screens; attendance; computed vs recorded outcome display.
- Acceptance: meeting below quorum flags the decision; a guarded financial operation without a passed linked decision → rejected.
- Decisions: PD-24.

### M11 — Versioned policy engine and value protection — XL

- REQ: 020.
- Migrations: `policies`, `policy_versions` (immutable after effective), entitlement ↔ version link, index-value table.
- Backend: effective-date resolution; approved formulas only; base right never overwritten; payable equivalent derived; simulation endpoint (no posting).
- Acceptance: entitlement crystallized under v1 keeps v1 after v2 becomes effective; value-protected payable computed from recorded inputs and reproducible.
- Decisions: PD-22 (cannot start formula work without it).

### M12 — Advanced investment accounting — L

- REQ: 021.a–g.
- Each sub-item is a separate STEP with its own AC (registry table).
- Financial integrity: forecast income never income; valuation never cash; disposal reversal restores state and reverses proceeds movement; partial disposal keeps remaining cost basis.
- Decisions: PD-23a–e.

### M13 — Notifications and tasks — L

- REQ: 025.
- Migrations: `notifications`, `tasks`, job tables (ADR-009).
- Backend: durable scheduler; idempotent trigger generation; completion never executes finance.
- Frontend: notification center; task list.
- Acceptance: period due-date reminder generated once per recipient; acknowledging a task posts nothing.
- Decisions: PD-25.

### M14 — Production provisioning and recovery — M (execution requires owner authorization)

- REQ: 030 (all subs).
- Scope: native PostgreSQL configuration, WAL archiving + PITR, encrypted off-site copies, production restore drill, release builds, systemd units, reverse proxy + HTTPS, deployment/rollback runbook, monitoring/alerting, operational security.
- Acceptance: ADR-013 Backup Readiness Gate (docs/26:127) passed with restore evidence; PITR to a chosen timestamp demonstrated; rollback rehearsed.
- Decisions: PD-27.

---

## E. Unresolved business-policy decision register

Only genuinely open implementation policies. Implementation of the
capability itself is not in question.

Already decided (not re-asked): ADR-004 NUMERIC + explicit value codes;
ADR-007 PostgreSQL metadata + private S3-compatible storage; ADR-008
central typed/versioned rendering; ADR-009 PostgreSQL durable jobs;
ADR-010 cursor pagination; docs/19 audit UI read-only + separate export
permission; docs/24 in-app notifications first; docs/25 archive instead
of delete; FUNC-FIX-002 default-account rules incl. multi-shareholder
no-inference.

| PD     | Exact question                                                                                                                                              | Recommended                                                                                                                                                                                                                  | Alternatives                                                              | Consequences                                                                                                                                             | REQ           |
| ------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------- |
| PD-01  | **APPROVED (owner, PROJECT-CONTROL-002)** — How are multiple cooperatives stored?                                                                           | One database, `cooperative_id` on every owned row, enforced in the repository layer (+ optional RLS)                                                                                                                         | Database per cooperative; schema per cooperative                          | Recommended keeps one migration path and one backup; DB-per-coop gives hard isolation but multiplies migrations, backups and connection pools on one VDS | 024           |
| PD-02  | **APPROVED (owner, PROJECT-CONTROL-002)** — May one user work in several cooperatives, with different roles each?                                           | Yes: membership table `(user, cooperative, roles)`                                                                                                                                                                           | One user = one cooperative                                                | Recommended avoids duplicate accounts for shared staff; requires a context switcher                                                                      | 024, 007      |
| PD-03  | Is the currency list fixed (TRY, USD, EUR) or configurable?                                                                                                 | Configurable catalog seeded with TRY, USD, EUR + gold code                                                                                                                                                                   | Fixed enum                                                                | Catalog allows later GBP etc. without migration                                                                                                          | 001           |
| PD-04  | Can a TRY assessment be paid in USD/EUR/gold?                                                                                                               | No — payments must match assessment currency; foreign cash enters via a separate conversion                                                                                                                                  | Allow with rate at payment time                                           | Allowing it requires a rate on every allocation and FX gain/loss handling                                                                                | 001, 015, 016 |
| PD-05  | Is in-system currency/gold conversion allowed, and where do rates come from?                                                                                | Allowed as an explicit conversion transaction with a manually entered, audited rate                                                                                                                                          | External rate feed; no conversion (outside system only)                   | Manual rate keeps the system offline-safe; feed adds dependency and trust questions                                                                      | 001, 002      |
| PD-06  | Is a consolidated TRY valuation needed in reports?                                                                                                          | Yes, as a separately labelled "valuation" using a dated rate, never mixed with balances                                                                                                                                      | No consolidation                                                          | Without it the board sees no single total; with it rate date/source must be shown                                                                        | 001, 002, 019 |
| PD-07  | Gold unit and purity?                                                                                                                                       | Grams of 24-ayar (has) gold as the single unit                                                                                                                                                                               | Multiple purities (22/24 ayar); pieces (çeyrek/yarım/tam) with conversion | Multiple units need per-unit accounts or conversion factors                                                                                              | 002           |
| PD-08  | Gold quantity precision?                                                                                                                                    | 3 decimal places (0,001 g)                                                                                                                                                                                                   | 2 or 4 decimals                                                           | Determines column scale; changing later rewrites tables                                                                                                  | 002           |
| PD-09  | Are gold purchases/sales conversions or investments?                                                                                                        | Conversion between value types (cash → gold account)                                                                                                                                                                         | Investment (STEP-012 model)                                               | Investment path brings valuation/gain rules; conversion keeps it a treasury operation                                                                    | 002, 021      |
| PD-10  | May Social Aid funds hold USD/EUR/gold?                                                                                                                     | Yes, reservation tracked per account value code                                                                                                                                                                              | TRY only                                                                  | Recommended needs reservation invariant per value code                                                                                                   | 001, 002      |
| PD-11  | Is location a configurable catalog, and is it mandatory on accounts?                                                                                        | Configurable catalog; mandatory for new accounts, backfill existing as "Belirtilmemiş"                                                                                                                                       | Fixed İstanbul/Köy; optional                                              | Mandatory gives reliable reports                                                                                                                         | 003, 016      |
| PD-12  | Opening balance rules?                                                                                                                                      | One active opening entry per account, dated, separate permission, correction only by reversal + re-entry                                                                                                                     | Multiple dated entries; equity-style account                              | Recommended is simplest to audit                                                                                                                         | 004           |
| PD-13  | May reconciliation post an adjustment, who approves, how often?                                                                                             | Yes, as explicit `reconciliation_adjustment` movement with reason, requiring a separate permission; frequency monthly recommendation (not enforced)                                                                          | Record difference only, never adjust                                      | Without adjustment balances never match counted cash                                                                                                     | 005           |
| PD-14  | How does an administrator reset a password (no e-mail exists)?                                                                                              | Admin sets a one-time temporary password; user must change it at next login                                                                                                                                                  | One-time reset link shown to admin; e-mail reset (needs mail integration) | Recommended works offline; temporary password is visible to admin once                                                                                   | 008, 007      |
| PD-15  | Who may view/export audit records, and how long are they kept?                                                                                              | View: dedicated "Denetçi"/admin roles; export separate; retention ≥ 10 years, no automatic deletion                                                                                                                          | Shorter retention                                                         | Legal/operational; deletion is irreversible                                                                                                              | 009           |
| PD-16  | Which person contact fields are required (phone, address, e-mail, national id?) and who may see them?                                                       | Phone, address, e-mail, free notes; no national id; visible only with a contact permission                                                                                                                                   | Include T.C. kimlik no                                                    | National id raises KVKK obligations (encryption, access logging)                                                                                         | 014           |
| PD-17  | Can guardian changes be back-dated?                                                                                                                         | Yes, with a mandatory reason; no overlap                                                                                                                                                                                     | Effective only from today                                                 | Back-dating supports correction of history                                                                                                               | 012           |
| PD-18  | Can a corrected family number be reused by another family later?                                                                                            | No — retired numbers stay reserved                                                                                                                                                                                           | Allow reuse                                                               | Reuse can confuse paper records/receipts                                                                                                                 | 011           |
| PD-19  | What happens when voiding an assessment that has allocations or credit applications, or is in a closed period?                                              | Block void until allocations/credit applications are reversed via the existing reversal workflow (released money returns as unallocated/credit per current rules); closed periods require a reason and a separate permission | Auto-reverse allocations inside the void; forbid void after close         | Recommended reuses verified reversal semantics; auto-reverse is faster but bundles two financial effects                                                 | 015           |
| PD-20  | Collection Session: can it restrict destination accounts, and is a cash count required at close?                                                            | Session has allowed accounts (default = all active of its location); close records counted cash per account (uses REQ-005 reconciliation)                                                                                    | No restriction; no count                                                  | Restriction prevents wrong-cashbox postings                                                                                                              | 016           |
| PD-21  | Receipt numbering, snapshots, QR/signature?                                                                                                                 | Per cooperative per year sequence (`2026-000123`), gap-free, immutable snapshot at issue; QR verification later; no e-signature                                                                                              | Per session / per location numbering                                      | Numbering cannot change after go-live                                                                                                                    | 017, 018      |
| PD-22  | Share-return formulas: principal, profit right, cutoff, waiting periods, value-protection modes and data sources, allocation priority                       | **Owner must supply**; no recommendation on formulas (charter forbids inventing). Recommended mechanism: each formula as a PolicyVersion approved by decision                                                                | —                                                                         | Blocks M11 formula sub-items                                                                                                                             | 020           |
| PD-23a | How is an investment-related expense classified?                                                                                                            | Capitalized to investment cost when tied to acquisition, otherwise investment operating expense (not cooperative operational expense)                                                                                        | Always operational expense                                                | Affects realized gain                                                                                                                                    | 021.a         |
| PD-23b | Realized gain/loss formula?                                                                                                                                 | Proceeds − (funded cost + capitalized expenses − prior partial-disposal cost)                                                                                                                                                | Include valuations                                                        | Owner/accountant to confirm                                                                                                                              | 021.b         |
| PD-23c | May a disposal be reversed, and until when?                                                                                                                 | Yes, while no distribution used its proceeds; reverses proceeds movement and restores status                                                                                                                                 | Never reversible                                                          | Reversal must respect reservation and balances                                                                                                           | 021.c         |
| PD-23d | Partial disposal cost basis?                                                                                                                                | Pro-rata by disposed percentage                                                                                                                                                                                              | Specific identification                                                   | Determines remaining cost                                                                                                                                | 021.d         |
| PD-23e | NAV and profit distribution to shareholders?                                                                                                                | NAV as labelled valuation report; distribution only by approved PolicyVersion + decision                                                                                                                                     | No distribution feature                                                   | Links to M10/M11                                                                                                                                         | 021.e         |
| PD-24  | Governance: body structure, quorum, majority/tie, electronic voting evidence, which financial operations need a passed decision and above which thresholds? | Owner/statute to supply rules; mechanism recommendation: thresholds as PolicyVersion; computed outcome shown beside recorded outcome                                                                                         | —                                                                         | Without rules only recording is possible                                                                                                                 | 022, 023      |
| PD-25  | Which notification triggers and lead times in the first release?                                                                                            | Period due date (−7 days), overdue debt (weekly), approval waiting, share-return due                                                                                                                                         | Full docs/24 list at once                                                 | Smaller first set reduces noise                                                                                                                          | 025           |
| PD-26  | File storage provider on a single VDS, size/type limits, retention?                                                                                         | MinIO on the VDS + encrypted off-site replication; 20 MB limit; PDF/JPEG/PNG; archive only                                                                                                                                   | External S3 provider                                                      | Storage must be inside backup scope                                                                                                                      | 026, 030      |
| PD-27  | Production targets: off-site backup location, RPO/RTO, domain/TLS, monitoring stack                                                                         | RPO ≤ 15 min (WAL), RTO ≤ 4 h; off-site encrypted object storage; Let's Encrypt; lightweight uptime + log alerting                                                                                                           | Daily dumps only                                                          | Determines WAL archiving effort                                                                                                                          | 030           |
| PD-28  | As-of reports: does a reversal recorded later change a past date's figure?                                                                                  | Two views: "as recorded at date" (ignores later reversals) and "as corrected" (applies them), both labelled                                                                                                                  | Single view                                                               | Defines REQ-019 semantics (STEP-015 §5 gap)                                                                                                              | 019           |

---

### E.1 Decision status and earliest milestone needing the answer

Maintained per PROJECT-CONTROL-002 §10. An open decision blocks only
the sub-items that name it, never unrelated milestones.

| PD    | Status       | Earliest milestone | PD       | Status | Earliest milestone |
| ----- | ------------ | ------------------ | -------- | ------ | ------------------ |
| PD-01 | **APPROVED** | M1                 | PD-15    | open   | M2                 |
| PD-02 | **APPROVED** | M1                 | PD-16    | open   | M3                 |
| PD-03 | open         | M6                 | PD-17    | open   | M3                 |
| PD-04 | open         | M6                 | PD-18    | open   | M3                 |
| PD-05 | open         | M6                 | PD-19    | open   | M4                 |
| PD-06 | open         | M6                 | PD-20    | open   | M9                 |
| PD-07 | open         | M6                 | PD-21    | open   | M8                 |
| PD-08 | open         | M6                 | PD-22    | open   | M11                |
| PD-09 | open         | M6                 | PD-23a–e | open   | M12                |
| PD-10 | open         | M6                 | PD-24    | open   | M10                |
| PD-11 | open         | M5                 | PD-25    | open   | M13                |
| PD-12 | open         | M5                 | PD-26    | open   | M7                 |
| PD-13 | open         | M5                 | PD-27    | open   | M14                |
| PD-14 | open         | M2                 | PD-28    | open   | M8                 |

Milestone-local decisions (M0): M0-D1 REQ-027 scope, M0-D2 `.kilo/`
removal, M0-D3 CI fix + drill log, M0-D4 historical E2E scripts — see
`implementation/M0-FOUNDATION-CLEANUP-PLAN.md` §4.

## F. Migration and financial-integrity risk assessment

| Milestone | Change                                                | Risk                                                                    | Mitigation                                                                                                                                          |
| --------- | ----------------------------------------------------- | ----------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| M0        | `deny_unknown_fields`                                 | Legitimate clients rejected                                             | Full Vitest + all E2E harnesses before commit                                                                                                       |
| M1        | `cooperative_id` on all tables, composite unique keys | Very high: any unscoped query leaks or mis-joins data; long ALTERs      | Three-phase migration (add nullable → backfill → NOT NULL); module-by-module STEPs; tenant-isolation suite; restore-verified backup before prod run |
| M2        | Password/user tables                                  | Lockout of last admin; secret leakage                                   | Existing lockout protections; tests asserting no secrets in events                                                                                  |
| M3        | Guardian temporal table backfill                      | Wrong initial interval                                                  | Backfill from current column with `started_at = shareholder.created_at`; verification query in migration test                                       |
| M4        | Assessment void                                       | Orphan allocations; settled > assessed                                  | Block void while active allocations exist (PD-19 recommended); DB constraint tests                                                                  |
| M5        | New movement source types                             | Opening balance counted as income; adjustment bypassing reservation     | Provenance class tests; reservation check on adjustments                                                                                            |
| M6        | Value code + precision change                         | Cross-unit sums; rounding loss; table rewrite on `NUMERIC` scale change | DB-level value-code match; golden multi-currency fixture; maintenance window; reversible migration with tested down-path                            |
| M7        | File store                                            | Evidence loss; public exposure                                          | Archive-only; backend-authorized streaming; storage in backup drill                                                                                 |
| M8        | Numbering                                             | Duplicate/gap under concurrency                                         | Row-locked sequence per cooperative/year; concurrency test                                                                                          |
| M9        | Session linkage                                       | New posting path diverging from verified command                        | Reuse existing payment command unchanged; regression via FF2 harness                                                                                |
| M10       | Financial authorization guard                         | Blocking legitimate operations                                          | Guard driven by policy version; default off until configured                                                                                        |
| M11       | Policy versions                                       | Rewriting crystallized rights                                           | Versions immutable; entitlement stores version id + base; tests that old entitlements are unchanged                                                 |
| M12       | Disposal reversal / partial                           | Balance or reservation violation                                        | Reuse reversal + reservation guards                                                                                                                 |
| M13       | Scheduler                                             | Duplicate notifications; task executing finance                         | Idempotent job keys; test zero movements on completion                                                                                              |
| M14       | Production                                            | Data loss, downtime                                                     | ADR-013 gate, PITR demonstrated, rollback rehearsed; owner authorization                                                                            |

Regression protection (every milestone): full Rust suite run
**sequentially** (avoid the reproduced concurrent-process PoolTimedOut
contention), full Vitest, `scripts/e2e-step017-rc.mjs` journeys A–K,
`scripts/e2e-step017-uiux.mjs`, FUNC-FIX-002 harness, clean-DB migration

- idempotent re-run, and a migration run against a populated snapshot.

---

## G. Proposed milestone sequence (summary)

| #   | Milestone                                       | REQ                                            | Size | Blocking decisions             |
| --- | ----------------------------------------------- | ---------------------------------------------- | ---- | ------------------------------ |
| M0  | Integrity defects + hygiene                     | 006, 027, 028, 029, (013 evidence), (030 plan) | S    | REQ-029 deletion authorization |
| M1  | Multi-cooperative foundation                    | 024 (+007.7)                                   | XL   | PD-01, PD-02                   |
| M2  | Users, passwords, audit viewer                  | 007, 008, 009                                  | M    | PD-14, PD-15                   |
| M3  | Party corrections                               | 010, 011, 012, 014                             | M    | PD-16, PD-17, PD-18            |
| M4  | Assessment correction/void                      | 015                                            | M    | PD-19                          |
| M5  | Location, opening balance, reconciliation (TRY) | 003, 004, 005                                  | M/L  | PD-11, PD-12, PD-13            |
| M6  | USD/EUR/gold value-type engine                  | 001, 002, 004.6                                | XL   | PD-03 … PD-10                  |
| M7  | Files/attachments                               | 026                                            | L    | PD-26                          |
| M8  | Documents, statements, as-of                    | 017, 018, 019, 009.5                           | L/XL | PD-21, PD-28                   |
| M9  | Collection Session                              | 016                                            | L    | PD-20                          |
| M10 | Meetings, quorum, financial authorization       | 022, 023                                       | M/L  | PD-24                          |
| M11 | Policy engine + value protection                | 020                                            | XL   | PD-22                          |
| M12 | Advanced investments                            | 021.a–g                                        | L    | PD-23a–e                       |
| M13 | Notifications/tasks                             | 025                                            | L    | PD-25                          |
| M14 | Production provisioning                         | 030                                            | M    | PD-27 + owner authorization    |

Milestones without blocking decisions (M0, and the non-BLOCKED-PD
sub-items of others) can start immediately after approval. No exact
dates are given.

---

## H. Git status and documentation changes

- Added: `implementation/REQUIREMENTS-REGISTRY.md`, `implementation/PROJECT-CONTROL-001-RECOVERY-ROADMAP.md`.
- No source, schema, test or configuration file changed.
- Committed locally only; nothing pushed; previous local commits preserved.

---

## I. Coverage confirmation

- Parent requirements accounted for: **30 / 30** (REQ-001 … REQ-030), each with source, status, milestone and acceptance criteria.
- Sub-requirements tracked individually: **170**, each with status and acceptance criterion; REQ-021's seven sub-requirements (021.a–g) each carry their own status and AC.
- No requirement is removed, downgraded, or permanently deferred; every item has a target milestone.
- Policy-blocked items are labelled `BLOCKED-PD` with a decision id; the capability remains mandatory.

---

## J. Sahip için kısa özet (Türkçe)

- 30 gereksinimin tamamı kayıt altına alındı; hiçbiri çıkarılmadı. Şu an 1'i doğrulanmış (varsayılan kasa), 6'sı kısmi, 3'ü hata, 1'i başlanmamış, 19'u eksik.
- En kritik tespit: sistem tek kooperatif için kurulmuş; çoklu kooperatif desteği için her tabloya kooperatif bilgisi eklenmesi gerekiyor. Sonradan eklemek her yeni modülle zorlaşacağı için bunu erken (M1) yapmayı öneriyoruz.
- İlk adım (M0) küçük ve risksiz: yanlış "USD" kaydının sessizce TL'ye dönmesini engellemek, sosyal yardım onay penceresini düzeltmek, README'yi güncellemek, eski `.kilo` klasörünü (onayınızla) temizlemek.
- Döviz/altın (M6) en büyük ve en riskli iş; başlamadan önce PD-03…PD-10 kararlarınıza ihtiyaç var (ör. altın gram mı, ayar ne, kur nereden girilecek).
- Hisse iadesi formülleri (PD-22) ve kurul yeter sayısı kuralları (PD-24) sizin veya tüzüğün belirlemesi gereken konular; bu formülleri biz önermiyoruz.
- Canlı sunucu (VDS) işlemleri ayrıca sizin onayınızla yapılacak.

**STOP — owner review and approval required before any implementation.**
