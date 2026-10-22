# 31 — Test Execution & Performance Policy

Status: **active policy** (established alongside the M1-P6 governance
task). Binding for agents and developers; supersedes informal habits of
re-running the full suite after every edit. `AGENTS.md` carries the
short binding summary; this document holds the rationale, timings and
operational detail.

## 1. Test execution tiers

| Tier | When | Scope |
|---|---|---|
| **T0 — development** | every change | `cargo fmt` + `cargo clippy --all-targets -- -D warnings` + focused tests for the modified component (`cargo test --test <binary>`, `--lib` for unit tests) + directly related regression suites. Frontend changes: `tsc --noEmit` (contracts), vitest, `svelte-check`. |
| **T1 — batch completion** | end of an approved batch | All binaries touched by the change + related integration/RLS/authorization/audit/financial regression suites. Schema changes additionally require clean migration replay into a NEW disposable database and restricted-role (`kooperatif_app`) checks. Authorization-contract changes additionally require the regrants fingerprint check (`scripts/backup/regrants.mjs`). |
| **T2 — milestone completion** | end of a phase/milestone | `cargo test --all-targets` (entire backend suite), frontend tests + static checks, clean migration replay, backup/restore drill when financial-data readiness is claimed (ADR-013). |

### When the full suite is mandatory at T1

- Changes to shared infrastructure: `db.rs` harness functions, `http/` router assembly, `auth/` extractors, `tenant/` extractors, realtime fan-out.
- Migrations touching shared or cross-cutting tables (`roles`, `permissions`, `cooperative_memberships`, `user_sessions`, `cooperatives`, `security_events`).
- Anything whose blast radius cannot be bounded to named binaries — when in doubt, run it once, deliberately, and say so in the report.

### Reporting discipline

- Every gate is reported as exactly one of **PASS / FAIL / NOT RUN / BLOCKED** with evidence (counts, times, fingerprints).
- A focused-suite pass is never presented as a full-suite pass; state the scope (`--test <name>`) explicitly.
- Flaky failures are investigated and reported, never silently retried into a PASS without a stated cause.

## 2. Database safety and cleanup restrictions

1. **Agents never run manual `DROP DATABASE`, bulk cleanup, or destructive recovery commands.** This is absolute — the historical incident (≈300 disposable databases deleted during crash-recovery cleanup under a prior no-deletion instruction) is the reason this rule exists.
2. Automated deletion is permitted **only** for disposable databases (`kooperatif_test_<32 lowercase hex>`) positively proven to belong to the **current** test run — i.e. the test's own `cleanup()`/`drop_probe` teardown of a database it just created and claimed.
3. The harness GC (`drop_stale_test_databases`) **never deletes**. It only classifies and marks stale candidates (`kooperatif-test-owned` lease → `kooperatif-gc-pending` demotion), all serialized by advisory locks. A marker proves only that a *previous* run created the database — outside the current-run deletion boundary — so marker-tagged stale databases are never deleted by any automatic sweep. There is **no environment variable or flag** an ordinary test run can set to change this; the test-only `*_authorized_*` GC helpers exist to keep the drop machinery under test, and they are additionally confined to names the current process itself registered as current-run-owned — they cannot target any pre-existing or unrelated database.
4. `kooperatif_tpl_<hash>` template caches, `kooperatif`, `kooperatif_dev`, `kooperatif_e2e_*`, backup/drill/regrant/baseline databases, stale test databases (including marker-tagged ones), and any database whose ownership cannot be proven are **protected**. When ownership cannot be proven: fail closed, preserve, and report the remaining resource instead of deleting it.
5. No resets, rewrites or deletions of migration history, `regrants.sql`, backup files, or protected-database contents.

**Historical cleanup is an operator procedure, not a harness feature.**
When stale `kooperatif_test_*` accumulation needs reclaiming:

1. Inventory: `SELECT datname, pg_size_pretty(pg_database_size(datname)), shobj_description(oid,'pg_database') FROM pg_database WHERE datname ~ '^kooperatif_test_[0-9a-f]{32}$' ORDER BY datname;`
2. Review the exact list — confirm none are in-flight test databases (zero connections, `gc-pending` markers, no active suite running).
3. Obtain explicit authorization for the reviewed list.
4. Drop only the reviewed names, one statement per database, after a final connection check.

**Operational consequence:** automatic deletion is disabled by design,
so stale databases accumulate until this procedure runs. Growth is
visible via `gc-pending` markers and `pg_database` listings; report
growth rather than deleting.

## 3. Harness architecture (what exists)

