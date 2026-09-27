# ADR-001 --- Application Architecture Baseline

**Status:** Accepted (2026-09-28, STEP-001 repository/architecture review)

**Supersedes:** none (this ADR was intentionally reserved in the v0.7
specification package and is now resolved as required by STEP-001).

## Context

The Kooperatif application is a financially authoritative cooperative
management system (docs/00-PROJECT-CHARTER.md). Correctness, auditability,
transactional integrity and recoverability dominate. The specification fixes
SvelteKit + TypeScript + shadcn-svelte frontend, Rust + Axum backend,
PostgreSQL, Docker-supported deployment, and a monorepo. ADR-002 through
ADR-012 are already Accepted and constrain this baseline.

STEP-001 must establish the architecture without implementing any domain
functionality, and without making ADR-002…ADR-012 harder or impossible.

## Decision

**Modular Monolith** with the concrete boundaries below. "Modular" is a
repository/code discipline with enforced dependency direction — not merely a
folder claim — and "monolith" means one deployable unit per process role,
one database, one codebase per language.

### 1. Process boundary

Deployable processes:

- **web** — SvelteKit (adapter-node) application.
- **api** — Rust/Axum HTTP server (`kooperatif-server` binary, `serve`).
- **migrate** — the same Rust binary with the explicit `migrate` subcommand
  (a controlled deployment step, not a runtime side effect; ADR-012).
- **worker** *(future, ADR-009)* — a second binary target inside the same
  Cargo package/workspace sharing the `kooperatif_server` library. It will
  NOT be a second backend codebase.

No other processes exist. No microservices, no Kubernetes, no
Kafka/RabbitMQ/Redis (ADR-009 chooses PostgreSQL-backed durable jobs).

### 2. Repository structure

``` text
apps/
  web/        SvelteKit + TypeScript + shadcn-svelte (pnpm workspace package)
  server/     Rust Cargo package (lib + api binary); Cargo workspace root
packages/
  contracts/  Shared typed API contract primitives (TypeScript source)
migrations/   Versioned SQL migrations, embedded into the server binary
docker/       Docker Compose for local development infrastructure
scripts/      Verification gates (db-verify.sh, verify.sh)
docs/         Authoritative specification package (v0.7) + ADRs
implementation/  Executed STEP instruction files (audit trail)
```

`packages/ui` from the charter's suggested shape is deliberately deferred:
shadcn-svelte primitives are vendored under
`apps/web/src/lib/components/ui`, and a shared UI package is created only
when a second consumer exists (no speculative structure).

### 3. Backend internal boundaries (domain / application / infrastructure)

Inside the Rust crate, modules are layered with a strict dependency
direction:

``` text
binaries (api main, future worker main)
   ↓ compose
infrastructure (HTTP, PostgreSQL, config, tracing, storage adapters)
   ↓ implements
application (use cases, transaction scripts, ports)
   ↓ depends on
domain (entities, invariants, policies — pure Rust, no I/O)
```

- Domain code has zero framework/database dependencies and is unit-testable
  in isolation.
- Application code depends on domain types and on trait `ports` (e.g. a
  future `StorageAdapter` per ADR-007).
- Infrastructure implements the ports and owns all I/O.
- Domain modules arrive with their own STEPs (identity, parties, shares,
  finance, …). A module owns its routes and its schema objects; direct
  cross-module table writes are prohibited — interaction happens through
  the owning module's API.

STEP-001 materializes only the infrastructure layer (`config`, `db`,
`http`, `observability`); no empty domain/application skeletons are
created.

### 4. Database ownership

- One PostgreSQL database owned exclusively by the backend.
- The schema evolves only through migrations in the repository-root
  `migrations/` directory, embedded into the server binary at compile time
  and applied by the explicit `migrate` subcommand.
- No production runtime auto-migration (ADR-012 controlled deployment
  step).
- Financial schema is deferred to the financial domain STEP (ADR-003/004).
  No authoritative financial value will ever use floating-point types.

### 5. Shared contracts (`packages/contracts`)

- TypeScript contract primitives consumed by the frontend; the Rust side
  mirrors the same shapes via serde. The wire format is the contract —
  both languages' types must match it, and contract changes are
  coordinated + tested (docs/21).
