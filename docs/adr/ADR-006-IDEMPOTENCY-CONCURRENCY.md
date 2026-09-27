# ADR-006 --- Idempotency and Concurrency

**Status:** Accepted

## Decision

Critical financial commands use PostgreSQL transactions, narrowly scoped
locking where required, optimistic stale-state/version checks, and
idempotency keys.

## Goals

Prevent: - double posting from double-click/retry; - two operators
over-allocating the same debt; - duplicate compensating/reversal
effects; - stale previews silently posting invalid allocations.

Do not globally lock the cooperative. Concurrency protection should be
scoped to affected obligations/resources.

A legitimate second Payment remains possible; duplicate-request
protection must not prohibit genuine additional payments.
