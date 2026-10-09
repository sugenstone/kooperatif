# TEST-INVENTORY — Active vs Historical Test Scripts

Status: authoritative for M0 (PROJECT-CONTROL-002, owner decision M0-D4).
Scope: scripts under `scripts/` plus the Rust/Vitest suites they
complement. Last updated during M0 implementation.

Owner decision M0-D4: older milestone scripts are **preserved as
historical evidence**, never deleted or silently disabled. A script is
historical when its coverage is superseded by a maintained equivalent,
not merely because it currently fails.

## Active regression suite

| Script / suite | Command | Prerequisites | Purpose |
|---|---|---|---|
| Rust unit + lib tests | `cargo test --manifest-path apps/server/Cargo.toml --lib` | none | in-crate logic |
| Rust HTTP integration | `cargo test --manifest-path apps/server/Cargo.toml --test http` | none | HTTP contract without DB |
| DB-gated integration | `pnpm db:verify` or `KOOPERATIF_TEST_DATABASE_URL=… cargo test --test <suite>` | Docker postgres | all `tests/*.rs` DB suites (fail fast without the env var — PILOT-FIX-001/F5) |
| Strict body validation | `cargo test --test strict_body_validation` (DB-gated) | Docker postgres | REQ-006 unknown-field rejection, no-mutation proof |
| Frontend checks | `pnpm check`, `pnpm lint`, `pnpm test`, `pnpm build` | pnpm install | svelte-check, eslint, Vitest, production build |
| FUNC-FIX-002 regression | `pnpm e2e:ff002` | Docker postgres | shareholder default collection account, 8 scenarios, UI + DB assertions |
| Confirmation dialogs | `pnpm e2e:dialogs` | Docker postgres | REQ-027: cancel=no mutation, confirm=mutation, one op per page ×6, DB assertions |
| Release candidate | `node scripts/e2e-step017-rc.mjs` | Docker postgres | journeys A–K + cross-browser smoke |
| UI/UX hardening | `node scripts/e2e-step017-uiux.mjs` | Docker postgres | STEP-017 UIUX acceptance |
| Backup unit tests | `pnpm backup:test` | none | backup logic (manifest, retention, paths) |
| Recovery drill | `pnpm backup:drill` | Docker | ADR-013 restore-verification evidence |
| Failure injection | `pnpm backup:failure-test` | Docker postgres | corruption/tamper/refusal paths |
| Full gate | `pnpm verify` | Docker | `scripts/verify.sh` end-to-end sequence |

## Historical milestone evidence (do not delete — M0-D4)

`scripts/e2e-step004.mjs` … `scripts/e2e-step016.mjs` and
`scripts/e2e-hotfix001.mjs` were each milestone's acceptance harness.
They are kept for provenance but are **not part of the maintained
regression suite**: later milestones changed APIs/pages in ways these
scripts were never updated for. Their durable coverage is carried
forward by the DB-gated Rust suites, Vitest specs, and the active
scripts above.

If a historical script ever holds unique business coverage with no
maintained equivalent, that coverage must be migrated into a maintained
test before the script may be removed from active consideration.

## Manual diagnostics

- `scripts/db-verify.sh` — clean-database migration verification
  (also the canonical DB-gated test runner).
- Temp-folder probes under `%TEMP%/funcfix002/`, `%TEMP%/funcaudit001/`
  are scratch evidence from earlier audits, not repository assets.

## CI mapping

`.github/workflows/ci.yml`:

| Job | Runs |
|---|---|
| frontend | format, lint, `pnpm check`, `pnpm test`, `pnpm build` |
| backend | `cargo fmt --check`, clippy, `--lib` + `--test http` + doc tests (no DB by design) |
| database | PostgreSQL service, migrations ×2, migration ledger, full `cargo test` with `KOOPERATIF_TEST_DATABASE_URL` |
| backup | `backup:test`, docker diagnostics, `backup:drill`, postgres logs on failure, `backup:failure-test` |

Browser-E2E scripts (ff002, dialogs, step017-*) are local/regression
gates; they are not CI jobs yet.
