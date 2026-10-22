# M1-P2-C5-2 — Application Change Inventory (design, NOT implemented)

Status: **inventory only — C5-2 must not be executed under this step.**

Scoped indexes already exist (migration 0024). C5-2 is the
application-side arbiter switch: every idempotent insert currently
targets the **global** `UNIQUE (idempotency_key)`; C5-2 retargets it to
the scoped composite, and C5-3 drops the globals.

## 1. Insert sites by conflict mechanism — all 20 idempotent tables

### `ON CONFLICT (idempotency_key) DO NOTHING` — 14 sites / 15 tables

| File:line | Table / function |
|---|---|
| `src/financial_accounts/repo.rs:785` | `account_transfers` (post transfer) |
| `src/governance/repo.rs:386` | `governance_bodies` (create_body) |
| `src/income_expense/repo.rs:596` | `income_entries` **and** `expense_entries` — shared `post_entry` helper (dynamic `{table}`) |
| `src/investments/repo.rs:459` | `investments` (create_investment) |
| `src/investments/repo.rs:724` | `investment_fundings` (post_funding) |
| `src/investments/repo.rs:932` | `investment_valuations` (record_valuation) |
| `src/investments/repo.rs:1138` | `investment_incomes` (post_income) |
| `src/investments/repo.rs:1385` | `investment_disposals` (dispose_investment) |
| `src/payments/repo.rs:825` | `payments` (create_payment) |
| `src/share_returns/repo.rs:548` | `share_returns` (initiate) |
| `src/share_returns/repo.rs:1272` | `share_return_settlements` (settle) |
| `src/social_aid/repo.rs:468` | `social_aid_funds` (create_fund) |
| `src/social_aid/repo.rs:809` | `social_aid_donations` (post_donation) |
| `src/social_aid/repo.rs:1091` | `social_aid_disbursements` (post_disbursement) |

### Unique-violation → replay/race re-read — 5 sites

| File:line | Table / function |
|---|---|
| `src/credits/repo.rs:161` | `credit_applications` (`insert_application`, sweep) |
| `src/credits/repo.rs:716` | `shareholder_credits` (assign_credit) |
| `src/governance/repo.rs:572` | `governance_memberships` (create_membership) |
| `src/governance/repo.rs:744` | `governance_decisions` (create_decision) |
| `src/governance/repo.rs:1027` | `governance_votes` (cast_vote) |

14 `ON CONFLICT` sites cover 15 tables (the `post_entry` helper serves
both income and expense), plus 5 violation-based sites = **all 20
idempotent tables accounted for**.

### Missing-from-list check

`share_return_entitlements` has no `idempotency_key` column (verified in
migration 0011); the 20 idempotency-table set is exactly the above.

## 2. Required C5-2 code changes

For each of the 20 sites, the conflict target becomes cooperative-aware:

- **Scoped lane** (`cooperative_id IS NOT NULL`):
  `ON CONFLICT (cooperative_id, idempotency_key)
   WHERE idempotency_key IS NOT NULL` — resolves to the
  `<t>_coop_idempotency_uq` index from 0024.
- **NULL legacy lane** (`cooperative_id IS NULL`): during C5-2 the
  preserved global `*_idempotency_key_key` still arbitrates
  `ON CONFLICT (idempotency_key)`; after C5-3 the arbiter must become
  `ON CONFLICT (idempotency_key) WHERE cooperative_id IS NULL
   AND idempotency_key IS NOT NULL` matching the NULL-lane partial
  index created at drop time.
- For the 6 violation-based sites no `ON CONFLICT` clause exists — the
  replay re-read following `is_unique_violation` must re-select with
  the cooperative-scoped predicate (already the case via C3's
  `IS NOT DISTINCT FROM` pre-reads) and a cross-coop duplicate key must
  keep surfacing as `IdempotencyRace`/`IdempotencyConflict`, not as a
  silent success.
- Replay/fingerprint semantics unchanged: same key + same fingerprint
  → replay; same key + different fingerprint → 409 conflict.

### Pre-read lookups — already correct

All 38 `WHERE idempotency_key` pre-reads were scoped in C3 with
`cooperative_id IS NOT DISTINCT FROM $n`; no change required.

## 3. Numbering — NO C5-2 code change

All 20 numbered columns stay `GENERATED ALWAYS AS IDENTITY` (Option A);
no application code generates numbers. `families.sequence_number`
remains caller-supplied. Independent per-coop numbering is M1-P3 work.

## 4. C5-3 migration sketch (for completeness)

Per idempotent table, in one transaction:

```sql
DROP INDEX <t>_idempotency_key_key;
CREATE UNIQUE INDEX <t>_legacy_idempotency_uq
    ON <t> (idempotency_key)
    WHERE cooperative_id IS NULL AND idempotency_key IS NOT NULL;
```

For numbering/category-name tables: `DROP INDEX <t>_<number>_key` /
`financial_categories_name_key` — no NULL-lane replacement needed for
numbering (IDENTITY keeps numbers globally unique regardless) — but
`financial_categories` DOES need
`UNIQUE (category_type, lower(btrim(name))) WHERE cooperative_id IS NULL`
to keep the legacy lane's name uniqueness.
