# M1-P3 — Cooperative-Local Numbering: Discovery and Implementation Design

Status: DESIGN + foundational batch implemented (allocator only).
No document series has been migrated yet. Existing PostgreSQL IDENTITY
sequences remain the live numbering mechanism for all 20 automatic
series until their explicit adoption batches.

## 1. Discovery — the 21 numbering fields

| # | Table | Column | Type | Mechanism today | Scope required |
|---|---|---|---|---|---|
| 1 | `account_transfers` | `transfer_number` | bigint | IDENTITY `account_transfers_transfer_number_seq` | per coop |
| 2 | `expense_entries` | `expense_number` | bigint | IDENTITY `expense_entries_expense_number_seq` | per coop |
| 3 | `families` | `sequence_number` | bigint | **user input** (not generated) | per coop |
| 4 | `governance_bodies` | `body_number` | bigint | IDENTITY | per coop |
| 5 | `governance_decisions` | `decision_number` | bigint | IDENTITY | per coop |
| 6 | `income_entries` | `income_number` | bigint | IDENTITY | per coop |
| 7 | `investment_disposals` | `disposal_number` | bigint | IDENTITY | per coop |
| 8 | `investment_fundings` | `funding_number` | bigint | IDENTITY | per coop |
| 9 | `investment_incomes` | `income_number` | bigint | IDENTITY | per coop |
| 10 | `investment_valuations` | `valuation_number` | bigint | IDENTITY | per coop |
| 11 | `investments` | `investment_number` | bigint | IDENTITY | per coop |
| 12 | `payments` | `payment_number` | bigint | IDENTITY | per coop |
| 13 | `periods` | `period_number` | bigint | IDENTITY | per coop |
| 14 | `share_return_entitlements` | `entitlement_number` | bigint | IDENTITY | per coop |
| 15 | `share_return_settlements` | `settlement_number` | bigint | IDENTITY | per coop |
| 16 | `share_returns` | `return_number` | bigint | IDENTITY | per coop |
| 17 | `shareholder_credits` | `credit_number` | bigint | IDENTITY | per coop |
| 18 | `shares` | `share_number` | bigint | IDENTITY | per coop |
| 19 | `social_aid_disbursements` | `disbursement_number` | bigint | IDENTITY | per coop |
| 20 | `social_aid_donations` | `donation_number` | bigint | IDENTITY | per coop |
| 21 | `social_aid_funds` | `fund_number` | bigint | IDENTITY | per coop |

Facts verified against `kooperatif_dev` (post-0025 catalog):

- 20 columns are `GENERATED ALWAYS AS IDENTITY`, all `bigint`, one
  shared global sequence per column.
- `families.sequence_number` is a plain `bigint` supplied by the user
  in the `CreateShareholder`/family API payload — it is NOT generated
  and is handled as a separate manual lane (§5).
- No application code calls `nextval`, `MAX(number)+1`, or
  `OVERRIDING SYSTEM VALUE`. All INSERTs omit the number column and
  read it back via `RETURNING <number>`.
- Every series already has a `*_coop_*_uq` scoped unique index from
  0024 — the enforcement point for per-cooperative uniqueness exists.
- Consumers are display/identity only: `RETURNING` → response JSON
  (`"payment_number": n` etc.), report columns (`reports/repo.rs`),
  and the SvelteKit UI showing `*Number` fields as document numbers
  (e.g. `Hisse No`, receipt number). No consumer performs arithmetic
  or range logic on numbers.

## 2. Why not keep IDENTITY

PostgreSQL sequences are global objects: `nextval` never restarts per
cooperative and `currval` cannot be partitioned. Per-cooperative
numbering therefore requires an allocator whose counter state is keyed
by `(cooperative_id, series_key)` — a sequence cannot express that.

Options considered:

- **Counter table + row lock (CHOSEN)** — one row per
  `(cooperative_id, series_key)`; the allocation UPDATE serializes
  concurrent writers inside the caller's transaction. Rollback rolls
  the counter back with the business write → gapless.
- Advisory locks + `MAX+1` — rejected: explicitly forbidden without
  proper locking; even with `pg_advisory_xact_lock` it is MAX+1 under
  a lock — heavier and easier to misuse.
- One PostgreSQL sequence per (coop, series) — rejected: unbounded
  object proliferation, no transactional semantics for "reserve".
- `DEFAULT allocate_document_number(...)` — impossible: column
  defaults cannot reference other columns (`cooperative_id`).

## 3. Architecture

### 3.1 Schema (migration `0026_cooperative_numbering.sql`)

```text
document_number_series(series_key PK)              -- registry, anti-typo
cooperative_number_counters(
    cooperative_id uuid REFERENCES cooperatives(id),
    series_key     text REFERENCES document_number_series(series_key),
    next_value     bigint CHECK (next_value >= 1),
    PRIMARY KEY (cooperative_id, series_key))
```

`allocate_document_number(p_cooperative_id uuid, p_series_key text)`
— SQL function; INSERT…ON CONFLICT DO UPDATE … RETURNING next_value-1.
On first allocation in a (coop, series) it inserts `next_value = 2`
and returns 1 — **each cooperative independently starts at 1**.

