# M1 — TENANT ISOLATION TEST MATRIX

Covers M1-D10. Levels: **DB** = PostgreSQL integration (real docker
postgres, `KOOPERATIF_TEST_DATABASE_URL`), **HTTP** = axum router tests
(real DB where marked), **E2E** = real browser (Playwright harness
pattern, `scripts/e2e-*`). Fixtures: coops A/B/C, users U1(A only),
U2(A:admin,B:treasurer), U3(B only), U4(none). Every scenario assumes
auth already passed — isolation is tested, not login.

Legend: expected status `404` = existence-hidden (`docs/21:86`);
`403` = membership/permission deny; `400` = missing/invalid ctx.

## T1 — Context & membership

| # | Setup | Action | Expected | DB assertion | Perm | Level |
| --- | --- | --- | --- | --- | --- | --- |
| T1.1 | U1 member of A only | GET `/api/shareholders` w/ `X-Cooperative-Id:A` | 200 A rows only | — | any | HTTP+DB |
| T1.2 | U1 | same w/ `X-Cooperative-Id:B` | 403 `coop_membership_denied` | zero queries reach rows | — | HTTP |
| T1.3 | U2 (A+B) | same w/ B header | 200 **B only** rows | — | B-scoped | HTTP+DB |
| T1.4 | U2 | no header, no session default | 400 `coop_context_required` | — | — | HTTP |
| T1.5 | U2 | `POST /api/auth/cooperative {B}` then request w/o header | session default used, B rows | `user_sessions.active_cooperative_id=B` | — | HTTP+DB |
| T1.6 | U2 membership B suspended | any B-header request | 403 immediately | sessions NOT revoked | — | HTTP+DB |
| T1.7 | U4 (no memberships) | login → `/api/auth/me` | 200, `cooperatives:[]`; any coop API → 403/400 | — | — | HTTP |
| T1.8 | forged header `X-Cooperative-Id:<random-uuid>` | any request | 403/404, never implicit coop | — | — | HTTP |

## T2 — IDOR / cross-tenant record access

| # | Setup | Action | Expected | DB assertion | Perm | Level |
| --- | --- | --- | --- | --- | --- | --- |
| T2.1 | B owns shareholder S_B | U2-as-A: `GET /api/shareholders/{S_B}` | **404** | — | A.shareholders.read | HTTP+DB |
| T2.2 | B owns payment P_B | U2-as-A: `GET /api/payments/{P_B}` | 404 | — | A.payments.read | HTTP+DB |
| T2.3 | B owns account F_B | U2-as-A: `GET /api/financial-accounts/{F_B}` | 404 | — | — | HTTP+DB |
| T2.4 | sweep: all 36 coop tables' list+get endpoints | A ctx, B ids | 404 everywhere, no 403 | — | — | HTTP (parameterized) |
| T2.5 | U2-as-A `PUT /api/shareholders/{S_B}` | modify B row | 404; B row unchanged byte-for-byte | `updated_at` unchanged | A.write | HTTP+DB |

## T3 — Cross-tenant writes / link attempts

| # | Setup | Action | Expected | DB assertion | Level |
| --- | --- | --- | --- | --- | --- |
| T3.1 | A payment to `destination_account_id=F_B` | POST payments | 422/`cross_tenant_reference` + FK violation net | `count(payments)` unchanged; **no movement** | HTTP+DB |
| T3.2 | A payment allocating B's `assessment_id` | POST w/ allocations | reject, tx rolled back fully | payments/allocations/movements counts unchanged | HTTP+DB |
| T3.3 | share transfer `from=A shareholder, to=B shareholder` | POST transfers | reject | `share_ownerships` unchanged | HTTP+DB |
| T3.4 | A expense with B `category_id` | POST expenses | reject (composite FK) | no row | DB+HTTP |
| T3.5 | A income reversal `reversal_of_id=<B income>` | POST | 404/reject | no reversal row; B income untouched | HTTP+DB |
| T3.6 | A shareholder w/ `default_collection_account_id=F_B` | POST/PUT | reject | — | HTTP+DB |
| T3.7 | social-aid donation: fund A, account B | POST | reject | fund balance unchanged | HTTP+DB |
| T3.8 | A idempotency_key == B idempotency_key (same string) | POST payments both coops | **both succeed** — `(coop,key)` scope | 2 rows | DB |
| T3.9 | direct SQL: `INSERT payment_allocations (A payment, B assessment)` | psql-level | composite FK violation | rolled back | DB |
| T3.10 | credit app referencing B shareholder | POST credits | reject | — | HTTP |

## T4 — Authorization isolation (PD-02)

