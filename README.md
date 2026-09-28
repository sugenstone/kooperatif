# Kooperatif

Production-grade cooperative management system: shareholders, individually
tracked shares, period obligations, collections, financial accounts,
investments, assets, governance, versioned policies, reporting, and a
financially isolated Social Aid subsystem.

**This repository is at the STEP-006 stage: repository/architecture
baseline, identity/authentication/sessions (ADR-002), the RBAC
authorization foundation, the Shareholder/Guardian/Family identity
model, the Share ownership/acquisition foundation, and the
Period/Assessment/Obligation foundation.** No Payments, Collection
Sessions, Ledger, Investments, Governance or Social Aid exist yet. See
[docs/29-IMPLEMENTATION-ROADMAP.md](docs/29-IMPLEMENTATION-ROADMAP.md) —
implementation proceeds one reviewed STEP at a time.

The authoritative project specification lives in
[docs/](docs/) (**specification baseline: v0.8**, package
`kooperatif-agent-spec-v0.8`), with binding architecture decisions in
[docs/adr/](docs/adr/). Start with
[docs/00-PROJECT-CHARTER.md](docs/00-PROJECT-CHARTER.md),
[docs/15-INVARIANTS.md](docs/15-INVARIANTS.md) and
[docs/28-ADR-INDEX.md](docs/28-ADR-INDEX.md). Working rules for agents:
[AGENTS.md](AGENTS.md).

The application architecture baseline is decided by
[ADR-001](docs/adr/ADR-001-APPLICATION-ARCHITECTURE-BASELINE.md):
a **modular monolith** — SvelteKit web, Rust/Axum API, one PostgreSQL
database, a future Rust worker sharing the backend library — in this
monorepo.

Production backup and disaster recovery are governed by
[ADR-013](docs/adr/ADR-013-BACKUP-RESTORE-DISASTER-RECOVERY.md)
(v0.8): PostgreSQL full/base backups + WAL/PITR, an encrypted off-site
copy (a backup stored only on the production host is not sufficient),
protected object-storage recovery, periodic isolated restore
verification ("untested backup is not a verified backup"), and a
**Backup Readiness Gate** — milestones introducing material financial
production data (Payments, Allocations, Ledger) cannot be declared
production-ready without verified backup/restore evidence. Ordinary
business mistakes use reversal/correction, never disaster-recovery
rollback. Concrete backup tooling is a future implementation STEP; no
backup infrastructure is configured in this repository yet.

Default product language/locale: **Turkish (`tr-TR`)**. Localization is
presentation-only and built in from the start.

## Repository layout

``` text
apps/
  web/        SvelteKit + TypeScript + shadcn-svelte frontend
  server/     Rust + Axum backend (library + `serve`/`migrate` binaries)
packages/
  contracts/  Shared typed API contract primitives (TypeScript)
migrations/   Versioned SQL migrations (embedded in the server binary)
docker/       Docker Compose for local development (PostgreSQL)
scripts/      Quality gate scripts (verify.sh, db-verify.sh)
docs/         Authoritative specification package + ADRs
implementation/  Executed STEP instruction files (audit trail)
.github/      CI workflow
```

## Toolchain

| Tool            | Version used                |
| --------------- | --------------------------- |
| Node.js         | 24 (≥22 required)           |
| pnpm            | 10.x (`packageManager` pinned) |
| Rust            | stable (1.92 verified)      |
| Docker Engine + Compose | any recent        |
| Git             | any recent                  |

On Windows, run scripts through **Git Bash** (the repo uses LF line
endings via `.gitattributes`).

## Getting started

