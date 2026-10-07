# Kooperatif

Production-grade cooperative management system: shareholders, individually
tracked shares, period obligations, collections, financial accounts,
investments, assets, governance, versioned policies, reporting, and a
financially isolated Social Aid subsystem.

**This repository is at the BACKUP-READINESS-001 stage:
repository/architecture baseline, identity/authentication/sessions
(ADR-002), the RBAC authorization foundation, the
Shareholder/Guardian/Family identity model, the Share
ownership/acquisition foundation, the Period/Assessment/Obligation
foundation, and the PostgreSQL backup/restore/disaster-recovery
foundation (ADR-013).** No Payments, Collection Sessions, Ledger,
Investments, Governance or Social Aid exist yet. See
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
rollback. The local technical foundation is implemented under
`scripts/backup/` (see the dedicated section below and the operator
runbook in [docs/30-BACKUP-OPERATIONS-RUNBOOK.md](docs/30-BACKUP-OPERATIONS-RUNBOOK.md));
production WAL/PITR, encrypted off-site copies and scheduling remain
environment provisioning duties.

Default product language/locale: **Turkish (`tr-TR`)**. Localization is
presentation-only and built in from the start.

## Repository layout

```text
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

| Tool                    | Version used                   |
| ----------------------- | ------------------------------ |
| Node.js                 | 24 (≥22 required)              |
| pnpm                    | 10.x (`packageManager` pinned) |
| Rust                    | stable (1.92 verified)         |
| Docker Engine + Compose | any recent                     |
| Git                     | any recent                     |

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
  - Unicode lowercase) so search is case-insensitive without mutating
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
  - exclusion constraint.
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

## Payments, Allocations & Collection (STEP-007, docs/06 + docs/19)

The first money-received domain. **A Payment is money the cooperative
actually received; an Assessment is an obligation; a Payment Allocation
is the application of received value to one or more obligations.** The
three concepts are never merged — a receipt never mutates an
assessment's stored amount, and balances are always derived.

- **`payments`** — `payment_number` from a PostgreSQL identity column.
  `payer_person_id` references a **Person**, never a Shareholder: a
  shareholder pays through their person identity and a third party pays
  identically — payer ≠ debtor is the default, not an exception.
  Lifecycle `posted → reversed` (terminal); no draft/pending state
  because no approval policy is approved yet (docs/16). Reversal keeps
  the original row and stamps `reversed_at`/`reversed_by`/
  `reversal_reason` — enforced by a DB CHECK, never a DELETE.
- **`payment_allocations`** — many-to-many application rows between
  payments and assessments, unique per `(payment_id, assessment_id)`.
  Reversal is status-based (`active → reversed`); a reversed allocation
  stays as history with its own reversal metadata.
- **Atomic posting**: `POST /api/payments` creates the payment and its
  requested allocations inside one transaction. Any invalid target,
  over-allocation or bad amount rolls the whole command back — the
  payment never persists half-allocated.
- **Over-allocation safety**: the assessment row is locked
  (`FOR UPDATE`) inside the posting transaction, so concurrent
  allocations serialize and the second one is rejected — a paid-in-full
  obligation can never exceed its amount even under races.
- **Unallocated remainder is explicit**: `unallocatedAmount` is derived
  (`amount − Σ active allocations`); a payment may be fully unallocated
  and the remainder can be distributed later via
  `POST /api/payments/{id}/allocations`. It stays ON the payment — never
  silently attributed to a family member or written off.
- **Idempotent creation** (ADR-006): a durable `idempotency_key` +
  payload fingerprint means a retry replays the existing receipt;
  a reused key with a different payload answers `409 conflict`.
- **Idempotent reversal**: `POST /api/payments/{id}/reverse` marks the
  payment and all its active allocations reversed with reason + actor;
  a replay answers the existing state, never a second effect. Single
  lines are corrected through
  `POST /api/payments/{id}/allocations/{id}/reverse` — other lines stay.
- **Derived surfaces**: `GET /api/assessments/{id}` returns
  `allocatedAmount`/`remainingAmount`/`settledAt` computed from active
  allocations; `GET /api/shareholders/{id}/financial-summary`,
  `GET /api/shareholders/{id}/open-assessments`,
  `GET /api/families/{id}/collection-context` (member-wise totals for
  bulk collection — the family is context, **never the debtor**),
  `GET /api/payments` (list/search), `GET /api/payments/{id}` (with
  allocation lines + payer identity),
  `GET /api/payments/payer-persons?search=` (payer picker),
  `GET /api/assessments/{id}/payments` (application history).
- **Exact money**: `NUMERIC(19,2)` TRY end to end; the API carries
  decimal strings, the frontend parses `tr-TR` operator input at the
  string level — no float ever touches a financial value.
- **Permissions**: `payments.read`, `payments.manage` — granted
  explicitly to the seeded `Sistem Yöneticisi` role by migration 0007.
- **Audit**: `payment_posted`, `payment_allocations_added`,
  `payment_reversed`, `payment_allocation_reversed` land in
  `security_events` with payment/allocation identifiers.
- **Frontend**: `/tahsilatlar` (list), `/tahsilatlar/yeni` (three-step
  collection: payer search-or-create → debt selection by shareholder
  or family members → exact preview of allocated/unallocated before
  confirm), `/tahsilatlar/{id}` (receipt detail, allocation lines,
  reversal with reason), payment history on `/tahakkuklar/{id}`,
  financial summary on `/hissedarlar/{id}`, collection context on
  `/aileler/{id}`. Turkish-first i18n; `can()` is UX-only.
- **E2E**: `node scripts/e2e-step007.mjs` drives the real stack through
  family bulk collection (one payer, two debtors) → assessment history
  → shareholder summary → full reversal with derived-balance proof.
- **Boundary**: no Cashbox, Bank Account, Ledger, Receipt or Collection
  Session objects exist — where received money physically enters and
  how it is documented are deferred domains (docs/06 §88–§92). The
  financial-account half of that boundary landed in STEP-008 below;
  Ledger/Receipt/Collection Session remain deferred.

## Financial Accounts, Movements & Transfers (STEP-008, docs/07 + docs/19)

The money-where domain. **A Payment is a record of money received; an
Account Movement is the account effect of that money — the two are
separate records, never merged. An account balance is never a stored
number: it is the sum of valid account movements (`inflow` adds,
`outflow` subtracts), always derived, never editable.**

- **`financial_accounts`** — one engine, two operational shapes:
  `account_type` `cash | bank`; TRY-only at this step. Lifecycle
  `active ↔ inactive` (operator-approved): an inactive account keeps
  its history and balance but accepts no new postings. `account_type`
  and `currency` are immutable after creation — updating them would
  silently rewrite financial history. Bank metadata (`bank_name`,
  `iban`) is allowed only on `bank` accounts.
- **`account_movements`** — the immutable financial history (ADR-003).
  Created **only by domain commands**: `source_type` is `payment`,
  `transfer`, `income` or `expense` (the last two landed in STEP-010)
  and `source_id` points at the originating row — there is
  deliberately no arbitrary movement endpoint. `UNIQUE
