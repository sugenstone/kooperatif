# M1 — IMPLEMENTATION ROADMAP

Covers M1-D12. Phase order: **secure DB/backend foundation first,
cooperative UI last** (per owner requirement). Each phase is a reviewable
STEP ending with the standard gates (fmt/clippy/lint/typecheck, full
Rust suite sequential, Vitest, build, clean-DB migrate + re-run, E2E,
registry update, STOP). Relative complexity on S/M/L/XL.

| Phase | Goal | Migrations | Depends on | Complexity | Risk |
| --- | --- | --- | --- | --- | --- |
| **M1-P0** | Tenant kernel: `cooperatives` + `cooperative_memberships` + ctx extractor | `0018` part 1 | owner gate | M | HIGH |
| **M1-P1** | RBAC retrofit: coop-scoped roles/assignments | `0018` part 2 | P0 | M | HIGH |
| **M1-P2** | Business tables: `cooperative_id` + composite FKs + app scoping | `0018` part 3 | P1 | XL | HIGH |
| **M1-P3** | Per-coop numbering + idempotency scope | `0019` | P2 | M | MED |
| **M1-P4** | RLS layer (Option B) + `kooperatif_app` role | `0020` | P2–P3 | L | HIGH |
| **M1-P5** | Realtime coop-scoping + audit coop | `0016`-adjacent trigger fn update | P2 | M | MED |
| **M1-P6** | Coop administration API (memberships, coop roles, coop switch endpoint) | — | P1 | M | MED |
| **M1-P7** | Frontend: selector, header propagation, per-tab ctx, WS rebind, cache isolation | — | P5, P6 | L | MED |
| **M1-P8** | Isolation test suite + E2E multi-tab + CI wiring | — | all | L | — |
| **M1-P9** | Per-coop export (backup drill extension) | — | P2 | M | MED |

---

## M1-P0 — Tenant kernel

- **Scope**: `cooperatives`, `cooperative_memberships`, seed default
  coop; `TenantCtx` extractor (`X-Cooperative-Id` header → membership
  check → ctx struct); `RequireAuth + TenantCtx` composite extractor;
  `POST /api/auth/cooperative` + `active_cooperative_id` on
  `user_sessions`; `auth/me` coop block.
- **Files**: `apps/server/src/tenant/{mod.rs,extractor.rs,repo.rs}` (new);
  `auth/extractor.rs`, `auth/routes.rs`, `auth/session.rs`,
  `migrations/0018`.
- **Acceptance**: T1.1–T1.8 green; `auth/me` returns
  `cooperatives[]` + `active_cooperative_id`; missing/foreign ctx →
  400/403; audit `cooperative_switched` rows.
- **Tests**: matrix T1 (DB+HTTP).
- **Rollback**: feature-flagged extraction — endpoints still work with
  single-coop fallback if ctx absent and exactly one coop exists
  (transition mode only).
- **Risk**: extractor change touches every handler signature → mitigate
  by keeping a `LegacyCtx` shim that injects the sole coop when only one
  exists (removed in P7).

## M1-P1 — RBAC retrofit

- **Scope**: `roles.cooperative_id`, `user_role_assignments` PK →
  `(coop,user,role)` + dual composite FKs (roles + memberships);
  `effective_permissions(user,coop)`; coop-scoped `roles` routes;
  `grant-role --cooperative`; per-coop advisory-lock key for admin paths.
- **Files**: `auth/{authz.rs,rbac.rs,routes.rs}`, `main.rs` (CLI),
  `migrations/0018` remainder.
- **Acceptance**: T4.1–T4.7; seeded "Sistem Yöneticisi" lands in default
  coop; `me` per-coop roles distinct.
- **Rollback**: N/A post-commit (schema) — covered by backup drill.

## M1-P2 — Business-table tenant retrofit (**the big one**)

- **Scope**: `cooperative_id` on all 36 coop tables + `UNIQUE(coop,id)`
  + composite FKs on the §3.1 hazard edges; every repo query gains the
  coop predicate; row-level 404 semantics; cross-ref validation.
- **Strategy**: split by module to keep reviews small —
  P2a parties (persons/families/shareholders), P2b shares,
  P2c periods/assessments, P2d payments/allocations/accounts/transfers,
  P2e credits/income/expense, P2f share-returns, P2g investments,
  P2h social-aid, P2i governance, P2j reports.
- **Files**: `migrations/0018` business section; every `src/<mod>/repo.rs`
  + `routes.rs` + `model.rs` where coop appears in DTOs; test suite per
  module.
