# Kooperatif

Production-grade cooperative management system: shareholders, individually
tracked shares, period obligations, collections, financial accounts,
investments, assets, governance, versioned policies, reporting, and a
financially isolated Social Aid subsystem.

**This repository is at the STEP-001 stage: repository/architecture
baseline only.** No domain functionality is implemented yet. See
[docs/29-IMPLEMENTATION-ROADMAP.md](docs/29-IMPLEMENTATION-ROADMAP.md) —
implementation proceeds one reviewed STEP at a time.

The authoritative project specification lives in
[docs/](docs/) (package `kooperatif-agent-spec-v0.7`), with binding
architecture decisions in [docs/adr/](docs/adr/). Start with
[docs/00-PROJECT-CHARTER.md](docs/00-PROJECT-CHARTER.md),
[docs/15-INVARIANTS.md](docs/15-INVARIANTS.md) and
[docs/28-ADR-INDEX.md](docs/28-ADR-INDEX.md). Working rules for agents:
[AGENTS.md](AGENTS.md).

The application architecture baseline is decided by
[ADR-001](docs/adr/ADR-001-APPLICATION-ARCHITECTURE-BASELINE.md):
a **modular monolith** — SvelteKit web, Rust/Axum API, one PostgreSQL
database, a future Rust worker sharing the backend library — in this
monorepo.

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