(account_id, source_type, source_id)` structurally prevents a source
  posting the same leg twice.
- **`account_transfers`** — one logical move between two cooperative
  accounts (never income or expense): a single transfer row plus two
  linked movement legs (source outflow, destination inflow) inserted
  in one transaction — a half-posted transfer is impossible.
- **No negative balances** (operator decision): a transfer whose source
  cannot cover the amount, or a reversal whose effect would take an
  account below zero, is rejected with `409` while the account rows
  are locked.
- **Deterministic locking**: every balance-affecting command locks
  account rows `FOR UPDATE` in ascending UUID order (transfers both
  accounts; payment commands the payment first, then the account) —
  deadlocks are impossible by construction and concurrent transfers
  serialize without overdraft.
- **Payment integration**: new payments require a
  `destination_account_id`; `POST /api/payments` posts the payment,
  its allocations and exactly one inflow movement atomically — a
  posted payment can never exist without its movement. Reversing a
  payment reverses its movement in the same transaction, blocked if
  the effect would violate the no-negative-balance rule (money already
  moved out — restore it first). Legacy payments with NULL destination
  stay readable and reversible with no account effect; no fabricated
  destination is invented for historical money.
- **Status-based reversal everywhere** (docs/19): payment, transfer
  and movement originals always survive; reversal stamps
  `reversed_at`/`reversed_by`/`reversal_reason` (DB-enforced CHECKs),
  reverses both transfer legs atomically, and replays idempotently.
- **Idempotent transfers** (ADR-006): durable `idempotency_key` +
  payload fingerprint — a retry replays the existing transfer, a key
  reused with a different payload answers `409 conflict`.
- **Derived surfaces**: `GET /api/financial-accounts` (list/search +
  derived balance), `GET /api/financial-accounts/options` (active
  accounts for pickers), `GET /api/financial-accounts/{id}`,
  `GET /api/financial-accounts/{id}/movements` (active + reversed
  history with source metadata), `GET|POST /api/account-transfers`,
  `GET /api/account-transfers/{id}`,
  `POST /api/account-transfers/{id}/reverse`.
- **Exact money**: `NUMERIC(19,2)` TRY end to end; movement `effect`
  is a signed decimal string computed at the API boundary.
- **Permissions**: `financial_accounts.read`, `financial_accounts.manage`
  — granted to `Sistem Yöneticisi` by migration 0008; every route is
  backend-authorized and every mutation is CSRF-protected.
- **Audit**: `financial_account_created`, `financial_account_updated`,
  `financial_account_status_changed`, `account_transfer_posted`,
  `account_transfer_reversed` land in `security_events`; payment
  posting/reversal reuses the STEP-007 payment audit events.
- **Frontend**: `/finansal-hesaplar` (list + derived balances),
  `/finansal-hesaplar/yeni` (create), `/finansal-hesaplar/{id}`
  (metadata, derived balance, movement history, edit + status
  controls), `/transferler` (list), `/transferler/yeni` (two-account
  preview before confirm), `/transferler/{id}` (detail + reversal),
  destination-account selector on `/tahsilatlar/yeni` (required) and
  the account shown on `/tahsilatlar/{id}`. Turkish-first i18n; all
  math stays at the string/`BigInt` level.
- **E2E**: `node scripts/e2e-step008.mjs` drives the real stack through
  payment → account posting → UI transfer → insufficient-funds
  rejection → transfer reversal → payment reversal with derived-balance
  proof at every step.
- **Boundary**: no General Ledger, Investment, Social Aid finance,
  Receipt, bank-reconciliation, gold/commodity, refund/disbursement or
  manual-balance-editing surface exists — deferred domains (docs/07
  open decisions). Income/Expense landed in STEP-010 below.

## Income & Expense (STEP-010, docs/07 + docs/19)

The money-why domain. **Income and Expense are business events that
explain why money moved; the Account Movement stays the authoritative
money record. A posted Income produces exactly ONE inflow movement, a
posted Expense exactly ONE outflow movement — bound 1:1 both ways
(`account_movement_id` FK plus `source_type`/`source_id`). There is no
income/expense balance and no second ledger.**

- **`income_entries` / `expense_entries`** — first-class posted events:
  stable `income_number`/`expense_number` identity, explicit
  `financial_account_id` (never inferred from persons), typed
  `category_id`, exact `NUMERIC(19,2)` TRY amount, `occurred_at`
  (backdating allowed), `description`, optional `counterparty`
  (descriptive text — never Person/Shareholder semantics) and
  `reference_no`. Lifecycle `posted → reversed` is terminal.
- **`financial_categories`** — one typed table (`income | expense`).
  A composite FK `(category_id, entry_kind) → (id, category_type)`
  makes a wrong-typed category impossible at the database level.
  Categories are creatable, renamable (optimistic `expectedUpdatedAt`)
  and activatable/deactivatable — never hard-deleted; deactivation
  blocks only NEW postings, history keeps the relation.
- **Atomic posting**: entry + exactly one movement insert in one
  transaction under the account row lock — an Income can never exist
  without its movement and vice versa. An Expense that would take the
  account below zero is rejected `409` (STEP-008 rule); an Income
  reversal that would do the same is rejected identically.
- **Boundaries hold**: Payments remain `payment`-sourced movements and
  never appear in Income; Transfers post `transfer` legs and are
  neither Income nor Expense; Shareholder Credits and Credit
  Applications still create zero movements. The operational summary
  (`GET /api/income-expense/summary`) sums posted entries only —
  `net = incomeTotal − expenseTotal` — and is explicitly NOT the
  account balance and NOT profit/loss.
- **Derived surfaces**: `GET|POST /api/incomes`,
  `GET /api/incomes/{id}`, `POST /api/incomes/{id}/reverse` (same for
  `/api/expenses`), `GET|POST /api/financial-categories`,
  `GET /api/financial-categories/options`, `PATCH
