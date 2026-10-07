# STEP-015 — REPORTING & TRACEABLE ANALYTICS

## Repository

```text
C:\Users\sugen\Documents\PROJE\kooperatif
```

Remote:

```text
https://github.com/sugenstone/kooperatif.git
```

Baseline before this step:

```text
main == origin/main == 4f263e2  (STEP-014)
```

Authoritative inputs:

```text
docs/01-DOMAIN-GLOSSARY.md
docs/14-REPORTING-AND-DOCUMENT-DESIGN.md
docs/15-INVARIANTS.md
docs/16-STATE-MACHINES.md
docs/17-CALCULATION-RULES.md
docs/18-AUTHORIZATION-APPROVALS.md
docs/19-AUDIT-REVERSAL-CORRECTION.md
docs/20-DATA-MODEL-DATABASE.md
docs/21-API-CONTRACTS.md
docs/22-SECURITY.md
docs/23-BACKUP-RECOVERY.md
docs/26-TESTING-QUALITY-GATES.md
ADR-003 (movement/ledger architecture)
ADR-004 (money representation)
ADR-006 (idempotency/concurrency)
ADR-013 (backup/restore)
```

Approved scope: cross-domain **read-only reporting** — one reporting
endpoint surface + the Reports hub screen — covering every existing
domain, reusing each domain's canonical derivation formulas.

---

# 1. DOMAIN PRINCIPLE — PROJECTION, NEVER STATE (docs/14, docs/15)

```text
REPORT        a read-only projection over authoritative domain rows.
              Every number is derived at query time from the SAME
              formula the owning module exposes as its read model.
              Reports store NOTHING, mutate NOTHING, and may be
              reconstructed after restore — which is exactly what the
              backup drill's parity stage proves.

NUMBERS       "not cash yet" obligations (assessment remaining),
              "not business income" flows (share capital, shareholder
              credit, investment funding/income/disposal, restricted
              social-aid donations/disbursements) are LABELLED as
              such. docs/14 forbids collapsing them into totals.

ABSENT        Net Worth, Total Assets, NAV, Profit, Balance Sheet,
              realized/unrealized gain, free-cash — no authoritative
              policy exists (docs/17 UNRESOLVED), so they are absent,
              never guessed.

NULL          undetermined share-return entitlements report NULL,
              never coerced to 0.00.
```

Reversed rows never contribute to current totals (movement/payment
summaries default to `status=active` / `posted`) while remaining
traceable in list outputs.

# 2. PERMISSION — ONE CROSS-DOMAIN GRANT

```text
reports.read        catalog 30 -> 31; seeded by migration
                    0015_reporting.sql onto `Sistem Yöneticisi`.
                    A scoped read permission does NOT unlock reports;
                    `reports.read` grants no mutation power
                    (denied-POST proven in tests).
```

Every endpoint is GET-only, `no-store`, backend-authorized.

# 3. ENDPOINT SURFACE (apps/server/src/reports/)

```text
GET /api/reports/overview                 single-snapshot dashboard
GET /api/reports/financial-accounts       balance + activity + status filter
GET /api/reports/movements                provenance ledger:
                                          source_type/direction/status/
                                          kind/date/pagination filters
GET /api/reports/assessments              + /summary (same filters)
GET /api/reports/periods                  docs/14 collection semantics
GET /api/reports/shareholders             + active-share rollup
GET /api/reports/families
GET /api/reports/payments                 + /summary (posted totals;
                                          reversed rows traceable)
GET /api/reports/credits                  availability "not cash location"
GET /api/reports/share-returns            open/partially_settled/settled
GET /api/reports/investments              funding != valuation !=
                                          income != proceeds — never netted
GET /api/reports/social-aid               fund + (fund,account) pairs;
                                          restricted != additive cash
GET /api/reports/governance               recorded evidence counts
GET /api/reports/income-expense-trend     monthly business-date buckets
```

Summary vs detail contract: a report's `summary` always covers the
FULL filtered set, independent of the returned page.

# 4. CONTRACT & UI

- `packages/contracts/src/reports.ts` — response/row types; README
  notes SCREAMING metric/contract keys.
- `/raporlar` hub: overview cards grouped per domain, report-type
  selector, shared filter bar (date range + domain filters where the
  backend defines them — no invented filters), summary chips,
  paginated generic table, drill-down links into owning domain detail
  screens (payments → Tahsilatlar, entitlements → Hisse İadeleri,
  funds → Sosyal Yardım, decisions → Yönetim, financial-account
  movements → `/finansal-hesaplar`, shareholders → `/ortaklar`,
  investments → `/yatirimlar`, governance → `/yonetim`, shares →
  `/hisseler`, periods → `/donemler`, incomes/expenses →
  `/gelir-gider`, families → `/aileler`).
- Turkish-first labels; `formatTry` (string-level TRY) + Intl dates;
  zero browser float arithmetic.
- `reports.read`-gated nav entry (`nav.reports`).

# 5. DELIBERATE BOUNDARIES (deferred, never invented)

```text
PDF/XLSX/print export        docs/14 leaves rendering technology open
Scheduled/snapshot reports   not specified
As-of point-in-time reports  reversal's historical-past effect is
                             undefined in docs/19 -> not fabricable
Receivable statements/documents   separate transactional-document class
Drill-through pages          reports link to canonical detail screens
```

# 6. VERIFICATION EVIDENCE

Backend (`apps/server/tests/reports.rs`, 18 tests):

```text
golden cross-domain fixture    every overview/grouped-total metric
                               hand-computed and asserted exactly
canonical formula reuse        settled = active allocations + active
                               credit applications (same SQL shape as
                               payments/repo.rs read models)
reversal                       reversed rows excluded from totals,
                               still listed (status=reversed traceable)
NULL preservation              undetermined entitlement => null
exact money                    NUMERIC(19,2) boundary + scale-2 string
filter/summary agreement       summary == full filtered set
pagination                     totals independent of page size
IDOR/permission matrix         no reports.read => 403; with grant => 200
read-only proof                row counts identical before/after
```

Frontend: `src/specs/reports-pages.spec.ts` (6 tests —
permission-gated nav, overview labels incl. "not cash" notes, exact
decimal display, NULL-as-"Belirlenmedi", filter/pagination wiring,
error state).

E2E: `scripts/e2e-step015.mjs` — real Docker stack: login, permission
gate (denied -> granted via DB update, mirroring e2e-step014), hub
render, exact values, drill-down navigation, reversed exclusion +
traceability, social-aid separation, investment dimension separation,
governance counts, read-only probe, logout cleanup.

Backup/restore: `scripts/backup/drill.mjs` parity stage — after
`pg_restore`, every report metric is recomputed on BOTH source and
restored databases and compared; reports contain no state so no
report-table backup exists — the PROOF is post-restore reconstruction.
