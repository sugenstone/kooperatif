# STEP-003 — RBAC & AUTHORIZATION FOUNDATION

Repository:

```text
C:\Users\sugen\Documents\PROJE\kooperatif
```

Remote:

```text
https://github.com/sugenstone/kooperatif.git
```

Expected baseline:

```text
main == origin/main == c86a197
```

Completed milestones:

```text
STEP-001          CLOSED
STEP-002          CLOSED
SPEC-UPGRADE-001  CLOSED
REMOTE-SYNC-001   CLOSED
```

Authoritative specification:

```text
Kooperatif Agent Specification v0.8
```

This STEP implements the application's authorization foundation:

> User → Role Assignment → Role → Permission → Authorization Decision

Authentication already exists from STEP-002.

This STEP answers:

> What is an authenticated User allowed to do?

It must NOT implement unrelated cooperative business domains.

---

# 0. OPERATING MODE

Work as a senior security-conscious Rust/SvelteKit engineer.

Follow:

```text
FETCH
→ VERIFY BASELINE
→ READ SPEC
→ INSPECT AUTH
→ MODEL AUTHORIZATION
→ THREAT-MODEL
→ PLAN
→ IMPLEMENT
→ MIGRATE
→ TEST
→ HOSTILE REVIEW
→ REVIEW DIFF
→ COMMIT
→ PUSH
→ VERIFY REMOTE
→ REPORT
→ STOP
```

Do not continue to STEP-004.

Do not implement Shareholders, Families, Shares, Periods, Payments, Ledger, Investments or Social Aid.

Do not invent business permissions for modules that do not yet exist merely to make the permission catalog look complete.

---

# 1. REMOTE / BASELINE VERIFICATION

Before modifying anything:

```text
git status --short
git branch --show-current
git remote -v
git fetch --prune origin
git rev-parse HEAD
git rev-parse origin/main
git log --oneline --decorate -n 10
```

Required baseline:

```text
branch = main
HEAD = c86a197
origin/main = c86a197
working tree = clean
```

Required invariant:

```text
HEAD == origin/main
```

If local and remote differ:

DO NOT reset.

DO NOT rebase.

DO NOT force-push.

DO NOT discard work.

Investigate and STOP if proceeding could overwrite legitimate work.

---

# 2. READ AUTHORITATIVE DOCUMENTATION

Before implementation read at minimum:

```text
AGENTS.md
README.md

docs/00-PROJECT-CHARTER.md
docs/01-DOMAIN-GLOSSARY.md
docs/02-ACTOR-ROLE-MATRIX.md
docs/15-INVARIANTS.md
docs/16-STATE-MACHINES.md
docs/18-AUTHORIZATION-APPROVALS.md
docs/19-AUDIT-REVERSAL-CORRECTION.md
docs/20-DATA-MODEL-DATABASE.md
docs/21-API-CONTRACTS.md
docs/22-SECURITY.md
docs/26-TESTING-QUALITY-GATES.md
docs/27-ARCHITECTURE.md
docs/28-ADR-INDEX.md

docs/adr/ADR-001-APPLICATION-ARCHITECTURE-BASELINE.md
docs/adr/ADR-002-AUTHENTICATION-SESSION.md
docs/adr/ADR-006-IDEMPOTENCY-CONCURRENCY.md
docs/adr/ADR-011-OBSERVABILITY.md
docs/adr/ADR-013-BACKUP-RESTORE-DISASTER-RECOVERY.md
```

Inspect STEP-002 authentication implementation before designing authorization.

Do not duplicate authentication concepts.

---

# 3. CORE DOMAIN DISTINCTION

Keep these concepts separate:

```text
User
Role
Permission
UserRoleAssignment
Shareholder
Guardian
Family
Governance position
```

A Role is an application authorization construct.

A Shareholder is a cooperative business record.

A board/governance position is a cooperative governance concept.

Do not equate:

```text
Shareholder == User
Board Member == Role
Chairman == administrator
```

A future governance position may influence authorization through explicit rules, but that relationship must not be invented in STEP-003.

---

# 4. AUTHORIZATION MODEL

