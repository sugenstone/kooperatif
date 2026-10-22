# AGENTS.md --- Kooperatif

Repository: `git@github.com:sugenstone/kooperatif.git`

## Mandatory reading order

Before any implementation: 1. Read `docs/00-PROJECT-CHARTER.md`. 2. Read
`docs/01-DOMAIN-GLOSSARY.md`. 3. Read `docs/15-INVARIANTS.md`. 4. Read
`docs/27-ARCHITECTURE.md`. 5. Read `docs/28-ADR-INDEX.md` and any
Accepted ADR relevant to the step. 6. Read the domain specifications
relevant to the step. 7. For user-facing work, read
`docs/12-UI-UX-SYSTEM.md` and `docs/13-SCREEN-SPECIFICATIONS.md`. 8. For
reports/documents, read `docs/14-REPORTING-AND-DOCUMENT-DESIGN.md`. 9.
For lifecycle work, read `docs/16-STATE-MACHINES.md`. 10. For
calculations, read `docs/17-CALCULATION-RULES.md`. 11. For
permissions/approvals, read `docs/18-AUTHORIZATION-APPROVALS.md`. 12.
For correction/audit, read `docs/19-AUDIT-REVERSAL-CORRECTION.md`. 13.
For schema work, read `docs/20-DATA-MODEL-DATABASE.md`. 14. For API
work, read `docs/21-API-CONTRACTS.md`. 15. For security-sensitive work,
read `docs/22-SECURITY.md`. 16. For file/notification/operations work,
read the corresponding specification. 17. Read
`docs/26-TESTING-QUALITY-GATES.md`. 18. Read only the approved
implementation STEP and its explicit dependencies. 19. Do not proceed to
another STEP without explicit instruction.

### Authority order when documents appear to conflict

1.  Explicit approved corrective instruction for the current STEP.
2.  Accepted ADR for architecture-specific decisions.
3.  System Invariants.
4.  Domain-specific specification.
5.  Project Charter.
6.  UI/UX and screen specifications.
7.  Roadmap.

If a conflict remains meaningful: **STOP and report it.**

## Non-negotiable rules

-   Never invent unspecified financial behavior.
-   Never silently choose a profit-sharing, exit, valuation, indexation,
    allocation, or excess-payment rule.
-   Never destructively delete a posted financial transaction.
-   Never silently rewrite historical ownership, debt, rights, policy
    versions, or financial movements.
-   Never treat every cash inflow as income or every cash outflow as
    expense.
-   Never merge Cooperative Finance and Social Aid Finance.
-   Never model multiple shares only as `share_count`.
-   Never assume payer == shareholder == debtor == guardian.
-   Never assign an excess family payment to an arbitrary family member.
-   Never rely on frontend authorization or tenant filtering for
    security.
-   Never hide a specification conflict by choosing an interpretation.

If a required rule is undefined or documents conflict: **STOP and report
the exact ambiguity.**

## Implementation discipline

For each approved step: 1. Verify repository baseline. 2. State exact
scope and exclusions. 3. Identify relevant invariants. 4. Implement only
that scope. 5. Add tests proving behavior and failure cases. 6. Run all
required gates. 7. Inspect final diff for scope creep. 8. Produce a
closure report with evidence. 9. Stop.

## Technology direction

-   SvelteKit + TypeScript
-   shadcn-svelte from project start
-   Rust + Axum
-   PostgreSQL
-   modular monolith
-   monorepo
-   migration-driven schema
-   Docker-supported development
-   backend-authoritative authorization
-   CI + automated tests + E2E

Generic starter infrastructure may be reused. Domain-specific code from
another application must not leak into this repository.

## Language and localization

-   Default product language and locale: **Turkish (`tr-TR`)**.
-   All user-facing UI copy, validation/errors, notifications, reports,
    receipts, PDFs, printouts and document templates must default to
    Turkish.
-   Technical identifiers in Rust/TypeScript/database/API code should
    remain English unless there is a strong reason otherwise.
-   The frontend must be i18n-ready from the beginning; do not scatter
    hard-coded user-facing strings across components.
-   Future languages such as English may be added without redesigning
    domain logic.
-   Dates, numbers and monetary/value formatting must use locale-aware
    formatting. Default presentation uses `tr-TR`.
-   Localization must never alter authoritative stored financial values;
    it affects presentation only.

## UI/UX discipline

-   Use shadcn-svelte as the default component foundation.
-   Do not invent a new interaction pattern when the UI/UX specification
    already defines one.
