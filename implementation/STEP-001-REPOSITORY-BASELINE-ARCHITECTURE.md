# STEP-001 — REPOSITORY BASELINE, ARCHITECTURE BOOTSTRAP & ADR-001

Repository:

```text
git@github.com:sugenstone/kooperatif.git
```

Specification package:

```text
kooperatif-agent-spec-v0.7
```

This is the first implementation step of the Kooperatif application.

Do **not** attempt to build the whole application in this step.

Your responsibility is to establish, verify, test, and document the repository/architecture baseline upon which all later implementation steps will depend.

---

# 0. OPERATING MODE

Work as a senior software architect and implementation engineer.

This repository will contain a financial/cooperative management system. Correctness, auditability, deterministic behavior, data integrity, security, maintainability, and recoverability take priority over implementation speed.

Follow this sequence:

```text
INSPECT
→ UNDERSTAND
→ PLAN
→ IMPLEMENT
→ TEST
→ REVIEW
→ REPORT
→ STOP
```

Do not continue to STEP-002.

Do not implement business functionality that belongs to future steps.

Do not invent missing business rules.

If a business decision is unresolved in the specification, preserve it as unresolved.

---

# 1. SPECIFICATION IS AUTHORITATIVE

Before modifying code, locate and read:

```text
AGENTS.md
README.md
docs/*
docs/adr/*
```

Follow the document precedence/routing rules defined by `AGENTS.md`.

The v0.7 specification package is the authoritative project specification.

Accepted ADRs are binding.

In particular, understand the implications of:

```text
ADR-002 Authentication & Session Strategy
ADR-003 Financial Ledger Architecture
ADR-004 Money & Value Representation
ADR-005 Real-Time WebSocket Transport
ADR-006 Idempotency & Concurrency
ADR-007 File/Object Storage
ADR-008 Document Rendering
ADR-009 Background Jobs & Transactional Outbox
ADR-010 Pagination
ADR-011 Observability
ADR-012 Deployment Topology
```

Do not reinterpret or silently replace an Accepted ADR.

---

# 2. DEFAULT PRODUCT LANGUAGE

The application's default product language and locale are:

```text
Turkish
tr-TR
```

Architecture must support future localization.

Do not hard-code user-facing Turkish strings throughout application components in a way that prevents future localization.

Initial fallback/default locale:

```text
tr-TR
```

English support may be scaffolded if appropriate, but building a complete English translation is NOT required in STEP-001.

---

# 3. REQUIRED TECHNOLOGY BASELINE

Target architecture:

```text
Frontend:
SvelteKit
TypeScript
shadcn-svelte

Backend:
Rust
Axum

Database:
PostgreSQL

Deployment:
Docker / Docker Compose
```

The repository should be organized as a maintainable modular monolith.

Do NOT introduce:

```text
microservices
Kubernetes
Kafka
RabbitMQ
Redis
```

unless an Accepted ADR explicitly requires them.

ADR-009 explicitly chooses PostgreSQL-backed durable jobs + Rust worker + Transactional Outbox for the initial architecture.

---

# 4. FIRST ACTION — REPOSITORY FORENSICS

Before changing anything, inspect the repository.

Report:

```text
current branch
HEAD commit
remote
working-tree status
existing files/directories
existing application code
existing configuration
existing Docker setup
existing CI
existing migrations
existing tests
existing documentation
existing dependency manifests
```

Determine whether the repository is:

```text
empty
partially initialized
or already contains implementation
```

Never destroy or overwrite legitimate existing work merely to match this prompt.

If existing implementation conflicts with the specification, document the conflict before changing architecture.

---

# 5. SPECIFICATION INSTALLATION / LOCATION

Ensure the v0.7 specification is available inside the repository in a stable, obvious location.

Prefer the existing intended documentation structure if already established.

At minimum the repository must contain/access:

```text
AGENTS.md
README.md
docs/
docs/adr/
```

Do not duplicate conflicting copies of the specification.

---

# 6. ADR-001 — MODULAR MONOLITH BASELINE

ADR-001 is intentionally unresolved in v0.7.

STEP-001 must evaluate and create:

```text
ADR-001 — Application Architecture Baseline
```

Preferred decision:

```text
Modular Monolith
```

But do not merely write the words "Modular Monolith".

Define the actual architectural boundaries.

At minimum ADR-001 must address:

```text
frontend boundary
backend boundary
domain/application/infrastructure boundaries
database ownership
module dependency direction
shared contracts
background worker boundary
WebSocket/event delivery boundary
document service boundary
object-storage boundary
migration ownership
testing boundaries
```

The architecture must make later domain modules possible without prematurely implementing them.

Likely future modules include:

```text
Identity / Access
Parties
Shareholders
Families
Guardians
Shares
Periods
Assessments
Payments
Allocations
Financial Accounts
Ledger
Investments
Assets
Real Estate
Business Holdings
Governance
Policies
Profit Rights
Social Aid
Documents
Reporting
Audit
```

Do NOT create meaningless empty folders for every future domain simply to claim modularity.

Create only the minimum useful architecture required now.

