# 22 --- Security

## Security model

This is a financial/administrative system. Security is a first-class
requirement.

## Authentication

Use secure session/auth architecture selected by ADR.

Requirements: - secure password handling if passwords are used; -
session expiry/revocation; - logout invalidation; - protection against
session fixation; - step-up authentication capability for future
sensitive actions.

## Authorization

Backend-enforced RBAC + Scope + Approval.

Never trust hidden frontend buttons as security.

## Tenant isolation

Every tenant-scoped read/write is protected server-side.

Cross-tenant identifiers must not bypass isolation.

## Web security

Address as applicable: - CSRF; - XSS; - CORS; - secure cookies; -
SameSite; - CSP; - clickjacking protections; - rate limiting; -
brute-force protection.

Exact settings are environment/architecture decisions.

## Secrets

No secrets in repository, client bundles, logs or committed config.

Use environment/secret management.

## Sensitive data

Minimize collection.

Apply least privilege to shareholder, financial, Social Aid and document
data.

Social Aid may contain especially sensitive beneficiary documentation;
access must be separately permissioned.

## Logging

Do not log: - passwords; - session secrets; - tokens; - full sensitive
documents; - unnecessary personal data.

Financial logs should use identifiers/correlation safely.

## Files

Uploaded documents require: - authenticated/authorized access; - safe
filenames/storage keys; - content/type/size validation; - malware
scanning strategy if required; - no public bucket-by-default for
sensitive files.

## Exports

Bulk export is a sensitive action.

Backend scope applies to exports.

Consider audit of sensitive/bulk exports.

## Document verification

Public QR verification, if enabled, must reveal minimal approved
information and resist enumeration.

## Dependency security

Use lockfiles and automated dependency/security scanning in CI where
practical.

## Database

Use least-privilege DB credentials per environment.

No direct database exposure to public network unless architecture
explicitly requires and secures it.

## Backups

Backups contain sensitive data and require encryption/access control
consistent with production data.

## Security testing

Include: - authorization tests; - tenant isolation tests; - IDOR
tests; - session tests; - CSRF/security-header checks where
applicable; - sensitive export/file access tests.

## Incident readiness

Before production, define: - session revocation procedure; - credential
rotation; - backup restore; - audit review; - breach/incident response
ownership.
