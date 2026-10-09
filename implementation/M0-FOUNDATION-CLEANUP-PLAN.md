# M0 — FOUNDATION CLEANUP AND IMMEDIATE DEFECTS — DISCOVERY & PLAN

Governance: PROJECT-CONTROL-002, Phase A (Discovery) + Phase B (Plan).
**Status: AWAITING OWNER APPROVAL (Phase C).** No application code,
schema, CI configuration or file outside `implementation/` was changed
while preparing this plan.

---

## 1. Verified baseline (Phase A)

| Item               | Verified value                                                                  |
| ------------------ | ------------------------------------------------------------------------------- |
| Branch / HEAD      | `main` @ `a9b845c` (PROJECT-CONTROL-001 docs)                                   |
| Local-only commits | `1470594`, `fb6793c`, `1a23024`, `a9b845c` (4) — nothing pushed                 |
| `origin/main`      | `a0fa441`; `git fetch --dry-run` reports no new remote commits                  |
| Working tree       | clean (the table re-formatting seen in the editor is already part of `a9b845c`) |
| Worktrees          | main + `.kilo/worktrees/versed-stallion` @ `c86a197` (detached)                 |
| Local backup drill | `node scripts/backup/drill.mjs` → **RECOVERY DRILL PASSED** (55 checks, 55.0 s) |

## 2. Discovery findings

### 2.1 REQ-006 — strict API write validation (DEFECT confirmed)

- 57 distinct top-level JSON request DTO type names (some names, e.g. `ReverseRequest`, are reused by several modules) across 13 routers (`auth/rbac`, `auth/routes`, `credits`, `financial_accounts`, `governance`, `income_expense`, `investments`, `parties`, `payments`, `periods`, `shares`, `share_returns`, `social_aid`) plus nested request types (`PersonRefRequest`, `FamilyRefRequest`, allocation lines, funding legs, entitlement specs …).
- `deny_unknown_fields`: **0 occurrences** in the codebase → every unknown body field is silently dropped.
- No conflict with serde limitations: every `#[serde(flatten)]` in the code is on **response** (Serialize) DTOs (`ShareholderDetailDto`, `PaymentDetailDto`, `PeriodDetailDto`, `AssessmentDetailDto`). The FUNC-FIX-002 triple-state field uses `deserialize_with` and is compatible.
- Rejection path: axum 0.8 reports body _data_ errors (unknown/missing field, wrong type) as **422**; the PILOT-FIX-001 F6 middleware (`apps/server/src/http/mod.rs:112-152`) rewrites the body to `{"error":{"code":"validation_failed"}}` and keeps the status. Malformed JSON _syntax_ is 400. Existing tests already accept 422 for body-shape errors (e.g. `financial_accounts.rs:558`).
  → Planned contract: unknown field = **422 `validation_failed`, nothing persisted** (consistent with existing body-shape errors). The registry AC previously said 400; it is corrected to 422 (technical alignment, not a business decision).
- **Silent-drop evidence in our own test clients** (would break under strict mode, and proves the defect matters):
  - FUNC-FIX-002 harness sends `idempotencyKey` to `POST /api/financial-accounts`, `POST /api/financial-accounts/{id}/status-change`, `POST /api/shareholders`, `POST /api/periods` — none of these DTOs has that field.
  - `scripts/e2e-step014/015/016/017-rc/017-uiux.mjs` send `idempotencyKey` to `POST /api/shareholders`; `e2e-step017-uiux.mjs` also to `POST /api/periods`.
  - The web UI does **not** send it to these endpoints (UI call sites with `idempotencyKey` all target DTOs that define it).
- Observation (not in M0 scope, recorded for M3/M5): shareholder, financial-account and period **creation is not idempotent** server-side; the scripts' `idempotencyKey` gave a false impression that it was. UI double-submit protection exists. Tracked as finding F-M0-03.

### 2.2 REQ-027 — confirmation dialog consistency (scope larger than audited)

FUNC-AUDIT-001 §E reported only Social Aid. Actual native confirmations: **20 calls in 6 routes**:

| Route                 | Calls                | Actions                                                                        |
| --------------------- | -------------------- | ------------------------------------------------------------------------------ |
| `sosyal-yardim/[id]`  | 4 (`window.confirm`) | fund close, fund cancel, donation reverse, disbursement reverse                |
| `yatirimlar/[id]`     | 5 (`window.confirm`) | investment cancel, funding reverse, valuation cancel, income reverse, disposal |
| `hisse-iadeleri/[id]` | 4 (`confirm`)        | return cancel, finalize, entitlement cancel, settlement reverse                |
| `donemler/[id]`       | 3                    | generate assessments, close period, delete draft                               |
| `hisseler/[id]`       | 2                    | ownership change (transfer/sale), status change                                |
| `hissedarlar/[id]`    | 2                    | family change, status change                                                   |