---

# 7. REPOSITORY STRUCTURE

Evaluate and establish an appropriate repository structure.

A likely direction is:

```text
apps/
  web/
  server/

packages/
  ui/
  contracts/

migrations/

docker/

scripts/

docs/
```

This is guidance, not permission to blindly create directories.

Use the simplest structure that satisfies ADR-001 and future project needs.

Avoid unnecessary workspace complexity.

---

# 8. FRONTEND BOOTSTRAP

Establish the SvelteKit + TypeScript frontend.

Requirements:

```text
SvelteKit
TypeScript strictness appropriate for production
shadcn-svelte foundation
responsive architecture
accessible primitives
tr-TR default locale
future i18n capability
```

Create only enough UI to prove the shell works.

A minimal application shell may include:

```text
application title
basic navigation placeholder
content area
health/backend connectivity indicator if useful
```

Do not design the full dashboard.

Do not implement Shareholder, Payment, Period, Investment, Social Aid, etc. screens yet.

---

# 9. SHADCN-SVELTE RULE

shadcn-svelte is the primary component foundation.

Do not create a competing custom design system.

Project-owned wrapper/composite components are allowed where useful, but low-level primitives should reuse shadcn-svelte patterns/components.

Ensure future theming and accessibility remain possible.

---

# 10. BACKEND BOOTSTRAP

Establish the Rust/Axum backend.

Required baseline:

```text
configuration loading
application startup
structured tracing
request/correlation ID foundation
error handling foundation
health endpoint
readiness endpoint
PostgreSQL connectivity
graceful shutdown
```

Do NOT implement authentication yet unless absolutely required by existing repository state.

ADR-002 defines the future authentication architecture; implementation belongs to a later STEP.

---

# 11. DATABASE BASELINE

Configure PostgreSQL as the production database.

Create the migration mechanism and verify migrations can run against a clean database.

Do not prematurely create the entire domain schema.

Only create infrastructure-level database objects if genuinely required by STEP-001.

Financial schema must wait until the relevant domain STEP because ADR-003 and ADR-004 require careful implementation.

Never use floating-point database types for future authoritative financial values.

---

# 12. LOCAL DEVELOPMENT ENVIRONMENT

Provide a reproducible local environment.

Docker Compose should support at minimum the infrastructure required for development/testing.

Likely:

```text
PostgreSQL
```

Do not introduce unnecessary services.

Application containers may be added if they improve reproducibility, but avoid making local development unnecessarily slow.

---

# 13. CONFIGURATION

Create a clear configuration strategy.

Separate:

```text
development
test
production
```

Never commit secrets.

Provide:

```text
.env.example
```

where appropriate.

Validate required configuration at startup.

Production configuration must fail clearly rather than silently falling back to insecure defaults.

---

# 14. SHARED CONTRACT STRATEGY

Define how frontend/backend API contracts will remain synchronized.

Do not create an elaborate code generator unless justified.

Document the chosen baseline.

The strategy must be compatible with:

```text
Rust backend
TypeScript frontend
typed API contracts
future pagination contracts
exact decimal representation
```

Do not allow future financial decimal values to become lossy JavaScript numbers.

---

# 15. OBSERVABILITY BASELINE

Implement the infrastructure-level subset of ADR-011.

At minimum:

```text
structured backend tracing
request/correlation IDs
health endpoint
readiness endpoint
```

Health and readiness must have distinct semantics.

Example:

```text
/health
```

means process is alive.

```text
/ready
```

means required dependencies such as PostgreSQL are available.

Do not implement a giant metrics platform in STEP-001.

---

# 16. QUALITY TOOLING

Establish deterministic commands for:

```text
format
lint
typecheck
unit tests
build
Rust fmt
Rust clippy
Rust tests
database/migration verification
```

Prefer root-level commands/scripts that make repository validation easy.

A future agent should be able to run one documented verification sequence without guessing commands.

---

# 17. CI BASELINE

If CI does not exist, establish a minimal CI workflow.

It should validate relevant gates such as:

```text
frontend formatting/lint/typecheck/test/build
Rust fmt
Rust clippy
Rust test
migration/database checks where practical
```

Do not create an unnecessarily complicated CI matrix.

---

# 18. TESTING BASELINE

Create enough tests to prove the bootstrap works.

At minimum verify appropriate equivalents of:

```text
backend health
backend readiness
frontend build
frontend basic render
database connectivity/migration
```

Tests must not depend on arbitrary sleeps when deterministic alternatives exist.

---

# 19. SECURITY BASELINE

Do not implement the full security system yet.

However:

```text
no committed secrets
safe default configuration
no public PostgreSQL production exposure
no debug secret leakage
reasonable HTTP security baseline
dependency hygiene
```

must be considered.

Do not weaken future ADR-002 session architecture.

---

# 20. WEBSOCKET ARCHITECTURE

ADR-005 is Accepted.

STEP-001 does NOT need to implement the complete WebSocket business system.

However, ADR-001 must leave a clear architectural boundary for:

```text
domain transaction
→ transactional outbox
→ event dispatcher
→ WebSocket delivery
→ connected clients / TV mode
```

Remember:

```text
WebSocket != source of truth
```

---

# 21. BACKGROUND WORKER ARCHITECTURE

ADR-009 is Accepted.

Do not implement business jobs yet.

ADR-001/repository architecture must allow a Rust worker process sharing appropriate application/domain/infrastructure code without duplicating business logic.

Avoid creating a second independent backend codebase.

---

# 22. FINANCIAL ARCHITECTURE GUARDRAIL

Do not implement the financial ledger in STEP-001.

But architecture must not make ADR-003 impossible.

Future financial writes require:

```text
immutable financial records
double-entry-inspired ledger
exact decimal values
reversals instead of destructive edits
transactional integrity
idempotency
```

Do not introduce mutable `balance` architecture as the financial source of truth.

---

# 23. NO PREMATURE DOMAIN IMPLEMENTATION

Specifically do NOT build complete versions of:

```text
Shareholder management
Family management
Guardian management
Shares
Periods
Assessments
Payments
Collection Sessions
Ledger
Investments
Real Estate
Business Holdings
Governance
Profit Rights
Social Aid
Reports
```

Those will be implemented in controlled later STEPs.

STEP-001 is foundation work.

---

# 24. NO FAKE COMPLETENESS

Do not create:

```text
TODO-filled fake services
empty repository layers
fake APIs
dummy domain entities
unused abstractions
```

merely to make architecture appear complete.

Prefer a small working vertical infrastructure baseline over a large empty skeleton.

---

# 25. DOCUMENTATION

Update repository documentation so a new engineer/agent can determine:

```text
what the project is
required toolchain
how to run it
how to run PostgreSQL
how to configure environment
how to migrate DB
how to run frontend/backend
how to execute quality gates
where specifications live
where ADRs live
what ADR-001 decided
```

---

# 26. VERIFY ADR CONSISTENCY

Before closure, explicitly check that STEP-001 did not contradict any Accepted ADR.

Produce an ADR consistency matrix:

```text
ADR
Relevant to STEP-001?
Implemented / Reserved / Not Yet Applicable
Evidence
Conflict?
```

Any conflict is a blocker.

---

# 27. FINAL QUALITY GATES

Before reporting completion, run all applicable gates.

At minimum report exact results for:

```text
git diff --check

frontend:
format
lint
typecheck
tests
build

backend:
cargo fmt --check
cargo clippy
cargo test

database:
migration/clean database verification

Docker:
compose/config/smoke verification where applicable
```

Do not say "tests pass" without giving actual command/result evidence.

If a gate cannot run, explain exactly why.

---

# 28. REPOSITORY HYGIENE

Before closure:

```text
git status
git diff
git diff --check
```

Check for:

```text
generated junk
temporary logs
test artifacts
secrets
.env files
unexpected binaries
editor files
accidental dependency caches
```

Do not commit generated junk.

---

# 29. COMMIT POLICY

Do not create a commit until:

```text
implementation complete
quality gates pass
diff reviewed
scope verified
```

If the operating environment permits commits and the repository workflow expects one, create one coherent STEP-001 commit.

Suggested commit message:

```text
chore: establish application architecture baseline
```

Do not push unless explicitly authorized by the surrounding execution environment/user workflow.

If you do not commit, clearly report the repository state.

---

# 30. REQUIRED CLOSURE REPORT

At completion produce:

```text
# STEP-001 — CLOSURE REPORT
```

with these sections:

## A. Baseline
- branch
- starting HEAD
- remote
- initial working-tree state

## B. Specification Review
- documents read
- relevant constraints discovered

## C. ADR-001 Decision
- exact decision
- module boundaries
- dependency direction
- rationale
- alternatives rejected

## D. Repository Structure
Show final relevant tree.

## E. Frontend Baseline
What was created/configured.

## F. Backend Baseline
What was created/configured.

## G. Database Baseline
PostgreSQL/migration approach.

## H. Shared Contracts
Chosen strategy.

## I. Observability
Tracing/request ID/health/readiness evidence.

## J. Local Development
How to run the system.

## K. Docker
Services and verification.

## L. CI
Workflow/gates established.

## M. Tests
Exact test counts/results where available.

## N. Quality Gates
Every command and result.

## O. ADR Consistency Matrix
ADR-002 through ADR-012.

## P. Security Review
Relevant baseline findings.

## Q. Files Changed
Every changed/created file grouped by purpose.

## R. Repository Hygiene
Final status and `git diff --check`.

## S. Remaining Risks / Deferred Work
Only genuine deferred work.

## T. Final Result

End with exactly one of:

```text
READY FOR STEP-001 REVIEW
```

or

```text
STEP-001 BLOCKED
```

If blocked, explain the blocker.

---

# 31. STOP CONDITION

After producing the Closure Report:

**STOP.**

Do not start:

```text
STEP-002
authentication implementation
Shareholder implementation
financial implementation
dashboard implementation
```

Wait for review.

The reviewer will inspect STEP-001 before authorizing the next step.