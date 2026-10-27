# Legacy Auth Recovery Runbook — schema ≤v33 → v79 upgrades

Scope: databases upgraded from schema versions that predate the crypt
`login_verifier` lane. Their `users` rows keep `login_verifier IS NULL`
(or empty) and `must_change_password = false`. Verified on synthetic
v20→v79 and v33→v79 upgrade rehearsals (2026-10-27).

## Symptom

`app_session_open` verifies `crypt(password, login_verifier)`.
For NULL/empty verifier rows the comparison yields NULL → **every login
attempt returns 401** while nothing flags the account. The user is
silently locked out.

Proven by `apps/server/tests/legacy_auth_upgrade.rs`:
`legacy_user_login_fails_until_platform_reset_recovery`.

## Pre-migration checklist (operator)

1. Snapshot the target database; restore-verify the snapshot in a
   scratch database before touching the target.
2. Measure the gap on the target (read-only, no secrets):

   ```sql
   SELECT count(*) FROM users
   WHERE login_verifier IS NULL OR btrim(login_verifier) = '';
   ```

   `db::legacy_verifier_gap(pool)` returns the same count from the app
   side and is exercised by tests. This is a COUNT only — it never
   surfaces credential material.

3. Confirm at least one **usable platform-administrator recovery
   path**: a platform administrator (NULL-scope `users.manage`) whose
   own credentials work and who can call
   `POST /api/platform/users/{id}/reset-password`.

   **If no authorized administrator can authenticate** (e.g. every
   platform admin is itself a verifier-less legacy user), the recovery
   chain is broken: there is no self-service path and no supported
   bypass. Stop and escalate — a platform admin credential must be
   re-provisioned by an authorized operator outside the app before
   end-user resets can proceed. Do NOT hand-edit `login_verifier`;
   there is intentionally no way to derive a bcrypt verifier from an
   Argon2id hash.

## Recovery procedure (per affected user)

1. Platform admin calls
   `POST /api/platform/users/{id}/reset-password` (audited
   `password_reset_by_admin`; revokes all existing sessions).
2. Deliver the temporary password to the user out-of-band. It is shown
   once, never logged, never in audit metadata.
3. First login consumes the temporary password atomically
   (single-use; concurrent attempts allow exactly one winner) and
   creates a forced-change session.
4. User completes `POST /api/auth/password` — the sealed definer
   rotates both credential representations (Argon2id `password_hash`
   + bcrypt `login_verifier`), clears `must_change_password`, and
   revokes all old sessions.
5. Verify: user logs in with the new password;
   `legacy_verifier_gap` no longer counts them
   (`login_verifier` non-NULL).

## What NOT to do

- Do not backfill `login_verifier` from `password_hash` — crypt
  cannot be derived from Argon2id; any such backfill corrupts
  credentials.
- Do not disable `must_change_password` or the extractor gate.
- Do not treat `/ready` as failed because gap > 0 — the gap is an
  operational signal, not a service outage.

## Residual risk

Legacy gap does not block app startup or `/ready`. Until every
affected user is reset, they remain locked out — operationally a
support queue, not a security hole (no elevated access results).
