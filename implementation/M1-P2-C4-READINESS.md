# M1-P2-C4 — Data Integrity Validation & C5 Readiness Report

Status: **PASS** (disposable two-cooperative database, 339/339 checks)
Artifacts: `M1-P2-C4-VERIFICATION.sql` (read-only gate queries),
`M1-P2-C4-RESULT.json` (machine-readable verdict),
`apps/server/tests/c4_integrity.rs` (executes the battery).

## Verified on populated disposable DB (2 cooperatives, all 37 tables non-empty)

| Gate | Result |
|---|---|
| C4-01 schema inventory | PASS — 37/37 tables carry `cooperative_id` |
| C4-02 NULL ownership | PASS — 0 NULL rows across all 37 tables |
| C4-03 orphan ownership | PASS — 0 dangling `cooperative_id` references |
| C4-04 composite FKs | PASS — 66 `*_coop_fk`, all `convalidated=true` |
| C4-05 cross-coop/dangling probes | PASS — 132 probes (66 FKs × cross + dangling), all 0 |
| C4-06 polymorphic movement sources | PASS — 10 source types, 0 dangling, 0 cross-coop |
| C4-07 duplicate identifiers | PASS — 43 identifier scans, 0 duplicates (NULL idem keys excluded by design) |
| C4-08 global UNIQUEs intact | PASS — 98 indexes preserved (37 pkey + 61 rescope candidates) |
| C4-09 entry→movement bindings | PASS — 8 binding probes, 0 violations |
| C4-10 seed categories | PASS — bootstrap claims all 10, 0 unclaimed |
| C4-11 population coverage | PASS — every business table non-empty |

Vacuity guard: injected NULL-ownership row and dangling movement source are
both detected as FAIL (`c4_checks_detect_injected_violations`).

## kooperatif_dev (real local DB, read-only audit)

- 37/37 columns, 66/66 validated composite FKs, 0 orphans, 0 cross-coop, 0 duplicates.
- **CLOSED (Phase A):** `financial_categories` previously had 10 NULL
  `cooperative_id` rows — the global seed set — because `kooperatif_dev`
  was bootstrapped before the P0 seed-claim code existed. On 2026-10-12
  a narrowly scoped transaction assigned exactly those 10 verified seed
  rows (all `created_by IS NULL`) to the existing bootstrap cooperative
  `Ana Kooperatif` (4d4db10d-e86f-4786-b963-a0f576765567). Result:
  `financial_categories` 10 owned / 0 NULL; zero unclaimed business rows
  on re-run of the full C4 SQL battery. No other business record was
  touched.

## C5 replacement inventory (from C4-08)

98 global UNIQUE indexes exist on the 37 business tables without
`cooperative_id`: 37 primary keys (stay global — `id` is globally unique)
and **61 non-PK candidates** for C5 rescoping:

- `*_number_key` on every numbered table (payment/transfer/credit/share/
  income/expense/return/entitlement/settlement/investment/funding/
  valuation/income/disposal/fund/donation/disbursement/body/decision/
  period/family sequence) → C5: `(cooperative_id, <number>)` composite
  unique + per-coop sequences.
- `*_idempotency_key_key` on 20 command tables → C5: scope key per
  cooperative so replay stays indistinguishable without global collision.
- Structural uniques needing C5 rescope: `shareholders_person_id_key`,
  `assessments_period_shareholder_key`,
  `payment_allocations_payment_assessment_key`,
  `account_movements_source_leg_key`, `*_movement_key` (entry/settlement/
  funding/income/proceeds/donation/disbursement),
  `share_returns_one_pending_per_share`,
  `share_return_entitlements_one_per_right`,
  `investment_disposals_one_per_investment`,
  `governance_memberships_active_key`, `governance_votes_one_per_member_key`,
  `financial_categories_name_key`, `financial_categories_type_key`,
  `share_ownerships`-related uniques, `assessment_share_sources_pkey`
  (composite (assessment_id, share_id) PK).

## Known deferred items (documented, not blockers)

- `account_movements.source_id` has no DB-level polymorphic FK (C2
  exception). Application layer guarantees ownership; C5/RLS should add
  the enforcement.
- Global unique collision → `NotFound` transition behavior is the
  documented interim state until C5 introduces scoped uniques.
- New business tables must be added to BOTH the 0021/P0 claim list and
  the C4 `BUSINESS_TABLES` constant (technical debt note).

## C5 readiness verdict

**GO** — the kooperatif_dev seed-claim item is closed (above). The full
C5 design is in `M1-P2-C5-DESIGN.md`: 61 candidates classified as
21 numbering + 20 idempotency + 20 structural (1 of which —
`financial_categories_name_key` — rescopes; the other 19 remain
global), staged dual-enforcement migration, and NULL-lane retirement
gated out of C5. C5 migrations were **not** executed.
