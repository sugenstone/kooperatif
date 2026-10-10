# M1 — TENANT DATA MODEL, CONTEXT, AUTHORIZATION AND RLS DESIGN

Companion to `M1-ARCHITECTURE-DISCOVERY.md` (evidence). Covers
M1-D02 (model), M1-D03 (active-cooperative context), M1-D04
(authorization), M1-D06 (RLS evaluation). **Planning only — not
implemented.**

---

## 1. M1-D02 — Canonical tenant data model

### 1.1 New tables (DESIGN RECOMMENDATION)

```sql
CREATE TABLE cooperatives (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    legal_name  TEXT,
    status      TEXT NOT NULL DEFAULT 'active'
                CHECK (status IN ('active','suspended','archived')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (lower(btrim(name)))
);

CREATE TABLE cooperative_memberships (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cooperative_id  UUID NOT NULL REFERENCES cooperatives(id),
    user_id         UUID NOT NULL REFERENCES users(id),
    status          TEXT NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active','suspended','ended')),
    joined_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at        TIMESTAMPTZ,
    created_by_user_id UUID REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (cooperative_id, user_id),
    UNIQUE (cooperative_id, user_id, id)  -- composite-FK target if needed
);
```

**Membership lifecycle** — one row per (coop, user); `status`
transitions `active → suspended → active` and `active/suspended →
ended`, each audited (new `SecurityEventType` values). Deletion is
prohibited — membership history must persist for audit attribution
of historical financial actions.

### 1.2 Altered identity tables (DESIGN RECOMMENDATION)

| Table | Change |
| --- | --- |
| `roles` | `+ cooperative_id UUID NOT NULL REFERENCES cooperatives`; name unique → `(cooperative_id, lower(name))`; `+ UNIQUE(cooperative_id, id)` |
| `user_role_assignments` | `+ cooperative_id NOT NULL`; PK → `(cooperative_id, user_id, role_id)`; **two FKs**: `(cooperative_id, role_id) → roles(cooperative_id,id)` **and** `(cooperative_id, user_id) → cooperative_memberships(cooperative_id,user_id)` — a role can only be assigned to a member, and only to a role of the same coop (DB-enforced) |
| `user_sessions` | `+ active_cooperative_id UUID NULL REFERENCES cooperatives` — UX default only, **not** the security context (see §2) |
| `security_events` | `+ cooperative_id UUID NULL`, `+ session_id UUID NULL`; NULL = platform-scope event (login, user created) |

### 1.3 Altered business tables (DESIGN RECOMMENDATION)

All 36 `COOP`/`COOP+` tables from the discovery inventory gain:

```sql
cooperative_id UUID NOT NULL REFERENCES cooperatives(id),
UNIQUE (cooperative_id, id)          -- composite-FK target
```

…plus composite FKs on every cross-tenant hazard edge listed in
discovery §3.1, and re-scoped unique/number constraints per §3.2.
`permissions` stays GLOBAL. `_sqlx_migrations` stays GLOBAL.

### 1.4 Global vs cooperative-owned identities (the four layers)

```
GLOBAL identity        users, user_sessions, permissions, security_events(+coop col)
        │
MEMBERSHIP             cooperative_memberships  (user ∈ coop, status)
        │
COOP AUTHORIZATION     roles(coop) → role_permissions; user_role_assignments(coop)
        │
BUSINESS OWNERSHIP     every row carries cooperative_id — immutable after insert
```

**`cooperative_id` is immutable** on business rows (no UPDATE path; a
"moved" row is a correction/reversal + new row — never silent rewrite,
per AGENTS.md).

### 1.5 Persons: per-coop vs global (OWNER DECISION REQUIRED — M1-K2)

Same human can be shareholder in coop A and beneficiary in coop B.

| Option | + | − |
| --- | --- | --- |
| **A — `persons` coop-owned** (recommended) | Strong PII isolation; no cross-coop read path at all; simplest consistency (every person-ref edge is same-coop by FK) | Same human = two rows; no cross-coop dedup/search |
| B — `persons` global | One identity per human | PII table readable from any coop context → breaks isolation badly; every person query needs a second-order scoping rule; high risk |

**Recommendation: A** (coop-owned). Cross-coop person linking can be
added later as an optional non-security feature; the reverse
(de-isolating a leaked PII table) is near-impossible.

### 1.6 Roles: per-coop copies vs templates (OWNER DECISION REQUIRED — M1-K5)

