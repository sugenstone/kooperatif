# ADR-009 --- Background Jobs, Scheduler and Transactional Outbox

**Status:** Accepted

## Decision

Initially use a PostgreSQL-backed durable job queue plus Rust worker. Do
not introduce Redis, RabbitMQ or Kafka without demonstrated need and a
new ADR.

Use a Transactional Outbox for reliable post-commit domain/event
delivery.

## Authority

Jobs are not the source of financial truth. Due dates, rights and
transactions remain authoritative domain data.

Workers/jobs must be idempotent and retry-safe. Failed jobs are
observable and retained for investigation/retry according to policy.

Outbox records are committed atomically with the authoritative
transaction and can drive WebSocket/event delivery after commit.
