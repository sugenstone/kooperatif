#!/usr/bin/env bash
# Full repository verification sequence (STEP-001 quality gates).
#
# Runs every gate a closure report must evidence. Exit code is non-zero
# if any gate fails; each gate prints its own exact command.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB_DIR="${ROOT_DIR}/apps/web"
SERVER_MANIFEST="${ROOT_DIR}/apps/server/Cargo.toml"

log() { printf '\n[verify] === %s ===\n' "$*"; }

log "frontend: format check"
pnpm --dir "${WEB_DIR}" exec prettier --check .

log "frontend: lint"
pnpm --dir "${WEB_DIR}" run lint

log "frontend: typecheck (contracts + web)"
pnpm --dir "${ROOT_DIR}" run check

log "frontend: tests"
pnpm --dir "${WEB_DIR}" run test

log "frontend: production build"
pnpm --dir "${WEB_DIR}" run build

log "backend: cargo fmt --check"
cargo fmt --manifest-path "${SERVER_MANIFEST}" --check

log "backend: cargo clippy (-D warnings)"
cargo clippy --manifest-path "${SERVER_MANIFEST}" --all-targets -- -D warnings

log "backend: cargo test"
cargo test --manifest-path "${SERVER_MANIFEST}"

log "database: clean-database migration verification (Docker)"
bash "${ROOT_DIR}/scripts/db-verify.sh"

log "git diff --check"
git -C "${ROOT_DIR}" diff --check

log "ALL GATES PASSED"