Implement an RBAC foundation equivalent to:

```text
User
  │
  ├── UserRoleAssignment
  │
  ▼
Role
  │
  ├── RolePermission
  │
  ▼
Permission
```

A User may have:

```text
0..N roles
```

A Role may have:

```text
0..N permissions
```

Effective permissions are the union of permissions granted by the User's active roles.

Do not implement explicit deny rules in this STEP unless the authoritative specification already requires them.

Absence of permission means denial.

Default posture:

```text
DENY
```

---

# 5. NO `is_admin`

Do NOT introduce authorization shortcuts such as:

```text
users.is_admin
users.is_superuser
users.is_accountant
users.can_manage_users
```

Authorization must flow through the permission model.

Do not hide hard-coded bypasses such as:

```rust
if username == "admin"
```

No User receives implicit authorization because of username, creation order, or bootstrap origin.

---

# 6. PERMISSION IDENTITY

Permissions require stable machine-readable identifiers.

Use a naming convention equivalent to:

```text
resource.action
```

Examples for functionality that actually exists in this STEP may include:

```text
users.read
users.manage
roles.read
roles.manage
```

Choose the exact minimal catalog based on real STEP-003 functionality.

Do NOT pre-create dozens of speculative permissions such as:

```text
payments.reverse
ledger.post
shares.transfer
social_aid.approve
```

before those modules exist and their semantics are defined.

Future STEPs will add permissions through controlled migrations/catalog updates.

---

# 7. PERMISSION CATALOG OWNERSHIP

Permission keys are security contracts.

They must NOT be freely typed by administrators.

Required principle:

> The application defines which permission keys exist.

The UI/database may store/display/assign known permissions, but administrators must not invent arbitrary permission keys through a text field.

Permission catalog changes occur through controlled application/specification changes.

This prevents:

```text
typos
orphan permissions
meaning drift
fake authorization keys
```

---

# 8. PERMISSION METADATA

A Permission should support enough metadata for understandable administration.

Conceptually:

```text
id
key
name
description
category/module
created_at
```

Exact schema should follow project conventions.

User-facing permission names/descriptions should support Turkish as the default language.

Do not make localized display text the authorization identifier.

Authorization checks use the stable machine key.

---

# 9. ROLE MODEL

Roles are administrator-defined authorization bundles.

Support conceptually:

```text
id
name
description
status
created_at
updated_at
```

Use the minimum fields justified by the system.

Role names should be unique according to an explicitly documented normalization strategy.

Do not use role name itself as authorization logic.

This must remain valid:

```text
Role renamed
→ authorization behavior unchanged
```

because authorization derives from assigned permission identities.

---

# 10. ROLE STATUS / LIFECYCLE

Support at minimum:

```text
active
disabled
```

A disabled Role must no longer contribute permissions.

Existing UserRoleAssignment records may remain for history, but authorization must ignore disabled Roles.

Test this invariant.

Avoid destructive deletion where historical assignments would become ambiguous.

---

# 11. ROLE DELETION POLICY

Prefer lifecycle control over destructive deletion.

If Role deletion is implemented at all, it must be safe.

A Role that has:

```text
assignments
audit history
security significance
```

should generally be disabled rather than physically deleted.

Do not cascade-delete security history.

For STEP-003, disabling is sufficient unless specification requires more.

---

# 12. USER ROLE ASSIGNMENT

Implement assignment of Roles to Users.

Required invariants:

```text
same Role cannot be assigned twice to same User
assignment must reference valid User
assignment must reference valid Role
disabled Role must not grant permissions
```

Use database constraints where appropriate.

Assignment/removal must be transactional.

---

# 13. ASSIGNMENT HISTORY / AUDITABILITY

Role assignment is security-sensitive.

At minimum durable audit history must make it possible to determine:

```text
who assigned a role
to which User
which Role
when

who removed a role
from which User
which Role
when
```

Role permission changes must likewise be auditable.

Do not store only the final state with no durable history.

Reuse/extend the audit foundation established in STEP-002 where appropriate.

Do not build an unrelated second audit system.

---

# 14. BOOTSTRAP AUTHORIZATION PROBLEM