```bash
# 1. Install workspace dependencies
pnpm install

# 2. Start local PostgreSQL (Docker; listens on localhost:5494)
docker compose -f docker/compose.yaml up -d

# 3. Configure environment (examples are committed)
cp .env.example .env                      # compose variables (optional)
cp apps/server/.env.example apps/server/.env
cp apps/web/.env.example apps/web/.env

# 4. Apply database migrations (explicit, controlled step)
KOOPERATIF_DATABASE_URL=postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif \
  cargo run --manifest-path apps/server/Cargo.toml -- migrate

# 5. Run the backend API (http://localhost:8080)
cargo run --manifest-path apps/server/Cargo.toml

# 6. Run the frontend dev server (http://localhost:5173)
pnpm dev
```

Health/readiness (distinct semantics, ADR-011):

- `GET /health` — process is alive (never touches PostgreSQL).
- `GET /ready` — instance can serve traffic; reports PostgreSQL as
  `ok` / `unconfigured` / `unavailable` (503 unless `ok`).

## Authentication (STEP-002, ADR-002)

Internal application **Users** (operators) authenticate with username +
password. Shareholders are business records and never log in. There is no
public registration; the first user is bootstrapped through the CLI.

### Creating the first user

```bash
KOOPERATIF_DATABASE_URL=... \
  cargo run --manifest-path apps/server/Cargo.toml -- \
  create-user --username yonetici --display-name "Yönetici"
# Password is read from a hidden prompt (entered twice), never printed
# or logged. For scripted dev/test use --password-stdin (documented
# limitation: the caller owns stdin hygiene, e.g. shell history).
```

### How login/session works

- `POST /api/auth/login` verifies Argon2id credentials and creates a
  **server-side, revocable session**. The browser receives only an
  opaque random token in the `kooperatif_session` cookie
  (`HttpOnly; SameSite=Lax; Path=/`; `Secure` in production — forcing
  it off in production is a startup error). PostgreSQL stores only the
  SHA-256 hash of the token, never the token itself.
- Every authenticated request re-validates server-side: session exists,
  not revoked, absolute TTL and idle TTL not exceeded (server clock is
  the only authority), owning user still `active`. A disabled user's
  existing sessions therefore stop working immediately.
- Idle tracking uses bounded touch: at most one `last_seen` write per
  5 minutes per session (idle semantics preserved).
- `GET /api/auth/me` restores auth state after page refresh and returns
  the per-session CSRF token. `POST /api/auth/logout` revokes the
  current session and clears the cookie (idempotent). Users list and
  revoke their own sessions: `GET /api/auth/sessions`,
  `DELETE /api/auth/sessions/{id}`, `POST /api/auth/sessions/revoke-others`.
- Lifetimes/rate limits/Argon2 parameters are environment-configured
  (see [apps/server/.env.example](apps/server/.env.example)); Argon2
  parameters have enforced floors so no environment can silently weaken
  hashing.

### CSRF protection

Two independent layers on state-changing auth requests (SameSite is
hardening, never the strategy): a strict **Origin allowlist**
(`KOOPERATIF_ALLOWED_ORIGINS`; required in production) on every non-GET
auth request including login, plus a **per-session synchronizer token**
(random, returned by login/me, required back in `x-csrf-token` on
authenticated mutations — a cross-site page can neither read it nor
attach custom headers). The same Origin check applies to the future
WebSocket upgrade handshake (ADR-005).

### Login rate limiting

In-process fixed-window limiter (documented single-node design per
ADR-012; revisit when a second node appears): failed logins count per
normalized **username**, all attempts count per **client IP** (defeats
username spraying; rightmost `X-Forwarded-For` only behind a trusted
proxy). Windows roll automatically — no permanent lockout, so an
attacker cannot lock someone else out. A successful login clears the
username bucket.

### Security events

Durable `security_events` rows (audit — distinct from observability
logs per ADR-011) record login succeeded/failed (with safe reason
categories), logout, session revocations and CLI user creation. No
passwords, tokens, cookie values or CSRF secrets ever reach them.

## Authorization (STEP-003, docs/18)

RBAC: **User → UserRoleAssignment → Role → RolePermission → Permission**.
Effective permissions are the union over a user's **active** roles;
absence means denial. No `is_admin`-style shortcuts, no name-based logic
(renaming a role never changes its behavior), no wildcard permissions,
and nothing authorization-related ever enters the session cookie.

