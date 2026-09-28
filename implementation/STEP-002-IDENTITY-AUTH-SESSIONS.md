# STEP-002 — IDENTITY, AUTHENTICATION & SERVER-SIDE SESSION FOUNDATION

Repository:

```text
git@github.com:sugenstone/kooperatif.git
```

Expected STEP-001 baseline commit:

```text
c7e8823
```

Specification baseline:

```text
Kooperatif Agent Specification v0.7
```

STEP-001 status:

```text
CLOSED
```

This STEP implements the first real application security/domain capability:

> Internal application Users + secure authentication + server-side session lifecycle.

This STEP does **NOT** implement Shareholder authentication.

Shareholders are records and do not log in to the application.

This STEP also does **NOT** implement the complete RBAC/role-management system.

That will be handled in a later controlled STEP.

---

# 0. OPERATING MODE

Work as a senior security-conscious Rust/SvelteKit engineer.

Follow:

```text
INSPECT
→ VERIFY BASELINE
→ READ SPEC
→ THREAT-MODEL
→ PLAN
→ IMPLEMENT
→ TEST
→ SECURITY REVIEW
→ REVIEW DIFF
→ REPORT
→ STOP
```

Do not continue to STEP-003.

Do not implement unrelated business domains.

Do not invent missing cooperative business rules.

Authentication is security-sensitive.

Prefer:

```text
simple
explicit
revocable
auditable
testable
server-controlled
```

over cleverness.

---

# 1. BASELINE VERIFICATION — BEFORE ANY CHANGE

Before modifying files, verify:

```text
git status
git branch --show-current
git rev-parse HEAD
git remote -v
git log --oneline --decorate -n 10
```

Expected baseline:

```text
branch: main
HEAD: c7e8823
working tree: clean
```

If HEAD differs:

**DO NOT reset, rebase, overwrite, or discard work.**

Inspect why.

If the difference represents legitimate work after STEP-001, report it and determine whether STEP-002 can safely proceed.

If the repository history conflicts with the expected baseline, stop if proceeding could destroy or misattribute work.

Also inspect whether `origin/main` exists.

If remote access is available, fetch metadata safely:

```text
git fetch origin
```

Do NOT push.

Do NOT rewrite remote history.

Because STEP-001 initialized Git locally, explicitly report whether:

```text
origin/main does not exist
origin/main == local baseline
origin/main diverges
```

This must be known before any future push.

---

# 2. READ AUTHORITATIVE SPECIFICATION

Before coding, read at minimum:

```text
AGENTS.md
README.md

docs/00-PROJECT-CHARTER.md
docs/01-DOMAIN-GLOSSARY.md
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
docs/adr/ADR-012-DEPLOYMENT-TOPOLOGY.md
```

Read additional documents if implementation touches their concerns.

Accepted ADRs are binding.

Do not silently change ADR-001 or ADR-002.

---

# 3. STEP SCOPE

Implement:

```text
Internal User identity
Password credential
Login
Server-side Session
Authenticated-current-user lookup
Logout current session
Logout all/other sessions where appropriate
Session listing
Session revocation
Authentication middleware/extractor
Minimal authenticated frontend shell
Login UI
Session-management UI foundation
Security/audit events required by this STEP
```

Do NOT implement:

```text
Shareholder login
Guardian login
Family login
Social Aid participant login

full RBAC
custom role builder
permission matrix editor
Board permissions
financial permissions
approval workflow
password reset email
email verification
SMS authentication
TOTP UI
passkeys
OAuth
SSO
WebAuthn
invitation workflow
public registration
```

Do not expand scope merely because these may be useful later.

---

# 4. CRITICAL DOMAIN DISTINCTION

These concepts must remain separate:

```text
User
Shareholder
Guardian
Party
Family
```

A `User` is an authenticated application operator.

A `Shareholder` is a cooperative business record.

A Shareholder does NOT automatically have a User account.

A User does NOT automatically have a Share.

Do not introduce a required FK:

```text
users -> shareholders
```

or:

```text
shareholders -> users
```

Future optional relationships may be introduced only when explicitly specified.

---

# 5. USER MODEL

Create the minimum production-worthy internal User model.

Suggested conceptual fields:

