#!/usr/bin/env bash
# Database/migration verification gate (STEP-001).
#
# Proves, against a CLEAN database:
#   1. the Docker Compose PostgreSQL service starts healthy;
#   2. the embedded migrations apply via the server's `migrate` subcommand;
#   3. re-running the migration command is a no-op (idempotency);
#   4. the DB-gated Rust integration tests pass against that database.
#
# Deterministic by design: readiness is polled via the container health
# state, never via fixed sleeps. The verification database is dropped and
# recreated on every run.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${ROOT_DIR}/docker/compose.yaml"
SERVER_MANIFEST="${ROOT_DIR}/apps/server/Cargo.toml"

POSTGRES_USER="${POSTGRES_USER:-kooperatif}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-kooperatif_dev_password}"
POSTGRES_PORT="${POSTGRES_PORT:-5494}"
VERIFY_DB="kooperatif_verify"
VERIFY_URL="postgres://${POSTGRES_USER}:${POSTGRES_PASSWORD}@localhost:${POSTGRES_PORT}/${VERIFY_DB}"

log() { printf '[db-verify] %s\n' "$*"; }

command -v docker >/dev/null || { log "ERROR: docker is not available"; exit 1; }

log "validating compose configuration"
docker compose -f "${COMPOSE_FILE}" config --quiet

log "starting postgres"
docker compose -f "${COMPOSE_FILE}" up -d postgres

# Poll the container health state (deterministic readiness wait).
log "waiting for postgres to become healthy"
container_id="$(docker compose -f "${COMPOSE_FILE}" ps -q postgres)"
status="unknown"
for _ in $(seq 1 60); do
  status="$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}unknown{{end}}' "${container_id}")"
  [ "${status}" = "healthy" ] && break
  sleep 1
done
[ "${status}" = "healthy" ] || { log "ERROR: postgres did not become healthy (status: ${status})"; exit 1; }
log "postgres is healthy"

log "sweeping per-test databases from failed runs (kooperatif_test_*)"
for leftover in $(docker compose -f "${COMPOSE_FILE}" exec -T postgres \
  psql -U "${POSTGRES_USER}" -d postgres -t -A \
  -c "SELECT datname FROM pg_database WHERE datname LIKE 'kooperatif_test_%';"); do
  docker compose -f "${COMPOSE_FILE}" exec -T postgres \
    psql -U "${POSTGRES_USER}" -d postgres -c "DROP DATABASE IF EXISTS ${leftover} WITH (FORCE);" >/dev/null
done

log "recreating clean verification database ${VERIFY_DB}"
# WITH (FORCE) drops stray connections (e.g. a locally running dev
# server) so the gate is reproducible without manual cleanup.
docker compose -f "${COMPOSE_FILE}" exec -T postgres \
  psql -U "${POSTGRES_USER}" -d postgres -v ON_ERROR_STOP=1 \
  -c "DROP DATABASE IF EXISTS ${VERIFY_DB} WITH (FORCE);" \
  -c "CREATE DATABASE ${VERIFY_DB};"

log "applying migrations to the clean database"
KOOPERATIF_DATABASE_URL="${VERIFY_URL}" \
  cargo run --manifest-path "${SERVER_MANIFEST}" -- migrate

log "re-running migrations (must be a no-op)"
KOOPERATIF_DATABASE_URL="${VERIFY_URL}" \
  cargo run --manifest-path "${SERVER_MANIFEST}" -- migrate

applied="$(docker compose -f "${COMPOSE_FILE}" exec -T postgres \
  psql -U "${POSTGRES_USER}" -d "${VERIFY_DB}" -t -A \
  -c "SELECT count(*) FROM _sqlx_migrations;")"
log "migrations recorded: ${applied}"
[ "${applied}" -ge 1 ] || { log "ERROR: no migrations were recorded"; exit 1; }

log "running DB-gated integration tests (database + full auth security matrix)"
KOOPERATIF_TEST_DATABASE_URL="${VERIFY_URL}" \
  cargo test --manifest-path "${SERVER_MANIFEST}" -- --nocapture

log "database verification PASSED"