- **Permission catalog** is application-owned: the four STEP-003 keys
  (`users.read`, `users.manage`, `roles.read`, `roles.manage`) are seeded
  by migration 0003 and mirrored in `apps/server/src/auth/authz.rs` and
  `@kooperatif/contracts`. There is deliberately no API to create
  permission keys — future STEPs add permissions via migration + catalog
  constant + contract update. No financial/production-restore
  permissions exist yet (ADR-013 keeps restore operator-privileged).
- **Freshness**: permissions are resolved server-side from PostgreSQL on
  every protected request — grants/revocations/role-disabling take
  effect immediately, no re-login needed.
- **Bootstrap authorization**: migration 0003 seeds the
  `Sistem Yöneticisi` role (explicitly granted the four permissions; the
  name is not special). Give the STEP-002 operator its administration:
  ```bash
  KOOPERATIF_DATABASE_URL=... cargo run --manifest-path apps/server/Cargo.toml -- \
    grant-role --username yonetici --role "Sistem Yöneticisi"
  ```
  (out-of-band operator command, audited as `bootstrap_role_granted`;
  it is also the documented recovery path if every administration path
  is ever lost).
- **Last-administration-path protection**: a mutation that would leave
  zero active users holding an active role with `roles.manage` is
  rejected with `409 lockout_prevented`. Lockout-sensitive mutations are
  serialized by a PostgreSQL advisory transaction lock, so concurrent
  removals cannot both succeed (proven by an HTTP-level concurrency
  test). No hidden superuser bypass exists.
- **APIs** (all server-side authorized, CSRF-protected, `no-store`):
  `GET /api/permissions` (roles.read) · `GET/POST /api/roles`,
  `GET/PATCH /api/roles/{id}`, `disable`/`enable` actions,
  `GET/PUT /api/roles/{id}/permissions` (roles.read/roles.manage) ·
  `GET /api/users`, `GET/PUT /api/users/{id}/roles`
  (users.read/users.manage). Replace-set PUT semantics are idempotent;
  unknown permission keys and disabled-role assignments are rejected;
  role names are unique case-insensitively.
- **Frontend**: `/me` returns the authoritative permission set; the
  `can()` helper drives permission-aware navigation (Roller/Kullanıcılar
  entries) — UX only, the backend enforces every endpoint. `/roller`
  manages roles/permissions (grouped catalog, Turkish labels);
  `/kullanicilar` manages user role assignments.
- **Audit** (`security_events`): role created/updated/enabled/disabled,
  permission-set changes (before/after), role assigned/unassigned (with
  actor), bootstrap grants and lockout-prevention rejections.
- **Running RBAC tests**: `pnpm db:verify` executes the whole Rust suite
  against a clean database, including the RBAC security matrix
  (resolution, HTTP 401/403 enforcement, freshness, lifecycle,
  assignment, lockout incl. concurrency, audit, DB constraints). Each
  RBAC test runs in its own throwaway database for full isolation.

## Shareholders, Guardians & Families (STEP-004, docs/04)

The first cooperative business domain: a normalized identity layer on
which future Share/Period/Payment work builds. **Person ≠ User ≠
Shareholder ≠ Guardian ≠ Family.**

- **`persons`** — one row per real-world person; a Person may act as a
  Shareholder, be referenced as a Guardian, both, or neither. Names are
  **never unique**: several distinct `Mehmet Yılmaz` records are valid.
  Only minimal identity fields are stored (data minimization); a
  computed `search_name` column carries a Turkish-aware fold (`İ/I/ı`→`i`
  + Unicode lowercase) so search is case-insensitive without mutating
  stored display values.
- **`shareholders`** — a cooperative business record linked 1:1 to a
  Person (`person_id` UNIQUE). `guardian_person_id` is an optional
  Person reference: a Guardian may or may not be a Shareholder and is
  never auto-promoted. Status lifecycle is `active ↔ inactive`, plus
  terminal `voided` for mistaken unused records — **no hard delete**.
