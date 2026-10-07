# STEP-014 — GOVERNANCE, DECISIONS, VOTING & APPROVAL FOUNDATION

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
main == origin/main == e4cc032  (STEP-013)
```

Authoritative inputs:

```text
docs/01-DOMAIN-GLOSSARY.md
docs/02-DOMAIN-MODEL.md
docs/09-GOVERNANCE-POLICIES-DECISIONS.md
docs/15-INVARIANTS.md
docs/16-STATE-MACHINES.md
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

Approved scope: first-class governance evidence — governing bodies,
temporal memberships, formal decisions, recorded votes, and the
operator-recorded outcome foundation.

---

# 1. DOMAIN PRINCIPLE — EVIDENCE, NOT MONEY (docs/09, docs/15, docs/18)

```text
BODY                 governance_bodies — identity of a governing
                     organ. body_type is bounded FREE TEXT: docs/09
                     states the legal organ catalog is configurable,
                     so no enum is invented.

MEMBERSHIP           governance_memberships — temporal Person -> Body
                     link for [started_at, ended_at); ended_at NULL =
                     active. At most ONE active membership per
                     (body, person). title is a bounded label with NO
                     attached powers.

DECISION             governance_decisions — first-class evidence.
                     decision_on (Decision Date) != effective_on
                     (Effective Date); reaching the effective date
                     never mutates status. Material content freezes
                     when voting opens.

VOTE                 governance_votes — one immutable row per
                     (decision, person). membership_id snapshots WHICH
                     membership established eligibility at cast time;
                     recorded_by is the login actor, never the voter.
```

Governance is **evidence**: creating bodies, memberships, decisions or
votes writes ZERO `account_movements` and touches no financial
aggregate. A governance row authorizes nothing by itself in STEP-014.

# 2. OPEN GOVERNANCE RULES — DEFERRED, NEVER INVENTED (docs/09)

docs/09 explicitly leaves these unresolved; STEP-014 does not fabricate
any of them:

- Quorum / required turnout — `DEFERRED — AUTHORITATIVE POLICY NOT DEFINED`
- Majority / tie-breaking / pass arithmetic — `DEFERRED`
- Weighted, share-based, proxy or secret-ballot voting — `DEFERRED`
- Legal organ catalog and mandatory body structure — `DEFERRED`
  (bodies are operator-configured free-text `body_type`)
- Meeting, agenda item, proposal, electronic-signature evidence —
  `DEFERRED`
- Policy / PolicyVersion engine — `DEFERRED`
- Operational approval gating on business commands (which action
  needs which approval at which threshold) — `DEFERRED`; therefore NO
  existing API gained an invented approval requirement and NO
  decision-to-transaction linkage is fabricated.
- Vote change/withdrawal and decision correction/reversal semantics —
  `DEFERRED` (docs/19 correction model undefined for governance; the
  conservative choice is immutable votes and no decision mutation
  once evidence exists).

## Model B — operator-recorded outcome

Because pass/quorum rules are undefined, `finalize` takes the formally
decided `outcome` (`approved | rejected`) as a parameter and freezes a
deterministic evidence snapshot: `eligible_count` (active memberships
at finalize time) + `approve_count`/`reject_count`/`abstain_count`.
The snapshot is evidence, not a verdict — the system never derives the
outcome from the tally.

# 3. SCHEMA (migration 0014)

- `governance_bodies` — `body_number` (GENERATED IDENTITY), `name`,
  free-text `body_type`, optional `description`,
  `active → closed` lifecycle with `closed_at`/`closed_by`
  consistency CHECK, durable `idempotency_key` UNIQUE + fingerprint.
- `governance_memberships` — body + person FKs, free-text `title`,
  `[started_at, ended_at)` window (`ended_at > started_at` CHECK),
  `ended_by`/`end_reason` consistency CHECK, partial UNIQUE index on
  `(body_id, person_id) WHERE ended_at IS NULL` (one active seat),
  durable idempotency pair.
- `governance_decisions` — `decision_number` (GENERATED IDENTITY),
  body FK, `title`, `decision_text`, `decision_on` DATE,
  `effective_on` DATE, lifecycle CHECK
  `draft → open → approved | rejected` + `draft → cancelled`,
  open/finalized/cancelled actor+timestamp consistency CHECKs, the
  frozen snapshot columns (`eligible_count` … `abstain_count`)
  required exactly when finalized, non-negative counts CHECK, durable
  idempotency pair.
- `governance_votes` — decision + membership + person FKs,
  `choice` CHECK (`approve|reject|abstain`), `cast_at`, optional
  `note`, `recorded_by` FK → users, `UNIQUE (decision_id, person_id)`,
  durable idempotency pair. No UPDATE/DELETE path exists.
- Permissions seeded: `governance.read`, `governance.manage` →
  `Sistem Yöneticisi` (catalog 28 → 30).

# 4. BEHAVIORAL CONTRACT

- `POST /api/governance/bodies` creates organ identity only — no
  members, no electorate, no money.
- `POST …/bodies/{id}/memberships` requires the canonical Person
  (resolved via `GET /api/governance/persons`); `started_at` may be
  backdated but never future; overlapping ACTIVE memberships for the
  same (body, person) are rejected (409). Membership is NEVER
  inferred from RBAC, shareholder status or family role.
- `POST …/memberships/{id}/end` sets `ended_at`/`ended_by`/
  `end_reason`; the historical row is never deleted or rewritten.
- `POST /api/governance/decisions` creates a `draft`; drafts are
  editable via `POST …/decisions/{id}/update` and may be cancelled
  with a reason (`cancelled` is terminal evidence).
- `POST …/decisions/{id}/open` freezes material content (title, text,
  decision_on, effective_on, body): voters and history must see the
  same text. Re-opening a non-draft answers `400`.