```text
id
username
display_name
status
password_hash
created_at
updated_at
last_login_at
```

Adjust naming if repository conventions justify it.

Do not add speculative profile fields.

Username requirements:

- login identifier;
- normalized consistently;
- unique under the chosen normalization;
- must not rely on application-only uniqueness;
- database constraint/index must enforce uniqueness.

Choose and document the normalization rule.

Do not silently assume email is the login identifier.

The cooperative may use operator usernames independent of email.

---

# 6. USER STATUS

User lifecycle must support at least:

```text
active
disabled
```

If an enum/type is used, design it so later lifecycle expansion is possible.

A disabled User:

```text
cannot create a new authenticated session
```

and existing sessions must no longer provide authenticated access.

Choose one robust strategy:

A. revoke sessions immediately when User becomes disabled;

and/or

B. authenticated request validation checks current User status.

The final design must guarantee:

> A disabled User cannot continue using an old session.

Document and test the invariant.

Do not implement a full user-administration UI unless needed to prove this invariant.

---

# 7. INITIAL ADMIN BOOTSTRAP

The system needs a safe way to obtain its first internal User.

There is no public registration.

Implement a controlled bootstrap mechanism.

Preferred direction:

```text
server CLI/admin command
```

Example conceptually:

```text
kooperatif-server create-user
```

or equivalent.

Requirements:

- explicit operator action;
- username;
- display name;
- password provided safely;
- password is never printed;
- password is never logged;
- duplicate username rejected cleanly;
- password hash generated using ADR-002 requirements.

Avoid requiring manual SQL to create the first User.

Do not build a public `/register` endpoint.

Do not hard-code a default admin password.

Do not seed a known production credential.

If non-interactive password input is supported for automated tests/development, document its security limitations and prevent accidental secret logging.

---

# 8. PASSWORD HASHING

ADR-002 requires:

```text
Argon2id
```

Use a maintained Rust implementation.

Password storage must contain only the password hash representation required for verification.

Never store:

```text
plaintext password
reversible encrypted password
password in audit logs
password in tracing fields
password in error responses
```

Use secure password verification.

Do not implement custom cryptography.

---

# 9. PASSWORD POLICY

Implement a reasonable baseline password policy without over-engineering.

The exact policy must be documented and centrally validated.

Prefer a minimum-length oriented policy rather than arbitrary composition rules such as:

```text
must contain uppercase
must contain symbol
must contain number
```

unless the specification already requires those rules.

Support long passphrases.

Reject clearly unacceptable password lengths.

Do not silently truncate passwords.

Password-policy errors must use safe localized messages.

---

# 10. SESSION MODEL

ADR-002 requires:

```text
server-side
revocable
sessions
```

The browser cookie must NOT contain authorization state.

The cookie contains only an opaque session secret/reference suitable for locating/verifying the server-side session.

A session record should support concepts equivalent to:

```text
id
user_id
secret verifier / token hash
created_at
last_seen_at
expires_at
idle_expires_at or equivalent idle tracking
revoked_at
revocation reason where useful
client metadata where privacy-safe/useful
```

Do not blindly add every field above.

Implement only what the lifecycle requires.

---

# 11. SESSION TOKEN STORAGE

Treat session tokens like credentials.

Preferred design:

```text
Browser stores:
raw opaque session token

Database stores:
hash/verifier of token
```

Do not store the raw bearer-equivalent session secret in PostgreSQL if avoidable.

Use cryptographically secure random token generation.

Session lookup must be indexed appropriately.

Do not use predictable sequential tokens.

---

# 12. COOKIE SECURITY

Use a dedicated authentication cookie.

Production cookie requirements:

```text
HttpOnly
Secure
SameSite appropriate to same-origin architecture
Path=/
```

Cookie name should be explicit and project-owned.

Do not use localStorage/sessionStorage for the session credential.

Development over plain localhost may require `Secure=false`.

If so:

- make this environment-aware;
- production must fail safe;
- do not silently ship insecure production cookies.

Document exact behavior.

---

# 13. SESSION EXPIRATION

ADR-002 requires configurable session behavior.

Implement configuration for at least:

```text
absolute session lifetime
idle timeout
```

Do not hard-code business-critical durations throughout application code.

Reasonable development defaults may exist.

