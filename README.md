# Kooperatif

Production-grade cooperative management system: shareholders, individually
tracked shares, period obligations, collections, financial accounts,
investments, assets, governance, versioned policies, reporting, and a
financially isolated Social Aid subsystem.

**This repository is at the STEP-003 stage: repository/architecture
baseline, identity/authentication/sessions (ADR-002) and the RBAC
authorization foundation.** No cooperative business domain is implemented
yet. See
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
- Financial schema intentionally does not exist yet (ADR-003/ADR-004
  implementation belongs to a later reviewed STEP).
