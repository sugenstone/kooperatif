# M1-P2-C5 — Cooperative-Scoped Uniqueness Design (read-only)

Status: **DESIGN ONLY — no migration executed.**
Source inventory: `kooperatif_dev` catalog, verified 2026-10-12 —
exactly **61 non-PK UNIQUE indexes on the 37 business tables whose key
columns do not contain `cooperative_id`** (query in
`M1-P2-C4-VERIFICATION.sql`, gate C4-08).

## 1. Classification of all 61 candidates

### A. Numbering invariants — 21 indexes

| Index | Generation today |
|---|---|
| `shares_share_number_key` | `GENERATED ALWAYS AS IDENTITY` (m.0005) |
| `periods_period_number_key` | IDENTITY (m.0006) |
| `payments_payment_number_key` | IDENTITY (m.0007) |
| `account_transfers_transfer_number_key` | IDENTITY (m.0008) |
| `shareholder_credits_credit_number_key` | IDENTITY (m.0009) |
| `income_entries_income_number_key`, `expense_entries_expense_number_key` | IDENTITY (m.0010) |
| `share_returns_return_number_key`, `share_return_entitlements_number_key`, `share_return_settlements_number_key` | IDENTITY (m.0011) |
| `investments_investment_number_key`, `investment_fundings_funding_number_key`, `investment_valuations_valuation_number_key`, `investment_incomes_income_number_key`, `investment_disposals_disposal_number_key` | IDENTITY (m.0012) |
| `social_aid_funds_fund_number_key`, `social_aid_donations_donation_number_key`, `social_aid_disbursements_disbursement_number_key` | IDENTITY (m.0013) |
| `governance_bodies_body_number_key`, `governance_decisions_decision_number_key` | IDENTITY (m.0014) |
| `families_sequence_number_key` | **caller-supplied** (`parties/repo.rs` — `INSERT INTO families (sequence_number, …)`, `CHECK (sequence_number > 0)`, m.0004) |

Fact established by code inspection: **no application code generates
numbers.** All 20 numbered columns are `GENERATED ALWAYS AS IDENTITY`;
there is no `nextval`, no `MAX(n)+1`, no `OVERRIDING SYSTEM VALUE`
anywhere in `src/`. The single exception is `families.sequence_number`,
which is user input (per-coop family registry number).

### B. Idempotency invariants — 20 indexes

`account_transfers`, `credit_applications`, `expense_entries`,
`governance_bodies`, `governance_decisions`, `governance_memberships`,
`governance_votes`, `income_entries`, `investment_disposals`,
`investment_fundings`, `investment_incomes`, `investment_valuations`,
`investments`, `payments`, `share_return_settlements`, `share_returns`,
`shareholder_credits`, `social_aid_disbursements`,
`social_aid_donations`, `social_aid_funds` — all
`UNIQUE (idempotency_key)`. One is already partial:
`credit_applications_idempotency_key_key … WHERE idempotency_key IS NOT NULL`.

### C. Structural invariants — 20 indexes

Movement-binding 1:1 keys (8): `expense_entries_movement_key`,
`income_entries_movement_key`, `investment_fundings_movement_key`,
`investment_incomes_movement_key`,
`investment_disposal_proceeds_movement_key`,
`share_return_settlements_movement_key`,
`social_aid_donations_movement_key`,
`social_aid_disbursements_movement_key` — all `UNIQUE (account_movement_id)`.

Relationship/ownership keys (7): `account_movements_source_leg_key`
`(account_id, source_type, source_id)`, `assessment_rules_period_id_key`,
`assessments_period_shareholder_key`, `payment_allocations_payment_assessment_key`,
`shareholders_person_id_key`, `investment_disposals_one_per_investment`,
`governance_votes_one_per_member_key`.

Partial uniques (3): `governance_memberships_active_key`
`(body_id, person_id) WHERE ended_at IS NULL`,
`share_return_entitlements_one_per_right`
`(share_return_id, entitlement_type) WHERE status <> 'cancelled'`,
`share_returns_one_pending_per_share`
`(share_id) WHERE status = 'pending'`.