Production configuration must be explicit or have clearly documented safe defaults according to the existing configuration philosophy.

Use server-side timestamps as authority.

Client clocks are not authoritative.

---

# 14. LAST-SEEN / IDLE EXPIRY

Avoid writing `last_seen_at` to PostgreSQL on every HTTP request if that would cause unnecessary write amplification.

Choose and document a strategy.

Examples:

```text
touch only after N minutes
```

or another bounded strategy.

But the strategy must preserve the intended idle-timeout security semantics.

Test boundary behavior.

---

# 15. SESSION ROTATION

Successful authentication must create a fresh session.

Do not reuse attacker-supplied session identifiers.

If session rotation is needed for sensitive state transitions, provide an architecture-compatible mechanism.

At minimum prevent session fixation at login.

---

# 16. LOGIN ENDPOINT

Implement an API equivalent to:

```text
POST /api/auth/login
```

Exact path may follow existing API conventions.

Input conceptually:

```json
{
  "username": "...",
  "password": "..."
}
```

Successful result:

- creates server-side Session;
- sets secure auth cookie;
- updates relevant login metadata;
- returns safe authenticated User information.

Do not return:

```text
password_hash
session token in JSON
internal secret values
```

---

# 17. LOGIN ERROR SEMANTICS

Do not reveal whether:

```text
username exists
password was wrong
user was disabled
```

to unauthenticated callers beyond what is operationally safe.

Prefer a generic authentication failure such as:

```text
Kullanıcı adı veya parola hatalı.
```

for invalid credentials.

A disabled account may also use generic external semantics if that better prevents account enumeration.

Internally/audit-wise the cause may be recorded safely.

Use stable machine-readable error codes consistent with `docs/21-API-CONTRACTS.md`.

---

# 18. RATE LIMITING / BRUTE-FORCE PROTECTION

ADR-002 requires login brute-force protection.

Implement an initial application-level mechanism suitable for the current single-node deployment.

It must:

- limit repeated authentication attempts;
- avoid storing plaintext passwords;
- avoid leaking account existence;
- recover automatically after a defined period;
- be testable;
- be observable without logging secrets.

Do NOT introduce Redis solely for this.

Because ADR-012 starts with a single-node deployment, an in-process limiter may be acceptable if clearly documented as such.

However:

> username-only limiting is insufficient.

Consider at least request origin + normalized username or equivalent abuse dimensions.

Do not create a denial-of-service primitive where an attacker can permanently lock another User account.

Document the scaling limitation and the trigger for revisiting it.

---

# 19. AUTHENTICATED REQUEST EXTRACTOR

Implement a reusable backend authentication mechanism.

Protected endpoints should not manually repeat:

```text
read cookie
query session
query user
check expiry
check revoked
check user active
```

Create an Axum extractor/middleware/application service appropriate to ADR-001 boundaries.

It must reject:

```text
missing session
unknown session
revoked session
expired session
idle-expired session
disabled User
```

with safe semantics.

---

# 20. CURRENT USER ENDPOINT

Implement an authenticated endpoint equivalent to:

```text
GET /api/auth/me
```

Return only safe User/session context needed by the frontend.

Example conceptual response:

```text
user:
  id
  username
  displayName

session:
  id
  createdAt
  expiresAt
```

Do not return token hashes or security secrets.

---

# 21. LOGOUT

Implement:

```text
POST /api/auth/logout
```

Logout must:

- revoke/invalidate current server-side Session;
- clear the browser cookie;
- be idempotent from the user's perspective.

After logout, the old cookie/session must not authenticate again.

---

# 22. SESSION MANAGEMENT

Implement the backend foundation for users to inspect and revoke their own sessions.

At minimum support equivalent capabilities:

```text
GET    /api/auth/sessions
DELETE /api/auth/sessions/:id
```

or explicit action endpoints following project conventions.

Requirements:

- User can see only their own sessions unless future authorization says otherwise;
- identify current session;
- revoke another own session;
- do not expose session token/hash;
- session metadata should be privacy-conscious;
- attempting to revoke another User's session must not succeed.

Also provide:

```text
logout/revoke all other sessions
```

if cleanly implementable within the same model.

Do not implement administrator session-management UI yet.

---

