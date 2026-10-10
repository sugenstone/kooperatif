# M1 — DATABASE MIGRATION PLAN & FINANCIAL TENANT PROPAGATION

Companion to discovery + data-model docs. Covers M1-D05 (propagation
through financial flows) and M1-D07 (safe migration of the existing
single-cooperative database). **Planning only — migrations are not
written or executed here.**

---

## 1. M1-D05 — Tenant propagation rules through financial flows

### 1.1 Universal rules (DESIGN RECOMMENDATION)

1. `cooperative_id` is resolved **once** per request by `TenantCtx` and
   is the only source of truth — never read from request body/path for
   business rows.
2. Every INSERT into a coop table sets `cooperative_id = ctx.coop`.
3. Every SELECT/UPDATE/DELETE carries `cooperative_id = ctx.coop` in
   WHERE/JOIN — cross-coop id → **404** (`docs/21:86`, existence not
   leaked).
4. Cross-row references (`destination_account_id`, `payer_person_id`,
   `assessment_id`, `share_id`, `entitlement_id`, `category_id`, `fund_id`,
   `reversal_of_*`, …) are validated same-coop **twice**: composite FK
   in schema + explicit check in repo (better error + defense in depth).
5. `FOR UPDATE` locks (`payments/repo.rs` pattern) include
   `cooperative_id` in the locked-row predicate — no cross-coop lock
   coupling or phantom ordering.
6. `idempotency_key` uniqueness scoped `(cooperative_id, key)` — retry
   semantics unchanged within a coop.
7. Audit rows record `cooperative_id` (from ctx, not from the target
   row — they match by construction).
8. Reversal of a row belongs to the same coop — enforced by
   self-referencing composite FK `(cooperative_id, reversal_of_id)`.
9. `PAYMENT IS NOT ACCOUNT MOVEMENT` and
   `ACCOUNT BALANCE = Σ VALID MOVEMENTS` invariants are unchanged —
   coop is an orthogonal ownership axis; all effects of one financial
   action stay inside **one coop, one tx**.

### 1.2 Flow-by-flow trace (VERIFIED FACT → required guard)

| Flow | Rows written | Same-coop edges to guard |
| --- | --- | --- |
| Create period+assessment rules | `periods`, `assessment_rules`, `assessment_share_sources`, `assessments` | share→coop; shareholder→coop |
| Collection payment | `payments`, `payment_allocations`, `account_movements`(+credit) | destination_account, payer_person, every allocated assessment, credit shareholder |
| Payment reverse | reversal `payments` + reversal allocations + movements | reversal_of chain, all inside tx |
| Share issue/sale/transfer | `shares`, `share_ownerships`, `share_events` (+ payment path) | from/to shareholders, share |
| Credit advance | `credit_applications`, `shareholder_credits`, movements | shareholder, account, movement pair |
| Income/expense | `income_entries`/`expense_entries`, movement | account, category(COOP+), reversal_of |
| Account transfer | `account_transfers`, 2 movements | src/dst accounts — CHECK both legs same coop |
| Share return | `share_returns`, `entitlements`, `settlements`, movements | shareholder, ownership, account, entitlement chain |
| Investment | `investments` + fundings/valuations/incomes/disposals/proceeds + movements | investment header, account, movement per leg |
| Social aid | `social_aid_funds`, `donations`, `disbursements`, movements | fund↔account↔person; aid finance stays separate from coop finance (AGENTS.md) but same coop |
| Governance | `bodies`, `memberships`, `decisions`, `votes` | body↔person↔membership |
| Reports/exports | read-only projections | every repo query gains coop predicate; exports included |
| Realtime | `kooperatif_notify_domain` trigger | payload becomes `<coop_uuid>:<domain>`; hub filters by socket's bound coop |

---

## 2. M1-D07 — Migration strategy (expand → backfill → validate → contract)

Single maintenance-window migration is acceptable (ADR-012 single
node; `migrate` subcommand exists). Because `sqlx::migrate!` embeds
files, the safe shape is **one transactional migration** doing
expand+backfill+validate+contract — a second migration only if Phase-1
deploy ordering requires code-first. Recommended: **two migrations**.

### `0018_cooperatives_memberships.sql` (expand + backfill)