STEP-002 created the first User without RBAC.

STEP-003 must solve the transition safely.

After RBAC enforcement is introduced, the existing bootstrap User must not accidentally become locked out of all administration.

Design an explicit bootstrap authorization migration/workflow.

Preferred direction:

```text
create a system-seeded administrative Role
+
assign it explicitly to the existing/bootstrap operator
```

BUT:

Do not identify the bootstrap User by:

```text
first row
lowest UUID
username == admin
```

If automatic migration cannot safely know which User should receive the initial administrative Role, provide an explicit operator command.

Example conceptually:

```text
grant-role <username> <role>
```

with safe bootstrap semantics.

Document the chosen transition.

No permanent authorization bypass is allowed.

---

# 15. SYSTEM-SEEDED ADMINISTRATIVE ROLE

It is acceptable to create an initial administrative Role such as:

```text
Sistem Yöneticisi
```

if needed.

Important:

The Role is not magical because of its name.

It receives explicitly defined permissions.

Authorization still evaluates permission assignments normally.

If the role name changes, its behavior must remain defined by permissions.

Do not implement:

```text
if role.name == "Sistem Yöneticisi" { allow_all }
```

---

# 16. NO GLOBAL WILDCARD BY DEFAULT

Avoid:

```text
*
*.*
all
superuser
```

wildcard permissions unless there is a compelling, documented reason.

For this system, prefer explicit permission grants.

Why:

Future financial/security permissions must not automatically become available to an old role merely because a wildcard existed.

When a future module adds a dangerous permission, existing roles should not silently acquire it unless explicitly intended.

This is a critical invariant.

---

# 17. EFFECTIVE PERMISSION RESOLUTION

Implement a centralized authorization service capable of answering:

```text
Does User X have Permission Y?
```

Conceptually:

```rust
authorize(user_id, permission_key)
```

or an equivalent typed mechanism.

Do not scatter SQL authorization queries throughout handlers.

Authorization belongs in a reusable application/security boundary.

---

# 18. REQUEST AUTHORIZATION

STEP-002 provides authenticated request context.

Extend it cleanly.

Protected operations should be able to require:

```text
AuthenticatedUser
+
Permission
```

without repeating role joins and error mapping manually in every handler.

Use an Axum-compatible authorization mechanism appropriate to ADR-001.

Possible patterns include:

```text
authorization service
guard/helper
extractor
middleware + handler-level requirement
```

Choose the simplest secure design.

Do not hide endpoint-specific permission requirements in difficult-to-audit magic.

---

# 19. AUTHENTICATION VS AUTHORIZATION ERROR SEMANTICS

Preserve the distinction:

```text
not authenticated → 401
authenticated but lacks permission → 403
```

Do not turn authorization failures into login failures.

Use stable API error codes consistent with existing contracts.

Conceptually:

```text
authentication_required
permission_denied
```

Do not expose sensitive internals.

---

# 20. PERMISSION CHECKS MUST BE SERVER-SIDE

Frontend authorization is UX only.

Never rely on:

```text
hidden button
disabled menu
client-side route guard
```

for actual security.

A malicious caller directly invoking the API must still receive 403.

Every privileged mutation/read implemented in this STEP must be protected server-side.

---

# 21. CURRENT USER AUTHORIZATION CONTEXT

Extend `/api/auth/me` or equivalent safe authenticated context so the frontend can understand the User's effective authorization.

Preferred representation:

```text
user
session
permissions
```

Do not return:

```text
password hash
session token
internal role joins
security secrets
```

You may include safe role summaries if genuinely useful.

Stable permission keys are useful for frontend UX.

Do not make the frontend calculate effective permissions by joining roles itself.

The backend is authoritative.

---

# 22. AUTHORIZATION FRESHNESS

Permission changes must take effect predictably.

Important scenarios:

```text
Role assigned
Role removed
Role disabled
Permission added to Role
Permission removed from Role
```

A User must not retain stale elevated authorization indefinitely.

Because STEP-002 uses server-side sessions, choose a strategy that ensures authorization reflects current database state within an explicitly bounded interval.