/api/financial-categories/{id}`, `POST
/api/financial-categories/{id}/status-change`, `GET
/api/income-expense/summary` (`dateFrom`/`dateTo`). Lists are
  server-paginated with date/account/category/status/search filters.
- **Idempotent commands** (ADR-006): posting carries a durable
  `idempotency_key` + payload fingerprint — same key + same payload
  replays the created entry, different payload answers `409`.
- **Status-based reversal** (docs/19): entries flip `posted →
reversed` with actor/time/reason and the bound movement reverses in
  the same transaction — no DELETE endpoint exists anywhere.
- **Permissions**: `income_expense.read`, `income_expense.manage` —
  granted to `Sistem Yöneticisi` by migration 0010.
- **Audit**: `income_created`, `income_reversed`, `expense_created`,
  `expense_reversed`, `financial_category_created`,
  `financial_category_updated`, `financial_category_status_changed`
  land in `security_events`.
- **Frontend**: `/gelirler` + `/giderler` (filtered, paginated lists),
  `/yeni` creation forms with preview and tr-TR money input
  (`parseTryInput` — the HOTFIX-001 canonical boundary), `/{id}`
  details with movement provenance and reasoned reversal,
  `/gelir-gider` operational summary (totals + recent entries — NOT a
  balance sheet) and `/kategoriler` category management. Nav entries
  are permission-gated (`income_expense.read`).
- **E2E**: `node scripts/e2e-step010.mjs` drives the real stack —
  UI income +5000, UI expense −1250, derived-balance checks at every
  step, summary totals, UI reversal, and a payment that provably never
  enters income totals.
- **Boundary**: no General Ledger/double-entry, invoices, vendors,
  Investment, Social Aid, tax/VAT engine, budgeting, purchase orders
  or approval workflows — deferred (docs/07, STEP-010 §53).

## Share Return, Exit & Deferred Entitlements (STEP-011, docs/10 + docs/16 + docs/19)

The cooperative-exit domain. **Three layers stay strictly separate:
(1) the Share lifecycle (`active → return_pending → closed`), (2) the
Entitlement — what the cooperative owes, which moves NO money — and
(3) the Settlement — real money leaving a Financial Account through
exactly ONE outflow Account Movement. Closing a share is not
recognizing a debt, and recognizing a debt is not paying it.**

- **`share_returns`** — the controlled workflow case: stable
  `return_number`, the share, the owner-at-initiation FK plus immutable
  `owner_display_name`/`share_number`/`ownership_started_at` snapshots,
  `requested_at`, and the authoritative `effective_return_date` — the
  business DATE on which economic participation ends; the ownership
  interval closes at that date's 00:00 Europe/Istanbul. Lifecycle
  `pending → finalized | cancelled`, both with DB-enforced consistency
  CHECKs; at most one `pending` return per share (partial unique
  index). Initiation moves the share to `return_pending`, cancellation
  restores `active`, finalization stamps `finalized_at/by` and closes
  the share — `closed` and `voided` are terminal, no resurrection path.
- **`share_return_entitlements`** — one row per right per return:
  `entitlement_type ∈ {principal, profit}` ("Refundable Invested
  Amount Right" / "Profit Right"), explicit beneficiary, `due_date`,
  `policy_reference` (the free-text decision under which the amount was
  set), `recognized_at`, `determined_at`. **`amount NULL` is a
  first-class state: "the right exists, quantification pending" — never
  rendered as `0.00`.** A CHECK enforces amount ↔ determined_at
  pairing; a partial unique index permits at most one non-cancelled
  entitlement per (return, type). Status `open → partially_settled →