| Option | + | − |
| --- | --- | --- |
| **A — roles coop-owned** (recommended) | Each coop customizes roles independently; matches PD-02 literally; simplest integrity (all edges same-coop) | New coop needs initial role set — clone from a seed/template at coop creation |
| B — global role catalog + coop assignments | One "Treasurer" definition everywhere | Coop cannot customize perms; template changes would silently alter every coop — dangerous for financial rights |

**Recommendation: A** with a **role template seed** applied at
cooperative creation (mirrors `financial_categories` seeding that
already exists in `0007`/`0010` — proven pattern in this codebase).

### 1.7 Platform administration (OWNER DECISION REQUIRED — M3-K6)

Who creates cooperatives and grants the first membership? Options:

- **A — CLI-only** (`kooperatif-server create-cooperative`,
  `add-member`): zero HTTP attack surface, matches existing
  `create-user`/`grant-role` (`main.rs:29`). Recommended for M1.
- B — `platform_admin` global role: needs a second authorization
  namespace; must never read business data; more code.
- C — first coop's admin manages everything: fails once coop #2 exists.

**Recommendation: A for M1**; revisit B only if multi-coop operations
staff emerge. **No global super-admin data bypass is designed** —
explicitly per the M1-D02 limit.

---

## 2. M1-D03 — Active cooperative context

### 2.1 Options evaluated

| Criterion | A. URL path (`/api/coops/{id}/…`) | B. Selection endpoint + session ctx | C. `X-Cooperative-Id` header per request | D. Signed context token | E. Hybrid: session default + header override |
| --- | --- | --- | --- | --- | --- |
| Security | ID in URL — leaks into logs/history; still server-validated | server-side truth | server-validated each req | crypto complexity | server-validated each req |
| CSRF | cookie+URL, same risk | cookie | **header = implicit CSRF resistance** | token adds burden | header = CSRF resistance |
| Stale context | visible in URL | **silent** — session flipped by another tab | per-request, explicit | token replay | per-request explicit |
| Multi-tab (A+B) | **natively works** | **fails** — one ctx per session | **works** — each tab sends its own header | works | **works** |
| API ergonomics | ugly, churns all 82 routes | simplest client | clean — one header in `apiFetch` | token mgmt | clean |
| WebSocket | path param at upgrade | session-bound at upgrade | header at upgrade | token at upgrade | header at upgrade |
| Auditability | id in access log | coop recorded at switch only | coop on every request | signed attestation | coop on every request |
| Failure modes | bookmarked foreign URL | mid-form silent switch = worst UX | missing header → 400/401 | token theft window | missing header → session default or 400 |
| Migration cost | high (route tree) | low | low | high | low |

**DESIGN RECOMMENDATION — Option E (hybrid):**

1. Client sends `X-Cooperative-Id: <uuid>` on **every** API request and
   the WS handshake. Backend validates: header well-formed → membership
   row exists and `status='active'` → else `403 coop_membership_revoked`
   / `400 coop_context_required` if absent.
2. Session stores `active_cooperative_id` as **UX default only**
   (`POST /api/auth/cooperative` sets it; used when header absent and
   for the first page load after login). It is never trusted alone.
3. Audit: `cooperative_id` stamped on every `security_events` row where
   context exists; a `cooperative_switched` event records each switch.
4. Fail closed: no header + no session default → context error, never
   an implicit "first coop".

This is the only option satisfying "simultaneous browser tabs showing
different cooperatives" (explicit M1-D03 requirement) without URL
churn, while the session default keeps refresh/login UX smooth.

### 2.2 Multi-tab semantics

Each tab keeps `activeCooperativeId` in `sessionStorage` (per-tab by
design — `localStorage` would couple tabs). Tab A in coop A, tab B in
coop B: both send different headers; server treats them independently.
Switching coop in a tab updates only that tab's sessionStorage + calls
`POST /api/auth/cooperative` for the session default. **UNRESOLVED
RISK (accepted)**: a shared HttpOnly session means tab B can act in
coop A's name *if the user has membership in A anyway* — acceptable
because authorization, not tab identity, is the boundary.

### 2.3 Membership loss while active

`require_tenant_ctx` checks `memberships.status='active'` on **every**
request (single indexed lookup — cheap). A suspended/ended member gets
`403` immediately; realtime heartbeat (already re-checks session per
signal) additionally drops the socket on membership loss; `auth/me`
returns updated coop list on next poll/navigation. Sessions are **not**
revoked — the user may still hold other memberships.