# 23. CSRF PROTECTION

ADR-002 explicitly requires CSRF protection appropriate to cookie authentication.

Implement CSRF protection for authenticated state-changing requests.

The exact mechanism must be compatible with:

```text
same-origin SvelteKit
Rust/Axum API
HttpOnly session cookie
future WebSocket auth
```

Possible architecture:

```text
Origin/Referer validation
+
CSRF token mechanism
```

or another robust standard design.

Do not invent cryptography.

Document:

```text
which requests require CSRF validation
how frontend obtains/sends required proof
how login is treated
how same-origin is validated
how tests prove cross-site mutation is rejected
```

Do not rely on `SameSite` alone as the complete CSRF strategy.

---

# 24. AUTHORIZATION BOUNDARY

Authentication answers:

> Who is this User?

Authorization answers:

> What may this User do?

STEP-002 implements the first.

Do NOT build full RBAC here.

However, architecture must leave a clean point where later authorization can consume:

```text
AuthenticatedUser
UserId
SessionId
```

without rewriting authentication.

Avoid fake booleans such as:

```text
is_admin
is_accountant
can_reverse_payment
```

on the User table.

Those belong to the later role/permission model.

---

# 25. AUDIT EVENTS

Authentication/security-sensitive actions require durable audit/security history appropriate to existing specifications.

At minimum consider:

```text
successful login
failed login
logout
session revoked
all-other-sessions revoked
User disabled/enabled if this STEP exposes that operation
```

Do not store passwords, raw session tokens, cookie values or other secrets.

Differentiate:

```text
Audit
```

from:

```text
Observability logs
```

per ADR-011.

If the existing audit architecture is not yet implemented, create only the smallest durable foundation required by this STEP without prematurely building the entire future audit domain.

Document the boundary.

---

# 26. DATABASE MIGRATIONS

Create migrations required by STEP-002.

Likely tables/concepts:

```text
users
user_sessions
authentication/security audit foundation if needed
```

Exact names follow project conventions.

Requirements:

- UUID identifiers according to project data conventions;
- database-enforced username uniqueness;
- FK integrity;
- appropriate indexes;
- timestamps;
- session lookup index;
- session-user index;
- migration runs on clean DB;
- migration rerun remains safe through sqlx migration ledger;
- no financial/domain tables.

Do not add:

```text
roles
permissions
shareholders
families
shares
payments
ledger
```

in this STEP.

---

# 27. CONCURRENCY

Session operations must remain correct under concurrency.

Examples:

```text
logout while request is in flight
two simultaneous login attempts
two simultaneous revocations
revoking an already revoked session
```

Use idempotent semantics where appropriate.

Do not introduce broad locks.

---

# 28. FRONTEND — LOGIN ROUTE

Create a polished but restrained login screen using shadcn-svelte.

Default language:

```text
Turkish
```

Use i18n infrastructure established in STEP-001.

Required UX:

```text
Kullanıcı adı
Parola
Giriş Yap
loading state
safe validation/error state
keyboard accessibility
focus behavior
```

Do not add public registration.

Do not add "Şifremi unuttum" unless it is clearly disabled/non-actionable and useful; preferably omit it until implemented.

No fake social-login buttons.

---

# 29. FRONTEND — AUTHENTICATED SHELL

The application shell must become authentication-aware.

Unauthenticated access to protected application routes should lead to login.

Authenticated shell should display safe User context such as:

```text
display name
username if useful
logout action
```

Do not implement role-based navigation yet.

Do not invent dashboard permissions.

---

# 30. FRONTEND — SESSION MANAGEMENT

Provide a small authenticated security/session page or equivalent UI.

User should be able to see:

```text
current session
other active sessions
created/login time
last activity if exposed
expiry
```

and revoke an eligible session.

Use human-readable Turkish labels.

Do not expose raw User-Agent unless deliberately normalized into useful client metadata.

A simple:

```text
Bu oturum
Diğer oturum
```

presentation is acceptable if richer device parsing would add unnecessary dependencies.

---

# 31. AUTH STATE ON PAGE LOAD

Do not treat frontend memory as authentication truth.

On page/app load, authenticated state must be recoverable from server-side session through `/api/auth/me` or an equivalent authoritative path.