`reserve_document_number(p_cooperative_id, p_series_key, p_value)` —
for imports/administrative corrections: raises the counter floor to
`p_value + 1` via `GREATEST`, so a manually supplied or imported
number can never be reissued by the allocator.

### 3.2 Concurrency and transaction semantics

- The ON CONFLICT loser waits on the winner's in-flight row lock
  until COMMIT/ROLLBACK → concurrent same-(coop,series) allocations
  serialize; duplicates are impossible.
- The counter lives in the CALLER's transaction: business write +
  counter bump commit or roll back together. A rolled-back attempt
  leaves no consumed number → **gapless by construction**.
- Lock scope is (coop, series): writes in different cooperatives or
  different series never block each other.
- Deadlock safety: the counter row is the last thing touched before
  the business INSERT; the row lock is short and uncontended across
  different series, so it does not widen the existing lock graph.

### 3.3 Rollback / gaps / imports / corrections

| Event | Behavior |
|---|---|
| Transaction rollback | Counter rolls back — number is reissued to the next attempt. Gapless. |
| Reversed financial document | Reversal never deletes the row — the number stays allocated forever. Reversal postings get their OWN new numbers. |
| Data import / legacy load | Caller writes explicit numbers and calls `reserve_document_number(coop, series, n)` per imported number (or once with `max(n)` before batch insert) so the allocator skips them. |
| Administrative correction | Corrections never renumber (invariant). An admin may only reserve/raise a floor; lowering counters is not exposed. |
| Manual `families.sequence_number` | Out of allocator scope (§5). |

### 3.4 Per-series adoption (subsequent batches, NOT this batch)

For each series, one bounded batch:

1. Migration: `ALTER TABLE <t> ALTER COLUMN <n> DROP IDENTITY`
   (keeps `bigint NOT NULL`), seed counters:
   `INSERT INTO cooperative_number_counters SELECT cooperative_id,
   '<series>', MAX(<n>)+1 FROM <t> GROUP BY cooperative_id` —
   existing numbers are preserved verbatim, never renumbered.
2. Repo change: the INSERT lists `<n>` explicitly with
   `allocate_document_number($coop, '<series>')` bound through the
   `numbering` module (the allocation runs in the same tx as the
   business write — `RETURNING <n>` contract unchanged → zero API/UI
   impact).
3. Verification: seeded counter continues from max, same-coop dup
   rejected, cross-coop independent.

The IDENTITY sequences themselves are left in place (inert) until a
final cleanup phase — rollback path stays trivial.

## 4. Rust surface (this batch)

`apps/server/src/numbering.rs`:

- `NumberingSeries` enum — compile-time whitelist of the 20 automatic
  series keys (`as_key()` returns the `document_number_series` value).
- `allocate(executor, cooperative_id, series) -> Result<i64>` —
  calls `allocate_document_number` on the provided executor (caller
  controls the transaction).
- `reserve(executor, cooperative_id, series, value) -> Result<i64>` —
  calls `reserve_document_number`, returns the resulting floor.

`families.sequence_number` is deliberately NOT in the enum.

## 5. Family sequence numbers

`families.sequence_number` is operator-supplied data (STEP-004: the
user picks the family's serial inside its cooperative). Uniqueness is
already enforced per cooperative by `families_coop_sequence_number_uq`
and surfaced as `DuplicateSequence` → 409. It stays manual — no
allocator involvement; documented separately as required.

## 6. API / reports / UI impact

None at the contract level: `RETURNING` keeps feeding the same
`*_number`/`*Number` fields; reports and screens read numbers as
opaque identifiers. Only the producer changes.

## 7. Batch plan

- **M1-P3-B1 (this batch)** — schema (`document_number_series`,
  `cooperative_number_counters`, both functions) + `numbering.rs` +
  disposable-database tests. No business table touched.
- M1-P3-B2..Bn — one bounded batch per series (or small cohesive
  groups, e.g. payments family) executing §3.4.
- M1-P3 cleanup — drop inert IDENTITY sequences only after all
  adoption batches are verified.

## 8. Verification evidence (B1)

`tests/m1p3_numbering.rs` (disposable `kooperatif_test_*` DBs,
`run_test_migrations` — includes 0026):

- `two_cooperatives_start_at_number_one` — independent counters.
- `concurrent_allocations_never_duplicate` — 16 racing transactions,
  all distinct values.
- `rollback_returns_the_number_to_the_pool` — gapless rollback.
- `reserve_raises_floor_and_allocate_continues` — import/correction
  semantics.
- `unknown_series_is_rejected` — registry FK guard.
- `manual_family_sequence_stays_untouched` — manual lane unaffected.

Full suite re-run with `--test-threads=4` after the batch.

## 9. Explicit exclusions

- No `ALTER TABLE ... DROP IDENTITY` yet; no business INSERT rewired.
- No renumbering, no historical data change, no RLS (M1-P4).
- No second-cooperative business activation.
