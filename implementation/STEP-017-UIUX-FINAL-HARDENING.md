# STEP-017 — UIUX-001, FINAL HARDENING & RELEASE CANDIDATE

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
main == origin/main == 18287be  (STEP-016)
```

Authoritative inputs:

```text
docs/12-UI-UX-SYSTEM.md
docs/13-SCREEN-SPECIFICATIONS.md
docs/14-REPORTING-AND-DOCUMENT-DESIGN.md
docs/15-INVARIANTS.md
docs/17-CALCULATION-RULES.md
docs/18-AUTHORIZATION-APPROVALS.md
docs/19-AUDIT-REVERSAL-CORRECTION.md
docs/22-SECURITY.md
docs/23-BACKUP-RECOVERY.md
docs/26-TESTING-QUALITY-GATES.md
docs/27-ARCHITECTURE.md
ADR-002, ADR-005, ADR-009, ADR-012, ADR-013
implementation/STEP-001…016 reports
```

Approved scope: make the existing system consistent, accessible,
mobile-friendly, secure and verifiable for a controlled pilot — no
architectural redesign, no new business policy, no new domains.

Internal phases: 17A UX audit & design system → 17B workflow & mobile →
17C security/reliability/performance → 17D release-candidate
verification.

---

## A. Baseline

`git status` clean, `main` checked out, `HEAD == origin/main ==
18287be` (STEP-016 pushed). Verified before starting.

## B. Phase 17A — UX Audit & Design System

### Screen inventory (all routes)

| Route | Purpose | Primary action | Density | Mobile | Issues found |
|---|---|---|---|---|---|
| `/login` | Authentication | Giriş Yap | low | ✓ | — |
| `/` | Dashboard/welcome | nav | low→med | ✓ | was bare placeholder; health card dropped during rewrite then restored via `BackendHealth` |
| `/hissedarlar`, `/aileler`, `/hisseler` | Identity lists | Yeni …, Ara | table | scroll table | now PageHeader |
| `/hissedarlar/[id]`, `yeni`, `/aileler/[id]` | Identity detail/create | Kaydet | form+detail | ✓ | duplicate-name guard exists |
| `/donemler`, `/donemler/yeni`, `/donemler/[id]` | Periods | Yeni Dönem, Tahakkuk Üret | table+ops | ✓ | busy-guarded actions |
| `/tahakkuklar/[id]` | Assessment detail | — | detail | ✓ | — |
| `/tahsilatlar`, `yeni`, `[id]` | Payments | Yeni Tahsilat | wizard | ✓ | decimal keypad added |
| `/finansal-hesaplar`, `yeni`, `[id]` | Accounts + movements | Yeni Hesap | table | scroll | balance derived only |
| `/transferler`, `yeni`, `[id]` | Transfers | Yeni Transfer | table+form | ✓ | decimal keypad added |
| `/gelirler`, `/giderler`, `yeni`, `[id]`, `/gelir-gider`, `/kategoriler` | Operational finance | Yeni Gelir/Gider | table+form | ✓ | decimal keypad added |
| `/hisse-iadeleri`, `yeni`, `[id]` | Share returns | Yeni İade | lifecycle | ✓ | — |
| `/yatirimlar`, `yeni`, `[id]` | Investments | Yeni Yatırım | lifecycle | ✓ | — |
| `/sosyal-yardim`, `yeni`, `[id]` | Social aid funds | Yeni Fon | detail | ✓ | — |
| `/yonetim`, `kurullar`, `kararlar` | Governance evidence | Yeni Karar/Kurul | detail | ✓ | — |
| `/raporlar` | Canonical reports | Filtrele | dense | scroll | — |
| `/canli-ekran` | TV/projector | (read-only) | low | n/a | recovery UX added in 17C |
| `/roller`, `roller/[id]`, `/kullanicilar`, `[id]`, `/oturumlar` | Administration | role/user/session mgmt | table+form | ✓ | — |

Not present (correctly absent, not invented): a dedicated audit UI
(audit rows are surfaced inside detail timelines), a collection-session
workspace (not part of the implemented scope), a Persons list screen
(persons are resolved through shareholder/governance lookups per spec).

### Design system

- Foundation kept: Tailwind + existing shadcn-svelte primitives
  (`alert`, `badge`, `button`, `card`, `input`, `label`, `table`) — no
  second component library, no framework change.
- Semantic tokens already in `layout.css` (background, foreground,
  muted, accent, destructive, sidebar, ring); dark palette defined.
- **New shared primitives** (small, typed, composable):
  - `app-nav.svelte` — grouped, permission-aware navigation shell:
    persistent sidebar (`md:`+), mobile top bar with `aria-expanded`
    menu, `aria-current="page"` on the active route, user/logout block.
  - `page-header.svelte` — title + description + action slot; applied
    to all 17 list screens (uniform pattern was already near-identical
    markup; now centralized).
  - `empty-state.svelte` — standard empty-state placeholder.
- Home route is now a real dashboard for `reports.read` users fed by
  the canonical `/api/reports/overview` (four KPI cards + restricted-
  balance disclaimer note) plus the existing `BackendHealth` card.
- Root layout stripped to skip-link + children so the `(tv)` display is
  no longer squeezed into the narrow app wrapper.
- `app.subtitle` typo fixed (`Kooparatif` → `Kooperatif`); nav group
  and menu labels added to tr-TR + en dictionaries — no hard-coded
  user-facing strings in the new shell.

### 17A gate

svelte-check 0/0 · eslint clean · prettier clean · vitest 145+ → nav
specs cover grouping, permission hiding, active route, mobile toggle.
Commit `43f61ae`.

## C. Phase 17B — Workflows & Mobile

- **Duplicate submission**: audited all forms — every mutation handler
  already guards with an in-flight flag (`submitting`/`creating`/
  `busy`/`acting`) plus `disabled` on the submit control; backend
  idempotency keys remain the authority.
- **Money inputs**: `inputmode="decimal"` added to payment amount,
  allocation rows, income/expense and transfer creation forms (detail
  screens already had it). `parseTryInput` canonical conversion
  unchanged — `300,00` → `"300.00"`, never `30000.00`.
- **Layout fix**: at exactly ≥768px the `w-full` main + `md:ml-60`
  overflowed the viewport by the sidebar width (240px). Offset moved to
  wrapper padding (`md:pl-60`); caught by real viewport testing.
- **Viewport evidence** — `scripts/e2e-step017-uiux.mjs` (real stack):
  9 routes × 360×800 / 390×844 / 768×1024 / 1440×900 → **zero
  horizontal page overflow**; mobile menu opens and navigates; desktop
  sidebar present; `inputmode=decimal` asserted on 4 money forms;
  screenshots under `artifacts/step017/` (gitignored).

Commit `5f29586`.

## D. Phase 17C — Security, Reliability & Performance

### Security

- **Baseline security headers** on EVERY API response (success, error,
  404): `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
  `Referrer-Policy: no-referrer` — global middleware in
  `http/mod.rs`, covered by a new `http.rs` test.
