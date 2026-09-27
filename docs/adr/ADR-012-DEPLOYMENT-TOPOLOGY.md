# ADR-012 --- Deployment Topology

**Status:** Accepted

## Decision

Initial production deployment uses a Docker Compose based
single-node/VDS topology with: - reverse proxy and HTTPS/TLS; -
SvelteKit web; - Rust/Axum API; - WebSocket endpoint; - private
PostgreSQL; - separate Rust worker process; - private object storage
integration; - encrypted off-site backups.

Prefer same-origin routing for web/API/WebSocket where practical.

## Security

PostgreSQL and worker services are not publicly exposed.

## Deployment

CI produces tested deployable artifacts/images. Production migrations
are a controlled deployment step, followed by health/readiness
verification.

## Evolution

Components may later move to separate hosts/instances and scale
independently without changing domain semantics.
Kubernetes/microservices are not initial requirements.
