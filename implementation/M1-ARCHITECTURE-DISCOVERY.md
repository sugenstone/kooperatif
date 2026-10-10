# M1 — MULTI-COOPERATIVE ARCHITECTURE DISCOVERY

Governance: PROJECT-CONTROL-002 / M1-ARCH-001, work packages M1-D01 …
M1-D09, M1-D11 (discovery). Baseline: `main` @ `16f59b0`, remotely
verified by GitHub Actions run `38032053376` (M0 REMOTELY VERIFIED).
**Status: discovery evidence only — no application code, schema or CI
was changed.** Approved inputs: PD-01 (shared PostgreSQL, strict
isolation, RLS evaluated as defense in depth only) and PD-02
(multi-cooperative users, independent roles per cooperative).

Finding labels: **VERIFIED FACT** (code/schema inspected), **DESIGN
RECOMMENDATION**, **OWNER DECISION REQUIRED**, **UNRESOLVED RISK**.

---

## 1. Requirement trace

| Source | Evidence |
| --- | --- |
| Charter | "Design for multiple cooperatives even if the first deployment uses one." `docs/00-PROJECT-CHARTER.md:88-90` |
| Invariant | "Every tenant-owned record must belong to exactly one cooperative/organization context. Backend authorization must enforce tenant isolation." `docs/15-INVARIANTS.md:10-15` |
| Security | "Every tenant-scoped read/write is protected server-side. Cross-tenant identifiers must not bypass isolation." `docs/22-SECURITY.md:23-27` |
| API contract | "Avoid leaking existence of cross-tenant resources." `docs/21-API-CONTRACTS.md:86` → cross-coop id → **404 semantics**, not 403 (403 would confirm existence) |
| Quality gates | hostile review must challenge "tenant isolation"; cross-tenant access must fail (§103) `docs/26-TESTING-QUALITY-GATES.md` |
| Registry | REQ-024 sub-items 024.1–024.9 (entity, coop columns, membership, isolation, tests, per-coop numbering, switcher UI, realtime scoping, data migration) |