Preferred simple initial approach:

```text
resolve effective permissions server-side for protected operations
```

possibly with carefully bounded caching only if justified.

Do not encode long-lived permission state into the session cookie.

Do not require logout/login for permission changes to become effective unless explicitly justified.

Security-sensitive permission revocation should become effective promptly.

---

# 23. NO AUTHORIZATION IN COOKIE

The session cookie remains:

```text
opaque session credential
```

Do not put:

```text
roles
permissions
isAdmin
authorization version
```

into a client-controlled authorization cookie.

ADR-002 session architecture remains authoritative.

---

# 24. ROLE MANAGEMENT API

Implement the minimum APIs required to manage Roles.

Equivalent capabilities:

```text
GET    /api/roles
POST   /api/roles
GET    /api/roles/:id
PATCH  /api/roles/:id
```

and lifecycle action if appropriate:

```text
disable / enable
```

Exact routing should follow project conventions.

Do not implement destructive delete merely for CRUD completeness.

All endpoints require appropriate permissions.

---

# 25. PERMISSION CATALOG API

Provide read access to the known Permission catalog for authorized administrators.

Equivalent:

```text
GET /api/permissions
```

Do NOT provide:

```text
POST /api/permissions
```

for arbitrary administrator-created permission keys.

Permission catalog ownership remains with application/spec migrations.

---

# 26. ROLE PERMISSION MANAGEMENT API

Provide controlled ability to change a Role's permissions.

Equivalent capabilities:

```text
GET /api/roles/:id/permissions
PUT/PATCH /api/roles/:id/permissions
```

Choose replace-vs-delta semantics deliberately.

Requirements:

```text
unknown permission key rejected
duplicate permissions impossible
transactional update
audited
disabled role semantics preserved
```

Prefer APIs that make final desired state explicit and idempotent.

---

# 27. USER ROLE MANAGEMENT API

Provide controlled APIs equivalent to:

```text
GET /api/users/:id/roles
PUT/PATCH /api/users/:id/roles
```

or explicit assignment/removal actions.

Requirements:

```text
cannot assign unknown Role
duplicate assignment impossible
cannot bypass permissions
transactional
audited
```

Do not build general User profile management beyond what STEP-003 needs.

---

# 28. SELF-LOCKOUT PROTECTION

Administrative authorization systems can accidentally lock out the last administrator.

Design protection against catastrophic self-lockout.

At minimum evaluate:

```text
removing own administration Role
disabling own only administrative Role
removing roles.manage from the only usable administrative Role
disabling the last Role capable of authorization administration
```

Do NOT solve this with a hidden permanent superuser bypass.

Preferred model:

> preserve at least one active path to authorization administration.

Define the invariant precisely.

The system should reject a mutation that would leave zero active Users capable of restoring RBAC administration through normal application authorization.

If an explicit infrastructure/operator recovery command is provided, it must be deliberately privileged, auditable where possible, documented, and not usable through ordinary HTTP.

Do not overcomplicate this; but do not leave the system trivially lockable.

---

# 29. CRITICAL FUTURE PERMISSIONS

Do not implement future financial permissions yet.

However, architecture must support later permissions such as:

```text
payments.read
payments.collect
payments.correct
ledger.read
reports.financial.read
shares.manage
governance.manage
social_aid.manage
```

without schema redesign.

These examples are architecture tests, NOT permission seeds for this STEP.

---

# 30. ADR-013 BOUNDARY

ADR-013 explicitly establishes:

```text
production restore
```

as infrastructure/operator privileged.

Therefore DO NOT create application permissions such as:

```text
backup.restore
database.restore
production.restore
```

Do not expose a production restore button.

Future backup-health viewing may have an application permission, but that functionality does not exist yet.

---

# 31. APPROVALS ARE NOT RBAC

Do not confuse:

```text
permission
```

with:

```text
approval workflow
```

Example:

A future User may have:

```text
payments.correct
```

but a particular correction might additionally require approval.

RBAC answers whether the User may participate in the action.

Approval rules answer whether the particular transaction may proceed.

Do not implement approval workflows in STEP-003.

Preserve architectural room for them.

