# 21 --- API and Contract Standards

## Purpose

Standardize communication between SvelteKit frontend and Rust backend.

## Contract ownership

API contracts should be explicit and shareable/validated where practical
through the repository's contracts package.

Do not let frontend infer undocumented response shapes.

## General response behavior

Use consistent: - identifiers; - timestamps; - pagination; -
filtering; - sorting; - validation errors; - authorization errors; -
conflict responses.

## Errors

User-facing frontend receives stable machine error codes plus
localizable context.

Do not expose raw database/Rust stack errors.

Candidate categories: - validation; - unauthenticated; - forbidden; -
not found; - conflict/stale state; - approval required; - idempotency
conflict; - business rule violation; - internal error.

## Validation

Backend is authoritative.

Frontend validation improves UX but never replaces backend validation.

## Pagination

Large collections use standardized pagination.

Exact cursor vs offset strategy is an architecture decision; avoid
endpoint-by-endpoint inconsistency.

## Filtering/sorting

Use explicit allowlists/typed parameters.

Do not expose arbitrary SQL-like query construction.

## Money/value serialization

Do not serialize authoritative decimal values through lossy JavaScript
floating-point assumptions.

Define decimal contract representation before financial API
implementation.

## Dates/times

API uses unambiguous machine formats.

Localization occurs in presentation layer.

## Commands vs reads

Critical state transitions should use explicit command-like
endpoints/actions rather than generic PATCH of a `status` field.

Examples: - post Payment; - reverse Payment; - transfer Share; - approve
Share Return.

## Idempotency

Retry-prone financial commands should accept/use an idempotency
mechanism.

## Concurrency conflicts

If preview/source state changed, return a specific conflict/stale-state
result so UI can recalculate.

## Authorization

API evaluates tenant + permission + scope + workflow guards.

Avoid leaking existence of cross-tenant resources.

## Audit correlation

Requests may carry/generate correlation IDs used for audit/support.

## Real-time events

Collection-session real-time payloads should be event contracts, not raw
database row dumps.

Examples: - payment_posted; - assessment_updated; -
financial_account_balance_changed; - collection_session_totals_changed.

Exact transport is ADR-controlled.

## Versioning

Avoid premature public API versioning, but breaking contract changes
must be coordinated and tested.

## Contract tests

Backend/frontend contracts require automated validation/tests.