**VERIFIED FACT** — no STEP ever implemented tenant context and no
recorded decision deferred it (`implementation/PROJECT-CONTROL-001-
RECOVERY-ROADMAP.md` REQ-024 row: "Omission against a charter principle
and system invariant"). All 17 migrations lack a cooperative column.

---

## 2. Current-state architecture snapshot

| Component | State (VERIFIED FACT) |
| --- | --- |
| DB | single database `kooperatif`, user `kooperatif` (superuser in dev container), pool max **5** connections / acquire timeout **3 s** (`apps/server/src/db.rs`) |
| Identity | `users` global; `user_sessions` has no coop column (11 cols: id, user_id, session_token_hash, csrf_token, ip, client_label, created_at, last_seen_at, absolute_expires_at, idle_expires_at, revoked_at) — `migrations/0002` |
| RBAC | `roles`/`permissions`/`role_permissions`/`user_role_assignments` all global; permission check = `effective_permissions(user_id)` — `auth/authz.rs:45-79` |
| Session | `RequireAuth` resolves `CurrentAuth{user_id, session_id, csrf_token,…}` — no coop — `auth/extractor.rs` |
| RBAC admin | `create-user`, `grant-role` CLI subcommands (`main.rs:29`); `grant-role` does not verify role↔user tenant relationship (n/a today) |
| Business tables | 43 application tables, all global; no `cooperative_id`, `tenant_id`, `organization_id`, `workspace_id` column anywhere |
| RLS | `pg_policies` = **0 rows**; `relrowsecurity`/`relforcerowsecurity` = **0** on every table |
| Realtime | 37 triggers on 20+ tables fire `kooperatif_notify_domain()` → `pg_notify('kooperatif_domain_changed', TG_ARGV[0])` — payload is the domain tag only (`migrations/0016_realtime.sql`); server-side `listen_loop` → `Domain::from_tag` → `HubSignal::DomainChanged(tag)` broadcast to all sockets filtered by permission only (`realtime/mod.rs`, `realtime/routes.rs`) |
| Idempotency | `payments.idempotency_key`, `income_entries`, `expense_entries` globally UNIQUE (`0007`/`0010`) |
| Business numbers | **20 tables use `GENERATED ALWAYS AS IDENTITY`** for human numbers (`share_number`, `period_number`, `payment_number`, `transfer_number`, `income_number`, `expense_number`, `credit_number`, `entitlement_number`, `settlement_number`, `return_number`, `fund/donation/disbursement_number`, `investment_*`, `body_number`, `decision_number`, `funding/valuation/disposal/income`); `families.sequence_number` is app-assigned. All sequenced **globally** — per-coop contiguous numbering (registry 024.6) is impossible with identity sequences |
| Audit | `security_events` (id, event_type, actor_user_id, subject, ip, metadata) — no cooperative, no session/correlation id (`auth/audit.rs`); `audit()` is fire-and-forget (`tokio::spawn`) |
| Frontend | `auth.svelte.ts` stores a flat global `permissions` array; `apiFetch` adds CSRF header only; `realtime.svelte.ts` opens one socket per tab |

---

## 3. M1-D01 — Table-by-table inventory and proposed ownership

Legend — **Owner**: `COOP` = cooperative-owned (gets `cooperative_id
NOT NULL`), `GLOBAL` = identity/platform table, `COOP+` = coop-owned
but seeded from a global template/catalog. **Risk**: tenant-isolation
criticality (H = financial/PII, M = business, L = catalog/infra).

| # | Table | Purpose | PK | Key FKs (→ target) | Unique constraints | Owner | Risk | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `users` | global identity | id | — | username CI | GLOBAL | — | PD-02: stays global; `status`/`password_hash` platform-level |
| 2 | `user_sessions` | auth sessions | id | →users | token hash | GLOBAL | M | gains `active_cooperative_id` (nullable, see M1-D03) |
| 3 | `roles` | role definitions | id | — | name CI | COOP | H | gains `cooperative_id`; name unique per coop; seeded "Sistem Yöneticisi" must be duplicated/assigned per coop |
| 4 | `permissions` | permission catalog | id | — | key | GLOBAL | L | global catalog, referenced by coop roles |
| 5 | `role_permissions` | role→perm | (role_id, permission_id) | →roles,→permissions | PK | COOP(via role) | H | coop implicit through roles |
| 6 | `user_role_assignments` | user→role | (user_id, role_id) | →users,→roles | PK | COOP | H | gains `cooperative_id`; must join membership |
| 7 | `security_events` | audit log | id | — | — | MIXED | H | gains nullable `cooperative_id` + `session_id`; global events (login) stay NULL |
| 8 | `persons` | natural persons | id | — | search idx | COOP | H | PII: same human may exist per coop (decision M1-K2) |
| 9 | `families` | family group | id | — | sequence_number | COOP | M | sequence_number must scope per coop |
| 10 | `shareholders` | party: owner of shares | id | →persons, →financial_accounts(default_collection) | person_id, search_name | COOP | H | **dual-tenant edge**: person_id and default_collection_account_id must be same coop |
| 11 | `shareholder_family_memberships` | person↔family | (shareholder_id, family_id) | →shareholders,→families | PK | COOP | H | composite edge — both ends same coop required |
| 12 | `shares` | share instrument | id | — | share_number | COOP | H | share_number identity → per-coop counter |
| 13 | `share_ownerships` | shareholder↔share | id | →shareholders,→shares | UNIQUE(share_id) WHERE active | COOP | H | cross-tenant transfer risk: from/to shareholders both must be same coop |
| 14 | `share_events` | ownership history | id | →shares,→shareholders (from,to) | — | COOP | H | from/to both coop-scoped |
| 15 | `periods` | assessment periods | id | — | period_number | COOP | M | number per coop |
| 16 | `assessment_rules` | period rule | id | →periods | period_id | COOP | M | |
| 17 | `assessment_share_sources` | rule sources | (assessment_id, share_id) | →assessments,→shares | PK | COOP | H | **composite PK**: coop must join key or composite FK needed |
| 18 | `assessments` | obligation | id | →periods,→shareholders | (period_id,shareholder_id) | COOP | H | |
| 19 | `payments` | money received | id | →financial_accounts(dest), →persons(payer),→payments(reversal),→users | payment_number; idempotency_key | COOP | H | payer person + dest account + reversal chain same coop |
| 20 | `payment_allocations` | payment↔assessment | id | →payments,→assessments,→payment_allocations(reversal) | — | COOP | H | **money edge**: both parents same coop; `FOR UPDATE` locks need coop scoping |
| 21 | `financial_accounts` | cash/bank/etc | id | — | — | COOP | H | referenced by payments/transfers/shareholders/settlements — primary guard point |
| 22 | `account_movements` | ledger lines | id | →financial_accounts,→account_movements(reversal) | — | COOP | H | invariant: balance = Σ valid movements (docs/03); reversal chain same coop |
| 23 | `account_transfers` | account↔account | id | →financial_accounts(src,dst),→account_movements×2,→account_transfers(reversal),→users | transfer_number | COOP | H | src/dst same coop; movement pair same coop |
| 24 | `credit_applications` | advance credit | id | →shareholders,→payments,→financial_accounts,→users | — | COOP | H | |
| 25 | `shareholder_credits` | credit ledger | id | →shareholders,→account_movements×2 | credit_number | COOP | H | both movements same coop |
| 26 | `income_entries` | income | id | →financial_accounts,→financial_categories,→account_movements,→income_entries(reversal),→users | income_number; idempotency_key | COOP | H | category must be same coop (COOP+ catalog) |
| 27 | `expense_entries` | expense | id | same as income | expense_number; idem key | COOP | H | same |
| 28 | `financial_categories` | income/expense catalog | id | — | (category_type,lower(name)); (id,category_type) | COOP+ | M | seeded rows duplicated per coop at coop creation |
| 29 | `share_returns` | exit/return header | id | →shareholders,→share_returns? | return_number | COOP | H | |
| 30 | `share_return_entitlements` | return rights | id | →share_returns,→share_ownerships?,→shareholders | entitlement_number | COOP | H | |
| 31 | `share_return_settlements` | return payout | id | →share_returns,→entitlements,→financial_accounts,→account_movements×2,→share_return_settlements(reversal) | settlement_number | COOP | H | |
| 32 | `investments` | asset header | id | →financial_accounts? | investment_number | COOP | M | |
| 33 | `investment_fundings` | investment funding | id | →investments,→financial_accounts,→account_movements | funding_number | COOP | H | |
| 34 | `investment_valuations` | revaluation | id | →investments | valuation_number | COOP | M | |
| 35 | `investment_incomes` | investment yield | id | →investments,→financial_accounts,→account_movements | income_number | COOP | H | |
| 36 | `investment_disposals` | sale header | id | →investments | disposal_number | COOP | H | |
| 37 | `investment_disposal_proceeds` | sale legs | id | →investment_disposals,→financial_accounts,→account_movements | — | COOP | H | |
| 38 | `social_aid_funds` | aid funds | id | →financial_accounts | fund_number | COOP | H | **invariant**: social aid ≠ coop finance (AGENTS.md) — still same-coop |
| 39 | `social_aid_donations` | donations in | id | →social_aid_funds,→persons(donor),→financial_accounts,→account_movements,→social_aid_donations(reversal) | donation_number; idem key? | COOP | H | donor person same coop |
| 40 | `social_aid_disbursements` | aid payout | id | →social_aid_funds,→persons(beneficiary),→financial_accounts,→account_movements,→disbursements(reversal) | disbursement_number | COOP | H | beneficiary person same coop — **PII** |
| 41 | `governance_bodies` | organs | id | — | body_number | COOP | M | |
| 42 | `governance_memberships` | organ members | id | →governance_bodies,→persons | — | COOP | M | person same coop |
| 43 | `governance_decisions` | resolutions | id | →governance_bodies | decision_number | COOP | M | |
| 44 | `governance_votes` | votes | id | →governance_decisions,→governance_memberships | (decision_id,membership_id)? | COOP | M | |
| — | `_sqlx_migrations` | sqlx ledger | version | — | PK | GLOBAL | — | never tenant-scoped |

### 3.1 Cross-tenant hazard map (VERIFIED FACT)

Foreign keys today enforce *existence only*. After coop columns are
added, an **IDOR-style write** could still link rows across coops unless
each edge is guarded. Critical edges:

| Edge | Hazard | Guard required |
| --- | --- | --- |
| `payments.destination_account_id → financial_accounts` | record money into another coop's account | composite FK `(cooperative_id, destination_account_id)` |
| `payment_allocations.payment_id/assessment_id` | allocate coop-A money to coop-B debt | app check + composite FK on both ends |
| `share_ownerships` + `share_events.from/to` | transfer share across coops | composite FK; `FOR UPDATE` row lock in transfer path |
| `shareholders.default_collection_account_id` | default account of other coop | composite FK |
| `account_transfers.src/dst` | move money between coops (must be impossible — no inter-coop transfers in domain) | composite FK both legs; same `cooperative_id` CHECK |
| `income/expense.category_id` | category of other coop | composite FK `(cooperative_id, category_id)`; the existing `(id,category_type)` UNIQUE trick becomes `(cooperative_id,id,category_type)` |
| `credit_applications`/`shareholder_credits` movement pair | movement of other coop linked | composite FK |
| `social_aid_*` (fund/account/person) | aid money into coop-A fund from coop-B account | composite FK on fund+account; person same coop |
| `investment_*` (account/movement) | investment funded by foreign account | composite FK |
| `assessment_share_sources` PK `(assessment_id, share_id)` | PK has no coop component | PK → `(cooperative_id, assessment_id, share_id)` or add coop col + composite FKs |
| `payment_allocations.reversal_of_allocation_id`, `payments.reversal_of_id`, `account_movements.reversal_of_id`, `*_entries.reversal_of_id`, `share_return_settlements.reversal_of_id`, `social_aid_*.reversal_of_id` | reversal links cross coop | same-table composite FK `(cooperative_id, reversal_of_id)` — **self-referencing composite FK is legal** with a `(coop,id)` UNIQUE |
| `user_role_assignments` | role of coop A assigned to user while active coop B | `cooperative_id` col + composite FK to `roles(cooperative_id,id)` + membership FK |

**VERIFIED FACT** — every tenant-owned table therefore needs
`UNIQUE (cooperative_id, id)` in addition to the surrogate PK, to serve
as composite-FK target. This doubles unique-index surface; see
M1-D11 sizing.

### 3.2 Unique-constraint re-scope (VERIFIED FACT → DESIGN RECOMMENDATION)

| Constraint today | After M1 |
| --- | --- |
| `shares.share_number`, `payments.payment_number`, `periods.period_number`, `account_transfers.transfer_number`, `income_entries.income_number`, `expense_entries.expense_number`, `shareholder_credits.credit_number`, `share_return_*` numbers, `social_aid_*` numbers, `investment*` numbers, `governance_*` numbers | `UNIQUE (cooperative_id, <n>)`; number generation moves from `IDENTITY` to a per-coop counter (see M1-D07 §numbering) |
| `families.sequence_number` | `UNIQUE (cooperative_id, sequence_number)` |
| `payments/income_entries/expense_entries.idempotency_key` | `UNIQUE (cooperative_id, idempotency_key)` — same key may legitimately recur per coop |
| `shareholders.person_id UNIQUE` | `UNIQUE (cooperative_id, person_id)` — same person row could not be shared anyway under coop-owned persons; keep to prevent duplicate shareholder per person per coop |
| `shareholders.search_name`, `persons.search_name` indexes | per-coop search → `(cooperative_id, search_name)` covering index |
| `share_ownerships` partial unique on active | `UNIQUE (cooperative_id, share_id) WHERE …` (coop in predicate not needed if share itself coop-unique, but keep for clarity/plans) |
| `roles` name CI unique | `UNIQUE (cooperative_id, lower(name))` |
| `user_role_assignments` PK | `(cooperative_id, user_id, role_id)` |
| `governance_votes (decision_id, membership_id)` | unchanged — coop implied; add composite FK |
| `assessment_rules.period_id`, `assessments(period_id,shareholder_id)` | unchanged locally; coop carried on row |

### 3.3 Objects beyond tables (VERIFIED FACT)

- **0 views** (`information_schema.views` empty).
- **0 sequences besides identity** (identity sequences listed in §2).
- **37 triggers** on business tables — all call
  `kooperatif_notify_domain()` with a static `TG_ARGV[0]` domain tag.
  Function body: `pg_notify('kooperatif_domain_changed', TG_ARGV[0])`.
  No row data → safe payload today, but **no coop dimension**.
- **1 function** (`kooperatif_notify_domain`) + `update_updated_at`-style
  helpers — inspect per-migration; notify function is the only
  cross-tenant surface.
- **0 RLS policies, 0 FORCED RLS** anywhere.
- `_sqlx_migrations` — 17 applied; `migrate` subcommand runs
  `sqlx::migrate!("../../migrations")` (`apps/server/src/db.rs`).

---

## 4. M1-D04 discovery — auth/authz mechanics today

| Mechanism | Evidence (VERIFIED FACT) |
| --- | --- |
| Login | `auth/routes.rs` — username+password → session row + HttpOnly SameSite cookie + CSRF token in body |
| Session validate | `auth/session.rs` `validate_session` joins `user_sessions`→`users`, checks revoked/expired/disabled each request |
| Extractor | `RequireAuth` → `CurrentAuth{user_id,username,display_name,session_id,session_created_at,session_expires_at,csrf_token}` |
| Permission check | `authz::require(&state,&auth,PERM)` → `effective_permissions(pool,user_id)` → `ura→roles→role_permissions→permissions`, `r.status='active'` |
| Admin-guard | `administration_path_count` advisory lock keyed globally (`auth/routes.rs` admin paths) — coop must join the lock key |
| Role admin | `auth/rbac.rs` routes under `ROLES_MANAGE`/`USERS_MANAGE`; `grant-role` CLI bypasses HTTP |
| `auth/me` | returns `{user, csrf_token, permissions[], roles[]}` — **global** |

**Implication (DESIGN RECOMMENDATION)** — `effective_permissions` becomes
`(user_id, cooperative_id)`: `membership(active) → ura(coop) → roles(coop)
→ role_permissions → permissions`. Every `authz::require` call site then
also asserts the target row's `cooperative_id == ctx.cooperative_id`
(via the tenant-scoped query layer, not per-call boilerplate).

---

## 5. M1-D05 discovery — money paths today

`payments/repo.rs` is the pattern all write repos follow
(VERIFIED FACT): `tx.begin()` → `SELECT … FOR UPDATE` ordered by id →
insert rows → `audit()` → commit. Idempotency via unique key + lookup.
Reversals insert compensating rows (`reversal_of_id`), never UPDATE/DELETE
(AGENTS.md invariant; `docs/19`). Concurrency: `FOR UPDATE` on
assessments `ORDER BY id`; same discipline must extend to per-coop
number counters (SELECT … FOR UPDATE on `(coop,kind)` counter row).

`PAYMENT IS NOT ACCOUNT MOVEMENT` and `ACCOUNT BALANCE = Σ VALID
MOVEMENTS` invariants are already coded (movements written inside the
same tx as the payment/income/expense). Tenant propagation must keep
this: one transaction, one coop.

---

## 6. M1-D08 discovery — API surface

**82 paths** enumerated from `apps/server/src/http/mod.rs` + module
routers. Classification:

| Family | Paths | Tenant treatment |
| --- | --- | --- |
| auth | `/api/auth/{login,logout,me,csrf,change-password,sessions,…}` | global; `me` gains coop block; new `POST /api/auth/cooperative` |
| identity admin | `/api/users*`, `/api/roles*` | global user mgmt **but coop-scoped roles/memberships** → split: platform-admin vs coop-admin (decision M1-K3) |
| new (M1) | `/api/cooperatives`, `/api/cooperatives/{id}/members*`, `/api/coops/{id}/roles*` | new router `admin/` or `cooperatives/` |
| parties | `/api/persons*`, `/api/families*`, `/api/shareholders*` | coop |
| shares | `/api/shares*` incl. sale/transfer/status | coop |
| finance | `/api/periods*`, `/api/assessments*`, `/api/payments*`, `/api/financial-accounts*`, `/api/account-transfers*`, `/api/credits*`, `/api/incomes*`, `/api/expenses*`, `/api/share-returns*` | coop — highest risk |
| investments | `/api/investments*` | coop |
| social aid | `/api/social-aid/*` | coop |
| governance | `/api/governance/*` | coop |
| reports | `/api/reports/{overview,movements,financial-accounts,social-aid,…}` | coop — exports included |
| realtime | `/api/realtime` (WS upgrade) | coop-bound socket |

Frontend: `apps/web/src/lib/api.ts` (`apiFetch`), `auth.svelte.ts`,
`realtime.svelte.ts`, ~40 `+page.svelte`/`+page.ts` routes under
`src/routes/` — all implicitly single-tenant today.

---

## 7. M1-D09 discovery — realtime + background

- `listen_loop` (`realtime/mod.rs`, spawned in `main.rs:94`): one
  `PgListener` on `kooperatif_domain_changed` → parses `TG_ARGV[0]` tag
  → `HubSignal::DomainChanged`. Reconnect is self-healing.
- Socket (`realtime/routes.rs`): cookie auth at upgrade; client sends
  `subscriptions[]` of permission scopes; delivery = signal.domain →
  required read-perm → `authz` recheck → push `{domain}`; heartbeat
  revalidates session. **No coop anywhere** — coop-A event wakes
  coop-B sockets (no data leaks today since payload is just a tag, but
  B clients would refetch and the registry 024.8 requires "events never
  cross coop").
- Background: `tokio::spawn(kooperatif_notify_domain listen_loop)` is
  the only always-on task; `audit()` fire-and-forget; no job queue,
  no scheduler (ADR-009 pending for M13). **UNRESOLVED RISK** — audit
  events written async could attribute the wrong coop if context is
  captured late; pass coop explicitly into `audit()` params.

---

## 8. M1-D11 discovery — performance/ops notes

| Item | Finding |
| --- | --- |
| Pool | max 5 conn / 3 s timeout (`db.rs`). `SET LOCAL` needs every query inside tx → more tx churn; measure. `after_release` reset mandatory if session-level GUC used. |
| Indexes | each table gains `(cooperative_id)` leading col on hot indexes; ~44 `UNIQUE(coop,id)` support constraints → index bloat is real but tables are small (coop scale); fine |
| Backups | `scripts/backup/drill.mjs` restores whole DB; per-coop restore/export is **new requirement** — `pg_dump --table` can't slice by predicate → logical export per coop needed (decision M1-K7) |
| Growth | counter-table numbering serializes writes per (coop,kind) — bounded contention; many coops share counters fine |
| Hard-to-reverse | persons-per-coop vs global (M1-K2); numbering strategy (M1-K4); whether roles are per-coop copies or global templates (M1-K5); RLS on/off affects every query writer |
