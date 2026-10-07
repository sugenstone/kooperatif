-- =====================================================================
-- 0014  GOVERNANCE, DECISIONS & VOTING FOUNDATION  (STEP-014)
-- docs/09-GOVERNANCE-POLICIES-DECISIONS.md, docs/02-DOMAIN-MODEL.md,
-- docs/15-INVARIANTS.md, docs/16-STATE-MACHINES.md,
-- docs/18-AUTHORIZATION-APPROVALS.md, docs/19-AUDIT-REVERSAL-CORRECTION.md,
-- docs/20-DATA-MODEL-DATABASE.md, docs/22-SECURITY.md, ADR-006/010/013.
--
-- Scope decisions (implementation record):
--   * GOVERNANCE is EVIDENCE, not money: creating bodies, memberships,
--     decisions or votes writes ZERO account_movements — governance
--     records authorize nothing by themselves in STEP-014.
--   * GOVERNING BODY STRUCTURE IS CONFIGURABLE (docs/09: "exact legal
--     board structure is configurable/domain-specific"): bodies are
--     created by authorized operators with a bounded free-text
--     `body_type` label. No fixed board/committee catalog is invented.
--   * MEMBERSHIP IS TEMPORAL AND PERSON-BASED: a membership links the
--     canonical `persons` row to a body for [started_at, ended_at).
--     At most ONE active membership per (body, person) — this is the
--     minimal structural rule that keeps one-vote-per-member
--     enforcement unambiguous; it is not a legal-policy claim.
--     `title` is a bounded free-text label only — no chair/secretary
--     powers are invented.
--   * DECISION lifecycle (docs/16 candidate, narrowed to defined
--     transitions):
--        draft -> open -> approved | rejected
--        draft -> cancelled
--     Material content (title, decision_text, decision_on,
--     effective_on, body) freezes when voting opens: members must vote
--     on the text history will keep. Drafts stay editable; cancel is
--     allowed from draft only. There is NO DELETE.
--   * OUTCOME IS OPERATOR-RECORDED (docs/09: "exact cooperative legal
--     rules remain unresolved until specified"): the system NEVER
--     computes approved/rejected from invented quorum/majority
--     arithmetic. Finalization takes the formally decided outcome as a
--     parameter and freezes a deterministic vote snapshot
--     (eligible/approve/reject/abstain counts) as evidence. Quorum,
--     majority, tie-breaking, weighted/share-based voting, proxy and
--     secret-ballot models are OPEN DECISIONS in docs/09 → deferred.
--   * VOTES are one recorded row per (decision, person), cast while
--     the decision is `open` by an operator holding governance.manage
--     (recorded-vote model: the login actor who records is stored
--     separately from the voting Person — docs/18 User != Person).
--     Eligibility is evaluated server-side at cast time: the person
--     must hold an ACTIVE membership (ended_at IS NULL,
--     started_at <= cast time) in the decision's body. Vote choices
--     are approve/reject/abstain — abstention is the only docs/09-
--     listed non-binary choice. Votes are immutable once cast:
--     change/withdrawal rules are undefined → the conservative choice
--     is no mutation (docs/19 correction semantics deferred).
--   * RBAC != GOVERNANCE (docs/18): `governance.read/manage` grant API
--     operation only. They do NOT make the user a member, voter or
--     approver — membership is a `persons` fact, and business approval
--     is a distinct layer that remains UNDEFINED (docs/09 open
--     decisions) → no financial command is approval-gated in STEP-014
--     and no decision-to-transaction linkage is fabricated.
--   * Decision Date != Effective Date (docs/15 invariant): both are
--     stored as business dates; `effective_on` is never auto-applied —
--     reaching the date does not mutate status.
-- =====================================================================

-- ---------------------------------------------------------------------
-- Governance body: identity of a governing organ. `body_type` is a
-- bounded free-text classification — docs/09 leaves the legal catalog
-- configurable, so no enum is invented. A body closes only when it has
-- no draft/open decisions and no active memberships (enforced in the
-- service layer); closed bodies keep every historical decision.
-- ---------------------------------------------------------------------
CREATE TABLE governance_bodies (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    body_number         BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    name                TEXT        NOT NULL,
    body_type           TEXT        NOT NULL,
    description         TEXT,
    status              TEXT        NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active','closed')),
    closed_at           TIMESTAMPTZ,
    closed_by           UUID        REFERENCES users (id),
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT governance_bodies_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT governance_bodies_closed_consistent CHECK (
        (status = 'closed'
            AND closed_at IS NOT NULL AND closed_by IS NOT NULL)
        OR
        (status <> 'closed'
            AND closed_at IS NULL AND closed_by IS NULL))
);

CREATE UNIQUE INDEX governance_bodies_body_number_key
    ON governance_bodies (body_number);
CREATE INDEX governance_bodies_status_idx
    ON governance_bodies (status, body_number DESC);

-- ---------------------------------------------------------------------
-- Governance membership: temporal Person -> Body link. ended_at NULL
-- means active. A person may hold historical memberships and rejoin
-- later; historical rows are never deleted or rewritten — ending a
-- term only sets the end marker (docs/15 relationship-history rule).
-- ---------------------------------------------------------------------
CREATE TABLE governance_memberships (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    body_id             UUID        NOT NULL
                        REFERENCES governance_bodies (id),
    person_id           UUID        NOT NULL REFERENCES persons (id),
    -- Bounded free-text role label (e.g. how the organ itself records
    -- the seat). No title semantics/powers are attached.
    title               TEXT,
    started_at          TIMESTAMPTZ NOT NULL,
    ended_at            TIMESTAMPTZ,
    ended_by            UUID        REFERENCES users (id),
    end_reason          TEXT,
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT governance_memberships_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT governance_memberships_window_check CHECK (
        ended_at IS NULL OR ended_at > started_at),
    CONSTRAINT governance_memberships_ended_consistent CHECK (
        (ended_at IS NOT NULL
            AND ended_by IS NOT NULL
            AND end_reason IS NOT NULL)
        OR
        (ended_at IS NULL
            AND ended_by IS NULL
            AND end_reason IS NULL))
);

-- One active membership per (body, person): keeps vote eligibility and
-- the one-record-per-member vote constraint unambiguous.
CREATE UNIQUE INDEX governance_memberships_active_key
    ON governance_memberships (body_id, person_id)
    WHERE ended_at IS NULL;
CREATE INDEX governance_memberships_body_idx
    ON governance_memberships (body_id, started_at DESC, id);
CREATE INDEX governance_memberships_person_idx
    ON governance_memberships (person_id, started_at DESC, id);

-- ---------------------------------------------------------------------
-- Decision / Resolution: first-class governance evidence.
--   decision_on     formal Decision Date (docs/01/15 — distinct from
--                   effective_on; never derived from created_at)
--   effective_on    Effective Date — stored, never auto-applied
--   status          draft -> open -> approved | rejected
--                   draft -> cancelled
-- Material content (title, decision_text, decision_on, effective_on,
-- body_id) is frozen by the service layer the moment status leaves
-- 'draft': voters and history must see the same text (docs/09
-- decision-preservation rule).
-- The *_count columns are the finalization snapshot: which electorate
-- size and cast-vote tally the recorded outcome stood on. They are
-- evidence, not a pass rule — no quorum/majority arithmetic is
-- performed (docs/09 open decisions).
-- ---------------------------------------------------------------------
CREATE TABLE governance_decisions (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    decision_number     BIGINT      NOT NULL GENERATED ALWAYS AS IDENTITY,
    body_id             UUID        NOT NULL
                        REFERENCES governance_bodies (id),
    title               TEXT        NOT NULL,
    decision_text       TEXT        NOT NULL,
    decision_on         DATE        NOT NULL,
    effective_on        DATE,
    status              TEXT        NOT NULL DEFAULT 'draft'
                        CHECK (status IN
                            ('draft','open','approved','rejected','cancelled')),
    opened_at           TIMESTAMPTZ,
    opened_by           UUID        REFERENCES users (id),
    finalized_at        TIMESTAMPTZ,
    finalized_by        UUID        REFERENCES users (id),
    eligible_count      INT,
    approve_count       INT,
    reject_count        INT,
    abstain_count       INT,
    cancelled_at        TIMESTAMPTZ,
    cancelled_by        UUID        REFERENCES users (id),
    cancellation_reason TEXT,
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    created_by          UUID        NOT NULL REFERENCES users (id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT governance_decisions_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT governance_decisions_open_consistent CHECK (
        (status IN ('open','approved','rejected')
            AND opened_at IS NOT NULL AND opened_by IS NOT NULL)
        OR
        (status IN ('draft','cancelled')
            AND opened_at IS NULL AND opened_by IS NULL)),
    CONSTRAINT governance_decisions_finalized_consistent CHECK (
        (status IN ('approved','rejected')
            AND finalized_at IS NOT NULL AND finalized_by IS NOT NULL
            AND eligible_count IS NOT NULL
            AND approve_count IS NOT NULL
            AND reject_count IS NOT NULL
            AND abstain_count IS NOT NULL)
        OR
        (status NOT IN ('approved','rejected')
            AND finalized_at IS NULL AND finalized_by IS NULL
            AND eligible_count IS NULL
            AND approve_count IS NULL
            AND reject_count IS NULL
            AND abstain_count IS NULL)),
    CONSTRAINT governance_decisions_cancelled_consistent CHECK (
        (status = 'cancelled'
            AND cancelled_at IS NOT NULL AND cancelled_by IS NOT NULL
            AND cancellation_reason IS NOT NULL)
        OR
        (status <> 'cancelled'
            AND cancelled_at IS NULL AND cancelled_by IS NULL
            AND cancellation_reason IS NULL)),
    CONSTRAINT governance_decisions_counts_nonnegative CHECK (
        eligible_count IS NULL
        OR (eligible_count >= 0 AND approve_count >= 0
            AND reject_count >= 0 AND abstain_count >= 0))
);

CREATE UNIQUE INDEX governance_decisions_decision_number_key
    ON governance_decisions (decision_number);
CREATE INDEX governance_decisions_body_idx
    ON governance_decisions (body_id, decision_number DESC);
CREATE INDEX governance_decisions_status_idx
    ON governance_decisions (status, decision_number DESC);

-- ---------------------------------------------------------------------
-- Vote: one recorded row per (decision, person). `membership_id`
-- snapshots exactly which membership row established eligibility at
-- cast time, so ending/replacing the membership later can never
-- rewrite who voted (docs/15). `recorded_by` is the login actor —
-- distinct from the voting Person (docs/18: User != Person). Votes
-- are immutable once cast: no UPDATE/DELETE path exists and no
-- correction model is invented.
-- ---------------------------------------------------------------------
CREATE TABLE governance_votes (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    decision_id         UUID        NOT NULL
                        REFERENCES governance_decisions (id),
    membership_id       UUID        NOT NULL
                        REFERENCES governance_memberships (id),
    person_id           UUID        NOT NULL REFERENCES persons (id),
    choice              TEXT        NOT NULL
                        CHECK (choice IN ('approve','reject','abstain')),
    cast_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    note                TEXT,
    recorded_by         UUID        NOT NULL REFERENCES users (id),
    idempotency_key     TEXT        NOT NULL,
    idempotency_fingerprint TEXT    NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT governance_votes_idempotency_key_key
        UNIQUE (idempotency_key),
    CONSTRAINT governance_votes_one_per_member_key
        UNIQUE (decision_id, person_id)
);

CREATE INDEX governance_votes_decision_idx
    ON governance_votes (decision_id, cast_at, id);
CREATE INDEX governance_votes_membership_idx
    ON governance_votes (membership_id);
CREATE INDEX governance_votes_person_idx
    ON governance_votes (person_id, cast_at DESC);

-- ---------------------------------------------------------------------
-- STEP-014 permission catalog additions (docs/18: governance view /
-- manage kept atomic and separate — permission to RECORD governance
-- never equals governance membership).
-- ---------------------------------------------------------------------
INSERT INTO permissions (key, name, description, category) VALUES
    ('governance.read',
     'Yönetimi Görüntüle',
     'Kurulları, kurul üyeliklerini, kararları ve oyları görüntüleyebilir.',
     'Yönetim'),
    ('governance.manage',
     'Yönetimi Yönet',
     'Kurul oluşturabilir/kapatabilir, üyelik başlatabilir/sonlandırabilir, karar açabilir/sonuçlandırabilir ve oy kaydedebilir.',
     'Yönetim');

INSERT INTO role_permissions (role_id, permission_id)
SELECT
    '00000000-0000-4000-8000-000000000001',
    p.id
FROM permissions p
WHERE p.key IN ('governance.read', 'governance.manage')
ON CONFLICT DO NOTHING;