- **`families`** — cooperative-scoped grouping with a manually assigned
  `sequence_number` (positive bigint, DB-enforced unique; duplicates
  surface as `409 conflict`, concurrency-safe without MAX+1).
- **`shareholder_family_memberships`** — temporal membership:
  `started_at`/`ended_at` intervals, at most one active membership per
  shareholder, overlap prevented by a PostgreSQL exclusion constraint
  (btree_gist). `POST /api/shareholders/{id}/family-change` ("Aile
  Değiştir") closes the current interval and opens the new one — plus
  optional family creation — inside ONE transaction; full history is
  retained for future "as of" queries.
- **Display identity**: same names must be distinguishable everywhere.
  Every shareholder DTO carries a server-composed `displayLabel`:
  `Ad Soyad · Vasi: Ad Soyad · Aile No 47` — always with guardian
  context (`Vasi: Belirtilmemiş` when absent). The UI never renders a
  bare name where ambiguity is possible; no placeholder persons.
- **Search**: folded substring over shareholder name, guardian name and
  family sequence number, server-side, Turkish-safe, offset-paginated
  (small admin registry — ADR-010).
- **Duplicate warning, not blocking**: the create flow probes
  `GET /api/shareholders/duplicates` and shows same-name candidates with
  guardian/family context; the operator confirms it is a different
  person.
- **APIs** (server-side authorized, both CSRF layers, `no-store`):
  `GET/POST /api/shareholders`, `GET/PATCH /api/shareholders/{id}`,
  `POST .../status-change`, `POST .../family-change`,
  `GET /api/shareholders/duplicates`, `GET /api/persons` (lookup only —
  no generic person CRUD), `GET/POST /api/families`,
  `GET /api/families/{id}` (with members). Mutations carry an
  `expectedUpdatedAt` optimistic-concurrency precondition
  (`409 stale_state`); family transfers also serialize on the row lock
  + exclusion constraint.
- **Permissions**: `shareholders.read`, `shareholders.manage`,
  `families.read`, `families.manage` — granted explicitly to the seeded
  `Sistem Yöneticisi` role by migration 0004 (no wildcard inheritance;
  custom roles are untouched).
- **Audit**: person/family/shareholder creation, identity and guardian
  updates (before/after), status transitions and family changes (old→new
  sequence) land in `security_events` with actor + timestamp.
- **Frontend**: `/hissedarlar` (list/search/pagination/create),
  `/hissedarlar/yeni` (person + guardian + family in one transactional
  form), `/hissedarlar/{id}` (identity, Aile Geçmişi, Aile Değiştir,
  status actions), `/aileler` + `/aileler/{id}` (member list). All
  Turkish-first, permission-aware navigation.
- **E2E**: `node scripts/e2e-step004.mjs` drives the real stack
  (PostgreSQL + API + SvelteKit dev server + Chromium) through
  login → create → list → detail → family member verification.

## Shares (STEP-005, docs/04 + docs/16)

The second cooperative domain: first-class, individually tracked Share
records with temporal ownership history. **A Share is a durable
business asset — never a `share_count` column or a balance.**

- **`shares`** — stable, human-readable `share_number` generated by a
  PostgreSQL identity column (concurrency-safe; no MAX+1). Lifecycle:
  `active ↔ suspended`, plus terminal `voided` for mistaken records
  that never went beyond their initial allocation — **no hard delete**;
  `return_pending`/`closed` belong to the future Share Return workflow
  (docs/16) and are intentionally absent.
- **`share_ownerships`** — temporal ownership intervals
  (`started_at`/`ended_at`); at most one open interval per share,
  enforced by a PostgreSQL exclusion constraint (btree_gist). Every
  interval references the `share_events` row that opened it
  (`source_event_id`), so "how did this ownership begin?" is always
  answerable.
- **`share_events`** — append-only business history (distinct from the
  `security_events` audit trail): `initial_acquisition` (founder |
  later_acquisition), `transfer`, `sale`, `status_change`, `voided`.
  Events carry canonical shareholder identity on both sides (`from`/`to`
  — `Ad Soyad · Vasi: … · Aile No N`), an optional `reason`, and the
  actor. No UPDATE/DELETE product path exists.
- **Agreed values, not money movement**: `amount`/`currency` on events
  record only the agreed business value (acquisition fee, private sale
  price) as exact `NUMERIC(19,2)` TRY — `NULL` = not specified, `0` =
  explicitly zero (the distinction is preserved end to end). **No
  Payment/Cashbox/Ledger/Debt objects are created** (docs/04: a private
  sale price is not automatically a cooperative financial-account
  movement; amounts follow ADR-004 exact-money rules and cross the API
  as decimal strings, never floats).
- **Chronology**: backdating is allowed but bounded — an ownership
  change cannot be dated before the current interval began, and no
  event may be dated in the future.
- **APIs** (server-side authorized, both CSRF layers, `no-store`):
  `GET/POST /api/shares`, `GET /api/shares/{id}` (with chronological
  event history), `POST /api/shares/{id}/transfer`,
  `POST /api/shares/{id}/sale`, `POST /api/shares/{id}/status-change`,
  `GET /api/shareholders/{id}/shares`. Ownership mutations carry an
  `expectedUpdatedAt` optimistic-concurrency precondition
  (`409 stale_state`); the share row lock plus the exclusion constraint
  serialize concurrent transfers — exactly one wins.
- **Permissions**: `shares.read`, `shares.manage` — granted explicitly
  to the seeded `Sistem Yöneticisi` role by migration 0005 (no wildcard
  inheritance; custom roles untouched).
- **Frontend**: `/hisseler` (list/search/pagination), `/hisseler/yeni`
  (shareholder picker + acquisition form), `/hisseler/{id}` (canonical
  owner identity, event history, transfer/sale with confirmation,
  suspend/reactivate/void), and a "Hisseler" section on the shareholder
  detail page. TRY amounts are parsed and formatted at the string level
  (`tr-TR` input such as `50.000,00`) — never via floats.
- **E2E**: `node scripts/e2e-step005.mjs` drives the real stack through
  share creation → sale with an agreed amount → owner change →
  shareholder "Hisseler" verification.

## Periods, Assessments & Obligations (STEP-006, docs/05 + docs/16)

The first obligation domain: a Period defines a collection window and a
durable assessment rule; finalization atomically generates one frozen
obligation row per eligible shareholder. **An Assessment is an
obligation record — not a payment, not a ledger entry.** Collection,
allocation and money movement belong to a later reviewed STEP.

- **`periods`** — `period_number` from a PostgreSQL identity column
  (concurrency-safe). Lifecycle `draft → open → closed` (docs/16); the
  `draft → open` transition happens only through assessment generation.
  `due_date >= collection_start_date` is DB-enforced.
- **`assessment_rules`** — exactly one rule per period (unique
  `period_id`): `per_shareholder` (one obligation per eligible
  shareholder) or `per_share` (one amount component per share owned at
  the explicit `assessment_effective_date`), plus the exact
  `NUMERIC(19,2)` TRY base amount.
- **`assessments`** — durable obligation rows, unique per
  `(period_id, shareholder_id)`. Each row snapshots the rule type, base
  amount, currency, effective date and generation actor/time, so later
  rule edits, ownership changes, sales or identity changes never rewrite
  history. Status is `active` (or reserved `voided` for a future
  correction workflow — no void action exists yet).
- **`assessment_share_sources`** — per-Share provenance: which
  share + ownership interval contributed which amount component
  (`per_share` only), so "tahakkukun kaynağı hangi hisseler?" is always
  answerable.
- **Eligibility** is resolved at the explicit effective date: inactive
  or voided shareholders are excluded; `per_share` uses temporal
  ownership intervals so a share transferred after the effective point
  still assesses its owner-at-that-date, and suspended/voided shares are
  excluded.
- **Preview before commitment**: `POST .../assessment-preview` returns
  the rule, counts, per-shareholder expected amounts and totals —
  persists nothing. `POST .../generate-assessments` finalizes
  atomically in one transaction with a row lock: exactly one winner
  under concurrency, and a second attempt answers `409`.
- **Draft-only mutation**: a draft's name/dates/rule may be PATCHed or
  the whole record DELETEd (rule cascades — it is a config row, never
  business history). After generation the period and rule are frozen:
  `409 conflict`.
- **APIs** (server-side authorized, both CSRF layers, `no-store`):
  `GET/POST /api/periods`, `GET/PATCH/DELETE /api/periods/{id}`,
  `POST /api/periods/{id}/assessment-preview`,
  `POST /api/periods/{id}/generate-assessments`,
  `POST /api/periods/{id}/close`,
  `GET /api/periods/{id}/assessments`, `GET /api/assessments/{id}` (with
  sources), `GET /api/shareholders/{id}/assessments`. Draft edits carry
  an `expectedUpdatedAt` optimistic-concurrency precondition.
- **Permissions**: `periods.read`, `periods.manage`,
  `assessments.read`, `assessments.manage` — granted explicitly to the
  seeded `Sistem Yöneticisi` role by migration 0006.
- **Audit**: period created/updated/deleted/closed and
  `assessments_generated` (period, counts, exact total) land in
  `security_events`.
- **Frontend**: `/donemler` (list/search/pagination), `/donemler/yeni`,
  `/donemler/{id}` (detail, rule card, Önizleme table, finalize/close
  commands, generated Tahakkuk list), `/tahakkuklar/{id}` (obligation +
  Kaynak Hisseler provenance) and a "Tahakkuklar" section on the
  shareholder page. Turkish-first i18n; `can()` is UX-only.
- **E2E**: `node scripts/e2e-step006.mjs` drives the real stack through
  login → fixtures → period create → preview → finalize → obligation
  detail → shareholder Tahakkuklar → close.

### Running auth tests

`pnpm db:verify` (or `scripts/db-verify.sh`) recreates a clean
verification database and runs the entire Rust suite against it,
including the HTTP-level auth security matrix (login failures, session
lifecycle/expiry/revocation, CSRF, rate limiting, ownership, database
constraints). Without `KOOPERATIF_TEST_DATABASE_URL` the DB-gated tests
skip with an explicit notice.

## Quality gates

One documented sequence runs everything (requires Docker for the database
gate):

```bash
pnpm verify        # = scripts/verify.sh
```

Individual gates:

```bash
pnpm format            # prettier write (web)
pnpm lint              # prettier check + eslint (web)
pnpm check             # contracts tsc + svelte-check
pnpm test              # vitest unit/component tests
pnpm build             # SvelteKit production build (adapter-node)
pnpm rust:fmt          # cargo fmt --check
pnpm rust:clippy       # cargo clippy --all-targets -D warnings
pnpm rust:test         # cargo test
pnpm db:verify         # clean-database migration verification (Docker)
```

CI (`.github/workflows/ci.yml`) runs the frontend, backend and database
gates on every push/PR.

## Configuration

Environment variables only; never committed secrets. See
[apps/server/.env.example](apps/server/.env.example) for the full backend
list. `KOOPERATIF_ENV=production` fails fast when required configuration
is missing — no insecure silent defaults.

## Notes

- PostgreSQL data volume persists between runs
  (`docker compose -f docker/compose.yaml down` keeps it; add `-v` to
  discard).
- The `migrate` subcommand is the only supported way to apply migrations.
- Obligation schema now exists (STEP-006); money-movement schema
  intentionally does not — Payments/Allocations/Ledger belong to a
  later reviewed STEP (ADR-003/ADR-004).