settled | cancelled`; remaining is always derived
  (`amount − posted settlements`), never stored.
- **No invented formulas**: the refundable amount and the profit right
  are `UNRESOLVED / POLICY_DRIVEN` in docs/10+docs/17 — the operator
  enters the cooperative-approved amount; the system never computes
  it. Profit rights may stay undetermined at finalize and be fixed
  later via `POST …/determine` (optimistic `expectedUpdatedAt`).
- **`share_return_settlements`** — real cash leaving an account:
  `settlement_number`, `entitlement_id`, `financial_account_id`, exact
  positive `NUMERIC(19,2)` amount, `settled_at`, and a hard 1:1
  `account_movement_id` binding. Each settlement posts exactly ONE
  `outflow` movement with `source_type = 'share_return_settlement'` —
  it is **not** Expense, Payment, Transfer or Shareholder Credit and
  never touches those tables. Over-settlement, undetermined-amount
  settlement and insufficient funds are all rejected `409`; the
  entitlement row lock serializes concurrent postings.
- **Status-based reversal** (docs/19): settlement `posted → reversed`
  flips the bound movement `active → reversed` in the same
  transaction; the entitlement's derived remaining amount is restored
  automatically. No DELETE endpoint exists anywhere in the domain.
- **Effective-date eligibility fix**: period/assessment share
  eligibility now consults temporal ownership intervals plus statuses
  `active | return_pending | closed` instead of "currently active" —
  a share closed today still rightly owes yesterday's periods.
- **Idempotent commands** (ADR-006): initiation, finalize, determine,
  settle and reversal all carry durable `idempotency_key` + payload
  fingerprints; replay returns the stored result, a reused key with a
  different payload answers `409 conflict`.
- **Derived surfaces**: `GET|POST /api/share-returns`,
  `GET /api/share-returns/{id}`, `POST …/finalize`, `POST …/cancel`,
  `POST /api/share-returns/{id}/entitlements`,
  `GET /api/share-return-entitlements` (beneficiary/due-state
  filtered), `POST /api/share-return-entitlements/{id}/determine` +
  `/cancel` + `/settlements`,
  `POST /api/share-return-settlements/{id}/reverse`. Lists are
  server-paginated; a derived `dueState`
  (`undetermined|not_due|due|overdue|settled|cancelled`) is computed at
  the API boundary — no rules engine yet.
- **Permissions**: `share_returns.read`, `share_returns.manage` —
  granted to `Sistem Yöneticisi` by migration 0011; every route is
  backend-authorized and every mutation is CSRF-protected.
- **Audit**: `share_return_requested`, `share_return_finalized`,
  `share_return_cancelled`, `share_return_entitlement_recognized`,
  `share_return_entitlement_determined`,
  `share_return_entitlement_cancelled`,
  `share_return_settlement_posted`,
  `share_return_settlement_reversed` land in `security_events`; share
  lifecycle milestones record `return_requested` /
  `return_finalized` / `return_cancelled` share events.
- **Frontend**: `/hisse-iadeleri` (filtered, paginated list with
  outstanding totals), `/hisse-iadeleri/yeni` (active-share picker +
  effective-date + preview), `/hisse-iadeleri/{id}` (finalize with
  explicit principal/profit specs, determine an undetermined right,
  cancel, settle against a financial account with remaining-aware
  client guard, reverse). Share detail gains a return-request banner
  and `return_pending`/`closed` badges; shareholder detail shows the
  return cases. Turkish-first i18n; tr-TR money inputs go through
  `parseTryInput`; NULL amounts render as "Henüz belirlenmedi".
- **E2E**: `node scripts/e2e-step011.mjs` drives the real stack —
  UI-initiated return (zero money moves), UI finalize with a
  determined principal + undetermined profit right, share closure,
  partial UI settlement (exactly one `share_return_settlement`
  outflow; income/expense/payment/transfer totals untouched), UI
  reversal with history preserved.
- **Boundary**: no formula engine, no exit valuation, no indexation,
  no profit distribution, no approval workflow, no automatic payment
  plan — deferred (docs/10 open decisions, docs/17).

## Shareholder Credit / Excess Payment (STEP-009, docs/05 + docs/19)

The money-who boundary. **Money location is not credit ownership: a
Shareholder Credit ("Fazla Ödeme") is an entitlement over a Payment's
explicitly assigned remainder — it is not new cash and it never creates
an Account Movement. A credit application is not a Payment. Credit
balance is never a stored number: it is `SUM(active credits) −
SUM(active applications)`, always derived.**

- **`shareholder_credits`** — the origin record: `source_payment_id` +
  exactly one beneficiary `shareholder_id` + `credit_number` (identity,
  also the FIFO order). The beneficiary is always chosen explicitly —
  never inferred from payer, guardian or Family. `SUM(credits) <=