---

# 32. OWNERSHIP / ROW-LEVEL RULES ARE NOT PURE RBAC

Future rules may depend on:

```text
organization
workspace
record ownership
family
cashbox
governance context
transaction amount
```

Do not force all future authorization into Role/Permission tables.

Design authorization so future policy/context checks can exist alongside RBAC.

Conceptually:

```text
RBAC permission
+
domain policy
+
approval policy
```

may all be required.

STEP-003 implements only the RBAC foundation.

---

# 33. DATABASE MODEL

Create only required authorization schema.

Likely concepts:

```text
roles
permissions
role_permissions
user_role_assignments
```

Exact names follow repository conventions.

Requirements:

```text
UUID IDs where appropriate
FK integrity
uniqueness
indexes for permission resolution
timestamps
role status
safe assignment constraints
```

Do not create business-domain tables.

---

# 34. DATABASE-ENFORCED INVARIANTS

Where appropriate enforce:

```text
permission.key unique
role normalized name unique
(role_id, permission_id) unique
(user_id, role_id) unique
```

Application validation alone is insufficient for these invariants.

Test PostgreSQL constraints directly.

---

# 35. PERMISSION SEEDING

Known application Permission records need deterministic provisioning.

Choose a controlled strategy such as:

```text
migration-controlled seeds
```

or another repository-owned deterministic catalog mechanism.

Requirements:

```text
same key always means same capability
deployment reproducible
unknown DB permission cannot silently become trusted
permission removal/change deliberate
```

Do not seed through random runtime startup side effects unless strongly justified.

---

# 36. AUDIT EVENTS

At minimum durable audit history should cover:

```text
role created
role renamed/updated
role enabled
role disabled
role permission set changed
role assigned to User
role removed from User
bootstrap authorization granted
lockout-protection rejection if appropriate
```

Capture actor where available.

Do not record secrets.

Useful before/after security state may be recorded in a structured, bounded form where consistent with the existing audit model.

---

# 37. FRONTEND — AUTHORIZATION-AWARE SHELL

Extend the authenticated SvelteKit shell.

Navigation/actions should respond to effective permissions.

Examples:

A User without:

```text
roles.read
```

should not see the role-management navigation entry.

But remember:

> hidden navigation is UX, not security.

Backend must still enforce.

---

# 38. FRONTEND — ROLE MANAGEMENT

Create a polished Turkish-first role-management interface using shadcn-svelte.

At minimum support:

```text
Rol listesi
Rol oluşturma
Rol detay/düzenleme
Aktif/Pasif durumu
Yetki seçimi
```

Use understandable Turkish terminology.

Suggested UI concepts:

```text
Roller
Yetkiler
Kullanıcı Atamaları
Aktif
Pasif
```

Do not expose internal UUIDs as primary user-facing identity.

---

# 39. PERMISSION SELECTION UX

Do not present a giant unstructured checkbox wall if avoidable.

Permission catalog should support grouping/category presentation.

For this STEP the catalog is small, but design the UI so future permissions can be grouped.

Example:

```text
Kullanıcı Yönetimi
Rol ve Yetki Yönetimi
```

Do not invent future categories with fake permissions.

---

# 40. FRONTEND — USER ROLE ASSIGNMENT

Provide the minimum UI needed for an authorized administrator to inspect and modify a User's Role assignments.

Do not build the full future User administration module.

A focused page/modal is sufficient.

Must clearly show:

```text
User
assigned Roles
available Roles
active/disabled role status
```

Prevent confusing assignment of disabled Roles if backend policy rejects it.

---

# 41. UI PERMISSION CHECK HELPER

Create a centralized frontend authorization helper/store derived from backend `/me`.

Conceptually:

```text
can("roles.read")
can("roles.manage")
```

Do not scatter:

```text
permissions.includes(...)
```

through dozens of components if a small reusable helper can provide consistent semantics.

Default:

```text
unknown/missing permission → false
```

---

# 42. DEFAULT LANGUAGE

Default language remains:

```text
Turkish
tr-TR
```

All new user-facing strings must use the existing i18n architecture.