- Existing controls re-verified by test suite: HttpOnly cookie auth,
  session expiry/revocation, CSRF synchronizer token, unconditional
  Origin allowlist on mutations and WS upgrade, RBAC default-deny,
  IDOR (unknown-id → 404/403 semantics), `Cache-Control: no-store` on
  every domain router, login rate limiter, security event audit,
  WS per-signal re-authorization.
- **Dependency audit**: `pnpm audit` flagged `source-map-js <1.2.2`
  (build-time DoS, high) and `cookie <0.7.0` (out-of-bounds cookie
  chars, low). Fixed via `pnpm.overrides` → `source-map-js@1.2.2`,
  `cookie@0.7.2` — `pnpm audit` now reports **zero** vulnerabilities;
  148 vitest + production build still pass (kit 2.70 compatible).
  `cargo audit` not installed locally — documented limitation; `cargo
  fmt`/`clippy -D warnings` clean.

### Financial integrity

Unchanged code paths re-proven by the full backend suite (289 tests,
real PostgreSQL, `--test-threads=8`, 0 ignored): exact `NUMERIC(19,2)`
↔ `Decimal` ↔ canonical string; allocations/applications create no
movements; settlement/donation/disbursement boundary semantics;
NULL-vs-zero; reversal history; concurrent-over-allocation
serialization; idempotent replay.

