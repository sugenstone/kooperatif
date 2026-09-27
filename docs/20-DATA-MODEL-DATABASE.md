# 20 --- Data Model and Database Principles

## Database

PostgreSQL.

Schema evolves through migrations committed to repository.

## Modeling principles

-   explicit foreign keys;
-   database constraints for critical invariants;
-   timestamps with clear timezone semantics;
-   immutable technical IDs;
-   separate business identifiers;
-   historical tables/intervals where current-state overwrite would lose
    meaning;
-   avoid polymorphic blobs for core financial relations when relational
    structure is known.

## IDs

Use robust technical IDs (UUID/approved equivalent).

Business numbers are separate: - Family Sequence Number; - Share
Number; - Payment/Receipt Number; - Document Number; - Decision Number.

## Money

Never use floating-point (`float`/`double`) for authoritative money.

Use an exact representation such as PostgreSQL `numeric` with
domain-appropriate scale or an approved integer-minor-unit model.

The exact money/value-type architecture must be decided before financial
migrations.

## Gold/commodity values

Do not assume fiat-money precision/model is sufficient.

Define unit, scale, conversion semantics and rounding before
implementation.

## Time

Persist authoritative timestamps in a timezone-safe representation.

Business dates such as Period date, Due Date and Decision Effective Date
may be date-only where semantics require.

Presentation defaults to `tr-TR`.

## Soft delete

Do not use a generic `deleted_at` as a substitute for real domain
lifecycle.

Use archive/close/reversal semantics where domain meaning exists.

## Uniqueness

Examples requiring scoped uniqueness may include: - Family Sequence
Number within Cooperative; - Share Number within Cooperative; - business
document number within its numbering scope.

Exact constraints belong to migrations/spec.

## Ownership intervals

Database must enforce or robustly protect against overlapping active
Share ownership intervals.

## Transactions

Financial posting uses PostgreSQL transactions.

Critical read-check-write flows must be concurrency-safe.

## Optimistic/pessimistic concurrency

Choose per workflow.

Collection/payment posting must prevent stale previews from violating
debt/allocation invariants.

## Idempotency

External/user retry-prone commands such as Payment posting and Reversal
should support idempotency strategy.

## Derived values

Avoid independently editable duplicate totals.

Materialized/cached totals are allowed only with explicit
source-of-truth and reconciliation strategy.

## JSON

JSONB may be used for extensible metadata/configuration but should not
hide core relational financial entities.

Policy parameters may use structured JSON only with versioned
validation/schema strategy.

## Migrations

Each migration must: - be deterministic; - preserve existing data; -
include constraints/indexes; - be testable; - avoid silent destructive
conversion.

Production migration rollback/recovery strategy belongs to
deployment/backup docs.

## Indexing

Design indexes from real query paths: - tenant + business number; -
shareholder/family search; - active ownership; - Period assessments; -
Payment date/session; - account movements; - audit filters.

Do not prematurely add random indexes without query justification.

## Personal data

Store only data required by product/legal needs.

Sensitive fields require access control and logging/privacy
consideration.

## Database tests

Test: - uniqueness; - foreign keys; - tenant boundaries; - ownership
overlap prevention; - financial reconciliation constraints where
DB-enforceable; - idempotency/concurrency behaviors; - migration
correctness.