Category-expression keys (2): `financial_categories_name_key`
`(category_type, lower(btrim(name)))`,
`financial_categories_type_key` `(id, category_type)`.

## 2. Rescope decision

| Class | Count | Decision |
|---|---|---|
| Numbering | 21 | **Rescope** → `UNIQUE (cooperative_id, <number>)` |
| Idempotency | 20 | **Rescope** → `UNIQUE (cooperative_id, idempotency_key)` |
| `financial_categories_name_key` | 1 | **Rescope** → `(cooperative_id, category_type, lower(btrim(name)))` — two cooperatives legitimately need the same category name ("Kira", "Kira Geliri") |
| Other structural (19) | 19 | **Remain global** — every key column is already a globally-unique PK; scoping is redundant and would only widen indexes |
| Primary keys | 37 | **Remain global** (not part of the 61) |
| `*_coop_id_uq` composite support indexes | 38 | **Remain** — they are the C2 composite-FK targets |

So C5 touches **42 indexes**: 21 numbering + 20 idempotency + 1 category
name. The other 19 structural uniques are documented as
remain-global.

## 3. Per-cooperative numbering design

### Option A (recommended for C5): keep IDENTITY, rescope only the index

- Columns stay `GENERATED ALWAYS AS IDENTITY` — allocation remains a
  global sequence per column; **zero application-code change** for
  number generation, zero concurrency risk, and numbers stay globally
  unique (stronger than required).
- `UNIQUE (number)` → `UNIQUE (cooperative_id, number)` is then
  trivially satisfiable and **fully rollback-safe**: the global index
  can always be recreated since no cross-coop duplicate can exist while
  IDENTITY allocates.
