-- M1-P0 — Tenant kernel (REQ-024.1/024.3 foundation, PD-01/PD-02,
-- M1-K1..K7 approved).
--
-- Additive only: two new tables plus nullable columns on existing
-- identity tables. NO business/financial table is touched — their
-- cooperative_id retrofit is P2. RLS is deliberately NOT enabled here
-- (P4, after tenant ownership of every business table is migrated and
-- validated; M1-K6/GATE E).
--
-- Business-enablement gate (M1-P0 §5): a cooperative starts
-- business_enabled=false. Tenant context can only be established for
-- business_enabled=true cooperatives, so a second cooperative can never
-- reach business modules whose tenant retrofit is still pending (P2).
-- The flag flips in a later audited step, not here.

CREATE TABLE cooperatives (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name             TEXT NOT NULL,
    legal_name       TEXT,
    status           TEXT NOT NULL DEFAULT 'provisioning'
                     CHECK (status IN ('provisioning', 'active', 'suspended', 'archived')),
    -- Exactly one cooperative may carry the bootstrap marker: the
    -- initial cooperative that receives the pre-M1 single-cooperative
    -- data set during the P2 backfill. Enforced by the partial unique
    -- index below — bootstrap can never silently duplicate.
    is_bootstrap     BOOLEAN NOT NULL DEFAULT false,
    -- M1-P0 safety gate: true only for cooperatives whose business
    -- modules may legitimately serve tenant-scoped traffic. New
    -- cooperatives are created false and stay unreachable for
    -- tenant-scoped operations until the gate is lifted.
    business_enabled BOOLEAN NOT NULL DEFAULT false,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX cooperatives_name_uq
    ON cooperatives (lower(btrim(name)));

-- At most one bootstrap cooperative may ever exist.
CREATE UNIQUE INDEX cooperatives_bootstrap_uq
    ON cooperatives (is_bootstrap) WHERE is_bootstrap;

-- PD-02: one global user, many cooperative memberships; membership
-- status is the per-request access gate (active/suspended/ended).
-- Rows are never deleted — history is required for audit attribution.
CREATE TABLE cooperative_memberships (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cooperative_id     UUID NOT NULL REFERENCES cooperatives (id),
    user_id            UUID NOT NULL REFERENCES users (id),
    status             TEXT NOT NULL DEFAULT 'active'
                       CHECK (status IN ('active', 'suspended', 'ended')),
    joined_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at           TIMESTAMPTZ,
    created_by_user_id UUID REFERENCES users (id),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (cooperative_id, user_id)
);

CREATE INDEX cooperative_memberships_user_idx
    ON cooperative_memberships (user_id, status);

-- Session carries the UX-level default cooperative (M1-K1: security
-- context still travels per request and is re-validated server-side;
-- this column is never trusted on its own).
ALTER TABLE user_sessions
    ADD COLUMN active_cooperative_id UUID REFERENCES cooperatives (id);

-- Tenant-aware audit attribution. NULL = platform-scope event
-- (login, user lifecycle); non-NULL = cooperative-scope event.
ALTER TABLE security_events
    ADD COLUMN cooperative_id UUID REFERENCES cooperatives (id);

CREATE INDEX security_events_cooperative_idx
    ON security_events (cooperative_id, occurred_at DESC);
