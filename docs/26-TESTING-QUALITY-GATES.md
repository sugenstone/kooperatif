# 26 --- Testing and Quality Gates

## Purpose

Define evidence required before an implementation step can close.

## Test layers

### Rust unit tests

Pure domain rules, calculations, state transitions.

### Backend integration tests

API, auth, authorization, transactions, PostgreSQL behavior.

### Database tests

Constraints, migrations, concurrency-sensitive invariants.

### Frontend tests

Components, forms, localization, permission-aware presentation.

### E2E

Critical real workflows through Svelte UI + Rust backend + PostgreSQL.

## Financial invariant tests

Mandatory for implemented invariants.

Examples: - Payment cannot allocate more than available value; - one
family Payment creates one cash inflow; - old excess applied is not new
cash; - reversal preserves original and restores defined financial
effect; - Share cannot have overlapping active owners; - cross-tenant
access fails.

## Concurrency tests

High priority for: - Payment posting; - duplicate/idempotent
submission; - simultaneous allocation; - document number generation; -
Share ownership transition; - balance/materialization updates.

## Authorization tests

Test both allowed and denied paths.

Test scope, not only Role names.

## Migration tests

From supported baseline: - migration applies; - constraints work; - data
preserved; - application starts; - relevant tests pass.

## Localization tests

Critical user-facing flows must not leak missing translation keys/raw
technical errors.

Default Turkish (`tr-TR`) must work.

## E2E critical paths

Eventually include: - create/find Shareholder; - Family lookup; -
create/open Period; - assessment generation; - individual payment; -
family payment; - overpayment handling; - Collection Session live
update; - reversal/correction; - Share transfer; - Share return; -
approval; - report/receipt generation; - tenant isolation.

## Quality gates

Exact commands will be bound to repository scripts, but closure should
cover as applicable: - format; - lint; - typecheck; - Rust fmt; -
clippy; - Rust tests; - frontend tests; - DB tests; - build; -
contracts; - E2E; - migration validation; - Docker smoke; -
`git diff --check`; - repository cleanliness; - CI.

## No hidden failures

A closure report must disclose flaky/failing tests rather than rerunning
until green and hiding evidence.

Flakes require classification and follow-up.

## Scope review

Before closure: - inspect changed files; - ensure no unrelated
refactor; - ensure no domain rule invented; - ensure docs/contracts
updated if behavior changed.

## Closure report

Each implementation step report includes: - baseline commit; - scope; -
files changed; - schema changes; - behavior implemented; - invariant
evidence; - test/gate results; - security/authorization evidence where
relevant; - unresolved findings; - final repository status; - explicit
stop.

## Hostile review

For critical milestones, a separate review pass should challenge: -
concurrency; - tenant isolation; - authorization; - financial
reconciliation; - reversal; - stale state; - hidden UI assumptions; -
migration safety.

## Definition of done

"Works on my machine" is not done.

Done means specified behavior + automated evidence + clean scoped diff +
closure report.