```sql
BEGIN;  -- migration is transactional by nature

CREATE TABLE cooperatives (…);
INSERT INTO cooperatives (id, name, legal_name)
VALUES ('<fixed-uuid>', 'Varsayılan Kooperatif', NULL);  -- seeded id for deterministic backfill & tests

CREATE TABLE cooperative_memberships (…);

-- RBAC retrofit
ALTER TABLE roles ADD cooperative_id UUID
  REFERENCES cooperatives(id) DEFAULT '<fixed-uuid>';
UPDATE roles SET cooperative_id='<fixed-uuid>';      -- backfill
ALTER TABLE roles ALTER cooperative_id SET NOT NULL,
                  ALTER cooperative_id DROP DEFAULT;
ALTER TABLE roles ADD UNIQUE (cooperative_id, id);
DROP CONSTRAINT <old name-unique>;
CREATE UNIQUE INDEX roles_coop_name_uq ON roles (cooperative_id, lower(btrim(name)));

ALTER TABLE user_role_assignments ADD cooperative_id UUID;
UPDATE user_role_assignments ura SET cooperative_id = r.cooperative_id
  FROM roles r WHERE r.id = ura.role_id;             -- derive from role
ALTER TABLE user_role_assignments …NOT NULL…
  ADD PRIMARY KEY (cooperative_id,user_id,role_id) [replace PK]
  ADD FOREIGN KEY (cooperative_id,role_id) REFERENCES roles(cooperative_id,id)
  ADD FOREIGN KEY (cooperative_id,user_id) REFERENCES cooperative_memberships(cooperative_id,user_id);

-- memberships synthesized BEFORE the FK above is enforced:
INSERT INTO cooperative_memberships (cooperative_id,user_id,status,created_by_user_id)
SELECT DISTINCT cooperative_id, user_id, 'active', user_id
FROM user_role_assignments;

ALTER TABLE user_sessions ADD active_cooperative_id UUID REFERENCES cooperatives;
ALTER TABLE security_events ADD cooperative_id UUID, ADD session_id UUID;

-- business tables: repeated per table t in §3 of discovery:
ALTER TABLE t ADD cooperative_id UUID
  REFERENCES cooperatives(id) DEFAULT '<fixed-uuid>';
UPDATE t SET cooperative_id='<fixed-uuid>';          -- all existing rows → default coop
ALTER TABLE t ALTER cooperative_id SET NOT NULL,
              ALTER cooperative_id DROP DEFAULT;
ALTER TABLE t ADD UNIQUE (cooperative_id, id);

-- composite FKs on hazard edges (discovery §3.1), e.g.:
ALTER TABLE payments ADD FOREIGN KEY (cooperative_id, destination_account_id)
  REFERENCES financial_accounts(cooperative_id,id);
…
COMMIT;
```

### Numbering retrofit (`0019_tenant_numbering.sql` or inside 0018)

Registry REQ-024.6 requires **per-coop numbering** — 20 IDENTITY
columns + `families.sequence_number` are globally sequenced today
(VERIFIED FACT). Recommended model:

```sql
CREATE TABLE cooperative_counters (
    cooperative_id UUID NOT NULL REFERENCES cooperatives(id),
    kind           TEXT NOT NULL,          -- 'payment_number','share_number',…
    next_value     BIGINT NOT NULL,
    PRIMARY KEY (cooperative_id, kind)
);
-- seed per existing data: next_value = max(current)+1 per kind
INSERT INTO cooperative_counters … SELECT '<default>', kind, MAX(n)+1 …;
```

Write path: `INSERT … ON CONFLICT (coop,kind) DO UPDATE SET
next_value=counters.next_value+1 RETURNING next_value-1` inside the
same tx — one row lock per (coop,kind), consistent with the existing
`FOR UPDATE` discipline. `GENERATED ALWAYS AS IDENTITY` columns become
plain `BIGINT NOT NULL` (`ALTER … DROP IDENTITY`), `UNIQUE` →
`UNIQUE (cooperative_id, n)`.

**Alternative (rejected)**: keep global identity + per-coop display
alias — fails 024.6 and produces gapped receipt sequences per coop.
**OWNER DECISION REQUIRED (M1-K4)**: confirm contiguous per-coop
numbering for `payment_number`/receipts (registry already mandates;
confirm scope = all 21 kinds or receipts-only).

### Validation block inside the migration

```sql
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM payments p
             JOIN financial_accounts a ON a.id=p.destination_account_id
             WHERE a.cooperative_id<>p.cooperative_id) THEN
    RAISE EXCEPTION 'cross-coop payment→account found';
  END IF;  -- repeated per hazard edge
END $$;
```

### Rollback

- Full pre-migration backup via existing `scripts/backup/` drill
  (ADR-013 evidence pattern already proven in M0).
- Migration is one tx → failure auto-rolls back; after commit, rollback
  = restore backup (no down-migration — same policy as existing 17).
- **UNRESOLVED RISK**: migration rewrites `user_role_assignments` PK
  and adds ~44 composite uniques — validate duration on a copy of the
  largest realistic dataset before scheduling the window.

---

## 3. Migration verification queries (to be shipped in test suite)

```sql
SELECT count(*) FROM <each coop table> WHERE cooperative_id IS NULL;        -- 0
SELECT cooperative_id, count(*) FROM payments GROUP BY 1;                   -- 1 row: default
SELECT count(*) FROM cooperative_memberships;                               -- ≥ users with roles
-- every composite FK checked with NOT VALID → VALIDATE result captured in CI drill
```