- The approved pattern exists only on `kullanicilar/+page.svelte` (shadcn `AlertDialog`, from line 169). docs/12:16 requires "same action type → same interaction pattern throughout the product"; docs/12:118 requires the confirmation to explain the consequence.
- Tests coupled to native dialogs: Vitest `periods-pages.spec.ts:282`, `shares-pages.spec.ts:286` (`vi.spyOn(window,'confirm')`); historical E2E scripts `e2e-hotfix001`, `e2e-step005…015` (`page.on('dialog', accept)`). The maintained harnesses (`e2e-step017-rc`, `e2e-step017-uiux`) do not depend on native dialogs.
- → Owner decision **M0-D1** (scope).

### 2.3 REQ-028 — README accuracy (DEFECT confirmed, specific errors)

| Location              | Problem                                                                                                                                                                                             |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `README.md:8-17`      | Claims "BACKUP-READINESS-001 stage … No Payments, Collection Sessions, Ledger, Investments, Governance or Social Aid exist yet" — false since STEP-007…017                                          |
| `README.md:190-196`   | "the four STEP-003 keys"; "No financial … permissions exist yet" — outdated (permission catalog now spans all modules)                                                                              |
| `README.md:1242-1243` | "Without `KOOPERATIF_TEST_DATABASE_URL` the DB-gated tests skip" — **wrong**: since PILOT-FIX-001 F5 they fail (panic) by design                                                                    |
| `README.md:1286-1290` | "Ledger/Receipt/Collection Session and the investment surfaces intentionally do not [exist]" — investments exist (STEP-012)                                                                         |
| `README.md:63`        | `scripts/` described as only `verify.sh, db-verify.sh` (E2E harnesses, backup toolkit omitted)                                                                                                      |
| missing               | PILOT-FIX-001, UI-TR-001, UIUX-018/019, FUNC-FIX-002 (default collection account) sections; known limitations; link to the requirements registry; maintained vs historical E2E harnesses; CI status |
| order                 | STEP-009 section placed after STEP-017                                                                                                                                                              |

### 2.4 REQ-029 — `.kilo/` legacy worktree (evidence complete, nothing deleted)

| Check                | Result                                                                                                                                                                                                           |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Tracked by main repo | No (`git ls-files .kilo` empty); ignored via local `.git/info/exclude:9` (`.kilo/worktrees/`) and `.kilo/.gitignore` (ignores itself, `node_modules`, lockfiles, `agent-manager.json`)                           |
| `.kilo/` top level   | `.gitignore` (107 B) + `worktrees/versed-stallion/`                                                                                                                                                              |
| Registered worktree  | yes — `git worktree list`: `versed-stallion` @ `c86a197` detached; metadata `.git/worktrees/versed-stallion/` incl. `kilo-agent-manager-metadata.json` (`{"pooled":true,"baseRef":"main","baseOid":"c86a197…"}`) |
| Content              | 158 tracked files = exact checkout of `c86a197` ("docs: adopt backup and disaster recovery ADR")                                                                                                                 |
| Untracked files      | **0**                                                                                                                                                                                                            |
| Ignored files        | **0**                                                                                                                                                                                                            |
| Commits              | reflog has a single entry (`c86a197`); every reflog commit is an ancestor of `main`; no per-worktree refs; stash list empty                                                                                      |
| Size                 | 988 KB                                                                                                                                                                                                           |

Conclusion: the worktree contains **no unique work**; everything is reproducible from `main`. Removal requires explicit owner authorization → **M0-D2**.

### 2.5 FUNC-FIX-002 harness (REQ-013 evidence preservation)

Current file `%TEMP%/funcfix002/e2e.mjs` (35 checks, last run 35/35) is **not reproducible as-is**:

| Coupling        | Problem                                                                               | Plan                                                                                                      |
| --------------- | ------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Location        | outside the repository                                                                | move to `scripts/e2e-funcfix002.mjs`                                                                      |
| Absolute paths  | `C:/Users/sugen/...` for ROOT and Playwright import                                   | derive from `import.meta.url` like `e2e-step017-uiux.mjs`                                                 |
| DB provisioning | DB create, `migrate`, `create-user`, `grant-role` were run manually before the script | self-provision: drop/create its own `kooperatif_e2e_ff002` (only that name), migrate, bootstrap user      |
| psql access     | `docker exec kooperatif-dev-postgres-1` (container name)                              | `docker compose -f docker/compose.yaml exec -T postgres psql`                                             |
| Ignored fields  | 4 endpoints receive `idempotencyKey` they do not accept                               | remove (otherwise fails after REQ-006)                                                                    |
| Ports           | 8099/5199 shared with `e2e-step017-*`                                                 | keep defaults, allow `E2E_API_PORT`/`E2E_WEB_PORT` override; document "do not run harnesses concurrently" |
| Exit status     | already non-zero on any FAIL                                                          | keep; print `DONE n/n`                                                                                    |
| Invocation      | none                                                                                  | root script `pnpm e2e:ff002`                                                                              |

### 2.6 New finding — remote CI has been red for 21 consecutive runs (F-M0-02)

GitHub Actions (public API, read-only):

| Job                                   | First failing run                    | Status at `a0fa441` (run 26) | Root cause                                                                                                                                                                                                                                                                                                                                   |
| ------------------------------------- | ------------------------------------ | ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Backup/restore (recovery drill)       | run 6 (`ffd43af`, backup foundation) | failing                      | **unknown** — job logs require authentication (HTTP 403); annotation only "exit code 1". Local drill passes → CI-environment-specific (Linux Docker networking/permissions are candidates, unverified)                                                                                                                                       |
| Backend (fmt, clippy, test)           | run 17 (`18287be`, STEP-016)         | failing                      | **confirmed**: the job runs `cargo test` without `KOOPERATIF_TEST_DATABASE_URL`; DB-gated tests panic by design (STEP-016 realtime first, generalized by PILOT-FIX-001 F5). Reproduced locally: `cargo test --test periods` without the variable → panics `KOOPERATIF_TEST_DATABASE_URL is not set`. Exit code 101 matches the CI annotation |
| Frontend                              | runs 19-21                           | passing since run 22         | resolved                                                                                                                                                                                                                                                                                                                                     |
| Database (migrations + full DB suite) | —                                    | passing                      | full suite runs here with a database                                                                                                                                                                                                                                                                                                         |

Consequence: all earlier closure reports' "CI" statements describe local gates; the remote pipeline has not been green since run 5. This affects REQ-030 evidence (ADR-013 restore verification in CI) and the docs/26 quality gates. CI changes can be prepared locally but **can only be verified after a push**, which requires owner authorization → **M0-D3**.

### 2.7 Production-readiness documentation review (REQ-030 planning, no deployment)

Exists: ADR-013, docs/23 (policy), docs/30 runbook (create/verify/retention/restore/failure modes/RPO-RTO notes/periodic duties), `scripts/backup/*` (verified locally), `apps/server/.env.example` with production fail-fast semantics.
Absent (no file in the repository mentions them): systemd units, reverse proxy/TLS configuration, WAL archiving/PITR procedure, off-site encrypted copy procedure, deployment/rollback runbook, monitoring/alerting definition, operational-security checklist. M0 produces a provisioning **plan document** only; nothing is executed against any server.

---

## 3. M0 detailed plan (Phase B)

### 3.1 Business objectives

1. The API never silently accepts unsupported data (a "USD" account can no longer be stored as TRY).
2. Every destructive/financial action asks for confirmation the same accessible way, explaining the consequence.
3. The README tells operators the truth about what exists, what is missing and how to verify.
4. The legacy `.kilo` worktree is removed safely (only if authorized).
5. FUNC-FIX-002 acceptance evidence becomes a reproducible regression test.
6. CI reflects reality and becomes green (as far as verifiable without push).
7. A production provisioning plan exists before any VDS work is requested.

### 3.2 Work packages

