# STEP-016 — LIVE OPERATIONS, REAL-TIME WEBSOCKET & TV/PROJECTOR FOUNDATION

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
main == origin/main == dc9eab2  (STEP-015)
```

Authoritative inputs:

```text
docs/13-SCREEN-SPECIFICATIONS.md   (§ Live TV / Projector Dashboard)
docs/15-INVARIANTS.md
docs/18-AUTHORIZATION-APPROVALS.md
docs/19-AUDIT-REVERSAL-CORRECTION.md
docs/21-API-CONTRACTS.md
docs/22-SECURITY.md
docs/26-TESTING-QUALITY-GATES.md
docs/27-ARCHITECTURE.md            (§ Real-time, § Jobs/Outbox)
ADR-005 (WebSocket selected; transport ≠ truth; snapshot-after-reconnect)
ADR-009 (PostgreSQL-backed infra; no extra broker)
ADR-012 (single-node VDS topology)
ADR-013 (backup/restore)
```

Approved scope: authenticated real-time change-signal layer +
permission-scoped subscriptions + client invalidation infrastructure +
read-only live TV/projector display.

---

## 1. ARCHITECTURE

```text
Command → Authorization → DB Transaction → COMMIT
   → AFTER-trigger pg_notify('kooperatif_domain_changed', <domain>)
   → sqlx PgListener (dedicated conn) → RealtimeHub (broadcast 256)
   → per-socket session+permission revalidation → authorized sockets
   → client invalidation → canonical REST/reporting API → UI refresh
```

- `migrations/0016_realtime.sql` — one trigger function +
  37 `FOR EACH ROW` AFTER-triggers, one per domain-mutating table.
- `apps/server/src/realtime/` — `Domain` mapping (tag ↔ read
  permission), `RealtimeHub` (bounded `tokio::broadcast`), LISTEN loop,
  `/api/realtime` route.
- `apps/web/src/lib/realtime/realtime.svelte.ts` — the single shared
  client; `(app)` layout connects with the session's read permissions;
  `(tv)` display connects with `reports.read`.
- `packages/contracts/src/realtime.ts` — wire envelope types.

## 2. POST-COMMIT DELIVERY

`pg_notify` is **transactional**: notifications fire only after COMMIT
and are discarded on ROLLBACK — the commit boundary is enforced by
PostgreSQL, not by application discipline. Identical
(channel, payload) notifications inside one transaction are coalesced,
so a multi-write command emits one signal per touched domain.
A durable transactional outbox table was deliberately NOT added: no
exact-replay requirement exists (ADR-009 allows LISTEN/NOTIFY when
recovery = canonical resync, which §17 requires anyway).

## 3. EVENT CONTRACT & MINIMIZATION

```json
{"type":"data-changed","domain":"payments","occurredAt":"RFC3339"}
{"type":"resync","reason":"listener|lagged","occurredAt":"…"}
{"type":"subscribed","granted":["reports.read"],"occurredAt":"…"}
```

No sequence/cursor is claimed durable; the payload is the domain tag
only — no ids, amounts, names, session data. `Domain::from_tag`
rejects unknown tags.

## 4. AUTHENTICATION & SUBSCRIPTION AUTHORIZATION

- Handshake: `CurrentAuth` extractor → full session classify + touch;
  unconditional Origin allowlist check (GET-upgrade exempts CSRF).
- Client sends `{type:"subscribe",scopes:[…]}`; server grants
  `requested ∩ known_scopes ∩ effective_permissions`.
- Delivery rule per signal: `granted` must contain the domain's read
  permission or `reports.read`, AND the FRESH effective permission set
  (re-queried per signal) must still contain it.
- Heartbeat every 30 s re-classifies the session; revoked / expired /
  idle-expired / disabled → close 1008. Lookup errors fail closed.

## 5. SNAPSHOT RACE & RECONNECT

Server order: `hub().subscribe()` happens at upgrade time, before the
client fetches its snapshot — commits after subscribe are signaled; the
snapshot covers earlier state. Every (re)connect emits a wildcard resync
client-side; the client re-reads canonical APIs. Missed events are
therefore unrecoverable by design and irrelevant: state truth is the
REST surface. Duplicates are harmless — invalidations coalesce.

## 6. BACKPRESSURE & RESOURCES

- Broadcast capacity 256; `Lagged` → `resync{reason:"lagged"}` to that
  socket (it resyncs rather than pretending currency).
- Sends are wrapped in a 10 s timeout; a wedged client is dropped.
- One WS = one task set; no per-socket DB connection held — the
  listener uses a single dedicated `PgListener` connection OUTSIDE the
  request pool, so socket count never consumes request capacity.
- Client reconnect: exponential backoff (500 ms→30 s) + jitter, hard cap
  of 10 attempts → `disconnected`; resumes on `resume()`/`online`.

## 7. TV/PROJECTOR — `/canli-ekran`

Chrome-free `(tv)` route group; `+layout.ts` requires an authenticated
session AND `reports.read`. Four bounded view modes (Genel Bakış /
Tahsilat / Finansal Hesaplar / Sosyal Yardım), distance-readable dark
layout, live clock, connection chip, last-sync time, explicit stale
banner, user-initiated fullscreen. Zero mutation controls (one button:
fullscreen; one select: view mode). All numbers come from
`/api/reports/overview` — the socket only refetches it.

## 8. VERIFICATION EVIDENCE

- `apps/server/tests/realtime.rs` — 10/10 against real PostgreSQL +
  real TCP + tokio-tungstenite: anonymous 401, bad/missing Origin 403,
  commit→signal / rollback→silence, cross-writer commit→signal,
  scope filtering, reports.read breadth, session revocation → close,
  permission revocation → silence, reconnect→canonical re-read,
  minimal envelope.
- `apps/web/src/specs/realtime.spec.ts` — 9/9: subscribe, coalescing,
  resync wildcard, malformed frame, bounded reconnect cap, disconnect
  stops retries, reconnect resync, TV read-only + stale indicator.
- `scripts/e2e-step016.mjs` — real stack PASS: commit→TV update,
  reversal→TV update, socket-drop gap commit→reconnect→canonical
  resync, anonymous socket rejected, viewer POST → 403, zero phantom
  `account_movements` rows.
- Backup drill: notify function + all 37 triggers restored; channel
  callable post-restore.

## 9. MULTI-INSTANCE SEMANTICS

LISTEN/NOTIFY is connection-level and instance-agnostic: any backend
process LISTENing on the same database receives every commit's signal.
The Rust suite proves this with a cross-writer commit (raw pool INSERT,
unrelated to the serving process's request path). Current deployment is
single-node (ADR-012); running N instances requires no additional infra —
each instance spawns its own listener task in `serve`.

## 10. DEFERRED / BOUNDARY

- Durable event log/replay and cursor resume — unneeded; canonical
  resync is the specified recovery (ADR-005).
- Per-user or per-aggregate event targeting; Collection Session live
  totals consume this foundation in their own STEP.
- Push/SMS/email, public kiosk tokens, dashboard designer, multi-screen
  choreography — out of scope.
- Single-organization deployment; no tenant isolation exists to leak.
