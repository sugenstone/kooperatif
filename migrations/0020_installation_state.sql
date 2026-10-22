-- ============================================================================
-- M1-P1-005 — Durable bootstrap lifecycle marker
-- ============================================================================
--
-- P1-c1 derived "was tenant bootstrap ever completed" from the ABSENCE
-- of rows (no cooperatives, no cooperative-attributed security_events).
-- Negative evidence is fragile: a previously bootstrapped database whose
-- bootstrap row was deleted, disabled, or flag-stripped — with the audit
-- trail lost, truncated, or never written — would silently reopen the
-- pre-tenant global-authorization lane.
--
-- `installation_state` is a dedicated, single-row installation record:
--
--   - Written transactionally by `bootstrap_initial_cooperative` in the
--     same transaction that creates the bootstrap cooperative — the
--     marker can never outlive its own write.
--   - `bootstrap_cooperative_id REFERENCES cooperatives(id)` makes the
--     bootstrap cooperative UNDELETABLE while referenced (FK NO ACTION
--     rejects the delete) — the marker and the row protect each other.
--   - No application endpoint, RBAC API, or CLI path writes or deletes
--     this table: it is not removable through ordinary cooperative
--     administration. Direct SQL remains out of threat scope (anyone
--     who can arbitrary-DELETE rows can arbitrary-UPDATE them).
--   - `tenant_bootstrapped_at` records when tenancy was engaged for
--     future audit/reporting.
--
-- Upgrade path: installations that already ran bootstrap before this
-- table existed get the marker backfilled in this same migration, so
-- post-migration `bootstrap_cooperative_id IS NULL` can only mean
-- genuinely-never-bootstrapped (or deliberate tampering — which fails
-- closed via the never_bootstrapped residue checks).
--
-- ============================================================================

CREATE TABLE installation_state (
    -- Enforce exactly one row ever existing.
    singleton                BOOLEAN     NOT NULL DEFAULT TRUE PRIMARY KEY
                                         CHECK (singleton),
    bootstrap_cooperative_id UUID        REFERENCES cooperatives (id),
    tenant_bootstrapped_at   TIMESTAMPTZ
);

INSERT INTO installation_state (singleton) VALUES (TRUE);

-- Backfill for already-bootstrapped installations (upgrade path). The
-- bootstrap row's own created_at is the best available engagement
-- timestamp; a fresh bootstrap stamps now() instead.
UPDATE installation_state s
SET bootstrap_cooperative_id = c.id,
    tenant_bootstrapped_at   = c.created_at
FROM cooperatives c
WHERE c.is_bootstrap;