| WP    | REQ       | Change                                                                                                                                                                                                                                                                                                 | Files (planned)                                                                                       |
| ----- | --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------- |
| WP-1  | 006       | `#[serde(deny_unknown_fields)]` on every JSON request DTO and nested request type; no change to query-string structs (read filters, not writes — documented)                                                                                                                                           | `apps/server/src/*/routes.rs`, `auth/rbac.rs`, `auth/routes.rs`                                       |
| WP-2  | 006       | Negative integration tests: one per router family (unknown top-level field → 422 `validation_failed`, row counts unchanged), nested unknown field (person ref, allocation line), AUD §F probe (`currency:'USD'` on account create → 422, zero accounts), login with unknown field → 422 and no session | `apps/server/tests/*.rs` (existing files)                                                             |
| WP-3  | 006 / 013 | Remove ignored `idempotencyKey` from maintained harnesses (`e2e-step017-rc`, `e2e-step017-uiux`)                                                                                                                                                                                                       | `scripts/*.mjs`                                                                                       |
| WP-4  | 013       | Commit the FF2 harness as `scripts/e2e-funcfix002.mjs` with the §2.5 changes; root script `e2e:ff002`                                                                                                                                                                                                  | `scripts/`, `package.json`                                                                            |
| WP-5  | 027       | Shared `ConfirmActionDialog` component (AlertDialog, title, consequence text, optional reason input, busy state, Turkish copy via i18n); replace native confirms per M0-D1 scope                                                                                                                       | `apps/web/src/lib/components/confirm-action-dialog.svelte`, affected routes, `i18n/tr-TR.ts`, `en.ts` |
| WP-6  | 027       | Vitest: per converted action — opens dialog, Cancel → no request, Confirm → exactly one request; replace `window.confirm` spies                                                                                                                                                                        | `apps/web/src/specs/*`                                                                                |
| WP-7  | 027       | Real-browser harness `scripts/e2e-m0-dialogs.mjs`: for each converted route, Cancel leaves the DB unchanged; Confirm produces the expected persisted state (e.g. donation `reversed` + movement reversed, period `closed`)                                                                             | `scripts/`                                                                                            |
| WP-8  | 028       | README corrections per §2.3; add "Implemented modules", "Known limitations → REQUIREMENTS-REGISTRY", "Regression harnesses (maintained / historical)", "CI status"                                                                                                                                     | `README.md`                                                                                           |
| WP-9  | 030 (CI)  | Backend CI job: run only non-DB tests (`cargo test --lib --test http`) — the Database job already runs the full suite with a database                                                                                                                                                                  | `.github/workflows/ci.yml`                                                                            |
| WP-10 | 030 (CI)  | Backup CI job: diagnose with the real log (M0-D3); add diagnostic output (`docker version`, connectivity probe, `--keep` artifact upload on failure) so the next run is self-explaining                                                                                                                | `.github/workflows/ci.yml`, possibly `scripts/backup/lib.mjs`                                         |
| WP-11 | 029       | Only if authorized: `git worktree remove .kilo/worktrees/versed-stallion`, `git worktree prune`, remove `.kilo/`, remove the now-unneeded `.kilo/worktrees/` line from local `.git/info/exclude`; verify `git worktree list`, `git fsck`                                                               | —                                                                                                     |
| WP-12 | 030       | `implementation/M14-PRODUCTION-PROVISIONING-PLAN.md`: PostgreSQL config, WAL/PITR, encrypted off-site copies, restore drill on the target, systemd units, reverse proxy + HTTPS, deploy/rollback, monitoring, operational security, PD-26/PD-27 inputs. Planning only                                  | `implementation/`                                                                                     |
| WP-13 | all       | Update registry + roadmap statuses/evidence at closure                                                                                                                                                                                                                                                 | `implementation/`                                                                                     |

### 3.3 Permissions, audit, correction behavior

- No new permissions; no new audit event types. WP-1 rejects before handlers run, so no audit event and no partial write can occur.
- WP-5 changes only the client confirmation step; server-side authorization, CSRF, idempotency and reversal semantics are untouched.
- No database migration in M0. Rollback = revert the M0 commits; nothing to undo in data.

### 3.4 Financial-integrity analysis

- WP-1 is the only backend behavior change and is strictly _more_ restrictive: requests that previously succeeded with ignored extra fields will now fail. Risk = a legitimate client sending an extra field. Mitigations: UI call sites audited (none send unknown fields to the affected endpoints); all maintained E2E harnesses run against the strict server; during every E2E run the API log is scanned for `sanitized framework rejection body` lines — the expected count from UI flows is **0**.
- No change to money arithmetic, allocations, credits, transfers, reservations, reversals, idempotency keys or reports.

### 3.5 Acceptance criteria