---

## 3. M1-D04 — Authorization architecture

```
Request ──► RequireAuth (session valid, user active)
        ──► TenantCtx extractor: coop_id = X-Cooperative-Id header
            ?? session.active_cooperative_id ?? error
            → SELECT status FROM cooperative_memberships
              WHERE cooperative_id=$1 AND user_id=$2
            → status='active' or 403
        ──► authz::require(state, auth, ctx, PERM):
            effective_permissions(user_id, coop_id)
            = ura(coop) → roles(coop, status='active')
              → role_permissions → permissions
        ──► handler runs only coop-scoped queries (tenant repos)
```

- **Default deny preserved** — same as today (`authz.rs`), now
  double-gated by membership.
- **Permission cache**: none today (query per check); keep same for M1
  (correctness first), revisit only with load evidence.
- **`/api/auth/me` contract** (new shape):
  `{user, csrf_token, cooperatives:[{id,name,status,roles[],permissions[]}], active_cooperative_id}` — frontend stores per-coop perms; `can()` stays UX-only.
- **Admin path split**: `USERS_MANAGE` stays global (users are global);
  `ROLES_MANAGE`/membership mgmt are **coop-scoped** permissions.
  `grant-role` CLI gains `--cooperative` arg. The global
  `administration_path_count` advisory lock becomes per-coop keyed.
- **Audit**: `audit()` signature gains `cooperative_id` + `session_id`;
  emitted inside the request tx where the event is coop-bound (fixes
  the async-attribution risk from discovery §7).

---

## 4. M1-D06 — Database isolation: Option A vs Option B

### Option A — application scoping + DB constraints only

Every repository query is built through a tenant-aware layer
(`cooperative_id` bound once, injected into WHERE/JOIN); composite FKs
from §3.1 enforce same-coop edges at write time.

| + | − |
| --- | --- |
| No GUC/pool machinery; current repos unchanged structurally | A missed `WHERE cooperative_id` leaks — detection relies on tests/review |
| Works with existing pool (5 conn) & non-tx reads | No DB-level safety net under bug or future ad-hoc SQL |
| Zero connection-model change | |

### Option B — A + PostgreSQL RLS

`ALTER TABLE … ENABLE/FORCE ROW LEVEL SECURITY` on all 36 coop tables +
policy `USING (cooperative_id = current_setting('app.cooperative_id')::uuid)
WITH CHECK (same)`. Context injected per transaction:

```sql
BEGIN; SET LOCAL app.cooperative_id = '<uuid>'; …queries…; COMMIT;
```

| + | − |
| --- | --- |
| **Fail-closed**: empty/missing GUC → `NULL::uuid` → zero rows — a missed WHERE yields 404/empty, not a leak | Every query (incl. reads) must run inside a tx that sets the GUC — `SET LOCAL` is tx-scoped; session-level `SET` would leak across pooled connections |
| Enforced even for future ad-hoc queries/reporters | Needs a `TenantConn`/`TenantTx` wrapper in the extractor — touches every handler's acquisition pattern |
| Industry-standard multi-tenant pattern | `current_setting()` per-row cost: negligible at coop scale but adds planner constant (acceptable) |
| | **Role split required**: dev/test connect as superuser `kooperatif` → bypasses RLS silently; meaningful RLS needs a non-superuser app role (`kooperatif_app`) — ops change in docker-compose + CI + migration tooling |
| | `FORCE ROW LEVEL SECURITY` also constrains table owner → migration role must either be superuser or policies need `BYPASS` strategy for `migrate`/`grant-role` CLI paths |
| | Background `listen_loop`/audit inserts need explicit coop GUC — already required for audit correctness |

**DESIGN RECOMMENDATION — Option B, phased**: Phase 2 delivers
Option A (mandatory either way); Phase 4 adds RLS on coop tables with
the `TenantTx` pattern (all handlers already acquire state-per-request;
reads wrapped in a lightweight tx). Rationale: RLS is the only layer
that makes a *missed* application filter non-leaking; retrofit cost
after the schema lands is much higher.

**Prerequisites recorded**: (1) dedicated non-superuser role
`kooperatif_app` for app+tests (OWNER DECISION — ops change);
(2) `SET LOCAL` discipline enforced by a single `TenantTx` helper —
never raw `pool.acquire()` in handlers; (3) `after_release` connection
hook resets `app.cooperative_id` as belt-and-braces; (4) CLI/migration
paths documented to run as owner or with explicit `SET`.