| # | Setup | Action | Expected | Level |
| --- | --- | --- | --- | --- |
| T4.1 | U2 admin-in-A, treasurer-in-B | A ctx: `POST /api/users` | per A perms only (deny if A lacks) | HTTP |
| T4.2 | U2 | B ctx: admin-only endpoint | 403 (A admin must not leak into B) | HTTP |
| T4.3 | role created in A invisible in B | `GET /api/roles` B ctx | B roles only | HTTP+DB |
| T4.4 | assign A role to user via B ctx | POST assignment | reject (coop FK) | HTTP+DB |
| T4.5 | `auth/me` | U2 | per-coop `roles[]`/`permissions[]` distinct | HTTP |
| T4.6 | `grant-role` CLI `--cooperative A` → perms apply only in A | me/e2e | verified | CLI+HTTP |
| T4.7 | membership ended → all coop endpoints 403, other coops still work | U2 loses B | B→403, A→200 | HTTP+DB |

## T5 — Financial invariants under tenant load

| # | Setup | Action | Expected | Level |
| --- | --- | --- | --- | --- |
| T5.1 | parallel payments A and B | simultaneous POST | no lock coupling; both commit; counters independent | DB |
| T5.2 | concurrent allocations same A assessment | 2 tx | existing `FOR UPDATE ORDER BY` order respected; per-coop | DB |
| T5.3 | per-coop numbering | create payments in A,B | A gets 1,2,3…; B gets 1,2,3…; unique `(coop,n)` | DB |
| T5.4 | reversal chain integrity | reverse A payment | B payment w/ same id-shape unaffected; reversal FK same-coop | DB |
| T5.5 | `ACCOUNT BALANCE=Σ` per coop | seed A,B movements | report balances correct per coop | DB |
| T5.6 | A+B in single malicious body (`cooperative_id` in JSON) | POST w/ injected field | 422 `validation_failed` (REQ-006 pattern) | HTTP |

## T6 — Realtime isolation (024.8)

| # | Setup | Action | Expected | Level |
| --- | --- | --- | --- | --- |
| T6.1 | socket A-bound + socket B-bound | trigger A-domain change | only A socket receives event | HTTP/WS test |
| T6.2 | B-bound socket | inspect payload | payload never contains A coop id/rows | WS |
| T6.3 | membership B revoked mid-socket | heartbeat/perm re-check | socket closed or events stop | WS |
| T6.4 | WS handshake w/ foreign coop header | upgrade attempt | rejected | WS |
| T6.5 | `pg_notify` payload format `<coop>:<domain>` | LISTEN raw | unit test of trigger fn | DB |

## T7 — UI / multi-tab E2E

| # | Setup | Action | Expected | Level |
| --- | --- | --- | --- | --- |
| T7.1 | U2 tab1→A, tab2→B (sessionStorage each) | both lists open | tab1 shows A data, tab2 B data, headers differ | E2E |
| T7.2 | switch tab1 A→B mid-form | coop switcher | dirty-form warning; nav resets; header now B | E2E |
| T7.3 | bookmarked URL under coop A, session default B | load deep link | coop header wins explicit ctx; UI banner shows active coop | E2E |
| T7.4 | revoke B while tab2 active | next action/tab poll | 403 → UI falls back to coop selector | E2E |
| T7.5 | list caches: A list → switch B → back A | cached data | no B rows visible in A (fetch keyed by coop) | E2E |
| T7.6 | reports export in A ctx | download | file contains only A data; filename/sheet tagged coop | E2E+DB |

## T8 — Stale session / edge cases

| # | Setup | Action | Expected | Level |
| --- | --- | --- | --- | --- |
| T8.1 | session `active_cooperative_id=B`, B membership ended | request w/o header | 403, not silent fallback | HTTP |
| T8.2 | session default points at archived coop | request | 403/`coop_inactive` | HTTP |
| T8.3 | coop suspended by admin | all member requests | 403; audits `coop_suspended_access_denied` | HTTP+DB |
| T8.4 | `security_events` after actions | query | `cooperative_id` + `session_id` populated; login events NULL coop | DB |
| T8.5 | migration re-run idempotency | `migrate` twice | no-op second pass (existing AC) | DB/CI |

## Coverage gates

- Parameterized **T2.4 sweep** must enumerate all coop-owned tables via
  a single source-of-truth list shared with the migration (prevents a
  new table shipping without isolation tests).
- Every scenario above is **mandatory** per `docs/26` hostile-review +
  `docs/15` invariants; mocked-auth tests may exist in addition but
  never substitute DB-level rows.
- CI: tenant tests join the existing `database` job (DB-gated) + E2E job.