| #     | Criterion                                                                                                              | Evidence                                                                                         |
| ----- | ---------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| AC-1  | `POST /api/financial-accounts` with `currency:"USD"` → 422 `validation_failed`; account count unchanged                | WP-2 test + manual probe against real stack                                                      |
| AC-2  | Every mutating router rejects an unknown top-level field and an unknown nested field without writing                   | WP-2 tests                                                                                       |
| AC-3  | All existing behavior intact                                                                                           | full Rust suite (clean DB, sequential), Vitest, svelte-check, lint, build, fmt, clippy all green |
| AC-4  | No UI flow sends an unknown field                                                                                      | API log scan during rc + uiux + ff002 + dialogs runs = 0 rejections                              |
| AC-5  | FF2 harness reproducible from a fresh clone via `pnpm e2e:ff002` with no manual setup; 35/35                           | run twice consecutively                                                                          |
| AC-6  | Zero `confirm(` / `window.confirm(` in the converted scope (M0-D1)                                                     | grep = 0                                                                                         |
| AC-7  | Each converted action: Cancel → no request and no DB change; Confirm → one request and the expected DB state           | WP-6 Vitest + WP-7 real-browser/DB harness                                                       |
| AC-8  | Dialogs keyboard-accessible (focus trap, Esc cancels), Turkish consequence text                                        | WP-7 harness keyboard checks                                                                     |
| AC-9  | README contains no statement contradicted by code/tests (§2.3 list closed)                                             | review checklist in closure report                                                               |
| AC-10 | (if M0-D2 authorized) `git worktree list` shows only main; `.kilo/` absent; `git fsck` clean; `main` history unchanged | command output                                                                                   |
| AC-11 | Backend CI job definition no longer runs DB-gated tests without a database                                             | workflow diff + local run of the same command                                                    |
| AC-12 | Provisioning plan document covers all REQ-030 sub-items 030.1–030.10                                                   | document checklist                                                                               |

AC-11 can be proven green on GitHub only after a push (M0-D3). Until then the closure report states "prepared, not remotely verified".

### 3.6 Regression protection (run at M0 closure)

1. `cargo fmt --check`, `cargo clippy --all-targets -D warnings`.
2. `scripts/db-verify.sh` — clean DB, migrations ×2, full Rust suite **sequentially** (avoid the reproduced concurrent-process pool contention).
3. `pnpm check`, `pnpm lint`, `pnpm test`, `pnpm build`.
4. `node scripts/e2e-step017-rc.mjs` (journeys A–K, cross-browser), `node scripts/e2e-step017-uiux.mjs` (6 viewports × 18 routes), `pnpm e2e:ff002` (35 checks), `node scripts/e2e-m0-dialogs.mjs`.
5. `node scripts/backup/drill.mjs` (restore proof unaffected).
6. API log scan (AC-4).

Harnesses run one at a time (shared ports 8099/5199).

### 3.7 Explicit exclusions

No tenant work (M1), no idempotency added to creates (F-M0-03, later milestone), no query-string strictness, no historical-harness repair beyond what M0-D4 decides, no push, no deployment, no VDS access, no deletion without M0-D2.

---

## 4. Owner decisions required before M0 implementation

| ID    | Question                                                                                                                                          | Recommended                                                                                                                                                        | Alternatives                                                                         | Consequence                                                                                                                 |
| ----- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------- |
| M0-D1 | REQ-027 covers only Social Aid (4 confirms) or **all 20 native confirms in 6 routes**?                                                            | **All 20** — docs/12:16 requires one pattern; same component, low risk                                                                                             | Social Aid only; the other 16 tracked as new REQ-027 sub-items for a later milestone | "All" adds ~5 route edits + tests to M0                                                                                     |
| M0-D2 | Authorize removal of `.kilo/` (worktree with no unique content, §2.4)?                                                                            | **Yes**, via `git worktree remove` + prune                                                                                                                         | Keep it; decide later                                                                | Without authorization REQ-029 stays open; nothing else is blocked                                                           |
| M0-D3 | CI: (a) include the CI workflow fixes in M0, and (b) provide the failing "Recovery drill" job log from GitHub (or allow read access with a token) | (a) yes; (b) paste the log of run 26, job "Backup/restore"                                                                                                         | Defer CI to M14                                                                      | Without the log WP-10 can only add diagnostics; remote green status can be verified only after a separately authorized push |
| M0-D4 | Historical E2E scripts (`e2e-step004…016`, `e2e-hotfix001`): maintain or mark historical?                                                         | **Mark historical** in README; maintained regression set = `e2e-step017-rc`, `e2e-step017-uiux`, `e2e-funcfix002`, `e2e-m0-dialogs` (+ future milestone harnesses) | Repair all 13 for native-dialog removal and current UI                               | Repairing all is large and duplicates rc coverage                                                                           |

Already decided — not asked again: PD-01 and PD-02 (**APPROVED**, recorded in the registry and roadmap for M1). Technical alignment (not a business decision): unknown field → 422 `validation_failed`.

## 5. Proposed next action

On approval of this plan (with answers to M0-D1…D4): implement WP-1…WP-13 within M0 scope only, run §3.6, produce the M0 closure report, update the registry, then STOP before M1. M1 starts with its own Discovery + detailed design for PD-01/PD-02 and a separate approval gate.