- `DecimalString`: authoritative decimal values cross the API ONLY as
  canonical decimal strings — never JavaScript numbers (ADR-004).
- Today: health/readiness DTOs + error-code foundation + decimal
  primitive. Pagination envelope (ADR-010) and domain DTOs join their own
  STEPs; no elaborate code generator is introduced until it pays for
  itself.

### 6. Background worker boundary (ADR-009, reserved)

The future worker is a sibling binary sharing this crate's library:
domain/application/infrastructure code is written once in the library and
reused. Jobs live in PostgreSQL; the Transactional Outbox table is written
atomically inside domain transactions. No business logic is duplicated in
worker-specific code.

### 7. WebSocket / event delivery boundary (ADR-005, reserved)

Reserved pipeline, not implemented in STEP-001:

``` text
domain transaction
  → transactional outbox (same DB transaction)
  → event dispatcher (worker / api process)
  → WebSocket delivery hub (api process)
  → connected clients / TV mode
```

WebSocket is transport, never financial truth; after reconnect, clients
fetch a fresh authoritative snapshot and then resume events. The HTTP
router is the single delivery surface for both REST and future WebSocket
upgrade routes.

### 8. Document service boundary (ADR-008, reserved)

PDF/XLSX/document rendering is a backend-owned centralized module
consuming the same authoritative data models; the frontend never renders
financial documents ad hoc. The concrete rendering library is a later
implementation spike decision.

### 9. Object storage boundary (ADR-007, reserved)

File metadata lives in PostgreSQL; bytes live behind a storage abstraction
trait with a local-filesystem development adapter and a private
S3-compatible production adapter. Never public-by-default URLs.

### 10. Frontend organization

- SvelteKit routes organized by feature/domain as screens arrive.
- shadcn-svelte is the component foundation; project-owned composites wrap
  it where useful; no competing design system.
- i18n from the start: typed dictionaries under `src/lib/i18n`, default
  and fallback locale `tr-TR`; user-facing strings are never scattered
  hard-coded literals. Locale affects presentation only.

### 11. Testing boundaries

- Rust unit tests: pure logic (config validation today; domain rules in
  later STEPs).
- Rust integration tests: router-level `oneshot` tests without network;
  DB-gated tests behind `KOOPERATIF_TEST_DATABASE_URL` (skip with explicit
  notice when absent).
- Migration verification: deterministic clean-database runs via
  `scripts/db-verify.sh` (Compose PostgreSQL) and the CI database job.
- Frontend: vitest unit + component tests (jsdom), prettier/eslint/
  svelte-check gates.
- E2E through the real UI is a later-step concern.

## Rationale

- A financial system needs multi-table transactional writes (one payment →
  receipt + inflow + many allocations + excess). A modular monolith keeps
  those inside one PostgreSQL transaction without distributed coordination.
- One codebase per language with enforced internal boundaries gives
  audit-friendly reviewability now, and preserves the option to extract
  modules later without domain rewrites.
- Deployment simplicity (Docker Compose single node, ADR-012) matches the
  actual operational scale.

## Alternatives rejected

- **Microservices / per-module databases** — premature; breaks transactional
  financial workflows; explicitly forbidden by docs/27 without an ADR.
- **Kafka/RabbitMQ/Redis for jobs/events** — rejected by ADR-009.
- **Separate worker codebase** — duplicates business logic; rejected by
  STEP-001 §21.
- **JavaScript/TypeScript backend** — contradicts the fixed technology
  direction (Rust + Axum).
- **ORM-first or mutable-balance financial storage** — would make ADR-003
  (immutable double-entry-inspired ledger) impossible; rejected.
- **Speculative `packages/ui` and empty domain folders** — fake
  completeness; rejected by STEP-001 §24.

## Consequences

- Module boundary discipline is enforced at review time (dependency
  direction, no cross-module table writes) until compile-time enforcement
  (e.g. workspace lints or crate splitting) is justified.
- Contracts are synchronized by typed mirroring + tests rather than code
  generation; revisit if drift appears.
- The worker, WebSocket hub, document service and storage adapters all
  have reserved homes and require no architectural change to introduce.