Refreshing the browser must preserve a valid session.

Expired/revoked sessions must return the UI to the unauthenticated state.

---

# 32. FRONTEND DECIMAL / FINANCIAL GUARDRAIL

Do not change the STEP-001 `DecimalString` contract strategy.

STEP-002 does not require financial values.

Do not introduce generic API helpers that parse future financial decimal strings into JavaScript `number`.

---

# 33. WEBSOCKET COMPATIBILITY

Do not implement full WebSocket functionality in this STEP.

But ensure the chosen session-cookie authentication architecture remains usable during a future same-origin WebSocket upgrade.

Document any relevant handshake/auth assumption.

Do not create a separate long-lived JWT solely for WebSocket.

---

# 34. ERROR CONTRACTS

Authentication endpoints must follow the established API error contract.

Errors need:

```text
stable machine code
safe localized/user-facing interpretation
correlation/request ID compatibility
```

Never send raw SQL/database/Argon2 errors to the browser.

Examples of possible machine concepts:

```text
authentication_failed
authentication_required
session_expired
csrf_failed
rate_limited
validation_failed
```

Use names consistent with existing conventions.

---

# 35. HTTP STATUS SEMANTICS

Use deliberate HTTP semantics.

Examples conceptually:

```text
200/204 success
400 invalid request
401 unauthenticated/invalid authentication
403 authenticated but security check forbids action where appropriate
409 state conflict where genuinely applicable
422 validation if project contract uses it
429 rate limited
500 unexpected internal failure
503 dependency unavailable
```

Follow the project's existing API conventions where already defined.

Do not use `200 OK` for failed authentication.

---

# 36. SECURITY HEADERS / CACHE CONTROL

Authentication responses and authenticated sensitive endpoints should not be accidentally cached by shared/browser caches where unsafe.

Apply appropriate:

```text
Cache-Control
```

semantics.

Review existing security headers.

Do not add a giant reverse-proxy-specific security configuration unless required for STEP-002.

---

# 37. TESTING — PASSWORDS

Test at minimum:

```text
password is hashed
plaintext is not stored
valid password verifies
invalid password fails
policy rejects invalid lengths
hash is Argon2id
```

Do not assert exact randomized hash output.

---

# 38. TESTING — LOGIN

Test:

```text
valid credentials -> session created
invalid password -> generic auth failure
unknown username -> same safe external failure
disabled user -> cannot authenticate
successful login sets expected cookie attributes
login rotates/creates fresh session
```

Verify password/raw session token never appears in returned JSON.

---

# 39. TESTING — SESSION LIFECYCLE

Test:

```text
valid session -> /me works
missing session -> rejected
unknown session -> rejected
revoked session -> rejected
absolute-expired session -> rejected
idle-expired session -> rejected
disabled User + old session -> rejected
logout invalidates session
logout clears cookie
repeated logout remains safe
```

Use controllable time where practical.

Avoid flaky wall-clock sleeps.

---

# 40. TESTING — SESSION MANAGEMENT

Test:

```text
list only own sessions
current session identifiable
revoke another own session
cannot revoke another User's session
revoke already revoked session safely
logout all other sessions
current session survives "all other" operation
```

---

# 41. TESTING — CSRF

Test authenticated mutation protection.

At minimum prove:

```text
valid same-origin request succeeds
missing/invalid CSRF proof fails
cross-origin mutation fails
safe read request remains usable
```

Do not rely solely on unit-testing a helper function.

Include HTTP-level integration coverage.

---

# 42. TESTING — RATE LIMITING

Test:

```text
normal login attempts allowed
repeated failures eventually return 429
window expires/reset logic works deterministically
successful login behavior is deliberate/documented
unknown-user attempts are also protected
```

Avoid real-time sleeps by injecting/test-controlling time or limiter state where feasible.

---

# 43. TESTING — DATABASE

Against a clean PostgreSQL database verify:

```text
migrations apply
migrations ledger correct
username uniqueness enforced by DB
session FK integrity
session lookup/revocation behavior
bootstrap user creation
login using bootstrapped User
```

Tests should prove actual PostgreSQL behavior where constraints matter.

---

# 44. FRONTEND TESTS

Add deterministic tests for important UI behavior.

At minimum:

```text
login form renders Turkish labels
loading state
successful login flow
safe failed-login error
authenticated shell/current User
logout flow
session-management render/revoke interaction
```

Do not test shadcn-svelte itself.

Test our behavior.

---

# 45. E2E DECISION

Evaluate whether this STEP now justifies introducing Playwright E2E.

Authentication is the first area where a small browser-level test may provide significant value because cookies, redirects, CSRF and refresh behavior interact.

If introduced, keep it minimal and deterministic.

A useful E2E may prove:

```text
bootstrap test User
open login
authenticate
refresh
session persists
logout
protected route no longer accessible
```

If you choose NOT to introduce E2E yet, explicitly justify why existing HTTP/component coverage is sufficient.

Do not introduce a large E2E suite.

---

# 46. TEST HELPERS

Security tests may require:

```text
test user factory
password helper
session factory
controlled clock
database cleanup
```

Keep helpers test-only where appropriate.

Do not expose insecure production shortcuts.

No `/test-login` production endpoint.

---

# 47. OBSERVABILITY

Extend ADR-011 appropriately.

Useful structured fields may include:

```text
user_id
session_id
auth outcome
reason category
request_id
```

but never:

```text
password
raw cookie
raw session token
password hash
CSRF secret
```

Rate-limit events and authentication failures should be diagnosable without leaking credentials.

---

# 48. PRIVACY

Do not store unnecessary client fingerprinting information.

If storing session client metadata, prefer coarse useful information.

Examples:

```text
created_at
last_seen_at
optional coarse client label
optional IP metadata only if justified
```

If IP is stored, document why, retention implications, and whether it is displayed.

Do not build invasive fingerprinting.

---

# 49. PERFORMANCE

Authentication is on the critical request path.

Review indexes and query count.

Avoid loading unnecessary User/session data for every request.

Argon2 must be intentionally configured for secure production use while keeping tests practical.

If tests use reduced Argon2 cost, make that test-only and prove production configuration cannot accidentally use insecure parameters.

---

# 50. DOCUMENTATION

Update README/docs as required.

Document:

```text
how to create the first User
how login/session works
cookie behavior in development vs production
session lifetime configuration
CSRF architecture
rate-limiting architecture
how to revoke sessions
how to run auth tests
security limitations/deferred features
```

Do not rewrite unrelated specification files.

If implementation reveals a genuine contradiction in an Accepted ADR, stop and report rather than silently editing the ADR.

---

# 51. NO FULL RBAC YET

This STEP intentionally stops before complete authorization.

Do not create:

```text
roles table
permissions table
role_permissions
user_roles
permission editor
role management UI
```

unless an existing Accepted specification explicitly makes one of them unavoidable for authentication itself.

The next controlled STEP can implement authorization/RBAC.

Authentication must expose a clean future input:

```text
AuthenticatedUser
UserId
SessionId
```

---

# 52. QUALITY GATES

Before closure run all applicable existing STEP-001 gates plus new auth-specific gates.

At minimum:

```text
git diff --check

pnpm format/check as established
pnpm lint
pnpm check / typecheck
pnpm test
pnpm build

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

database verification
clean migration verification
Docker compose verification
auth integration tests
```

If E2E is added:

```text
E2E test
```

must also pass from a clean reproducible state.

Do not hide skipped security tests.

A test that requires PostgreSQL may explicitly skip only in the normal unit-test environment if the existing project convention allows it, but the final DB gate must actually execute it against PostgreSQL.

---

# 53. HOSTILE SECURITY REVIEW BEFORE CLOSURE

Before declaring completion, actively try to break your implementation.

Review at minimum:

### Session fixation

Can an attacker choose/reuse the victim's session identifier?

### Raw token leakage

Can session secrets appear in:

```text
database
logs
JSON
URLs
error messages
```

### Account enumeration

Do unknown username and wrong password reveal materially different external behavior?

### Disabled User

Can a disabled User continue using an old session?

### CSRF

Can a cross-origin request perform logout/session revocation or another authenticated mutation?

### Session ownership

Can User A revoke/list User B's sessions by guessing UUIDs?

### Expiry

Are absolute and idle expiry both enforced server-side?

### Cookie

Are production cookie flags correct?

### Rate limiting