- `db.rs::create_test_database` — `CREATE DATABASE <disposable>` (template-clone fast path, §4) + ownership claim with heartbeat lease (120 s TTL, renewed every 15 s).
- `db.rs::run_test_migrations` — identity probe **first** (fail-closed disposable-name check), then the embedded sqlx migrator. On template clones this validates `_sqlx_migrations` checksums and returns fast — clone integrity is verified on every test.
- `db.rs::drop_stale_test_databases` — leader-elected best-effort sweep; throttled per process (≥20 s between elected sweeps) so per-test `setup()` calls do not multiply admin-pool scans. Marks/demotes only; never deletes without the authorized opt-in (§2.3).
- `db.rs::{drop_stale_test_databases_forced, drop_stale_test_databases_gated}` — test-only variants exempt from leader election/throttle; used by the GC behavior tests in `tests/db.rs`.
- `db.rs::{drop_stale_test_databases_authorized_forced, drop_stale_test_databases_authorized_gated}` — test-only variants that exercise the drop machinery explicitly, keeping it and its connection-race protections under test. They are confined to databases the current process registered as current-run-owned (`register_current_run_database`, called by `create_test_database`/`claim_*` or by probe-creating tests) — they cannot reach pre-existing or unrelated databases.
- `db.rs::register_current_run_database` — doc-hidden provenance registry; the deletion boundary for the authorized test path.
- `scripts/backup/drill.mjs` — backup/restore verification (ADR-013); creates and drops its own `kooperatif_backup_drill_*` run databases.
- `scripts/backup/regrants.mjs` — grants fingerprint verification against `regrants.sql`.

## 4. Template-clone provisioning (optimization)

**Before:** every test ran `CREATE DATABASE` + replayed the full
migration set (67+ migrations) — ≈2.3 s of provisioning per test,
≈740 tests per suite run.

**After:** `create_test_database` clones a migrated template cache via
`CREATE DATABASE <name> TEMPLATE kooperatif_tpl_<hash>`. The template
name is keyed by a hash of every embedded migration's `(version,
checksum)` — a changed migration set automatically yields a new
template, so staleness is impossible by construction. Template creation
is serialized across processes by `pg_advisory_lock`; any failure falls
back to empty `CREATE DATABASE` + full migration replay. Opt-out:
`KOOPERATIF_TEST_TEMPLATE=0`.

Isolation is unchanged: every test still owns a private disposable
database. Template names never match the disposable regex, so GC and
teardown never touch them; they accumulate at most one per
migration-set revision and are preserved like any other non-provable
database (§2.4).

### Measured results (local dev machine, PostgreSQL 17 docker)

| Suite | Before | After | Speedup |
|---|---|---|---|
| `m1p6_b1_cooperative_authorization` (9 tests) | 23.4 s | 1.66 s | ~14× |
| `rbac` (11 tests) | ~35 s | 1.51 s | ~23× |

Full-suite wall time is bounded by binary serialization (cargo runs
test binaries sequentially), so the suite-level win is dominated by the
~2 s/test provisioning removal — expected several-fold faster end to
end. Re-measure on the next full run and record actuals here.

## 5. Parallelism budget (do not raise blindly)

`.cargo/config.toml` pins:

- `RUST_TEST_THREADS = 6` — derived from `max_connections = 100` on the
  dev server: each test chain holds admin pool (5) + claim connection +
  test pool (5) + optional app pool (5) ≈ 16 worst-case connections.
  6 × 16 ≈ 96 stays under the server limit; the num_cpus default (28)
  oversubscribed and produced `PoolTimedOut` flakes.
- `jobs = 4` — rustc codegen/link parallelism bound (pagefile
  oversubscription corrupted compiler metadata on this machine).

Raising either requires redoing the connection math against the live
`max_connections` and re-verifying suite stability.

## 6. CI evaluation and recommendations

Current pipeline (`.github/workflows/ci.yml`) already separates:

- `frontend`: prettier, eslint, typecheck, vitest, build.
- `backend`: fmt, clippy, dependency-free tests (`--lib`, `--test http`, `--doc`).
- `database`: migration apply + idempotent re-apply + ledger check + full DB-gated suite on a PostgreSQL service.
- `backup`: backup unit tests, real recovery drill, failure-injection tests.

Recommendations (NOT applied — CI changes require explicit approval
since they can affect protection guarantees):

1. Keep the current four-job split; it already matches the T0/T1/T2
   intent — PR-time cost is dominated by the `database` job's full
   DB-gated suite, which is the security-sensitive surface of this
   repository and should stay mandatory for merge.
2. Optional future split if PR latency becomes a problem: move the
   full DB suite to a merge-queue/nightly lane while keeping a named
   security regression subset (`db`, `rbac*`, `tenant_*`,
   `m1p4_rls_pilot`, `audit_read`, `realtime`, `c5_scoped_uniques`) on
   PRs. Any such change must keep branch protection's required checks
   non-weakened and needs user approval.
3. The template-clone fast path benefits CI too (the `database` job
   exercises it automatically — `KOOPERATIF_TEST_TEMPLATE` unset →
   enabled). No CI change needed to gain it.

## 7. Remaining risks and open items

- `kooperatif_tpl_*` caches are preserved artifacts; each migration-set
  revision leaves one small database. Removal, if ever wanted, requires
  the same explicit authorization as any database deletion.
- Template clone assumes migrations produce a deterministic,
  clone-safe catalog (true today: seeds are fixed rows; no per-database
  secrets are minted by migrations). If a future migration seeds
  randomness that must differ per database, provisioning must revisit
  this — add such seeds via runtime instead.
- The elected-GC throttle (20 s/process) is conservative; GC behavior
  tests use the forced path and are unaffected.
- Full-suite wall time was previously ~25–30 min and is the main PR
  latency cost; nextest-style per-test process isolation or job
  splitting are future options, not implemented here.