payment remainder` is enforced under the Payment row lock.
- **`credit_applications`** — settlement history: which credit consumed
  which Assessment, `mode` `automatic | manual`. Applications are not
  Payments and create no movements; the Assessment's settled amount is
  `SUM(active allocations) + SUM(active applications)`.
- **Automatic offset**: assigning a credit sweeps the beneficiary's
  existing open Assessments oldest-due-first; generating new
  Assessments sweeps the beneficiary's held credit FIFO
  (`credit_number` ASC) inside the same transaction — deterministic
  and explainable (docs/05).
- **Manual apply**: `POST /api/assessments/{id}/credit-applications`
  consumes the Assessment's own debtor's credits FIFO — a manual
  command may span multiple credit rows, the idempotency key lands on
  the first inserted row only (UNIQUE key).
- **Status-based reversal** (docs/19): credits and applications flip
  `active → reversed` with actor/time/reason; originals survive. An
  unconsumed credit reversal restores the Payment remainder; a consumed
  credit cannot be reversed and also blocks the source Payment's
  reversal (`409`) — money ownership is never silently rewritten.
- **Derived surfaces**: `GET /api/shareholders/{id}/credits` (ledger:
  summary + origins + applications), `GET|POST
/api/payments/{id}/credits`, `POST /api/credits/{id}/reverse`,
  `GET|POST /api/assessments/{id}/credit-applications`,
  `POST /api/credit-applications/{id}/reverse`. Payment detail exposes
  its sourced credits; unallocated amount subtracts assigned credit.
- **Idempotent commands** (ADR-006): assign + manual apply carry
  durable `idempotency_key` + payload fingerprint — same key + same
  payload replays, different payload answers `409 conflict`; a
  first-writer race resolves via replay lookup (unique-violation →
  re-read).
- **Permissions**: `credits.read`, `credits.manage` — granted to
  `Sistem Yöneticisi` by migration 0009.
- **Frontend**: credit assign panel + origin list on
  `/tahsilatlar/{id}`, credit ledger on `/hissedarlar/{id}`, mahsup
  history + manual apply + reversal on `/tahakkuklar/{id}`, member
  credit column on `/aileler/{id}`. Turkish-first i18n.
- **E2E**: `node scripts/e2e-step009.mjs` drives the real stack through
  payment remainder → UI credit assign → auto-offset on generation →
  application reversal → manual apply → consumed-credit reversal
  blockade — asserting zero extra account movements throughout.
- **Boundary**: no refund/disbursement, no credit transfer between
  shareholders, no interest/indexation/expiry — deferred.

## Backup & Disaster Recovery (BACKUP-READINESS-001, ADR-013)

PostgreSQL logical-backup toolkit plus a proven restore path. Operator
procedure: [docs/30-BACKUP-OPERATIONS-RUNBOOK.md](docs/30-BACKUP-OPERATIONS-RUNBOOK.md).

- **Artifacts**: `kooperatif_YYYYMMDDTHHMMSSZ_<rand8>.dump`
  (`pg_dump --format=custom --no-owner --no-privileges`) + a sidecar
  `.manifest.json` (schema version, SHA-256, pg/git versions, migration
  ledger, extension list). Publication is atomic — a failed run never
  leaves a file that looks like a completed backup.
- **Tooling**: pg binaries run inside the pinned `postgres:17-alpine`
  image (`KOOPERATIF_BACKUP_TOOLING=docker`, default) so the client
  major always matches the server; `host` mode uses PATH binaries.
  Credentials travel only via `PGPASSWORD` environment — never argv,
  logs or manifests.
- **Commands**: `pnpm backup:create` · `pnpm backup:verify -- <name>`
  (checksum + manifest + archive TOC) · `pnpm backup:restore --