Can trivial username variation bypass the limiter?

Can an attacker permanently lock out another User?

### SQL injection

Are authentication queries parameterized?

### Password handling

Can plaintext survive in logs, errors, structs longer than necessary, fixtures or snapshots?

### Caching

Could authenticated `/me` or session data be cached improperly?

### Redirect safety

If login supports a return path, can it become an open redirect?

Fix discovered problems before closure.

---

# 54. DIFF SCOPE REVIEW

Before commit, inspect:

```text
git status --short
git diff --stat
git diff
git diff --check
```

Classify every changed file.

There should be no unexplained changes.

Spec files should remain untouched unless STEP-002 legitimately requires documentation updates.

No generated junk.

No secrets.

No database dumps.

No temporary authentication logs.

No `.env`.

---

# 55. COMMIT

Only after all gates and hostile review pass.

Create one coherent STEP-002 commit if repository workflow permits.

Suggested message:

```text
feat: add authentication and session foundation
```

Do NOT push.

Report resulting commit hash.

If commit cannot/should not be created, explain exactly why.

---

# 56. REQUIRED CLOSURE REPORT

Produce:

```text
# STEP-002 — IDENTITY, AUTHENTICATION & SESSION
# CLOSURE REPORT
```

with ALL sections below.

## A. Baseline

Report:

```text
branch
starting HEAD
origin state
working tree
STEP-001 baseline verification
```

## B. Specification Review

List documents actually read and constraints relevant to this STEP.

## C. Threat Model

Summarize the authentication/session threats considered and mitigations selected.

## D. User Model

Exact schema and normalization rules.

## E. Initial User Bootstrap

Exact CLI/workflow.

Explain secret handling.

## F. Password Architecture

Report:

```text
Argon2 variant
parameter strategy
password policy
verification flow
```

Do not disclose passwords.

## G. Session Architecture

Explain:

```text
token format concept
DB representation
token hashing
cookie
absolute expiry
idle expiry
last-seen strategy
revocation
disabled-user behavior
```

## H. Authentication API

List implemented endpoints and request/response semantics.

## I. CSRF

Explain mechanism and HTTP-level evidence.

## J. Rate Limiting

Explain:

```text
key dimensions
window
threshold
recovery
single-node limitation
```

## K. Audit / Security Events

List durable events implemented and sensitive-data exclusions.

## L. Frontend

Describe:

```text
login
authenticated shell
auth-state restoration
logout
session-management UI
Turkish/i18n behavior
```

## M. Database

List migrations/tables/indexes/constraints.

Confirm no unrelated domain tables were added.

## N. Tests

Provide exact counts grouped by:

```text
Rust unit
Rust integration
DB integration
frontend
E2E if applicable
```

## O. Security Test Matrix

Report pass/fail evidence for:

```text
invalid login
unknown user
disabled user
session fixation
revoked session
absolute expiry
idle expiry
CSRF
cross-origin mutation
session ownership
rate limiting
cookie attributes
secret leakage review
```

## P. Quality Gates

List every command and exact result.

## Q. Performance / Query Review

Summarize auth-path query/index decisions and Argon2 considerations.

## R. ADR Consistency

At minimum review:

```text
ADR-001
ADR-002
ADR-006
ADR-011
ADR-012
```

and any other ADR touched.

## S. Files Changed

Group every changed/created file by purpose.

## T. Repository Hygiene

Report:

```text
git status
git diff --check
temporary artifacts
secret scan/review
```

## U. Deferred Work

Clearly list what remains intentionally deferred, especially:

```text
full RBAC
roles/permissions
2FA/TOTP
passkeys
password recovery
WebSocket implementation
admin user management
```

## V. Commit

Report commit hash and whether anything was pushed.

## W. Final Result

End with exactly one:

```text
READY FOR STEP-002 REVIEW
```

or:

```text
STEP-002 BLOCKED
```

---

# 57. STOP CONDITION

After the Closure Report:

**STOP.**

Do NOT proceed to:

```text
STEP-003
RBAC implementation
role management
Shareholder implementation
Family implementation
Share implementation
financial implementation
dashboard implementation
```

Wait for reviewer authorization.

The reviewer will inspect the implementation and determine whether STEP-002 is closed or requires a corrective patch.