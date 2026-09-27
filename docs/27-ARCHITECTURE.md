# 27 --- Architecture

## Status

High-level architecture direction. Critical choices requiring
comparison/evidence must be recorded as ADRs.

## Stack

-   SvelteKit + TypeScript
-   shadcn-svelte
-   Rust + Axum
-   PostgreSQL
-   Docker-supported development
-   monorepo

## Architectural style

**Modular Monolith** initially.

Reasons: - strong domain boundaries without distributed-system
overhead; - transactional financial workflows; - simpler deployment; -
future extraction remains possible if justified.

Do not create microservices without an approved ADR.

## Backend modules

Candidate modules: - auth; - organizations; - authorization; -
parties; - families; - shareholders; - shares; - periods; -
collections; - finance; - investments; - real_estate; -
business_holdings; - governance; - documents; - social_aid; - audit; -
notifications.

Module boundaries should prevent arbitrary cross-domain table
manipulation.

## Frontend

SvelteKit application organized by feature/domain with shared UI
primitives.

shadcn-svelte is primary component foundation.

i18n from start, default `tr-TR`.

## Contracts

Shared/validated API contract approach through `packages/contracts` or
equivalent.

## Financial architecture

Before core financial implementation, an ADR must choose/evaluate a
ledger/double-entry-inspired internal architecture.

No mutable-balance-only design is authorized.

## Real-time

Collection Session requires real-time update capability.

WebSocket is selected by Accepted ADR-005.

Real-time transport is not authoritative transaction processing.

## Background jobs

Needed eventually for: - notifications; - due-date processing; -
scheduled policy activation if applicable; - document/export jobs if
heavy; - maintenance.

PostgreSQL-backed durable jobs + Rust worker + Transactional Outbox are
selected by Accepted ADR-009.

## Files

Use private file/object storage abstraction selected by ADR/deployment
design.

## Documents

Centralized rendering architecture selected by ADR.

Do not render unrelated PDFs independently in page components.

## Observability

Production architecture should include: - structured logs; - correlation
IDs; - health/readiness; - error monitoring; - metrics where useful.

Do not log secrets/sensitive payloads.

## Environments

At minimum: - local/dev; - test/CI; - production.

Configuration/secrets remain environment-specific.

## Deployment

Exact hosting is not decided here.

Production design must include: - TLS; - DB security; - backup; -
migrations; - health checks; - rollback/forward-fix plan.

## Scalability

Correctness first.

Optimize after measuring: - indexes; - pagination; - materialized/cached
reporting; - background exports; - real-time fanout.

Do not sacrifice financial integrity for premature optimization.