--artifact <n> --target-db <db> --confirm <db>` (Level-1 gate first;
  refuses the source DB, unsafe names, mismatched confirm, non-empty
  targets without explicit flags; single-transaction restore) ·
  `pnpm backup:retention` (ADR-013 daily 30d / monthly 12mo / yearly
  anchor; dry-run by default, `--apply` to delete; the newest artifact
  is never a deletion candidate) · `pnpm backup:status`
  (`HEALTHY`/`STALE`/`FAILED`/`NEVER_RUN`, non-zero exit for monitors) ·
  `pnpm backup:drill` (full source→backup→fresh-restore→verify proof on
  isolated databases) · `pnpm backup:failure-test` (corruption,
  tampering, missing artifacts, unsafe targets) · `pnpm backup:test`
  (pure-logic unit tests).
- **Status**: `backups/status.json` records attempts/success/verify/
  restore/drill/retention — it lives outside PostgreSQL so it survives
  database loss. `backups/` is gitignored; never commit dumps.
- **Boundary**: this is the _technical_ recoverability layer.
  Production policy requires environment provisioning beyond it: WAL
  archiving + PITR (RPO ≤ 15 min), an encrypted off-site copy (3-2-1),
  object-storage protection for uploaded files, scheduled execution and
  alert wiring (ADR-013; runbook §2).

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
pnpm backup:test       # backup unit tests (pure logic, no Docker)
pnpm backup:drill      # full recovery drill on real PostgreSQL (Docker)
pnpm backup:failure-test  # backup failure-injection tests (Docker)
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
- Obligation + money-received + financial-account + income/expense
  schema now exists (STEP-006/007/008/010); Ledger/Receipt/Collection
  Session and the investment surfaces intentionally do not — they belong
  to later reviewed STEPs (ADR-003/ADR-004).