- `POST …/decisions/{id}/votes` requires the decision `open`, then
  proves eligibility server-side: the `personId` must hold an ACTIVE
  membership (`ended_at IS NULL`, `started_at <= now`) in the
  decision's body — `403` otherwise. One vote per person (409 on
  duplicate); votes are immutable once cast.
- `POST …/decisions/{id}/finalize` — decision must be `open`; stores
  the operator-declared outcome + the frozen electorate/tally
  snapshot atomically under the decision row lock. Replay answers
  `400` (state-transition convention).
- `POST …/bodies/{id}/close` requires no draft/open decisions and no
  active memberships (`409` otherwise); closed bodies keep all
  history. Reopen is undefined policy — not implemented.
- Canonical lock order: decision row first, then the membership row —
  documented in `repo.rs`; votes, open and finalize serialize on the
  decision row so a finalized decision's vote set is deterministic.
- All create commands carry durable `idempotency_key` + payload
  fingerprints; replay returns the stored result, a conflicting reuse
  answers `409`.
- Governance commands create NO Account Movement, Payment, Income,
  Expense, Credit, Transfer, Share Return, Investment or Social Aid
  row — ever.

# 5. API SURFACE

```text
GET|POST /api/governance/bodies
GET      /api/governance/bodies/{id}
POST     /api/governance/bodies/{id}/close
GET|POST /api/governance/bodies/{id}/memberships
POST     /api/governance/memberships/{id}/end
GET|POST /api/governance/decisions       (?bodyId=…&status=…)
GET      /api/governance/decisions/{id}
POST     /api/governance/decisions/{id}/update
POST     /api/governance/decisions/{id}/open
POST     /api/governance/decisions/{id}/cancel
POST     /api/governance/decisions/{id}/votes
POST     /api/governance/decisions/{id}/finalize
GET      /api/governance/persons         (?search=…)
```

Lists are server-paginated. Decision detail returns votes plus the
live electorate (`eligibleMembers` — active memberships only). All
mutations are CSRF-protected and require `governance.manage`; reads
require `governance.read`.

# 6. RBAC ≠ GOVERNANCE (docs/18)

`governance.manage` is API operation permission — it lets a User
RECORD governance facts. It does not make them a member or voter:
voter identity is a `persons` fact proven by an active membership row.
System Administrator receives the permissions by migration but holds
no seat until a membership is created for their Person. Conversely a
governance member with no login/permission cannot act through the API —
the recorded-vote model separates `recorded_by` (User) from
`person_id` (Person voting).

# 7. FRONTEND

- `/yonetim` — decision register: status badges (text + icon, never
  color alone), body filter, decision numbers, vote counts.
- `/yonetim/kurullar` — body register with derived active-seat counts;
  `/yonetim/kurullar/yeni` create form.
- `/yonetim/kurullar/{id}` — membership timeline with explicit
  current/historical distinction, Person-search member add, term end
  with reason, body close (with the draft/open + active-membership
  precondition surfaced in the UI).
- `/yonetim/kararlar/yeni` — draft creation (body, title, text,
  Decision Date, Effective Date).
- `/yonetim/kararlar/{id}` — lifecycle panels: draft edit, voting
  open, per-member vote recording (select limited to the live
  electorate), finalize with the operator-recorded outcome, frozen
  tally snapshot ("Sonuç Anıtı") on finalized decisions, immutable
  vote list naming voter + recorder separately.
- Turkish-first i18n (`tr-TR` default, `en` present); manage controls
  hidden without `governance.manage` — UX only, backend enforces.
- Governance screens make the money-neutrality explicit: nothing on
  them carries amounts or accounts.

# 8. DEFERRED SCOPE (never invented)

- Quorum, majority, pass-rule arithmetic — outcome is recorded, not
  computed (`DEFERRED — AUTHORITATIVE POLICY NOT DEFINED`).
- Vote weighting, share-based voting, proxy voting, secret ballot,
  electronic signature / legal evidentiary formats.
- Meeting, Agenda Item, Proposal entities.
- Policy / PolicyVersion engine and retroactive-policy authority.
- Approval-requirement model: which business action requires which
  approval, scope binding, one-time consumption, threshold tables —
  no gating added to any existing API; no fabricated approval on
  pre-STEP-014 records.
- Decision-to-business-action linkage storage (execution provenance)
  — no authoritative linkage policy exists yet.
- Vote change/withdrawal, decision amendment/reversal, body reopen —
  governance correction semantics are undefined in docs/19.
- General assembly / election systems.

# 9. VERIFICATION

- 16 backend tests (`apps/server/tests/governance.rs`): body
  lifecycle, membership temporal rules (overlap rejection, future
  start rejection, end-window check, history preservation), decision
  draft→open→finalize lifecycle, content freeze at open, cancelled
  draft preserved, vote eligibility (ended/never-member rejected),
  duplicate-vote rejection, immutable votes, finalize snapshot +
  immutability, idempotent replay + conflict, concurrent votes and
  concurrent finalizations cannot double-apply, permission denials
  (`governance.read` vs `governance.manage`), zero financial side
  effects.
- 11 frontend spec tests
  (`apps/web/src/specs/governance-pages.spec.ts`).
- Real-stack E2E `scripts/e2e-step014.mjs` — full lifecycle through
  the UI (body → membership via Person lookup → term end → draft →
  open → vote → finalize → register) plus API-level rejection proofs
  and the zero-money assertion.
- Backup drill extended: body/membership/decision/vote fixtures
  (incl. a closed body, an ended membership whose vote survives, an
  approved decision with frozen snapshot, a cancelled draft, a
  rejected decision on the closed body), evidence joins, the
  zero-movement check, and 11 constraint-behavior checks on the
  restored copy.