### WebSocket long-running recovery

Gap identified: after the bounded reconnect cap the client stayed
`disconnected` forever — a sleeping laptop / suspended tab could leave a
permanently stale display, and `online` only fires on network change.
Fix:

- `RealtimeClient.wake()` — on `disconnected` → `resume()`; on a socket
  open but silent >90 s → force-close so the bounded reconnect produces
  a fresh resync (half-open TCP is indistinguishable in browsers).
- Wired to `visibilitychange` in the `(app)` layout and the TV page;
  `online`→`resume()` kept.
- TV stale banner gains a manual `Yeniden Bağlan` action when
  `disconnected` — the operator recovery path.
- 3 new client specs prove exhausted→wake→resume, stale-socket
  force-cycle and healthy-socket no-op; `scripts/e2e-step016.mjs`
  re-run end-to-end after the change — full PASS.

### Database pool

- Confirmed: `max_connections=5`, 3 s acquire timeout; each DB test
  builds an isolated DB + own pool + admin pool; at `--test-threads=8`
  (~80 conns worst case) the suite is stable — PostgreSQL
  `max_connections=100`. CI (2–4 vCPU → ≤4 threads) is unaffected.
  No leak: pools are dropped per test. Reproducible local config:
  `cargo test -- --test-threads=8`.
- Query plans spot-checked on real data: hash/group aggregates over
  small pilot-scale tables use seq scans — optimal at this volume; no
  speculative indexes added.

### Backup/restore

`scripts/backup/drill.mjs` — **PASS in 80 s**: fresh-target restore,
migration ledger, temporal history, exact decimal precision, derived
balances/entitlements/credits, restricted availability, report parity
across all overview metrics, and the realtime trigger/function/channel
restore checks.

### Frontend performance

Bundle unchanged in shape (nav/sidebar adds <3 kB gzip); invalidation
coalescing already bounds refetch storms; no full-table client fetches
(pagination preserved server-side).

Commit `14f04b7`.

## E. Phase 17D — Release Candidate Verification

`scripts/e2e-step017-rc.mjs` on a **fresh migrated database** through
the real stack:

| Journey | Result |
|---|---|
| A onboarding (person→family→shareholder→share; duplicate names) | PASS |
| B period → generate → partial + full payment → remaining debt | PASS |
| C overpayment → explicit credit → auto-application on next period, no cash movement, duplicate apply rejected | PASS |
| D account transfer → derived balances | PASS |
| E income + expense → canonical movements | PASS |
| F share return → entitlement → settlement → share closed | PASS |
| G investment funding → valuation (zero movements) → income | PASS |
| H social aid donation → disbursement → restricted availability | PASS |
| I governance body → membership → decision → open → vote → finalize | PASS |
| J reports overview parity (hand-computed: cash 1725.00, assessed 3300.00, debt 1200.00, aid 250.00, …) | PASS |
| K live TV: connect → canonical value rendered | PASS |

- **Browser coverage**: Chromium (full journeys + viewports), Firefox
  and WebKit (login → sidebar nav → shareholder list render). Real
  device hardware NOT covered — emulation only, documented.
- **Accessibility evidence** (DOM-level assertions): labelled login
  inputs, skip link present, tab order reaches submit without traps,
  mobile menu `aria-expanded`, `aria-current` on active route.
  Automated axe-core audit NOT run (package not installed) — a11y
  claim is bounded to the checks above.