-   Do not optimize visual minimalism at the cost of financial meaning
    or identity safety.
-   Collection Session is a specialized high-throughput operational UI
    and must follow its dedicated rules.
-   Do not use color as the only status signal.
-   Reports/PDF/XLSX are governed by the reporting/document
    specification, not ad-hoc page code.

## Financial integrity

High-level user actions may atomically produce several authoritative
effects.

Example: one shareholder/family payment can produce: - one
receipt/payment, - one financial-account inflow, - many debt
allocations, - a remaining excess-payment effect.

Do not make users manually create these effects separately.

## UI principle

**Simple UI, strict backend.**

Financial action screens should preview consequences before confirmation
where practical.

## Completion

Compiling is not closure. A completed step must pass the project's
formatting, linting, typechecking, testing, migration, build, E2E and
repository-cleanliness gates applicable to that step.


## Backup / Disaster Recovery Gate

ADR-013 is binding.

Before declaring a milestone involving material financial production data production-ready, verify the Backup Readiness Gate in `docs/26-TESTING-QUALITY-GATES.md`.

Do not treat a successful backup command/job as proof of recoverability. Restore verification evidence is required.

Do not use disaster-recovery rollback to correct ordinary business mistakes; use the specified reversal/correction model.

## Test execution policy (permanent)

Detailed rationale and timings: `docs/31-TEST-EXECUTION-POLICY.md`.

### During development
- Run focused tests for the modified component only: `cargo test --test <binary>` (or `--lib` for unit tests) plus directly related regression suites.
- Always run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and frontend `tsc`/vitest when the corresponding surface changed.
- Do NOT re-run the full backend suite after small, well-scoped edits.

### At batch completion
- Run all directly affected test binaries plus related integration, RLS, authorization, audit and financial regression suites.
- Schema changes: run a clean migration replay in a NEW disposable database (never reset an existing one) and verify restricted-role (`kooperatif_app`) behavior.
- Authorization contract changes: verify grants/regrants fingerprint.
- Run the full suite only when the change's blast radius makes focused testing insufficient to establish confidence; say why.

### At milestone completion
- Full backend suite (`cargo test --all-targets`), frontend tests + static checks, clean migration replay, and backup/restore verification when applicable.

### Gate reporting
- Report every gate as exactly one of PASS / FAIL / NOT RUN / BLOCKED, with evidence.
- A passing focused suite is NEVER equivalent to a passing full suite — state scope explicitly.

## Test infrastructure rules (permanent)

- Disposable test databases are created as clones of the migrated template cache (`kooperatif_tpl_<hash>`; `KOOPERATIF_TEST_TEMPLATE=0` opts out). Per-test isolation is unchanged.
- Test parallelism is bounded by `.cargo/config.toml` (`RUST_TEST_THREADS = 6`, `jobs = 4`) — derived from `max_connections=100` (~16 conns/test chain). Do not raise without redoing this math against the live server limit.
- `cargo fmt` is the formatting gate; do not hand-format.
- `KOOPERATIF_TEST_DATABASE_URL` (local dev: `postgres://kooperatif:kooperatif_dev_password@localhost:5494/<db>`): template-cloning suites accept any base DB, but direct-connect suites (`tests/auth.rs`, `tests/db.rs`) enforce the identity guard and require `kooperatif_test` or a fresh `kooperatif_test_<32hex>` you created yourself — never point them at `postgres`.

## Database safety (permanent)

- Agents NEVER run manual `DROP DATABASE`, bulk cleanup, or destructive recovery commands. Automated deletion is permitted ONLY for disposable databases positively proven to belong to the current test run (the test's own `cleanup()`/`drop_probe` teardown of its claimed database).
- There is NO environment variable, test helper, or GC path through which an ordinary agent execution or automated test run can delete historical databases. The harness GC only demotes/marks stale `kooperatif_test_<32hex>` candidates and retains them; the test-only authorized helpers are confined to databases the current process itself registered (created or claimed) this run.
- Marker-tagged or stale databases from previous runs are NEVER deleted programmatically. Historical cleanup is a separate operator procedure (see docs/31 §2): review the exact target inventory first, obtain explicit authorization, then execute — never via the test harness.
- Protected: `kooperatif`, `kooperatif_dev`, `kooperatif_e2e_*`, backup/drill/regrant/baseline databases, `kooperatif_tpl_*` caches, historical and stale test databases (including marker-tagged ones), backup files, migration history, and every database whose ownership cannot be proven. When in doubt: preserve and report.