Do not hard-code an unmaintainable parallel localization mechanism.

---

# 43. API CONTRACTS

Update shared contracts appropriately.

Authorization-related request/response contracts must remain synchronized between Rust and TypeScript.

Do not expose DB row shapes directly as API contracts.

Do not expose internal security implementation unnecessarily.

---

# 44. CONCURRENCY

Review authorization mutations under concurrency.

Examples:

```text
two admins assign same Role simultaneously
two admins update same Role permissions
Role disabled while assignment occurs
last-admin protection under concurrent mutations
```

Database constraints and transactions must preserve invariants.

The last-administration-path invariant is especially concurrency-sensitive.

Use appropriate PostgreSQL transactional locking/serialization strategy for mutations that could remove the final administration path.

Do not solve with process-local mutexes.

---

# 45. IDEMPOTENCY

Where API semantics naturally allow it, prefer idempotent final-state operations.

Examples:

```text
setting Role permission set to X
setting User role set to Y
disabling an already disabled Role
```

Repeated identical requests should not corrupt state or create duplicate assignments.

Follow ADR-006 where applicable.

---

# 46. RATE LIMITING

Do not blindly apply login rate limiting to all authorization endpoints.

Authenticated authorization endpoints are protected primarily by:

```text
authentication
authorization
CSRF
validation
audit
```

If an endpoint creates a specific abuse risk, address it deliberately.

Do not introduce arbitrary throttling without need.

---

# 47. CSRF

All authenticated state-changing RBAC endpoints must remain protected by the STEP-002 CSRF architecture.

Test this at HTTP level.

Do not create an authorization endpoint that bypasses existing CSRF protection.

---

# 48. CACHE CONTROL

Sensitive authorization data such as:

```text
/me
roles
permissions
User role assignments
```

must not receive unsafe shared caching behavior.

Preserve existing security/cache-control architecture.

---

# 49. TESTING — PERMISSION RESOLUTION

Test at minimum:

```text
User with no Roles → denied
User with Role without Permission → denied
User with Role containing Permission → allowed
User with multiple Roles → union of permissions
duplicate assignment impossible
disabled Role → grants nothing
removed Role → permission disappears
removed RolePermission → permission disappears
```

---

# 50. TESTING — SERVER ENFORCEMENT

HTTP-level tests must prove:

```text
unauthenticated → 401
authenticated without permission → 403
authenticated with permission → success
```

Do not test only helper functions.

Direct API calls must be protected.

---

# 51. TESTING — PERMISSION FRESHNESS

Prove that:

```text
permission granted
→ subsequent authorized request gains access

permission revoked
→ subsequent request loses access

Role disabled
→ subsequent request loses access
```

without requiring a new login unless architecture explicitly and convincingly requires it.

---

# 52. TESTING — ROLE MANAGEMENT

Test:

```text
create Role
duplicate normalized Role name rejected
rename Role
disable Role
enable Role
unknown permission rejected
set permissions
repeat same permission-set operation safely
```

---

# 53. TESTING — USER ROLE ASSIGNMENT

Test:

```text
assign Role
duplicate assignment rejected or idempotently resolved by chosen API semantics
remove Role
assign unknown Role rejected
cannot assign disabled Role if policy forbids it
cannot modify assignments without roles.manage
```

---

# 54. TESTING — SELF-LOCKOUT

Explicitly test the chosen last-administration-path invariant.

At minimum:

```text
cannot remove final authorization-admin path
cannot disable final administrative Role
cannot strip required administration permission from final path
operation succeeds when another valid administrative path exists
```

Also test the relevant concurrent mutation behavior against real PostgreSQL if necessary.

This is security-critical.

---

# 55. TESTING — AUDIT

Verify durable audit records for security-sensitive mutations.

Test actor/action/target semantics.

Do not snapshot secrets.

---

# 56. TESTING — CSRF

HTTP integration tests must prove state-changing RBAC APIs reject invalid/missing CSRF proof according to STEP-002 architecture.

Do not duplicate the entire STEP-002 suite unnecessarily.

Add representative coverage.

---

# 57. DATABASE TESTS

Against real PostgreSQL verify:

```text
migrations apply cleanly
permission key uniqueness
Role normalized-name uniqueness
RolePermission uniqueness
UserRole uniqueness
FK integrity
disabled Role behavior
authorization joins
lockout protection transaction behavior
```

Use actual PostgreSQL where DB semantics matter.

---

# 58. FRONTEND TESTS

At minimum cover:

```text
role list rendering
permission-aware navigation
Role create/edit flow
permission selection
User Role assignment
403/error handling where relevant
unknown permission defaults to denied
```

Test application behavior, not shadcn-svelte internals.

---

# 59. E2E

STEP-002 deliberately established auth behavior.

Evaluate adding a focused RBAC E2E test.

A useful scenario:

```text
Admin logs in
→ creates Role
→ grants permission
→ assigns Role to another User
→ second User gains access to permitted UI/API
→ permission removed
→ access disappears
```

Keep E2E deterministic and small.

If not added, explain why HTTP + frontend integration coverage sufficiently proves the boundary.

---

# 60. HOSTILE AUTHORIZATION REVIEW

Before closure actively attempt to bypass the system.

Review at minimum:

### Direct API bypass

Can a User invoke a hidden frontend action directly?

### Role-name bypass

Does any code grant power because a Role is named "Admin" or similar?

### User-name bypass

Does bootstrap User receive hidden privileges?

### Stale permissions

Can revoked permission survive through session state/cache?

### Disabled Role

Does it still contribute permissions?

### Assignment ownership

Can unauthorized User modify another User's Roles?

### Arbitrary permission injection

Can an administrator or crafted request create/use a permission key not owned by the application catalog?

### Last-admin lockout

Can concurrent requests remove all authorization-admin paths?

### CSRF

Can a cross-origin request alter Roles/permissions/assignments?

### IDOR

Can UUID manipulation expose or mutate unauthorized Role/User data?

### Audit bypass

Can sensitive RBAC mutation occur without durable audit?

### Wildcards

Can future permissions be silently inherited through a wildcard?

### Production restore

Did any backup/restore infrastructure permission accidentally enter application RBAC?

Fix defects before closure.

---

# 61. DOCUMENTATION

Update project documentation as required.

Document:

```text
authorization architecture
Role/Permission distinction
permission-key convention
permission catalog ownership
effective-permission semantics
Role lifecycle
User Role assignment
bootstrap authorization procedure
last-admin protection
frontend authorization behavior
server-side enforcement
how to add a new permission in a future STEP
how to run RBAC tests
```

Do not rewrite unrelated specification.

Do not change Accepted ADRs unless a genuine contradiction is discovered.

If an ADR conflict appears:

STOP and report it.

---

# 62. BACKUP READINESS GATE

ADR-013 is binding.

However STEP-003 introduces authorization metadata, not material financial production data.

Therefore:

```text
Backup Readiness Gate
```

does NOT need full production backup tooling implementation to close STEP-003.

Still:

- do not weaken ADR-013;
- do not add production restore permission to RBAC;
- preserve future backup observability boundary.

Explicitly report ADR-013 consistency.

---

# 63. QUALITY GATES

Run all established gates.

At minimum:

```text
git diff --check

frontend:
format
lint
typecheck/check
tests
build

backend:
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

database:
clean migration verification
RBAC PostgreSQL integration tests

Docker:
existing smoke verification where applicable

E2E:
if introduced
```

Report exact results/counts.

Do not hide skipped DB tests.

The final database gate must execute DB-dependent RBAC tests against PostgreSQL.

---

# 64. DIFF REVIEW

Before commit:

```text
git status --short
git diff --stat
git diff
git diff --check
```

Classify every changed file.

Check for:

```text
unrelated domain implementation
generated junk
logs
test artifacts
secrets
.env files
database dumps
temporary credentials
```

No unexplained changes.

---

# 65. COMMIT

Only after:

```text
implementation complete
security review complete
all gates pass
diff reviewed
```

Create one coherent STEP-003 commit.

Suggested:

```text
feat: add role based authorization foundation
```

Do not amend STEP-001/002/spec commits.

Do not squash history.