- Consequence: each cooperative's numbering is unique but sparse
  (global interleaving). The spec requires *uniqueness* within the
  cooperative (`docs/20-DATA-MODEL-DATABASE.md` §Uniqueness: "business
  document number within its numbering scope"); contiguous 1..N per
  cooperative is **not** a stated requirement.
- `families.sequence_number` is the exception: it is caller-supplied,
  so the rescoped index is the real enforcement — two coops can both
  use sequence `1`. No generation logic changes needed.

### Option B (deferred): per-cooperative counters

If a future spec/ADR requires contiguous per-coop numbering
(e.g., fiscal receipt series), replace IDENTITY with
`GENERATED BY DEFAULT` (or plain BIGINT) plus a
`cooperative_counters (cooperative_id, kind, next_value)` table:
`INSERT … ON CONFLICT (cooperative_id, kind) DO UPDATE … RETURNING`
inside the business transaction gives an atomic, gap-under-rollback
counter. This is a behavioral change (app supplies numbers) and makes
global-index rollback impossible once divergent — **documented, not
recommended for C5**.

## 4. Idempotency rescope design

Today: pre-read is already cooperative-scoped
(`cooperative_id IS NOT DISTINCT FROM $2`, C3); `ON CONFLICT
(idempotency_key)` still relies on the global index — a cross-coop
collision surfaces as indistinguishable `NotFound` (documented interim).

C5 target, per table:

```sql
-- scoped lane
CREATE UNIQUE INDEX <t>_coop_idempotency_uq
    ON <t> (cooperative_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
-- legacy NULL lane keeps GLOBAL key uniqueness
CREATE UNIQUE INDEX <t>_legacy_idempotency_uq
    ON <t> (idempotency_key)
    WHERE cooperative_id IS NULL AND idempotency_key IS NOT NULL;
```

Why two indexes: `UNIQUE (cooperative_id, key)` treats NULL
cooperative as distinct → NULL-lane rows could otherwise share keys,
breaking the documented legacy-lane replay semantics.

Application change (all 20 sites already pass `cooperative_id`):
- scoped lane → `ON CONFLICT (cooperative_id, idempotency_key)
  WHERE idempotency_key IS NOT NULL`;
- NULL lane → `ON CONFLICT (idempotency_key)
  WHERE cooperative_id IS NULL AND idempotency_key IS NOT NULL`.
The correct arbiter is chosen by the already-known
`Option<Uuid>` context — no new lookup needed. Replay pre-reads and
fingerprint conflict (`IdempotencyConflict` → 409) semantics are
unchanged.

## 5. Staged migration plan

| Stage | Content | Risk |
|---|---|---|
| C5-1 | Add all 42 scoped indexes + 20 NULL-lane partial idempotency indexes; **keep** the old global indexes (dual enforcement) | Index build cost only |
| C5-2 | App release: `ON CONFLICT` arbiter targets switch to scoped form | Must deploy after C5-1 |
| C5-3 | Drop the 42 old global single-column indexes | Point of no return for idempotency/category-name — see §6 |
| C5-4 (later phase) | RLS enablement, `NOT NULL cooperative_id`, NULL-lane retirement | Out of C5 scope |

`CREATE UNIQUE INDEX` in a sqlx migration runs inside a transaction —
non-CONCURRENT builds take brief table locks; all these tables are
small in dev. For production-sized data, split into
`CREATE INDEX CONCURRENTLY` pre-steps outside the migration tx.

## 6. Rollback strategy

- **C5-1 / C5-2 rollback:** drop the new scoped indexes — global
  indexes still enforce everything; trivially safe.
- **C5-3 rollback for numbering (21):** safe — IDENTITY keeps numbers
  globally unique forever, so the global index can always be rebuilt.
- **C5-3 rollback for idempotency (20) and category name (1):** safe
  only *until the first cross-cooperative duplicate exists* — a
  duplicate blocks global-index recreation. Mitigation: verified
  backup immediately before C5-3 (ADR-013 restore-verified backup),
  plus a pre-drop snapshot query recording `cooperative_id, key` pairs.
- Ordering therefore: numbering indexes may drop freely; idempotency /
  category-name drops are the deliberate commit point.

## 7. Legacy NULL-cooperative lane retirement

Retirement is a **separate later step**, gated on:

1. Zero NULL `cooperative_id` in all 37 business tables on every
   installation (C4-02 gate, already enforced on new writes).
2. `bootstrap_initial_cooperative` seed-claim has run everywhere
   (closed this session in `kooperatif_dev`).
3. Deprecation window where `NeverBootstrapped` legacy context is
   still honored (B1 lockout-prevention invariants).

Then, in order: `SET NOT NULL cooperative_id` per table → drop the 20
`*_legacy_idempotency_uq` NULL-lane indexes → collapse
`IS NOT DISTINCT FROM` scopes to plain `=` → remove legacy-lane test
suites and the documented cross-coop-collision interim behavior.

## 8. Constraints that must remain global (explicit)

- All 37 `id` primary keys — globally unique identity, FK targets.
- The 19 structural uniques in §1C (except
  `financial_categories_name_key`) — they bind globally-unique PKs.
- `financial_categories_type_key (id, category_type)` — `id` alone is
  already globally unique; the pair is a composite-FK support index.
- Non-business tables are outside C5 entirely: `users`,
  `user_sessions`, `roles`, `permissions`, `system_role_templates`,
  `idempotency_records`, `audit_events` stay globally keyed
  (`users_username_key`, `user_sessions_token_hash_key`, … are
  identity-security invariants — rescoping them would be a bug).

## 9. C5 execution recommendation

**GO** — with conditions:

1. Execute under Option A (index-only numbering rescope).
2. C5-1/C5-2/C5-3 as three separately-releasable stages; dual
   enforcement between 1 and 3.
3. Verified-restore backup gate before C5-3 (ADR-013).
4. NULL-lane retirement explicitly excluded from C5 (§7 gates).
5. No `NOT NULL cooperative_id` in C5.
