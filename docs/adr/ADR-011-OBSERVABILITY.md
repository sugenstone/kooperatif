# ADR-011 --- Observability

**Status:** Accepted

## Decision

Use structured Rust tracing/logging, correlation/request IDs,
health/readiness endpoints, an error-monitoring integration boundary and
baseline operational metrics.

## Correlation

A request/correlation ID should trace a business operation across HTTP,
Payment/domain handling, ledger work, outbox/job processing and relevant
event delivery.

Safe support reference IDs may be shown to users on technical failures.

## Separation

Observability logs are not Audit records. Audit is durable business
history; observability describes technical execution.

Never log secrets, session tokens, passwords or unnecessary sensitive
document/personal content.
