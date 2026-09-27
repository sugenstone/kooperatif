# ADR-002 --- Authentication and Session Strategy

**Status:** Accepted

## Decision

Use server-side, revocable sessions with secure `HttpOnly`, `Secure`,
appropriately configured `SameSite` cookies. Password authentication
uses Argon2id. Session architecture must support expiry, idle timeout,
rotation, revocation, logout invalidation and future step-up
authentication, TOTP/2FA or passkeys.

Shareholders are records, not authenticated Users by default.

## Security

Apply CSRF protection appropriate to same-origin cookie sessions, login
rate limiting and brute-force controls. Never store session secrets in
browser localStorage. Active sessions can be centrally revoked.

## WebSocket

WebSocket authorization derives from authenticated session and backend
permissions/scopes. Public-display/TV mode should use a restricted
read-only identity/session rather than an administrator account.