- **Acceptance**: matrix T2, T3, T5; the migration-time `DO $$`
  validation block passes on a fixture with attempted cross-links;
  existing full suites still green (single-coop regression).
- **Risk**: highest — mitigate via the shared table-list driving both
  migration and T2.4 sweep test so no table is skipped.

## M1-P3 — Per-coop numbering

- **Scope**: `cooperative_counters` + allocator fn; 20 IDENTITY→BIGINT
  conversions + `families.sequence_number`; `UNIQUE(coop,n)` re-scopes;
  idempotency keys → `(coop,key)`.
- **Acceptance**: T3.8, T5.3; existing number formats unchanged per coop.
- **Risk**: MED — allocator must live inside the existing financial tx's.

## M1-P4 — RLS layer

- **Scope**: `kooperatif_app` non-superuser role (docker-compose, CI,
  `.env.example`); `ENABLE/FORCE ROW LEVEL SECURITY` + policy per coop
  table; `TenantTx` helper issuing `SET LOCAL app.cooperative_id`;
  `after_release` reset hook; CLI/migrations documented as owner-role.
- **Files**: `db.rs`, new `tenant/tx.rs`, `migrations/0020`, compose/CI.
- **Acceptance**: T2/T3 suites re-run **through RLS**; raw `psql` as
  `kooperatif_app` with wrong/absent GUC sees zero rows; superuser
  bypass documented for ops only.
- **Rollback**: `DISABLE RLS` migration; app scoping (Option A) remains
  the real boundary.
- **Decision gate**: M1-K6 (accept ops cost of second role).

## M1-P5 — Realtime + audit coop

- **Scope**: trigger fn emits `<coop>:<domain>`; hub parses coop;
  `HubSignal{coop_id,domain}`; socket binds coop at upgrade (header);
  delivery filters `signal.coop==socket.coop` + perm re-check;
  `security_events.cooperative_id/session_id` populated; `audit()`
  signature updated; per-coop events.
- **Acceptance**: T6.1–T6.5, T8.4.
- **Risk**: MED — trigger-fn signature change requires recreating 37
  triggers (single migration).

## M1-P6 — Coop administration API

- **Scope**: `GET /api/cooperatives` (my coops), `POST /api/auth/cooperative`
  (switch), membership CRUD under a new `coop_members.manage` perm,
  coop-scoped roles UI APIs; CLI `create-cooperative`/`add-member`.
- **Acceptance**: T1, T4, T7.4 paths have working endpoints.
- **Risk**: MED.

## M1-P7 — Frontend

- **Scope**: coop store (`activeCooperative` in sessionStorage +
  `cooperatives[]` from `me`); selector in shell (shadcn Select);
  `apiFetch` injects `X-Cooperative-Id`; `auth/me` consumption;
  WS reconnect on switch; query-cache keyed by coop (or `invalidateAll`
  on switch); dirty-form guard on switch; active-coop banner.
- **Files**: `apps/web/src/lib/auth/auth.svelte.ts`, `lib/api.ts`,
  `lib/realtime/realtime.svelte.ts`, `+layout.svelte`, new
  `components/coop-switcher.svelte`, i18n `tr-TR.ts` strings.
- **Acceptance**: T7.1–T7.6; stale-data test T7.5.
- **Risk**: MED — largest user-visible change; comes after backend is
  proven.

## M1-P8 — Isolation test suite + CI

- **Scope**: implement matrix end-to-end; add `database` job step for
  the T2.4 sweep + RLS-verification suite; E2E multi-tab spec.
- **Acceptance**: all matrix rows automated; hostile-review checklist
  (`docs/26`) explicitly walked.

## M1-P9 — Per-coop export

- **Scope**: `scripts/backup/` gains per-coop logical export
  (`pg_dump` can't predicate-slice → table-level `COPY (SELECT … WHERE
  coop=…)` per dependency order); drill verifies export completeness +
  restore-into-empty-schema.
- **Acceptance**: ADR-013-style evidence for per-coop artifact.
- **Risk**: MED; dependency-ordered table list = same source-of-truth
  list as P2.

---

## Implementation gates

1. **Gate G1 (owner)**: approve owner decisions M1-K1…K7 before P0.
2. **Gate G2**: P0+P1 merged & green → no UI yet, ctx enforced.
3. **Gate G3**: P2 all sub-modules green → business data fully owned.
4. **Gate G4**: P4 RLS verified through non-superuser role.
5. **Gate G5**: P7 E2E multi-tab green → milestone functionally complete.
6. **Milestone close**: full matrix + regression + `pnpm`/`cargo` gates
   + backup drill w/ tenant fixture + registry rows → owner report (TR).