---

# 66. PUSH

Unlike earlier STEPs, HTTPS origin is now verified.

Before pushing:

```text
git fetch --prune origin
git rev-parse origin/main
```

Verify `origin/main` still equals the STEP-003 starting baseline:

```text
c86a197
```

If origin changed unexpectedly:

STOP.

Do not overwrite remote work.

If safe:

```text
git push origin main
```

Never force-push.

---

# 67. POST-PUSH VERIFICATION

After push:

```text
git fetch origin
git rev-parse HEAD
git rev-parse origin/main
git status
git branch -vv
```

Required:

```text
HEAD == origin/main
working tree clean
main tracks origin/main
```

---

# 68. REQUIRED CLOSURE REPORT

Produce:

```text
# STEP-003 — RBAC & AUTHORIZATION FOUNDATION
# CLOSURE REPORT
```

with ALL sections below.

## A. Baseline

Report:

```text
branch
starting HEAD
origin/main
working tree
HEAD == origin/main
```

## B. Specification Review

Documents read and binding constraints.

## C. Authorization Architecture

Explain:

```text
User
Role
Permission
UserRoleAssignment
RolePermission
authorization service
request enforcement
```

## D. Permission Catalog

List exact Permission keys introduced in STEP-003.

Explain why each exists.

Confirm no speculative financial/domain permissions were seeded.

## E. Role Model

Schema, normalization, lifecycle and disabled behavior.

## F. User Role Assignment

Semantics, constraints and transactional behavior.

## G. Effective Permissions

Explain union semantics, default deny and freshness strategy.

## H. Bootstrap Authorization

Explain how the existing first User obtains legitimate RBAC administration without a hidden bypass.

## I. Last-Administration-Path Protection

State the exact invariant and concurrency strategy.

## J. APIs

List all new/modified endpoints and required permissions.

## K. `/me` Authorization Context

Explain safe permission/Role data exposed to frontend.

## L. Frontend

Describe:

```text
authorization-aware navigation
Role management
permission selection
User Role assignment
Turkish/i18n behavior
```

## M. Audit

List security-sensitive events recorded.

## N. Database

Tables, indexes, FKs, uniqueness constraints and migrations.

Confirm no unrelated business-domain tables.

## O. Security Review

Report findings for:

```text
direct API bypass
role-name bypass
username bypass
stale permissions
disabled Role
arbitrary permission injection
IDOR
CSRF
last-admin lockout
audit bypass
wildcards
production restore boundary
```

## P. Tests

Exact counts grouped by:

```text
Rust unit
Rust HTTP/integration
PostgreSQL
frontend
E2E if applicable
```

## Q. Quality Gates

Every command and exact result.

## R. ADR Consistency

At minimum:

```text
ADR-001
ADR-002
ADR-006
ADR-011
ADR-013
```

## S. Backup Readiness

Confirm STEP-003 does not trigger the financial-production Backup Readiness Gate and did not weaken ADR-013.

## T. Files Changed

Every file grouped by purpose.

## U. Repository Hygiene

Report:

```text
git diff --check
secret review
temporary artifacts
final working tree
```

## V. Commit

Report STEP-003 commit hash.

## W. Remote Synchronization

Report:

```text
pre-push origin/main
push result
post-push HEAD
post-push origin/main
HEAD == origin/main
```

## X. Deferred Work

Explicitly list intentionally deferred work such as:

```text
Shareholder domain
Family/Guardian domain
Share lifecycle
financial permissions
approval workflows
contextual/domain policies
backup tooling
production restore
```

## Y. Final Result

End with exactly one:

```text
READY FOR STEP-003 REVIEW
```

or:

```text
STEP-003 BLOCKED
```

---

# 69. STOP CONDITION

After producing the Closure Report:

**STOP.**

Do NOT begin:

```text
STEP-004
Shareholder implementation
Guardian implementation
Family implementation
Share implementation
Period implementation
Payment implementation
Ledger implementation
Investment implementation
Social Aid implementation
backup tooling implementation
```

Wait for reviewer authorization.

The reviewer will inspect STEP-003 before the next domain STEP is authorized.