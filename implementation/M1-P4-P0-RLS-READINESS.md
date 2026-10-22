# M1-P4-P0 — RLS Readiness and Database Stability Audit

Read-only discovery output for the M1-P4 row-level-security phase.
No policies implemented; no migration executed; no cleanup performed.

## 1. Database stability — incident analysis

**Event:** at 2026-10-13 ~20:05 UTC the `kooperatif-dev-postgres-1`
container was stopped and restarted while the backend test suite was
running. Postgres then spent ~7 minutes in crash recovery, during which
every pooled connection failed with `PoolTimedOut` — the symptom seen
in the suite, NOT the cause.

Evidence:

| Signal | Value | Meaning |
|---|---|---|
| `docker inspect` `OOMKilled` | `false` | not an OOM kill |
| `ExitCode` / `RestartCount` | `0` / `0` | container stop/start cycle, not a Docker restart-policy event |
| `docker stats` | ~978 MiB / 15.47 GiB | no memory pressure |
| postgres log | `database system was not properly shut down; automatic recovery in progress` | Postgres did NOT crash; the container was killed mid-flight (SIGKILL-class termination: docker stop timeout, Docker Desktop/WSL restart, or host event — coincides with the interactive session's connection drops) |
| `recovery_init_sync_method` | `fsync` | Postgres fsyncs the ENTIRE data directory before accepting connections |
| `ls data/base` | **291 database directories** | the fsync storm scales with directory count — root amplifier of the outage |

Verdict: external container/host restart; the long outage was recovery
`fsync` amplified by ~291 databases (258 disposable `kooperatif_test_*`
leaks included). Identical crash-recovery signatures appear at
2026-10-11 19:36, 2026-10-13 06:12 and 16:47 — the environment kills
the container periodically (WSL/Docker Desktop lifecycle on Windows).

## 2. Test database lifecycle

Inventory (`pg_database` on the dev container):

- **258** disposable `kooperatif_test_<32hex>` databases, all carrying
  `kooperatif-gc-pending:<ts>` markers — the GC has classified them,
  none reached drop.
- Named/protected: `kooperatif` (historical, v20 — untouched),
  `kooperatif_dev` (v31), `kooperatif_test` (standing direct-connect
  DB), `kooperatif_test_seed`, `kooperatif_e2e*`, `kooperatif_c1_shadow*`,
  `kooperatif_c53_restore_verify`, `kooperatif_backup_verify`,
  `kooperatif_audit_001`, `kooperatif_funcaudit_001`,
  `kooperatif_gc_keep_*`, `kooperatif_mig12`, `kooperatif_verify`.
- Total cluster size ~3.6 GB across 291 databases.

Harness flow (`src/db.rs` + `tests/*::setup()`): every DB-gated test
creates `kooperatif_test_<hex>` (CREATE DATABASE), runs the full
embedded migrator (~31 migrations), stamps the `kooperatif-test-owned:`
heartbeat lease, and `DROP DATABASE … WITH (FORCE)` on cleanup.
`drop_stale_test_databases` GC: leader election via advisory lock,
`owned:<fresh>` → skip, expired → `gc-pending`, `gc-pending` + 30-min
grace → non-FORCE drop. Leak sources: process kills between create and
heartbeat, crash-aborted sweeps, and grace-period accumulation during
long runs.

Recommendations (deferred to implementation batch):

- Bound suite-wide concurrency lower than `max_connections=100` minus
  headroom; `--test-threads=4` inside binaries is not the whole story —
  each test ALSO holds the admin pool, heartbeat conn and per-repo pool
  of 5 (acquire_timeout 3s is too tight under recovery load; raise to
  ~30s or make it configurable for tests).
- Periodic GC invocation is best-effort; a scheduled sweep or a
  post-suite cleanup step would keep the directory count bounded.
- Consider `recovery_init_sync_method=syncfs` on the DEV container to
  shrink recovery time (per-filesystem sync instead of per-file fsync).
- No database deletion without separate authorization — P0 performs
  none.

## 3. RLS architecture discovery

### 3.1 Schema inventory (`kooperatif_dev`, v31)

52 public base tables:

- **39 cooperative-owned business tables** — `cooperative_id NOT NULL`
  (0025 closed the NULL lane): account_movements, account_transfers,
  assessment_rules, assessment_share_sources, assessments,
  cooperative_memberships, cooperative_number_counters,
  credit_applications, expense_entries, families, financial_accounts,
  financial_categories, governance_bodies, governance_decisions,
  governance_memberships, governance_votes, income_entries,
  investment_disposal_proceeds, investment_disposals,
  investment_fundings, investment_incomes, investment_valuations,
  investments, payment_allocations, payments, periods, persons,
  share_events, share_ownerships, share_return_entitlements,
  share_return_settlements, share_returns, shareholder_credits,
  shareholder_family_memberships, shareholders, shares,
  social_aid_disbursements, social_aid_donations, social_aid_funds.
- **3 system-lane tables with NULLABLE cooperative_id**: `roles`,
  `user_role_assignments`, `security_events` (NULL = platform/global
  scope — e.g. login audit, legacy global roles).
- **10 platform tables without `cooperative_id`**: `_sqlx_migrations`,
  `cooperatives`, `document_number_series`, `installation_state`,
  `permissions`, `role_permissions`, `system_role_template_permissions`,
  `system_role_templates`, `user_sessions`, `users`.
- `allocate_document_number`/`reserve_document_number` are `prosecdef =
  false` — no SECURITY DEFINER surface in app code (catalog functions
  only from extensions).
- 37 AFTER-trigger `pg_notify('kooperatif_domain_changed', …)` emitters
  on business tables (migration 0016) — triggers inherit the writing
  session's privileges; RLS does not interfere with trigger execution,
  but policies must still permit the triggering row operation.

### 3.2 Access paths

| Path | Mechanism | Notes for RLS |
|---|---|---|
| HTTP business handlers | `TenantCtx` / `BusinessAuth` extractors → `state.db` `PgPool` (max 5, acquire 3s) | request-scoped verification already exists; GUC must be stamped per transaction |
| Repo functions | `pool.begin()` tx OR direct `fetch_*(pool)` statements | **direct pool statements run on arbitrary pooled connections — a session-level GUC would be correct per-tenant but leaks on reuse; a tx-local GUC doesn't exist for non-tx reads.** This is THE structural constraint. |
| Tenant/business resolution | `resolve_bootstrap`, `resolve_membership` on pool BEFORE any coop context exists | chicken-and-egg: membership lookup must work with no tenant GUC (see §4) |
| Audit writer | `audit::record_scoped` — fire-and-forget INSERT on pool | runs OUTSIDE the business tx (failures swallowed by design) → needs its own context or exempt lane |
| Realtime | dedicated `PgListener` conn (`listen_loop`), LISTEN only | no table access — unaffected |
| CLI | `migrate`, `create-user`, `grant-role`, `create-cooperative`, `add-member`, `bootstrap-cooperative` — separate pool per invocation | administrative lane; must be explicitly controlled |
| Test harness | admin pool (postgres), per-test pool, heartbeat `PgConnection` | disposable DBs only |

### 3.3 Connection principals

`kooperatif` is **superuser + BYPASSRLS + CREATEDB** and owns all
objects. With this principal, RLS would be inert — the single most
important readiness finding.

### 3.4 Tenant-context design (proposal)

- New login role `kooperatif_app`: `NOSUPERUSER NOBYPASSRLS
  NOCREATEDB`, member of nothing else; granted CRUD on the 42
  coop-scoped/system-lane tables, SELECT on needed platform tables,
  EXECUTE on `allocate_document_number`/`reserve_document_number`.
- `kooperatif` remains owner + migration/DDL/admin lane only; the app
  connects as `kooperatif_app`.
- GUC `app.cooperative_id` (uuid text), stamped `SET LOCAL` (or
  `set_config(…, true)`) inside every business transaction; policies
  read `current_setting('app.cooperative_id', true)` — `NULL`/empty/
  malformed/unknown-coop ⇒ deny.
- Pool-leak guard: `SET LOCAL` never survives a tx; PLUS a
  `PgPoolOptions::before_acquire` (or equivalent checkout hook) that
  RESETs `app.cooperative_id` — belt and suspenders against a stray
  session-level `SET`.
- Read path problem: direct `fetch_*(pool)` statements outside a tx
  cannot carry a tx-local GUC. Options (evaluate in P1 batch): (a) run
  every tenant read in a thin `BEGIN/SET LOCAL/COMMIT` wrapper
  (`tenant_tx` helper), or (b) keep a dedicated per-request pooled
  connection with session GUC + mandatory reset on release. Option (a)
  is simpler and fails closed.
- Membership/bootstrap resolution (pre-context) needs a controlled
  bypass: e.g. `SECURITY DEFINER` function owned by `kooperatif`
  (`resolve_bootstrap_ctx(user)`) returning the verified row — the
  ONLY sanctioned bypass, minimal and auditable — or a narrow policy
  letting `kooperatif_app` read `cooperative_memberships`/`cooperatives`
  rows where `user_id = current_setting('app.user_id')` (a second,
  request-scoped GUC set by the auth extractor).
- Audit (`security_events`): either accept the second GUC
  `app.user_id` for platform-scope NULL-coop inserts, or a SECURITY
  DEFINER insert function.

## 4. RLS security model (proposal)

- `ENABLE ROW LEVEL SECURITY` + **`FORCE ROW LEVEL SECURITY`** on all
  39 business tables + the 3 system-lane tables → even the table owner
  is subject; owner-only ops (migrations) run as `kooperatif` anyway
  but FORCE makes it honest.
- Policies per table, all four commands:
  - `USING (cooperative_id = current_setting('app.cooperative_id', true)::uuid)`
    for SELECT/UPDATE/DELETE.
  - `WITH CHECK (cooperative_id = current_setting('app.cooperative_id', true)::uuid)`
    for INSERT/UPDATE — blocks cooperative_id reassignment.
  - Missing/invalid GUC ⇒ cast fails or NULL ⇒ fail closed.
- Nullable-coop system tables: `cooperative_id IS NOT DISTINCT FROM
  current_setting(...)::uuid` is WRONG for RLS (NULL context would see
  platform rows); use `cooperative_id = …::uuid` (NULL rows invisible
  to tenant context) + a separate admin/definer path for NULL rows.
- `BYPASSRLS` must NOT be granted to `kooperatif_app`. `SECURITY
  DEFINER` functions: only the membership-resolution shim; they must
  `SET search_path` and never expose raw table access.
- Trust boundary verdict: **session variables alone are sufficient IFF**
  (i) the role cannot `SET` arbitrary context that bypasses… — actually
  `SET app.cooperative_id` is settable by the app role itself, so the
  GUC is NOT a secret — the boundary holds because the app layer only
  ever stamps the VERIFIED id, and RLS makes every row operation
  consistent with it. A compromised query string (SQL injection in app
  SQL) could `SET` another coop — mitigations: keep custom GUC
  settable but ALSO require `app.user_id` membership evidence via a
  SECURITY DEFINER attestation, or accept the residual risk as
  documented (injection already implies arbitrary SQL within app role).
- `FORCE RLS` vs ownership: owner (`kooperatif`) is normally exempt
  from RLS unless FORCE — apply FORCE so even owner sessions (CLI
  mistakes) are policy-checked; admin lane goes through explicit
  `SET ROLE`/BYPASS-capable maintenance role if ever needed.

## 5. Verification design (for P1+ batches)

Negative tests, all must fail closed:

- No `app.cooperative_id` → every business SELECT/INSERT/UPDATE/DELETE
  yields 0 rows / error.
- Wrong coop GUC → reads return nothing, writes rejected (WITH CHECK).
- Cross-tenant direct SQL (`SET` foreign coop then write) — documents
  the residual surface; assert business-table invariants still block
  garbage (composite FKs).
- Connection reuse: commit coop-A tx, check out same conn for coop-B —
  `before_acquire` reset proves no bleed.
- Concurrent mixed-coop load — no cross-leak under contention.
- `UPDATE … SET cooperative_id = other` — WITH CHECK rejects.
- SECURITY DEFINER surface: function list frozen (inventory test).
- Reporting/background/audit paths operate under their declared lanes.
- Migration lane: `kooperatif` owner applies DDL with FORCE RLS intact.

## 6. Proposed M1-P4 batches

- **P1 — principals & plumbing**: `kooperatif_app` role + grants,
  `before_acquire` reset hook, `tenant_tx` helper, config for app URL.
  No policies yet — behavior must be identical (gates: full suite).
- **P2 — context stamping**: extractor → tx GUC propagation; membership
  resolution lane (SECURITY DEFINER shim or `app.user_id` GUC);
  audit/realtime/CLI lane decisions applied.
- **P3 — policies**: ENABLE+FORCE RLS and USING/WITH CHECK policies on
  the 39 business tables (bounded sub-batches by module), then the 3
  nullable-coop system tables.
- **P4 — verification & hardening**: §5 negative suite, concurrency
  races, docs, rollback path (role swap back), RLS-aware C4 checks.

## 7. P2A addendum — runtime principal hardening (migration 0033)

### Credential lanes, final form

| Lane | Principal | Guard |
|---|---|---|
| `migrate`, `create-user`, `create-cooperative`, `add-member`, `bootstrap-*` | `KOOPERATIF_DATABASE_URL` → `kooperatif` (schema owner) | `KOOPERATIF_EXPECTED_DATABASE` identity check |
| `serve` (HTTP) | `KOOPERATIF_APP_DATABASE_URL` → `kooperatif_app` | startup principal inspection: fail closed on SUPERUSER / BYPASSRLS / CREATEROLE / CREATEDB / public-table ownership |
| `serve` dev fallback | admin URL | only behind explicit `KOOPERATIF_ALLOW_ADMIN_RUNTIME=true`; rejected in production; privileged principal announced via WARN |

`db::inspect_runtime_principal` reads `current_user` against `pg_roles`
plus public-table ownership — a mispointed URL fails closed rather than
relying on configuration intent.

### `kooperatif_app` grant matrix (after 0033)

| Table | Verbs | Why |
|---|---|---|
| 38 business tables (coop NOT NULL) | SELECT/INSERT/UPDATE/DELETE | all repo paths |
| `users` | SELECT, UPDATE | login `last_login_at`, rbac enable/disable |
| `user_sessions` | SELECT, INSERT, UPDATE | login, touch, revoke, coop switch |
| `security_events` | SELECT, INSERT | audit writer + `never_bootstrapped` probe |
| `roles` | SELECT, INSERT, UPDATE | rbac router (status flips, never DELETE) |
| `role_permissions` | SELECT, INSERT, DELETE | set_role_permissions replace-set |
| `user_role_assignments` | SELECT, INSERT, DELETE | set_user_roles replace-set |
| `permissions`, `system_role_templates`, `system_role_template_permissions`, `cooperatives`, `cooperative_memberships`, `installation_state`, `document_number_series` | SELECT only | reads for resolve/bootstrap/RBAC catalogs |
| `_sqlx_migrations` | none | migration ledger is DDL-lane only |

`ALTER DEFAULT PRIVILEGES` grants were revoked — every new table must
be granted explicitly by its own migration (post-check enforces).

### SECURITY DEFINER audit — `app_verify_membership`, `app_resolve_bootstrap`

- Ownership: `kooperatif` (owner runs with definer rights).
- `SET search_path = public, pg_catalog` pinned; all sensitive
  references fully qualified (`public.cooperative_memberships`, …).
- No dynamic SQL, no identifier parameters, STABLE, no function
  chaining — the shims call nothing SECURITY DEFINER themselves.
- EXECUTE revoked from PUBLIC, granted only to `kooperatif_app`.
- **Caller binding (P2B contract)**: `app_verify_membership` refuses to
  resolve a `p_user` different from a stamped `app.user_id` GUC. An
  unset GUC keeps the pre-context bootstrap lane working until P2B
  wires `session → app.user_id` stamping: the extractor will open
  `tenant_tx(NULL coop, user)` — i.e. stamp only the verified session
  user — then call the shim; the claim `p_user` becomes a checked
  assertion of the authenticated identity, never a lookup of someone
  else. `app_resolve_bootstrap` carries no user input and needs no
  binding.
- Residual risk until P2B: an unstamped connection can enumerate
  membership only by knowing BOTH UUIDs (unguessable) and learns only
  status/name/enabled — no row content. Accepted for P2A; the P2B
  wiring makes the GUC mandatory on business paths.

## 8. P2B addendum — authenticated tenant-context propagation (migrations 0034–0036)

### Identity binding, final form

`db::request_tx(pool, session_token_hash, cooperative_id, now)` implements the
pre-context chain in a single request-scoped transaction:

1. `public.app_open_request_context(token_hash, p_now)` (0034 + 0036,
   SECURITY DEFINER, `search_path` pinned, EXECUTE → `kooperatif_app`
   only) validates the session row — revoked / expired / idle-expired /
   disabled-user all reject — and stamps `app.user_id` tx-locally.
   `p_now` is the application-clock instant captured at extraction
   (`CurrentAuth.authed_at`): session rows are written by the app
   clock (`AuthRuntime`/`MutableClock` in tests), so expiry must be
   evaluated in the same clock domain — the database wall clock is not
   authoritative for session lifetime.
   The application hands over the token hash (the credential it already
   verified); the DATABASE derives the user id. A caller-supplied UUID
   is never trusted.
2. `app_verify_membership(user_id, coop)` — its 0033 caller-binding
   guard now fires: `p_user` must equal the stamped `app.user_id`. The
   claim is checked, not accepted.
3. Only after active membership in an active, business-enabled
   cooperative is proven is `app.cooperative_id` stamped — a valid
   session can never hop into a cooperative it does not belong to.

`db::request_user_tx` covers the user-scoped no-cooperative lane
(NeverBootstrapped legacy lane, session management): `app.user_id`
only, no cooperative stamp.

`TenantCtx::open_tx` / `BusinessAuth::open_tx` carry
`session_token_hash` on `CurrentAuth` so every business handler opens a
context-stamped transaction; repository signatures take
`db::DbSource` (pool, connection, or in-flight transaction/savepoint —
sqlx's diagonal `Acquire` impl was unusable in async handlers for
HRTB/`Send` reasons) instead of `&PgPool`.

### Special lanes (explicit, unchanged)

- Membership resolution / bootstrap probes / session writes run on the
  pool pre-context (`tenant/repo.rs`, `auth/*`) — they are what derives
  the context and need no GUC.
- Audit writes (`security_events`) keep their dedicated pool writer so
  an audit row survives the business transaction's rollback.
- Realtime revalidation re-reads session + scoped permissions per
  delivered signal on the pool — a stale socket can never ride a revoked
  session.
- `rbac` guarded mutations nest via savepoint:
  `authz::begin_guarded_transaction` now takes `&mut PgConnection`;
  inside a request tx sqlx emits a savepoint and the scoped advisory
  `xact` lock then holds until the OUTER commit — lockout-prevention
  remains atomic with the mutation.

### Grant audit resolution — the 39th table

`cooperative_number_counters` (cooperative_id NOT NULL since 0026) is
the business table not enumerable in the 0025 NOT NULL list: it received
full CRUD from the 0033 loop. The runtime only reaches it through the
SECURITY INVOKER allocator/reserve functions, which need
SELECT/INSERT/UPDATE but never DELETE — a deleted counter row would
silently restart the series. Migration 0035 revokes DELETE; the
fail-closed post-check pins the verb set. `cooperative_memberships` is
deliberately SELECT-only (membership writes are privileged CLI-lane
operations).

## 9. P3A addendum — RLS security pilot (migration 0037)

### Pilot scope and rationale

`periods` + `governance_bodies` — ENABLE + FORCE ROW LEVEL SECURITY,
two policies each:

```sql
CREATE POLICY <t>_owner_lane ON public.<t> TO kooperatif
    USING (true) WITH CHECK (true);
CREATE POLICY <t>_app_cooperative ON public.<t> TO kooperatif_app
    USING  (cooperative_id
        = nullif(current_setting('app.cooperative_id', true), '')::uuid)
    WITH CHECK (cooperative_id
        = nullif(current_setting('app.cooperative_id', true), '')::uuid);
```

Selection rationale: both are cooperative-owned with
`cooperative_id NOT NULL` (0025), neither carries monetary movements
(`periods` scopes collection windows, `governance_bodies` is
governance metadata — "evidence, not money"), `periods` is the only
business table with a real application DELETE (draft removal) so all
four verbs are exercised by production code paths, and neither is
reachable outside the verified `request_tx` envelope except the
bootstrap orphan scan (handled below). No NULL branch is needed in
the policy: post-0025 `cooperative_id` is NOT NULL, and the
NeverBootstrapped lane only exists on databases that predate tenancy
— where 0037 can never be applied.

### Bootstrap lane fix

The 37-table orphan scan previously ran per-table `EXISTS` probes as
the runtime principal — under RLS those probes would be silently
filtered (fail-open). It now runs through
`public.app_bootstrap_orphan_scan()` (SECURITY DEFINER, owner-side
visibility, pinned `search_path`, EXECUTE only to `kooperatif_app`,
PUBLIC revoked). `tenant::repo::bootstrap_initial_cooperative` calls
it once and keeps the same fail-closed error contract.

### Trust boundary — explicitly NOT a hard boundary

Item 4 requires honest accounting: `app.cooperative_id` is an
ordinary custom GUC. Demonstrated in
`tests/m1p4_rls_pilot.rs::context_spoofing_residual_risk_demonstrated`:

* `SET LOCAL app.cooperative_id = <foreign>` executed AS
  `kooperatif_app` reads the foreign cooperative's pilot rows —
  the policy cannot distinguish a forged GUC from a verified stamp.
* `kooperatif_app` holds SELECT on `user_sessions` — token hashes
  are readable, so `app_open_request_context` can be invoked with a
  victim's hash to mint their `app.user_id` context.

RLS therefore protects against application-layer scope mistakes
(a missing `WHERE cooperative_id`, a wrong bind) inside the verified
`request_tx` envelope — it is NOT a boundary against arbitrary SQL
executed with the runtime credential. The HTTP attack surface
remains safe because callers never reach the database directly and
the GUC is only stamped inside the request transaction after
session+membership verification, with pool-checkout residue wiped.

Hardening path for a real boundary (before fleet rollout):
1. Mint a cryptographic context seal inside the SECURITY DEFINER
   open-context function (`app.ctx_seal = HMAC(coop_id, secret)`,
   secret held in a table with no app grant; policy requires
   `seal` to match the stamped GUC) — direct `SET` then fails.
2. Remove `user_sessions` SELECT from `kooperatif_app`; route all
   session reads/writes through narrow SECURITY DEFINER accessors —
   otherwise a stolen hash still mints a sealed context for that
   user's own cooperatives.
3. Neither is implemented in P3A; both are required before calling
   RLS a trust boundary.

### Verification evidence

`tests/m1p4_rls_pilot.rs` — 9 tests on REAL `kooperatif_app`
connections (not SET ROLE): missing context → 0 rows + INSERT
rejected; invalid GUC → 0 rows; cross-coop read → 0; cross-coop
INSERT → WITH CHECK error; foreign UPDATE/DELETE → 0 rows;
`cooperative_id` reassignment → WITH CHECK error; rollback + pool
reuse → clean; role attributes (no superuser/bypassrls/ownership);
pilot bound (exactly 2 RLS tables); owner lane unaffected;
`request_tx` workflow works end-to-end as the app role.

`kooperatif_dev` deliberately NOT migrated: head stays v33, zero
RLS-enabled tables — pilot applies only to disposable test
databases via `run_test_migrations`.

## 10. P3B addendum — trust-boundary hardening (migration 0038)

### Sealed context — final form

The plain `app.cooperative_id` GUC proven forgeable in §9 is now
backed by an HMAC chain minted exclusively inside SECURITY DEFINER
functions; the key lives in `public.app_secrets` (`ctx-seal`, 32
random bytes, owner-only, zero grants):

1. `app_open_request_context(token_hash, now)` — resolves the session
   row, derives `user_id`+`session_id`, stamps tx-local
   `app.user_id`, `app.session_id`, `app.ctx_txid = txid_current()`
   and `app.user_seal = hmac(user|session|txid)`.
2. `app_stamp_cooperative(coop)` — recomputes the user seal from the
   stamped GUCs and refuses unless it matches AND the stamped txid
   equals `txid_current()` (same-transaction freshness), then runs
   `app_verify_membership`; only then stamps `app.cooperative_id` and
   `app.ctx_seal = hmac(user|session|coop|txid)`.
3. Pilot policies additionally require `public.app_ctx_ok()` —
   recompute-and-compare under the owner-only secret.

Consequences for a `kooperatif_app`-level arbitrary-SQL attacker:

* `SET LOCAL app.cooperative_id` alone → `app_ctx_ok` fails (no seal).
* Setting every GUC including a fabricated `app.ctx_seal` → fails
  (cannot compute hmac without the secret; `app_seal_of` is not
  granted to the runtime role).
* Replaying a captured complete GUC set on a fresh transaction →
  fails (`app.ctx_txid` binds `txid_current()`; ids differ).
* Calling `app_stamp_cooperative` with forged user/session GUCs →
  refused before membership is consulted (user-seal mismatch).

Pool checkout cleanup now resets all six context GUCs + ROLE.

### Session credential isolation — final form

`kooperatif_app` holds **no privilege** on `user_sessions`
(SELECT/INSERT/UPDATE/DELETE all revoked) and no access to the
`users` credential columns (`password_hash`, `login_verifier` are
column-invisible for read AND write; UPDATE narrowed to
`(status, updated_at)`; SELECT narrowed to non-credential columns).

All session operations run through credential-bound SECURITY
DEFINER functions (pinned `search_path`, EXECUTE granted only to
`kooperatif_app`, PUBLIC revoked):

| Function | Binding |
|---|---|
| `app_session_resolve(hash, now)` | returns validity projection, never the hash |
| `app_session_touch(hash, now, idle_ttl, min_age)` | hash-bound last-seen refresh |
| `app_session_open(user, pw, hash, csrf, now, ttls, label)` | **plaintext password verified against `users.login_verifier` (crypt/bcrypt) inside the DB** — sessions cannot be fabricated for unknown credentials; writes the reason-granular `login_failed` audit row itself and returns a uniform empty set on failure |
| `app_session_revoke(hash, session_id, now, reason)` | revokes only hash-owner sessions; `already_revoked`/`not_found` distinction stays server-side |
| `app_session_revoke_others(hash, now, reason)` | hash-owner's other sessions |
| `app_session_list(hash, now)` | hash-owner's sessions only |
| `app_session_set_cooperative(hash, coop)` | hash-bound AND membership-checked |
| `app_admin_revoke_user_sessions(user, now)` | RBAC disable path; invoker-broad but matches the pre-hardening UPDATE surface |

`users.login_verifier` is written by the privileged creation lane
(`users::create_user`, CLI) — `crypt(password, gen_salt('bf'))`.

### Residual risks (explicit)

* `app_admin_revoke_user_sessions` can revoke any user's sessions —
  DoS-only, mirrors the previous UPDATE surface; a later phase can
  scope it to stamped-context admins.
* `txid_current()` inside `app_ctx_ok` runs per evaluated row — under
  read-only transactions it requires an assigned xid, which
  `app_open_request_context` always assigns first; a hypothetically
  seal-free read context therefore fails closed by design.
* Argon2id remains the stored `password_hash`; `login_verifier`
  (bcrypt/crypt) is an auxiliary login-only verifier. If the two
  drift (direct password_hash edits bypassing `create_user`), login
  fails closed — an availability, not confidentiality, risk.
* Pilot scope unchanged: RLS+FORCE only on `periods` and
  `governance_bodies`; `kooperatif_dev` not migrated.

### Verification evidence

`tests/m1p4_rls_pilot.rs` — 13 tests on REAL `kooperatif_app`
connections: full P3A matrix retained plus forged-GUC rejection,
fully-forged-GUC-set rejection, seal replay across transactions
(txid mismatch), forged user context cannot stamp, session/credential/
secret isolation, `app_seal_of` not callable, `app_session_open`
credential binding.

## §11 M1-P4-P3C — final trust-boundary security gates

Migration `0039_p3c_security_gates.sql`. No RLS expansion — the pilot
remains exactly `periods` + `governance_bodies`.

### 1. Privileged session revocation

`app_admin_revoke_user_sessions(user, now)` no longer trusts the bare
caller. It now requires, inside the SAME transaction:

* a valid user seal (`app.user_seal` recomputed via `app_seal_of`
  over stamped `user|session|txid` and `app.ctx_txid =
  txid_current()`);
* a sealed cooperative context (`app.ctx_seal` match) with the caller
  holding `users.manage` in that scope — bootstrap-attribution-aware
  (NULL-attributed rows only through the marker-consistent bootstrap
  cooperative, mirroring `effective_permissions_in_bootstrap`) — and
  the target user member-visible in that scope;
* OR, in the never-bootstrapped lane, zero cooperatives and a global
  `users.manage` grant.

Every failure raises `insufficient_privilege` (42501) — fail-closed
AND loud, because the legitimate path (`set_user_status` in
`auth/rbac.rs`) always runs inside a sealed `request_tx` where the
RBAC permission was already resolved. Audit rows for the legitimate
disable are unchanged (the app-layer `user_disabled` event).

### 2. Login abuse resistance

`app_session_open` gained a database-enforced throttle:

* `pg_advisory_xact_lock(hashtextextended('app_session_open:' ||
  username))` serializes concurrent attempts per username — the
  counter check and the verdict insert are atomic per attempt.
* At most **10 verdict-bearing failures per username per 300 s**
  window (application clock `p_now`); further attempts return the
  uniform empty set — indistinguishable from a wrong password,
  so no account-enumeration oracle is added.
* Window expiry is automatic (purely time-bounded — no permanent
  lockout state exists).
* 'throttled' rows are audited but EXCLUDED from the counter, so a
  flood cannot extend its own lockout; app-side 'rate_limited' rows
  likewise do not feed the DB counter.

### 3. Cryptographic context review

Audited all 15 `public.*` SECURITY DEFINER functions: all have pinned
`search_path`, PUBLIC revoked, EXECUTE granted only where required.
Changes:

* `app_verify_membership` EXECUTE revoked from `kooperatif_app` — with
  an unstamped connection it resolved ARBITRARY (user, coop) pairs,
  an enumeration oracle. Only definer-internal callers remain
  (`app_stamp_cooperative`, `app_session_set_cooperative`), which do
  not need EXECUTE.
* `allocate_document_number` / `reserve_document_number` are now
  SECURITY DEFINER: when `session_user = 'kooperatif_app'` they
  require `app_ctx_ok()` AND the stamped cooperative to equal the
  requested scope. Direct table grants on
  `cooperative_number_counters` (SELECT/INSERT/UPDATE/DELETE) are
  fully revoked — arbitrary SQL can no longer burn or inflate a
  foreign cooperative's numbering sequence. The owner lane
  (migrations, fixtures, CLI, SET ROLE sessions) is unaffected.

Same-transaction attacks tested and rejected: overwriting
`app.cooperative_id`, `app.ctx_seal` or `app.user_id` inside a live
sealed tx fails closed (recompute mismatch poisons the whole
context); re-stamping a foreign coop in the same tx refuses and
leaves the original context intact; `app_open_request_context` with
a garbage hash returns NULL without clearing a live context.

### 4. Credential lifecycle

Audit result: the ONLY writer of `password_hash`/`login_verifier` is
`users::create_user` (CLI provisioning lane) — both are written
atomically in one INSERT, so they cannot drift through any code path.
No password-change or password-reset workflow EXISTS yet
(`grep change_password|reset_password` → nothing). **Documented
blocker:** when such a workflow is implemented it MUST update both
verifiers in one statement (or migrate `password_hash` to a single
bcrypt verifier); until then the risk is theoretical only — the
runtime principal cannot write either column (column-level grants
exclude both), so no drift vector exists at the SQL boundary.
Disabling a user revokes sessions via the now-authorized admin
function and `app_session_open` rejects non-active users — the
verifiers remain consistent by construction.

### Verification evidence

`tests/m1p4_rls_pilot.rs` — 18 tests on REAL `kooperatif_app`
connections (13 P3B + 5 new):

* `admin_revoke_requires_sealed_administrative_context` — raw SQL
  denied, forged GUCs denied, sealed non-admin denied, out-of-scope
  target denied, legitimate sealed admin revoke succeeds (1 row).
* `same_transaction_context_tampering_is_rejected` — GUC/seal/user
  overwrite mid-tx fail closed; refused re-stamp preserves context.
* `login_throttle_blocks_excessive_attempts_and_recovers` — 10
  verdicts, 11th correct-password attempt denied inside the window,
  'throttled' audited, recovery after window expiry.
* `granted_function_surface_cannot_act_without_authorization` —
  garbage-hash session API calls refused/uniform, `app_verify_membership`
  not executable, unsealed allocation denied, sealed same-scope
  allocation works, foreign-scope allocation denied, counter table
  unreadable.
* `concurrent_login_attempts_stay_under_the_cap` — 15 parallel
  attempts serialize under the advisory lock: exactly 10 verdicts +
  6 throttled audit rows, no double-count, no session opened.

Suite: `auth` 15, `rbac` 11, `rbac_cooperative` 11,
`authz_cooperative` 19, `m1p4_tenant_context` 19, `m1p4_rls_pilot` 18
focused — full backend suite **601 tests / 0 failures** (was 596).
`cargo fmt --check` clean, `clippy --all-targets` 0 warnings.
`kooperatif_dev` untouched (head v33, no RLS).

### Residual risks (explicit)

* `app_bootstrap_orphan_scan` / `app_resolve_bootstrap` remain
  executable pre-context by design (bootstrap lane) — they disclose
  only table-name/bootstrap facts, no row data.
* `app_session_open` is throttled per-username at the DB; an
  attacker spreading attempts across many usernames still faces the
  app-level IP limiter, which is deliberately separate.
* Dual verifier (`password_hash` argon2 + `login_verifier` bcrypt)
  remains until a password-change/reset workflow lands — see §4
  blocker note above.

## §12 M1-P4 — Controlled RLS fleet rollout, Batch 1 (migration 0040)

The sealed-context model from migrations 0038–0039 is extended to six
additional cooperative-owned tables. Selection rationale: all six are
dependency **leaves** — no other table holds a foreign key INTO them,
so enabling `FORCE ROW LEVEL SECURITY` cannot break referential
integrity checks on other tables, and every application access path
already runs inside the sealed `request_tx` envelope (`open_tx`
→ `app_stamp_cooperative` → `app_ctx_ok()`).

Selected tables:

| Table                        | Why batch 1                                        |
|------------------------------|-----------------------------------------------------|
| `income_entries`             | financial inflow ledger; leaf; high protection value |
| `expense_entries`            | financial outflow ledger; leaf                       |
| `payment_allocations`        | payment→debt allocation history; leaf; append-mostly |
| `shareholder_family_memberships` | identity join fact; leaf                        |
| `governance_votes`           | recorded vote evidence; leaf                         |
| `investment_incomes`         | investment yield records; leaf                       |

Policies per table mirror the pilot exactly: `*_owner_lane` for
`kooperatif` (`USING (true) WITH CHECK (true)`) and
`*_app_cooperative` for `kooperatif_app`
(`cooperative_id = app.cooperative_id GUC AND public.app_ctx_ok()`,
both `USING` and `WITH CHECK`). Post-checks assert ENABLE+FORCE on all
six, exactly 2 policies each, and that no table outside
pilot+batch-1 carries RLS.

Access-path audit findings:

* All repo queries touching these tables run inside `ctx.open_tx`
  (sealed `request_tx`) — verified across `income_expense`,
  `investments`, `payments`, `credits`, `governance`, `parties`.
* Sub-selects on batch tables keyed by parent ids
  (`payment_allocations WHERE payment_id = $1`,
  `governance_votes WHERE decision_id = $1`, …) stay consistent: the
  parent row was already scope-verified in the same sealed tx, and the
  RLS filter only removes foreign rows the query could never
  legitimately resolve.
* SECURITY DEFINER surfaces (`app_bootstrap_orphan_scan`, numbering
  allocators, session API) execute as `kooperatif` → owner lane covers
  them unchanged.
* `tenant_tx` (test/pre-context shim) runs on the owner pool — owner
  lane unaffected; it never drives `kooperatif_app` connections.
* Outbound composite FKs (`(cooperative_id, parent_id)` from 0022)
  are unaffected: FK enforcement is not subject to RLS.

### Verification evidence

`tests/m1p4_rls_pilot.rs` — 6 new batch-1 tests on REAL
`kooperatif_app` connections:

* `batch1_missing_context_fails_closed` — every batch table returns 0
  rows unauthenticated; raw INSERT is denied by the RLS WITH CHECK.
* `batch1_forged_guc_context_sees_nothing` — raw `set_config` stamp
  without the seal chain sees zero rows on all six tables.
* `batch1_cross_cooperative_reads_are_isolated` — sealed coop-A tx
  sees own row only on all six tables.
* `batch1_cross_cooperative_writes_and_reassignment_rejected` —
  foreign UPDATE/DELETE affect 0 rows; `cooperative_id` reassignment
  on own rows is rejected, on all six tables.
* `batch1_legitimate_workflow_still_works` — sealed tx can
  SELECT/UPDATE/INSERT own rows and the committed write stays visible.
* `batch1_rollback_and_reuse_do_not_leak` — pooled checkout after
  rollback sees nothing.

Boundary assertions updated (additive, same bound):
`m1p4_rls_pilot` and `m1p4_tenant_context` now assert exactly the
eight RLS tables (pilot + batch-1) and 16 policies total.

### Residual risks

* 37 cooperative-owned tables remain outside RLS — they keep relying
  on `request_tx` scope verification until later batches.
* `financial_categories` global seeds are bootstrap-attributed; the
  batch tables referencing them via composite FKs are unaffected.

## 13. Batch 2 rollout — receivables / collection cluster (0041)

### Selection

Six tables forming the receivables/collection chain, closed
end-to-end because the chain's endpoints were already forced
(`periods` in the pilot, `payment_allocations` in batch 1):

| Table                      | Why it belongs                              |
|----------------------------|---------------------------------------------|
| `assessment_rules`         | per-period obligation rule snapshot         |
| `assessments`              | durable per-shareholder obligation          |
| `assessment_share_sources` | per-share provenance of an assessment       |
| `payments`                 | the collection receipt (money in)           |
| `shareholder_credits`      | unallocated remainder credit                |
| `credit_applications`      | credit applied to an obligation             |

Dependency view (inbound referrers): `assessments` is referenced by
`payment_allocations` (batch-1, already RLS), `assessment_share_sources`
and `credit_applications` (both in this set); `payments` by
`payment_allocations` (RLS) and `shareholder_credits` (in-set);
`shareholder_credits` by `credit_applications` (in-set). Every
referrer is either already under RLS or inside the batch, so no
partial-visibility anomaly can appear along the chain. Inbound-FK
count alone was NOT the criterion — referential enforcement is not
governed by RLS; the audit instead traced which *application paths*
resolve rows across these tables.

Policies per table mirror batches 0–1 exactly (`*_owner_lane` +
`*_app_cooperative` with the GUC + `public.app_ctx_ok()` check).
Post-checks: ENABLE+FORCE on all six, exactly 2 policies each, and no
RLS table outside pilot + batch-1 + batch-2.

### Access-path audit findings

* Every repo access runs inside `ctx.open_tx` / sealed `request_tx`
  (verified across `payments`, `credits`, `periods`, `investments`,
  `social_aid`, `reports`, `financial_accounts`).
* Numbering allocators (`payments.payment_number`,
  `shareholder_credits.credit_number`, `shares.share_number`)
  execute as `kooperatif` via the P3C-hardened definer; under
  `session_user = 'kooperatif_app'` they additionally require a
  matching sealed scope — unchanged by this batch.
* `app_bootstrap_orphan_scan` reads metadata through the owner lane —
  unaffected.
* Realtime notify triggers only `pg_notify` — no table access.
* `tenant_tx` (test/pre-context shim) runs on the owner pool —
  unaffected.
* Composite FKs (`(cooperative_id, parent_id)`) continue to enforce
  regardless of RLS visibility.

### Verification evidence

`tests/m1p4_rls_pilot.rs` — 6 new batch-2 tests on REAL
`kooperatif_app` connections, mirroring the batch-1 matrix:

* `batch2_missing_context_fails_closed` — all six tables return 0
  rows; raw INSERT is denied by the RLS WITH CHECK.
* `batch2_forged_guc_context_sees_nothing` — raw `set_config` stamp
  without the seal chain sees zero rows.
* `batch2_cross_cooperative_reads_are_isolated` — sealed coop-A tx
  sees own rows only on all six tables.
* `batch2_cross_cooperative_writes_and_reassignment_rejected` —
  foreign UPDATE/DELETE affect 0 rows; `cooperative_id` reassignment
  on own rows is rejected.
* `batch2_legitimate_workflow_still_works` — sealed tx
  SELECT/UPDATE/INSERT on own rows, committed writes stay visible
  (including an allocator-backed `payments` insert inside the sealed
  tx, proving the definer + RLS interplay).
* `batch2_rollback_and_reuse_do_not_leak` — pooled checkout after
  rollback sees nothing.

Boundary assertions updated (additive, same bound):
`m1p4_rls_pilot` and `m1p4_tenant_context` now assert exactly the
fourteen RLS tables (pilot + batch-1 + batch-2) and 28 policies.

### PoolTimedOut diagnosis (batch-2 deliverable)

Measured root cause of the batch-1 full-suite flakes:

* `max_connections = 100` (`superuser_reserved = 3` → 97 usable).
* `cargo test` defaults to `--test-threads = num_cpus = 28` per
  binary; each parallel test chain holds an admin pool (max 5) + a
  dedicated claim/heartbeat connection + a test pool (5) + an
  optional `kooperatif_app` pool (5) — worst-case demand ≈ 28 × 16 ≈
  450 connections.
* Every `setup()` additionally runs a leader-elected GC sweep and a
  15s `COMMENT ON DATABASE` heartbeat — shared-catalog DDL convoys
  under load, stretching acquisitions past the 3s `acquire_timeout`
  → `PoolTimedOut` on `admin connect` / provisioning, never inside
  RLS assertions.

Mitigation (infrastructure-safe, no test weakened):
`.cargo/config.toml` sets `RUST_TEST_THREADS = 6`, bounding
per-binary parallelism so worst-case connection demand (≈ 96) stays
under the server limit. Mirrors the existing `M1-INFRA-001`
`build.jobs = 4` machine-bound precedent.

### Residual risks

* 31 cooperative-owned tables remain outside RLS — `request_tx`
  scope verification still protects them until later batches.
* The 332 accumulated `kooperatif_test_*` orphans keep GC sweeps
  non-trivial; they age out through the grace-window path — no manual
  cleanup was performed (out of scope).

## 14. Batch 3 rollout — investment lifecycle + governance decisions (0042)

### Inventory reconciliation (batch-3 deliverable)

Enumerated directly from the current schema (`kooperatif_test` at
v41): 53 `public` tables total.

| Category                                        | Count |
|-------------------------------------------------|-------|
| cooperative-owned, `cooperative_id NOT NULL`    | 39    |
| system-lane, `cooperative_id` NULLABLE          | 3     |
| platform/global, no `cooperative_id` column     | 11    |

The 39-table inventory = 42 tables carrying `cooperative_id` minus
the 3 nullable system-lane tables (`roles`, `security_events`,
`user_role_assignments` — bootstrap/audit attribution lanes that
structurally cannot become NOT NULL). After pilot(2) + batch-1(6) +
batch-2(6) = 14 forced tables, the NOT-NULL remainder is 25 — the
earlier "31 remaining" was the post-batch-1 figure (39 − 8). The
inventory is consistent.

Remaining NOT-NULL cooperative-owned tables without RLS (25):
`account_movements`, `account_transfers`, `cooperative_memberships`,
`cooperative_number_counters`, `families`, `financial_accounts`,
`financial_categories`, `governance_memberships`,
`investment_disposals`*, `investment_disposal_proceeds`*,
`investment_fundings`*, `investment_valuations`*, `investments`*,
`persons`, `share_events`, `share_ownerships`,
`share_return_entitlements`, `share_return_settlements`,
`share_returns`, `shareholders`, `shares`,
`social_aid_disbursements`, `social_aid_donations`, `social_aid_funds`,
`governance_decisions`* (* = selected for batch 3).

### Selection

Six tables closing the investment lifecycle chain plus the
governance decision record — both chains' referrers are already
RLS-forced (`investment_incomes`, `governance_votes` — batch 1) or
inside the set:

| Table                        | Why it belongs                    |
|------------------------------|-----------------------------------|
| `investments`                | the investment asset              |
| `investment_fundings`        | cash outflow funding the asset    |
| `investment_valuations`      | recorded valuation snapshots      |
| `investment_disposals`       | the disposal event                |
| `investment_disposal_proceeds`| cash inflow from a disposal      |
| `governance_decisions`       | the decision a vote belongs to    |

Policies per table mirror prior batches exactly; post-checks assert
ENABLE+FORCE on all six, exactly 2 policies each, and no RLS table
outside pilot + batch-1/2/3 (20 tables, 40 policies total).

### Access-path audit findings

* All repo access runs inside `ctx.open_tx` / sealed `request_tx`
  (verified: `investments/repo.rs`, `investments/routes.rs`,
  `governance/repo.rs`, `reports/repo.rs` via `open_tx_repeatable`).
* Numbering allocators (`investments.investment_number`,
  `investment_fundings.funding_number`,
  `investment_valuations.valuation_number`,
  `investment_disposals.disposal_number`,
  `governance_decisions.decision_number`) run through the
  P3C-hardened definer — owner lane for fixtures, sealed scope for
  the runtime.
* `account_movements.source_id` is polymorphic (no FK) — fixture
  chains create the movement first, then the funding/proceeds row;
  no RLS interplay since `account_movements` itself is not yet
  forced.
* `app_bootstrap_orphan_scan` and `tenant_tx` stay on the owner
  lane — unaffected.

### Verification evidence

`tests/m1p4_rls_pilot.rs` — 6 new batch-3 tests on REAL
`kooperatif_app` connections: missing-context fail-closed + INSERT
rejection, forged-GUC zero visibility, cross-cooperative SELECT
isolation, foreign UPDATE/DELETE + `cooperative_id` reassignment
rejection on all six tables, legitimate sealed workflow (including an
allocator-backed `investments` insert inside the sealed tx), and
rollback/pool-reuse non-leakage.

Boundary assertions updated (additive): `m1p4_rls_pilot` and
`m1p4_tenant_context` now assert exactly the twenty RLS tables and
40 policies.

### Residual risks

* 19 NOT-NULL cooperative-owned tables remain outside RLS —
  `request_tx` scope verification protects them until later batches.
* The 3 nullable system-lane tables (`roles`, `security_events`,
  `user_role_assignments`) are deliberately out of the fleet — their
  bootstrap/legacy NULL-attribution lanes would break under a naive
  `cooperative_id = GUC` policy; a dedicated system-lane policy design
  is required before they can join the rollout.

## 15. Batch 4 rollout — identity + share-ownership cluster (0043)

### Selection

Six tables covering the person/family/shareholder identity and the
share-ownership chains:

| Table             | Why it belongs                          |
|-------------------|-----------------------------------------|
| `persons`         | the physical person record              |
| `families`        | the family unit                         |
| `shareholders`    | the cooperative member party            |
| `shares`          | the share certificate                   |
| `share_events`    | append-only share lifecycle events      |
| `share_ownerships`| temporal ownership assignments          |

Dependency view (inbound referrers): `persons` ← payments,
governance_votes, shareholder_family_memberships (all already RLS),
governance_memberships, social_aid_donations/disbursements
(deferred), shareholders (in-set); `shareholders` ← assessments,
shareholder_credits, shareholder_family_memberships (RLS),
share_events/ownerships (in-set), share_returns/entitlements
(deferred); `shares` ← assessment_share_sources (RLS),
share_events/ownerships (in-set), share_returns (deferred);
`families` ← shareholder_family_memberships (RLS). Every deferred
referrer is composite-FK-bound to the same cooperative, so no
partial-visibility anomaly can arise; they join the fleet in later
batches.

Policies per table mirror prior batches exactly; post-checks assert
ENABLE+FORCE on all six, exactly 2 policies each, and no RLS table
outside pilot + batch-1/2/3/4 (26 tables, 52 policies total).

### Access-path audit findings

* All repo access runs inside `ctx.open_tx` / sealed `request_tx`
  (verified: `parties`, `shares`, `share_returns`, `payments`,
  `credits`, `periods`, `social_aid`, `governance`, `reports`).
* `share_returns` (deferred) inserts `share_events` rows inside the
  sealed tx with the same cooperative stamp — WITH CHECK passes;
  unaffected.
* `persons` inserts inside the payment/party flows run under the
  sealed scope; the user-only `request_user_tx` lane (no cooperative
  stamp) cannot reach business endpoints — the tenant gate rejects
  them before any query.
* `authz.rs` "persons/shareholders" matches are permission constants
  only — no table access outside sealed tx.
* Composite FKs guarantee referrers and parents share a cooperative;
  FK enforcement is not RLS-filtered, so cross-coop composite
  references remain rejected (verified by test).

### Deferred tables

`governance_memberships`, `social_aid_donations`,
`social_aid_disbursements`, `share_returns`,
`share_return_entitlements`, `share_return_settlements` reference
into this batch but stay unforced for now — their own chains
(social-aid and share-return domains) open as coherent groups in
later batches. Their composite FKs already bind them to the same
cooperative, and their access paths are sealed-tx only, so deferral
creates no isolation gap on *their* rows (RLS is applied per-table).

### Verification evidence

`tests/m1p4_rls_pilot.rs` — 6 new batch-4 tests on REAL
`kooperatif_app` connections: missing-context fail-closed + INSERT
rejection, forged-GUC zero visibility, cross-cooperative SELECT
isolation, foreign UPDATE/DELETE + `cooperative_id` reassignment
rejection on all six tables, composite-FK rejection of a cross-coop
`share_events` parent inside a sealed tx (FK enforcement is not
RLS-filtered), legitimate sealed workflow (visibility, touch updates,
in-scope INSERT, `share_ownerships`⋈`shares` join), and
rollback/pool-reuse non-leakage.

Boundary assertions updated (additive): `m1p4_rls_pilot` and
`m1p4_tenant_context` now assert exactly the twenty-six RLS tables
and 52 policies.

### Residual risks

* 13 NOT-NULL cooperative-owned tables remain outside RLS —
  `request_tx` scope verification protects them until later batches:
  `account_movements`, `account_transfers`, `cooperative_memberships`,
  `cooperative_number_counters`, `financial_accounts`,
  `financial_categories`, `governance_memberships`,
  `share_return_entitlements`, `share_return_settlements`,
  `share_returns`, `social_aid_disbursements`,
  `social_aid_donations`, `social_aid_funds`.
* The 3 nullable system-lane tables stay out of the fleet (see §14).

## 16. Batch 5 — share-return and social-aid chains (migration 0044)

Six tables carry `ENABLE` + `FORCE` row security with the established
owner/application policy pair — **both chains close completely** (the
only foreign-key referrers into this set are in-set):

* `share_returns` — the return request and its lifecycle.
* `share_return_entitlements` — rights crystallized at finalization.
* `share_return_settlements` — cash outflow settling an entitlement.
* `social_aid_funds` — the social-aid fund.
* `social_aid_donations` — cash inflow into a fund.
* `social_aid_disbursements` — cash outflow to a beneficiary.

`cooperative_memberships` and `cooperative_number_counters` stay
excluded — they need a dedicated security design (membership drives
the seal chain itself; counters sit fully behind the P3C definer
allocators).

* Access path: every repo path runs inside the sealed
  `ctx.open_tx`/`request_tx` (`share_returns`, `social_aid` verified);
  outbound FKs to already-forced parents (`shares`, `shareholders`,
  `persons`) or deferred financial-core tables (`financial_accounts`,
  `account_movements`) are enforced independently of RLS.
* Migration post-checks bound the fleet at pilot + batch 1-5 =
  **32 tables / 64 policies**; each batch table carries exactly two
  policies.
* Real-`kooperatif_app` tests (migration 0044):
  `batch5_missing_context_fails_closed`,
  `batch5_forged_guc_context_sees_nothing`,
  `batch5_cross_cooperative_reads_are_isolated`,
  `batch5_cross_cooperative_writes_and_reassignment_rejected`,
  `batch5_legitimate_workflow_still_works`,
  `batch5_rollback_and_reuse_do_not_leak` — all under
  `apps/server/tests/m1p4_rls_pilot.rs`.

### Remaining unprotected cooperative-owned tables

Seven NOT NULL cooperative-owned tables remain outside the fleet;
`request_tx` scope verification protects them until later design:

* `account_movements`, `account_transfers` — financial-core ledger;
  polymorphic `source_id` (no FK) needs a dedicated design.
* `financial_accounts`, `financial_categories` — referenced by nearly
  every financial table; closing the ledger set is the last step.
* `cooperative_memberships` — drives the seal chain itself; a query
  on it must work *before* the seal exists (dedicated design).
* `cooperative_number_counters` — already fully behind P3C
  SECURITY DEFINER allocators.
* `governance_memberships` — references `persons`; residual leaf.

The 3 nullable system-lane tables stay out of the fleet (see §14).

## 17. Batch 6 — financial core + governance memberships (migration 0045)

Five tables carry `ENABLE` + `FORCE` row security with the established
owner/application policy pair — the financial core **closes
completely** (every referrer is already forced or in-set):

* `financial_accounts` — cash/bank accounts.
* `financial_categories` — income/expense categories.
* `account_movements` — the money ledger; polymorphic `source_id`
  (no FK, nothing for RLS to interact with).
* `account_transfers` — net-zero account-to-account moves.
* `governance_memberships` — body membership windows; its only
  referrer is the already-forced `governance_votes`, so it is a safe
  independent leaf.

* Access path: every repository write runs inside the sealed
  `ctx.open_tx`/`request_tx` with `cooperative_id` stamped. The
  bootstrap category seed and the `bootstrap-cooperative` CLI run on
  the administrative pool — the owner lane covers them. The only
  SECURITY DEFINER touching the set is `app_bootstrap_orphan_scan`
  (owner `kooperatif`); the owner lane keeps its NULL-orphan probes
  correct. Realtime triggers only call `pg_notify`.
* Migration post-checks bound the fleet at pilot + batch 1-6 =
  **37 tables / 74 policies**.
* Real-`kooperatif_app` tests (migration 0045):
  `batch6_missing_context_fails_closed`,
  `batch6_forged_guc_context_sees_nothing`,
  `batch6_cross_cooperative_reads_are_isolated`,
  `batch6_cross_cooperative_writes_and_reassignment_rejected`
  (incl. a cross-coop composite-FK rejection on
  `account_movements→financial_accounts`),
  `batch6_legitimate_workflow_still_works` (movement→account join +
  in-scope movement insert inside the sealed tx),
  `batch6_rollback_and_reuse_do_not_leak` — all under
  `apps/server/tests/m1p4_rls_pilot.rs`. Financial invariants are
  covered by the unmodified business suite (payments/credits/
  reservation/share_returns/social_aid binary tests all pass under
  the sealed context).

### Remaining unprotected cooperative-owned tables

Two NOT NULL cooperative-owned tables remain outside the fleet, both
awaiting dedicated security designs (they are not ordinary business
rows — one feeds the seal chain itself, the other is already fully
behind P3C SECURITY DEFINER allocators):

* `cooperative_memberships`
* `cooperative_number_counters`

The 3 nullable system-lane tables stay out of the fleet (see §14).

## 18. S1 — cooperative_number_counters (migration 0046)

Per the special-tables audit, the counters table is the only one of
the five security-sensitive tables that can take FORCE RLS without a
code refactor: kooperatif_app holds zero direct grants and every
allocation/reservation flows through the P3C SECURITY DEFINER
allocators (`allocate_document_number`, `reserve_document_number`;
owner = kooperatif, sealed-context + scope gate inside).

* Policy set: exactly ONE policy — `cooperative_number_counters_owner_lane`
  (`kooperatif`, USING(true)/WITH CHECK(true)). No app policy exists;
  under FORCE RLS that is a full deny for the runtime role — the
  intended surface.
* Grant-vs-RLS nuance verified in tests: privilege checks run before
  RLS, so the proof temporarily GRANTs kooperatif_app full table
  privileges owner-side, then shows SELECT/UPDATE/DELETE still see
  zero rows and INSERT is rejected — RLS is what actually denies; the
  privileges are revoked again in the same test.
* Sealed-context allocation still works (definer bodies execute as
  owner → owner lane); foreign-coop allocate/reserve is refused by the
  allocator's own seal gate; rollback does not consume a number;
  concurrent allocations stay unique and monotonic; owner/admin direct
  operations are unaffected.
* Migration post-checks bound the fleet at pilot + batch 1-6 + S1 =
  **38 tables / 75 policies**.
* Tests (real `kooperatif_app` connections):
  `s1_counters_invisible_to_app_even_in_sealed_tx`,
  `s1_sealed_allocation_and_reservation_work`,
  `s1_forged_guc_cannot_allocate`,
  `s1_rollback_and_concurrency_preserve_numbering`,
  `s1_owner_lane_operations_unaffected` — under
  `apps/server/tests/m1p4_rls_pilot.rs`.

### Remaining unprotected tables

* `cooperative_memberships` — needs the definer-mediated authz/context
  refactor (audit §5/§6 S4) before RLS.
* `roles`, `user_role_assignments`, `security_events` — the nullable
  system lane; need the S2 definer layer (audit §6) plus the
  bootstrap-bound NULL-attribution lane.

## 19. S2A — credential-bound authorization read layer (migration 0047)

Code + migration step; no table gained RLS here — this is the refactor
the special-tables audit required before `cooperative_memberships`,
`roles`, `user_role_assignments` (and later `security_events`) can take
FORCE RLS without total lockout.

### What changed

* Nine `app_session_*` SECURITY DEFINER functions (owner =
  `kooperatif`, pinned `search_path`, PUBLIC revoked). The caller
  presents only the session token hash; `app_session_owner` (internal,
  NOT granted to `kooperatif_app`) derives the principal from the
  session row using the P3B validity rule (not revoked, absolute+idle
  expiry on the app clock, user active). Caller-supplied `user_id` is
  gone from every pre-context authorization read.
* Request-facing readers (EXECUTE → `kooperatif_app`):
  `app_session_membership`, `app_session_memberships`,
  `app_session_business_membership_ok`,
  `app_session_permissions_scoped`, `app_session_role_summaries_scoped`
  (NULL-attribution arm encoded — fires only when the scope re-verifies
  as the marker-consistent bootstrap cooperative),
  `app_session_permissions_legacy`, `app_session_role_summaries_legacy`
  (never-bootstrapped predicate re-verified inside the function).
* Internal helpers (`app_session_owner`, `app_session_legacy_lane_ok`)
  are reachable only through the definer chain — no EXECUTE grant.
* Call-site migration: `TenantCtx`/`BusinessAuth` membership
  verification, tenant gate (explicit header + headerless lanes),
  cooperative switch + `/api/cooperatives`, login + `/me`
  authorization context, `require_business` (both scopes), realtime
  revalidation + subscribe-time permission sets.
* NOT migrated (deliberately): sealed-transaction RBAC reads in
  `rbac.rs` (`scoped_visibility`, `member_visibility`,
  `administration_path_count`) still run as `kooperatif_app` inside
  `request_tx` — they are S4/S5 RLS policy inputs, not unsealed reads.
  Table grants therefore remain unchanged in this step.

### Verified (real `kooperatif_app` connections)

* `s2a_readers_fail_closed_without_a_valid_session` — garbage / revoked /
  expired credentials resolve to zero rows / false on every reader.
* `s2a_readers_are_bound_to_the_session_owner` — own memberships only;
  foreign cooperative and non-member pairs are indistinguishable from
  nonexistence.
* `s2a_permissions_cannot_be_escalated` — own scope returns exactly the
  granted keys; foreign scope empty; definer answer proven equal to the
  raw owner-lane resolution; legacy lane empty on a tenanted database.
* `s2a_readers_leave_no_context_residue` — no `app.*` GUC is stamped;
  pool reuse is clean.
* `s2a_function_surface_is_bounded` — helper EXECUTE denial, reader
  EXECUTE grants, definer + pinned-path post-checks.
* `audit.rs` untouched — `security_events` write path and its
  swallow-and-log semantics are unchanged (S3 scope).
* Existing suites cover the migrated call sites end-to-end:
  `tenant_kernel`, `tenant_isolation_gate`, `realtime`, `rbac`, `auth`.

### Residual notes for S3–S5

* `never_bootstrapped`'s `security_events` probe still runs on the bare
  pool: under S3 RLS it would see zero coop-attributed rows — but the
  `cooperatives`/marker conditions still gate it correctly because
  `security_events.cooperative_id` carries an FK to `cooperatives`
  (coop-attributed events cannot exist without a cooperative row).
  Re-verify at S3 anyway.
* `cooperative_memberships` still holds a direct SELECT grant for
  `kooperatif_app` — required by the sealed-tx joins above until S4.

## 20. S2B — sealed security-event WRITE boundary (migration 0048)

S1/S2A secured reads; S2B redesigns how `security_events` rows are
written. RLS is still NOT enabled on the table (S3 scope); this step
makes the write surface exclusive to hardened definers so that S3 can
switch on FORCE RLS without losing a single event.

### Pre-S2B behavior (documented before changing)

* Every writer called `audit::record` / `audit::record_scoped`, which
  ran a bare `INSERT INTO security_events` on whichever pool was passed.
* Identity (`user_id`, `session_id`, `cooperative_id`) was supplied by
  the caller — the database never verified it.
* Business-route calls made inside an open request transaction still
  wrote via the POOL (separate connection): an audit row could commit
  for an operation that later rolled back (phantom audit), and the
  inverse (operation committed, audit lost) was equally possible.
* Every failure was logged and swallowed, including events that should
  have been atomic with the protected action.
* `kooperatif_app` held a direct INSERT grant — any SQL the runtime
  principal could issue could fabricate events with arbitrary actor /
  cooperative attribution.
* `app_session_open` (0039) already minted `login_failed` rows inside
  the definer — that path was correct and is unchanged.

### Write-path inventory and classification

| Writer | Caller auth | Attribution | Semantics after S2B |
|---|---|---|---|
| Business/RBAC mutation events (~80 sites, 12 route files) | authenticated, sealed tx | seal-derived coop | MANDATORY — `record_tx` inside request_tx; failure aborts the op |
| `logout`, `session_revoked`, `sessions_revoked_all_others` | authenticated | session owner | MANDATORY — folded into `app_session_revoke(_others)` (same tx as the revocation; also fixes the dead-session trap: a just-revoked token cannot resolve a post-hoc write) |
| `cooperative_context_switched` | authenticated | session owner + verified coop | MANDATORY — folded into `app_session_set_cooperative` |
| `login_failed` minted in `app_session_open` (unknown user, disabled, bad credentials, DB throttle) | unauthenticated/pre-credential | NULL | MANDATORY inside the definer (unchanged 0039 path) |
| `login_succeeded` | just-authenticated | session owner, NULL coop | BEST-EFFORT — `app_session_audit_platform`; a valid login must never 500 because an audit write failed; loud `tracing::error` on failure |
| `lockout_prevented` (last-administration-path refusals) | authenticated | verified membership coop | BEST-EFFORT — `app_session_audit_scoped`; the protected tx already rolled back and cannot carry the row, so an independent lane is required; denial is already final, failure must not change it |
| HTTP-layer `login_failed` (malformed username, app-side rate limit) | unauthenticated | NULL, no event-type choice | BEST-EFFORT — `app_audit_login_failed`; must never expose internals or block authentication |
| CLI/owner events (`user_created`, `bootstrap_role_granted`, `cooperative_created`, `cooperative_bootstrapped`, `cooperative_membership_granted`) | owner lane (`kooperatif` role) | as before | unchanged — `record`/`record_scoped` retained for the privileged pool only |

### New definer boundary (all SECURITY DEFINER, `search_path =
public,pg_catalog`, fully qualified, PUBLIC revoked, EXECUTE only to
`kooperatif_app`)

* `app_ctx_audit(event, metadata)` — sealed-tx writer. Verifies the
  ctx_seal on the cooperative lane OR the user_seal on the platform/
  never-bootstrapped lane; a stamped cooperative can never degrade to
  NULL attribution (forged coop GUC is rejected, not downgraded).
* `app_session_audit_platform(hash, type, meta, now)` — session-bound,
  cooperative forced NULL, whitelisted to `login_succeeded`.
* `app_session_audit_scoped(hash, coop, type, meta, now)` — session-
  bound, whitelisted to `lockout_prevented`, requires an active
  membership of the session owner in the claimed cooperative; NULL coop
  only on the re-verified never-bootstrapped lane.
* `app_audit_login_failed(meta, now)` — the only unauthenticated lane:
  hardcoded `login_failed`, NULL user/session/cooperative.
* `app_session_revoke`, `app_session_revoke_others`,
  `app_session_set_cooperative` — CREATE OR REPLACE (same signatures)
  minting their events atomically in the definer.
* `REVOKE INSERT ON security_events FROM kooperatif_app` — direct
  writes are closed for the runtime role; SELECT stays (the
  `never_bootstrapped` probe).

### Verified (real `kooperatif_app` connections, 9 new tests)

* `s2b_ctx_audit_writes_sealed_attribution_and_rolls_back` — event row
  inside the open tx is invisible to a second connection, carries ONLY
  seal-derived identity, and rollback erases it (no phantom audit).
* `s2b_ctx_audit_fails_closed_without_a_seal` — bare connection cannot
  write; forged GUCs cannot.
* `s2b_user_seal_lane_writes_null_attribution_and_rejects_forged_coop` —
  user-seal-only tx writes NULL-attributed event; a forged
  `app.cooperative_id` on that lane is rejected (no downgrade).
* `s2b_platform_lane_is_session_bound_and_whitelisted` — non-whitelisted
  event type and dead session rejected; valid call writes NULL-coop
  row owned by the session user.
* `s2b_scoped_session_writer_enforces_membership_scope` — foreign
  cooperative rejected; own cooperative accepted.
* `s2b_login_failed_lane_cannot_impersonate` — metadata-supplied ids
  never reach the row (user/session/cooperative stay NULL).
* `s2b_revocation_writes_its_event_atomically` — `app_session_revoke`
  mints `session_revoked` in the same call; dead-session post-write
  trap closed.
* `s2b_context_switch_is_audited_atomically` — member switch writes the
  event; non-member switch refuses AND leaves no audit row.
* `s2b_direct_insert_is_denied_to_the_runtime_role` — direct INSERT
  denied, SELECT retained.
* `m1p4_tenant_context::app_role_holds_only_the_minimal_grant_surface`
  updated: `security_events` INSERT moved to the denied list.

### Residual notes for S3–S5

* `security_events` RLS is still OFF. The definer boundary already
  makes the runtime write path exclusive, so S3 can enable FORCE RLS
  with only the owner-lane policy — no application change needed.
* `record`/`record_scoped` remain owner-lane-only helpers in audit.rs;
  they must never be wired to the `kooperatif_app` pool.
* The best-effort lanes intentionally do not block: their failures are
  observable via `tracing::error` but there is no metric yet — wire a
  counter when the metrics surface exists.

## 21. S3 — FORCE RLS on `security_events` (migration 0049)

With S2B making the definer boundary the exclusive write path, S3
closes the table entirely.

### Preflight findings

* Owner `kooperatif` (superuser+BYPASSRLS — RLS does not bind it; the
  owner policy is belt-and-braces); `kooperatif_app` is neither.
* After S2B the only direct runtime dependency left was the
  `never_bootstrapped` probe (`tenant/repo.rs`), whose fourth
  condition reads `security_events` for audit-evidence. Under FORCE
  RLS that arm would silently see zero rows — weakening tamper
  detection. The identical predicate already existed definer-side as
  `app_session_legacy_lane_ok()` (0047), so the probe was repointed to
  it and EXECUTE granted (zero-arg boolean — a narrower surface than
  the SELECT it replaced).
* All remaining test reads use the owner pool.

### Migration 0049

* `GRANT EXECUTE app_session_legacy_lane_ok()` to `kooperatif_app`
  (probe reachability).
* ENABLE + FORCE RLS + `security_events_owner_lane` policy on
  `kooperatif` only — no app policy is justified: every runtime access
  is inside a `kooperatif`-owned definer.
* `REVOKE SELECT` — with INSERT already gone (0048) the table is fully
  closed to the runtime role.
* Post-checks: RLS enabled+forced, exactly 1 policy, zero direct
  privileges for `kooperatif_app`, probe grant present, fleet bound
  39 tables / 76 policies.

### Verified (real `kooperatif_app` connections)

* `s3_security_events_fully_closed_to_runtime_role` — direct
  SELECT/INSERT/UPDATE/DELETE all denied.
* `s3_rls_independently_enforced_under_temporary_grants` — under a
  temporary owner-lane `GRANT SELECT,INSERT,UPDATE,DELETE` the app
  role still sees zero rows, INSERT hits the WITH CHECK wall,
  UPDATE/DELETE affect zero rows (invisible), and even a sealed
  cooperative tx sees nothing — there is no app policy at all. Grants
  revoked within the disposable fixture.
* `s3_definer_writers_and_probe_survive_force_rls` — sealed ctx write,
  session-bound platform write, anonymous login-failure write all mint
  rows under FORCE RLS (definers run as owner); rolled-back sealed
  write leaves no row; `app_session_legacy_lane_ok()` answers false on
  a tenanted DB — the audit-evidence arm is not RLS-blinded.
* Assertion updates: fleet list now includes `security_events`
  (39 tables / 76 policies); `s2a_function_surface_is_bounded` now
  expects the probe EXECUTE grant; throttle tests count events on the
  owner lane; `s2b` direct-INSERT test now also asserts SELECT denied;
  `m1p4_tenant_context` denied-list gained the SELECT case.
* Regressions green: auth (login/throttle/history), business_auth
  (never_bootstrapped lanes incl. audit-evidence-loss), tenant_kernel,
  tenant_isolation_gate, rbac, realtime — all unchanged semantics.

### Residual notes for S4–S5

* `cooperative_memberships`, `roles`, `user_role_assignments` remain
  the last three unprotected special tables — their sealed-tx reads
  (rbac.rs CRUD, `administration_path_count`) still need direct grants
  until S4/S5 land.
* `record`/`record_scoped` in audit.rs remain owner-lane-only helpers.

## 22. S4 — FORCE RLS on `cooperative_memberships` (migration 0050)

`cooperative_memberships` is the table every other scope decision joins
against — sealing it closes the last direct read path the runtime role
still had on authorization data.

### Access-path inventory

* **Owner lane (unchanged):** `bootstrap_initial_cooperative`,
  `add_member` (CLI in `main.rs`) — owner pool, unaffected by FORCE RLS
  (superuser/BYPASSRLS).
* **Definer lane (unchanged):** `app_session_membership`,
  `app_session_set_cooperative`, `app_stamp_cooperative`,
  `app_session_legacy_lane_ok`, realtime revalidation — all
  `kooperatif`-owned SECURITY DEFINER.
* **Sealed-tx lane (needs the app policy):** `member_visibility`,
  `administration_path_count`, rbac EXISTS joins — run inside
  `request_tx` transactions and require same-transaction
  visibility/locking, so they keep a narrowly scoped direct read:
  `m.cooperative_id = sealed coop` only.
* **Dead at runtime:** pool-based helpers
  (`effective_permissions_in_cooperative` et al.) are only invoked by
  owner-pool tests/CLI paths — verified no live route path uses them
  on the app pool.
* No runtime query ever reads NULL-cooperative membership rows
  directly; NULL-lane facts flow exclusively through definers.

### Migration 0050

* ENABLE + FORCE RLS on `cooperative_memberships`.
* `cooperative_memberships_owner_lane` — `kooperatif`, ALL, true/true
  (CLI/bootstrap mutations).
* `cooperative_memberships_app_scope` — `kooperatif_app`, ALL,
  USING/WITH CHECK `public.app_ctx_ok() AND cooperative_id =
  public.app_current_cooperative()` — sealed transactions see only
  their verified cooperative's rows; unsealed or forged contexts see
  none; NULL rows never match (NULL = NULL is not true).
* SELECT grant retained (the sealed-join lane needs it); INSERT,
  UPDATE, DELETE were never granted — nothing left to revoke.
* `roles` and `user_role_assignments` explicitly untouched (S5).
* Post-checks: RLS enabled+forced, exactly 2 policies, NULL-row
  policy evaluation proven unsatisfiable, fleet bound
  40 tables / 78 policies.

### Verified (real `kooperatif_app` connections)

* `s4_membership_reads_are_sealed_scope_only` — unsealed direct read
  sees zero rows; sealed tx sees only its own cooperative's rows.
* `s4_rls_independently_enforced_under_temporary_grants` — under a
  temporary owner-lane `GRANT INSERT,UPDATE,DELETE`: in-scope insert
  succeeds inside a sealed tx; cross-cooperative and NULL-cooperative
  inserts each rejected by WITH CHECK in their own sealed tx (abort
  isolation — a rejected statement poisons its tx); cross-coop
  UPDATE/DELETE affect zero rows. Grants revoked in the fixture.
* `s4_legitimate_membership_paths_still_work` — sealed-tx stamping,
  cooperative switch (definer + atomic audit), scoped authorization,
  administration-path counting all green under FORCE RLS.
* Regressions green: rbac, rbac_cooperative, rbac_cooperative_schema,
  authz_cooperative, business_auth, tenant_kernel,
  tenant_isolation_gate, auth, realtime — membership suspension,
  lockout prevention, bootstrap, dual-membership switching, realtime
  revalidation all unchanged.

### Residual notes for S5

* `roles` and `user_role_assignments` remain the last two unprotected
  authorization tables — rbac CRUD still reads them in sealed txs.
* `app_ctx_ok()`-gated visibility means a sealed tx can read ALL rows
  of its own cooperative (any member's id/status), not only the
  caller's row — acceptable: member_visibility intentionally answers
  "does user X belong here". Per-row caller scoping would break RBAC
  administration views.

## 23. S5 — FORCE RLS on `roles` + `user_role_assignments` (migration 0051)

The last two special tables. With S5 every cooperative_id-bearing
table in the schema carries FORCE RLS (verified: zero exceptions).

### Access-path inventory

* **Owner/CLI:** `main.rs` user administration (role lookup,
  assignment insert) — superuser/BYPASSRLS, unaffected.
* **Definer (all pre-context authz reads):**
  `app_session_permissions_scoped/legacy`,
  `app_session_role_summaries_scoped/legacy` — owner lane inside
  definers.
* **Sealed tx (direct, needs the app policy):** rbac.rs CRUD and
  `administration_path_count`, always through `scoped_visibility`:
  scope sees own rows + NULL legacy rows only when the scope
  re-verifies as the bootstrap cooperative; NULL scope
  (request_user_tx) sees NULL rows only while zero cooperatives
  exist.
* **Dead at runtime:** `effective_permissions*`,
  `active_role_summaries*`, `require*` — owner-pool tests only; all
  route callers moved to S2A credential-bound definers.
* Legitimate NULL rows: migration-seeded platform templates
  ('Sistem Yöneticisi' et al.) and pre-tenancy assignments — visible
  only in the bootstrap scope or the pre-tenancy user lane.

### Migration 0051

* `app_user_ctx_ok()` — the missing user-seal predicate (HMAC verify
  of `app.user_seal` with NULL cooperative + txid freshness),
  mirroring `app_ctx_ok()`. GUCs + owner-only secret only — no RLS
  recursion.
* ENABLE + FORCE RLS on both tables; owner lane + ONE app policy each
  reproducing `scoped_visibility` at the row level:
  `(app_ctx_ok() AND (coop = sealed OR (NULL AND bootstrap-marker
  reverified))) OR (app_user_ctx_ok() AND coop IS NULL AND NOT EXISTS
  (SELECT 1 FROM cooperatives))`. USING = WITH CHECK.
* Grants unchanged — already minimal and all exercised: roles
  SELECT/INSERT/UPDATE; user_role_assignments SELECT/INSERT/DELETE.
* Post-checks: RLS on both, 2 policies each, exact grant surfaces,
  app_user_ctx_ok EXECUTE, fleet bound 42 tables / 82 policies.

### Verified (real `kooperatif_app` connections)

* `s5_unsealed_access_is_denied_and_invisible` — zero visibility,
  INSERT hits WITH CHECK, UPDATE/DELETE affect zero rows.
* `s5_sealed_scope_isolates_roles_and_assignments` — own-coop only;
  cross-coop update/delete no-op; cross-coop and NULL inserts
  rejected; in-scope role create + assignment succeed.
* `s5_forged_gucs_reveal_nothing` — hand-set GUCs satisfy neither
  seal arm.
* `s5_bootstrap_scope_inherits_null_rows_only` — bootstrap scope sees
  own + NULL (incl. seeded 'Sistem Yöneticisi'); a non-bootstrap
  scope sees neither NULL nor foreign rows and cannot mint NULL.
* `s5_never_bootstrapped_lane_opens_then_closes` — pre-tenancy user
  lane manages NULL rows; once a cooperative exists the same seal
  sees zero rows and cannot write — NULL scope can never become
  unrestricted post-tenancy.
* `s5_rls_independently_enforced_under_temporary_grants` — granted
  DELETE/UPDATE verbs still affect zero foreign rows; cooperative_id
  reassignment rejected. Grants restored in fixture.
* Regressions green: rbac (all four binaries incl. lockout guard,
  NULL-lane management, cross-coop rejection), authz_cooperative,
  business_auth, tenant_kernel, tenant_isolation_gate, auth,
  realtime — last-admin protection, self-promotion denial, coop
  switching and realtime revalidation unchanged.

### M1-P4 RLS end state

42 FORCE-RLS tables / 82 policies. Every cooperative_id-bearing
table is covered. Runtime role can reach authorization data only
through: (a) credential-bound definers, (b) sealed cooperative
transactions scoped to the verified cooperative (+ bootstrap-pinned
NULL lane), or (c) the pre-tenancy user lane that provably cannot
outlive bootstrap. Remaining non-RLS tables are system/auth data
without cooperative_id (`users`, `user_sessions`, `permissions`,
`role_permissions`, `cooperatives`, `installation_state`,
`_sqlx_migrations`, `app_secrets`, `document_number_series`) —
`users`/`user_sessions`/`app_secrets` are already sealed behind
definer APIs + column-narrowed grants (P3B); `role_permissions` is
reachable only through scoped-role FK chains.

### Residual notes for the closure audit

* `role_permissions` remains grant-guarded (SELECT/INSERT/DELETE to
  app) without RLS — its rows carry no cooperative_id; scope
  enforcement rides on the roles join. If the audit wants it
  covered, scope derives through `roles` — assess whether a
  subquery-based policy is worth it vs. FK + grant discipline.
* `users` SELECT is column-narrowed, not row-scoped — membership
  visibility for admin views is enforced in queries
  (`member_visibility`), not RLS. Listing non-member users is
  prevented at the query layer; a table-level proof would need a
  membership-derived policy.
* The bootstrap scope's NULL-row manageability intentionally
  survives RLS (legacy administration); the marker+flag reverification
  keeps it from ever applying to a non-bootstrap cooperative.

## 24. R1 — RBAC database boundary (migration 0052)

The closure audit's two CRITICAL findings are remediated: the sealed
context alone no longer authorizes RBAC mutations — every write goes
through a permission-re-verifying SECURITY DEFINER, and the direct
write grants are revoked.

### Findings reproduced (pre-fix state)

* F1: `role_permissions` INSERT/DELETE/SELECT, no RLS, no
  cooperative_id — an unsealed `kooperatif_app` connection could
  graft any permission onto any role or wipe the map entirely.
* F2: sealed-scope INSERT on `user_role_assignments`/`roles` passed
  the WITH CHECK for own-cooperative rows — any member could
  self-assign the admin role at the SQL layer; permission checks
  lived only in the HTTP layer.

### Boundary (migration 0052)

* Internal helpers (no EXECUTE for the app role):
  `app_rbac_in_scope` (row-cooperative ↔ sealed-scope predicate,
  bootstrap-NULL and pre-tenancy arms),
  `app_admin_path_count` (ports `administration_path_count`),
  `app_rbac_verified_scope` (seal verification +
  `roles.manage`/`users.manage` re-verified inside the database;
  NULL scope only while zero cooperatives exist).
* Mutators (EXECUTE granted): `app_role_create`,
  `app_role_update`, `app_role_set_status`,
  `app_role_permissions_set`, `app_user_roles_set`. Each re-verifies
  the sealed context AND the required permission, enforces row scope
  (role_permissions scope is enforced through the parent role —
  never the bare FK), and raises `lockout_prevented`
  (check_violation) when the mutation would drop the last
  administration path. New roles are attributed to the sealed
  cooperative from the verified GUC — never caller input.
* Owner lane: non-app session_user skips the permission check
  (trusted CLI/fixture lane) but still takes attribution from the
  stamped GUCs so owner-pool sealed transactions write their real
  cooperative — this fallback is what the test server's owner pool
  exercises.
* Grants revoked: `roles` → SELECT only (was SELECT,INSERT,UPDATE);
  `user_role_assignments` → SELECT only (was SELECT,INSERT,DELETE);
  `role_permissions` → SELECT only (was SELECT,INSERT,DELETE).
* rbac.rs rewired: create/update/set_status/set_permissions/
  set_user_roles call the definers inside the existing guarded tx —
  the cooperative-scoped advisory lock and same-transaction audit
  (`audit::record_tx`) are unchanged; `lockout_prevented` maps to
  LockoutPrevented with the existing session-bound denial audit.
* Post-checks in the migration: zero residual write grants, exactly
  three SELECTs retained, exactly five mutator EXECUTEs, internal
  helpers unreachable.

### Verified (real `kooperatif_app` connections, 7 new tests)

* `r1_role_permissions_direct_write_denied` — unsealed AND sealed:
  INSERT/DELETE/UPDATE on role_permissions all 42501.
* `r1_sealed_member_has_no_rbac_mutation_path` — the F2 exploit
  (member self-assigning the admin role) is a grant-layer error;
  every definer raises insufficient_privilege for a member.
* `r1_authorized_admin_mutations_work_through_definers` — verified
  admin creates (scope-attributed), updates, sets permissions and
  assignments; rollback leaves nothing.
* `r1_cross_cooperative_rbac_mutation_denied` — foreign role update/
  permission-graft/status/assignment all denied.
* `r1_platform_role_immutable_outside_bootstrap_scope` — NULL role
  unreachable from a non-bootstrap scope.
* `r1_lockout_protection_inside_boundary` — perm-strip, role-disable
  and last-assignment-removal all raise lockout_prevented with tables
  untouched.
* `r1_forged_guc_cannot_invoke_mutators` — hand-set GUCs and unsealed
  calls both raise 42501.
* S5 assertions updated to the new invariant: sealed-scope direct
  writes are now denied everywhere (grant layer), not merely
  scope-filtered; the never-bootstrapped lane keeps NULL reads but
  requires NULL-arm permission through the definers.
* Regressions green: rbac×3, authz_cooperative, business_auth,
  tenant_kernel, tenant_isolation_gate, auth, full pilot binary.

### Residual notes (for R2+)

* `role_permissions` SELECT remains granted (read joins in sealed
  txs) — the global enumeration surface noted in the audit is
  unchanged by R1 (F6).
* HTTP-layer tests run on the owner pool: the definer permission
  gate is exercised by the app-pool R1 tests, not by the HTTP suite
  (owner lane skips it by design).
* `users` UPDATE(status)/SELECT scoping, business-table DELETE
  hygiene and audit event-type whitelisting remain open audit
  findings — deliberately untouched in R1.

## 25. R2 — users database privilege boundary (migration 0053)

Closure-audit **F3** remediation. Reproduce (pre-fix, real
`kooperatif_app` connection on `kooperatif_test` v52): cross-coop
`UPDATE users SET status='disabled'` → `UPDATE 1` (oturumsuz global
kilitleme); `SELECT count(*) FROM users` → 1102 satır enumeration;
`SELECT password_hash` → zaten 42501 (kolon görünmezliği tutuyordu).

Boundary:
- `users` ENABLE+FORCE RLS; `users_owner_lane` (kooperatif, ALL) +
  `users_app_scope` (kooperatif_app, FOR SELECT): mühürlü kapsam
  yalnız kapsam-üyesi satırları görür (üyelik satırı her statüde —
  member_visibility eşdeğeri); user-seal NULL kolu yalnız
  `NOT EXISTS cooperatives` iken açılır (pre-tenancy).
- Tek mutasyon kulvarı `app_user_set_status(user_id, status)`:
  `app_rbac_verified_scope('users.manage')` + üyelik görünürlüğü +
  `app_admin_path_count` son-admin koruması tek statement'ta.
- Revoke: `UPDATE(status, updated_at)`; SELECT `created_at,
  updated_at, last_login_at` — kalan okuma: id, username,
  display_name, status. EXECUTE yüzeyi +1 (`app_user_set_status`);
  iç yardımcılar kapalı kaldı.
- İş anlamı notu: `users.status` global kimlik özniteliği — çok-
  kooperatifli kullanıcı herhangi bir üyelik kooperatifinin admin'i
  tarafından kapatılabilir (member_visibility semantiği korundu;
  coop-bazlı status spec değişikliği ister — R2 dışı).

Tests (`m1p4_rls_pilot`): `r2_users_direct_write_denied`,
`r2_users_enumeration_is_scope_bound` (suspended-member görünürlüğü
+ credential kolonlar dahil), `r2_user_set_status_definer_paths`,
`r2_shared_user_status_is_global_by_membership`,
`r2_user_disable_lockout_inside_boundary`,
`r2_forged_guc_cannot_set_user_status`,
`r2_never_bootstrapped_lane_opens_then_closes`.
`m1p4_tenant_context` boundary testi güncellendi (UPDATE/3 kolon
denied'a taşındı); filo 43 tablo / 84 politika.

## 26. R3 — financial history & grant-surface hardening (migration 0054)

Closure-audit **F4** remediation. Pre-fix repro (gerçek `kooperatif_app`,
mühürlü coop tx, `kooperatif_test` v53): `UPDATE payments SET amount=5`
→ UPDATE 1; `UPDATE account_movements SET amount=1, direction='outflow'`
→ UPDATE 1; `DELETE FROM payments`/`account_movements` → DELETE 1'er.
Post-fix: hepsi `42501` / 0 satır.

### Grant yüzeyi (öncesi → sonrası)

- DELETE: 37 business tablo → **yalnız `periods` + `assessment_rules`**
  (draft-cleanup) ve orada bile RESTRICTIVE guard: dönem `status='draft'`
  değilse, kuralın ebeveyn dönemi draft değilse silme/update/insert
  reddedilir.
- UPDATE: 7 tabloda tamamen revoke (append-only: assessments,
  assessment_share_sources, investment_disposals,
  investment_disposal_proceeds, share_events, families,
  governance_votes); 30 tabloda **kolon-bazlı** grant — repository
  SET listeleri birebir. amount/payer_person_id/account_id/
  cooperative_id/created_by/source_* gibi tarihsel alanlar runtime
  yüzeyinden tamamen çıktı.
- RESTRICTIVE FOR UPDATE terminal guard'ları (18 tablo): tek izinli
  geçiş forward transition (posted/active/recorded/pending →
  reversed/cancelled/closed/disposed/finalized). Terminal satır
  frozen — aynı tx'te bile geri alınamaz, ikinci yazım 0 satır.
- RESTRICTIVE FOR INSERT live guard'ları (18 tablo): terminal
  status'lü satır basılamaz.
- RLS kapsam politikaları, `*_coop_fk` kompozitleri ve reversal-
  atıf CHECK'leri değişmedi — migration yalnız grant daraltması +
  restrictive predikat ekliyor.

### Tests (m1p4_rls_pilot, gerçek `kooperatif_app`)

`r3_posted_financial_history_is_immutable` (10 kolon-yazım + 12 tablo
DELETE probe'u → hepsi 42501),
`r3_terminal_transitions_are_forward_only` (18 tabloda meşru geçiş
tek tx'te → frozen + attribution'sız reversal → 23514),
`r3_terminal_rows_cannot_be_inserted` (pre-reversed/pre-disposed
insert → 42501),
`r3_deletion_is_draft_confined` (draft rule/period silinir; open
ebeveyn donuk; open-period'a kural graft reddedilir),
`r3_operational_edits_still_work_attribution_frozen` (hesap/kategori/
kişi/ortak/pay/karar/konf edits geçer; attribution kolonları 42501),
`r3_cross_cooperative_financial_mutation_denied`.
Batch testleri yeni invariant'a güncellendi: `cooperative_id` artık
hiçbir tabloda yazılabilir kolon değil → foreign/in-scope yazımlar
42501 (grant katmanı, RLS'ten önce).

### Residual notes (R4+)

- ~~Lifecycle geçiş yazımları doğrudan SQL'de~~ → **R3B (0055) ile
  kapatıldı**, aşağıda §27.
- ~~`share_return_entitlements` kolon-grant'lı ama geçiş-guard'sız~~ →
  R3B: tüm yazım izin-doğrulamalı definer'a taşındı.
- ~~`periods` UPDATE guard'sız~~ → R3B: `status` grant'i kaldırıldı;
  `draft→open→closed` definer-precondition'lı.
- Açık: F5 (audit event-type whitelist), F6 (enumeration).
- **Ayrı closure item:** `users.status` global-vs-coop-bazlı kararı
  audit dokümanında açık madde olarak kayıtlı — semantik değişmedi.

## 27. R3B — financial reversal authorization hardening (migration 0055)

R3 residual kanıtı (v54, gerçek `kooperatif_app`, mühürlü tx, izinsiz
üye): `UPDATE payments SET status='reversed', reversal_reason='fabricated'`
→ **UPDATE 1**; `UPDATE financial_accounts SET status='inactive'` →
UPDATE 1. Post-fix: `UPDATE payments SET status='reversed'…` →
**42501**; `app_payment_reverse(...)` aynı aktör için →
`insufficient_privilege: permission payments.manage required`.

### Mimari

- **39 dar SECURITY DEFINER** (`app_*`): her biri tek domain geçişi,
  `app_rbac_verified_scope('<module>.manage')` ile mühürlü bağlam +
  üyelik + rol iznini DB içinde yeniden doğrular; kendi transition
  precondition'ını (status domaini, terminal guard) taşır.
- **Scope** çağrıcıdan değil mühürlü bağlamdan türetilir; satır
  `cooperative_id`'si stamped scope'a `IS NOT DISTINCT FROM` eşleşir
  (NULL = pre-tenancy kulvarı). `v_restricted` app-rolü VEYA sealed
  bağlamda true — mühürsüz owner bağlantısı (migrasyon/CLI/fixture/
  repo testleri) kısıtsız kalır, ama app-rolü için geniş bypass yok.
- `app_movements_reverse_of_source` izni `source_type`'tan türetir —
  `payments.manage` sahibi social-aid hareketini tersleyemez.
- `app_rbac_verified_scope_any` yalnız pay-sahipliği aralığı kapanışı
  gibi meşru çift-izinli işlemler için (`shares.manage` ∨
  `share_returns.manage`); helper EXECUTE'u `kooperatif_app`'e kapalı.
- EXECUTE 39 fonksiyona ayrı ayrı grant'li; helper'lar kapalı.

### Grant yüzeyi (post-0055)

- Lifecycle/terminal kolonlarının tamamı (30+ tabloda status,
  reversed_*, cancelled_*, finalized_*, ended_*, entitlement alanları,
  karar sayaçları) runtime UPDATE yüzeyinden çıktı.
- Kalan kolon grant'leri yalnız draft/konfigürasyon: assessment_rules
  (3 alan + updated_at), financial_accounts (name/desc/bank/iban/
  updated_at), financial_categories (name/desc/updated_at),
  governance_decisions draft alanları, periods draft alanları,
  persons (3 alan + updated_at), shareholders (guardian/account/
  updated_at), shares.updated_at (dokunma kolonu — sahiplik-aralığı
  kapanışından sonra repo bump'ı; status definer-only).
- DELETE yüzeyi değişmedi (periods + assessment_rules draft-only).
- 0054 RESTRICTIVE guard'ları defense-in-depth olarak duruyor;
  policy sayısı 124.

### Testler

- `r3b_lifecycle_transitions_require_verified_permission`: mühürsüz
  bağlantı reddi; izinsiz üyede 7 definer → 42501; doğrudan lifecycle
  UPDATE 42501; izinli aktörde definer geçişi + atıf doğrulaması;
  terminal re-reverse no-op; cross-coop definer 0-etki.
- `r3_terminal_transitions_are_forward_only` güncellendi: meşru geçişler
  artık definer üzerinden (izinli aktör); frozen probe'ları 42501
  seviyesinde.
- `r3_operational_edits_still_work_attribution_frozen`: status/ended_at
  yazımları definer'a taşındı.
- `m1p4_tenant_context`: `payments.status` UPDATE artık reddediliyor +
  `app_payment_reverse` EXECUTE assertion'ı eklendi.
- Tüm finansal HTTP suitleri (payments, financial_accounts,
  income_expense, credits, investments, share_returns, social_aid,
  periods, shares, parties, governance — 107 test) owner+mühürlü
  kulvarda definer semantiğiyle yeşil.

## 28. R3C — atomic financial reversal integrity (migration 0056)

R3B residual kanıtı (v55, gerçek `kooperatif_app`, mühürlü tx,
`payments.manage` izinli üye): `SELECT app_payment_reverse(id, ...)` tek
başına → payment `reversed` ama kaynak hareket `active`, bakiye 100 —
*reversal, kaynak satır + telafi etkilerini atomik yapmalı* invariantı
ihlal edildi. Aynı sınıf: `app_movements_reverse_of_source` (satır
geçişi olmadan hareket), `app_transfer_reverse`, settlement/donation/
disbursement definers, `app_share_return_finalize` (ownership+event+
entitlement atlanabilir), `app_investment_cancel`/`app_period_open`/
`app_social_aid_fund_close`/`app_governance_decision_open`
(önkoşullar repo'da kalıyordu), `app_governance_decision_finalize`
(çağrıcı üretimi sayım snapshot'ı), `app_share_set_status` ve
ownership-end helper'ları (eventsız/ardılsız kapanış), transfer.

### Mimari

- **24 paket definer** (`app_*_tx`): kilit + scope + izin + önkoşul +
  telafi hareket/yan-yazı + satır geçişi TEK atomik çağrı. Runtime rolü
  eksik paket üretemez.
- **Verified actor**: sealed kulvarda `p_by` çağrıcıdan bağımsız —
  `app.user_id` mühüründen türetilir (sahte atıf imkansız).
- **Owner kulvarı** korunuyor: mühürsüz owner bağlantısında `p_coop`
  pin + caller `p_by` geçerli (CLI/fixture/repo testleri).
- İki-kulvar semantics: `v_restricted` (app rolü ∨ sealed) → scope
  mühürden; aksi hâlde `p_coop`.
- Governance tally artık definer içinde hesaplanıyor — çağrıcı üretimi
  snapshot kabul edilmiyor.
- Bileşen/alt-seviye definers (27 fonksiyon) PUBLIC + `kooperatif_app`
  EXECUTE'undan düşürüldü; owner-internal çağrılar paket içinden.

### Grant yüzeyi (post-0056)

- EXECUTE `kooperatif_app`'e açık: 24 `*_tx` paketi +
  status-only/derived definers (`app_credit_application_reverse`,
  `app_payment_allocation_reverse`, `app_family_membership_end`,
  `app_governance_body_close`, `app_governance_decision_cancel`,
  `app_governance_membership_end`, `app_investment_valuation_cancel`,
  `app_period_close`, `app_share_return_entitlement_determine`/
  `refresh`, `app_financial_*_set_status`, session/role/user/admin
  yardımcıları).
- Kapalı: `app_movements_reverse_of_source`, `app_payment_reverse`,
  `app_transfer_reverse`, income/expense/credit/investment reversals,
  `app_investment_dispose`, `app_investment_cancel`, `app_period_open`,
  `app_share_return_*` (cancel/finalize/settlement_reverse/mark_pending/
  entitlement_cancel), `app_share_set_status`, `app_share_transfer`,
  `app_share_ownership_end(+open_interval)`, `app_shareholder_set_status`,
  `app_social_aid_*_reverse`, `app_social_aid_fund_close`/`cancel`,
  `app_governance_decision_open`/`finalize` (eski imza DROP edildi).

### Testler

- `r3c_incomplete_reversal_package_is_impossible` (yeni, gerçek
  `kooperatif_app`): mühürsüz ret; izinsiz üyede paket 42501; izinli
  aktörde bileşen definer EXECUTE yok (3 probe); tam paket tek
  çağrıda payment+movement+allocation+credit tersliyor; mühürsüz
  `p_by` yerine seal atfı; `credit_effect_blocked`'da TAM rollback;
  concurrent iki tx'te tam olarak bir gerçek reversal + bir replay;
  cross-coop P0002; settlement paketi aynı sözleşme.
- `r3_terminal_transitions_are_forward_only` paketlere çevrildi
  (sıralama bağımlı: uygulama → payment; disbursement → donation).
- `r3_operational`: status geçişleri `_tx` paketleriyle.
- Fixture düzeltmeleri: transfer çift bacak, settlement/donation/
  disbursement hareketlerinin `source_id`'si artık satır id'si
  (üretim yazımıyla uyumlu).