- **UI journeys**: login form → dashboard → list → detail navigation
  verified in the real browser.

## Release blocker register

- P0: 0 open
- P1: 0 open
- P2: 0 open — (sidebar 240px overflow at ≥768px was found and fixed
  during 17B; exhausted-reconnect permanent-stale was found and fixed
  during 17C)
- P3: dense list tables scroll horizontally inside their card on
  mobile instead of reflowing to cards — deliberate per docs/12
  (overflow-x container, labels preserved); KPI card currency may wrap
  under the ₺ symbol at 360px; dev-machine full-parallel test runs need
  `--test-threads=8` (documented, CI unaffected).

## Test evidence

- `cargo test` (real PostgreSQL, `--test-threads=8`): **289 PASS, 0
  failed, 0 ignored** — includes new security-headers test.
- `vitest`: **148 PASS** (nav-shell + realtime wake specs added).
- `svelte-check` 0/0 · `eslint` clean · `prettier` clean · `vite build`
  PASS · `cargo fmt --check` PASS · `clippy -D warnings` PASS ·
  `git diff --check` PASS.
- E2E real stack: `e2e-step017-uiux` PASS · `e2e-step017-rc` PASS ·
  `e2e-step016` PASS (re-run after realtime changes).
- Backup/recovery drill PASS (restore verified, not just created).
- `pnpm audit`: 0 vulnerabilities after overrides.

## Deployment safety checklist (pilot, not production authorization)

- Env inventory: `KOOPERATIF_DATABASE_URL`, `KOOPERATIF_ENV`,
  `KOOPERATIF_ALLOWED_ORIGINS`/`KOOPERATIF_CORS_ORIGINS`,
  `KOOPERATIF_HOST`/`PORT`, Argon2 cost knobs (defaults are floors).
- Secrets: never committed; passwords via `--password-stdin`; session
  cookie `Secure` enforced in production env (config test rejects
  insecure production cookies).
- Migrations: explicit `cargo run -- migrate` before serving
  (migration-driven schema, no implicit migration).
- Health: `/health` (liveness), `/ready` (DB readiness) — used by the
  drill and E2E harness.
- Backup: `scripts/backup/create.mjs` + `verify.mjs` + `drill.mjs`;
  restore procedure proven by the drill.
- Rollback: reverse proxy swap to previous binary + DB restore via the
  drill-tested path; never rewrite financial history.
- Single-node topology per ADR-012; LISTEN/NOTIFY needs no extra infra.

## Files changed

- 17A `43f61ae`: `app-nav.svelte` (new), `page-header.svelte`,
  `empty-state.svelte` (new), root/`(app)` layouts, home dashboard,
  17 list pages → PageHeader, i18n tr/en, `app-user-bar.svelte`
  removed, nav/authz/reports specs updated, `nav-shell.spec.ts` new.
- 17B `5f29586`: `(app)` layout padding fix, `inputmode=decimal` on
  payment/income/expense/transfer forms, `e2e-step017-uiux.mjs`,
  `.gitignore` artifacts entry.
- 17C `14f04b7`: `http/mod.rs` security-headers middleware,
  `tests/http.rs`, `realtime.svelte.ts` `wake()` + stall detection,
  `(app)` layout + TV page recovery triggers, TV retry action,
  `tv.retry` i18n, `realtime.spec.ts` +3 tests, `package.json` +
  `pnpm-lock.yaml` overrides.
- 17D: `scripts/e2e-step017-rc.mjs` (new), this report, `README.md`.

## Deferred scope (not invented)

Durable event log/replay, per-user event targeting, push/SMS/email,
collection-session workspace, automated axe-core audit, real-device
lab, `cargo audit` installation, statutory accounting compliance,
production deployment — all out of scope or awaiting authorization.

## Pilot readiness

All required evidence passes → **RELEASE CANDIDATE — PILOT READY**.

Not claimed: production deployed, security certified, legally
compliant, statutory accounting compliant, independently audited.
