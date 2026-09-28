-- STEP-002: identity, authentication and server-side session foundation.

-- Internal application Users (operators). A User is NOT a Shareholder
-- (docs/01-DOMAIN-GLOSSARY.md); no FK between them exists or is implied.
-- `username` stores the normalized login identifier (see
-- src/auth/identity.rs: ASCII-lowercased, 3..=64 chars). Uniqueness is
-- enforced by the DATABASE, and the unique index is an expression index
-- over lower(username) so uniqueness holds under the normalization rule
-- even if a caller ever bypassed application-side normalization.
CREATE TABLE users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username      TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'active'
                  CONSTRAINT users_status_check
                  CHECK (status IN ('active', 'disabled')),
    -- Argon2id PHC string. Never plaintext, never reversible input.
    password_hash TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX users_username_key ON users (lower(username));

-- Server-side revocable sessions (ADR-002). The browser cookie carries
-- only the raw opaque token; its SHA-256 hash is what this table stores
-- (token_hash). The CSRF synchronizer token is stored raw: it is a
-- per-session proof the legitimate frontend can already read; its
-- security relies on the same-origin policy (a cross-site attacker can
-- neither read it nor attach it as a custom header), not on
-- server-side secrecy.
-- Absolute expiry: expires_at. Idle expiry: idle_expires_at (refreshed
-- on bounded touch, see src/auth/session.rs).
CREATE TABLE user_sessions (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id           UUID NOT NULL REFERENCES users (id),
    token_hash        BYTEA NOT NULL,
    csrf_token        TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at        TIMESTAMPTZ NOT NULL,
    idle_expires_at   TIMESTAMPTZ NOT NULL,
    revoked_at        TIMESTAMPTZ,
    revocation_reason TEXT,
    -- Coarse, privacy-conscious client label (e.g. "Chrome / Windows").
    -- No raw User-Agent, no IP address, no fingerprinting.
    client_label      TEXT
);

CREATE UNIQUE INDEX user_sessions_token_hash_key ON user_sessions (token_hash);
CREATE INDEX user_sessions_user_id_idx ON user_sessions (user_id);

-- Durable security/audit events for authentication (STEP-002 foundation
-- of the future audit domain; observability logs are NOT audit records —
-- ADR-011). Secrets (passwords, raw tokens, cookie values, CSRF secrets)
-- are never stored here.
CREATE TABLE security_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    event_type  TEXT NOT NULL,
    user_id     UUID REFERENCES users (id),
    session_id  UUID,
    metadata    JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX security_events_occurred_at_idx ON security_events (occurred_at DESC);
CREATE INDEX security_events_user_id_idx ON security_events (user_id);

-- No roles, permissions, shareholder, family, share, payment or ledger
-- tables: those belong to their own reviewed STEPs (STEP-002 §51).
