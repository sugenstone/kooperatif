# 18 --- Authorization and Approvals

## Model

Use:

**RBAC + Scoped Permissions + Approval Policies**

These are separate concerns: - Permission: what action may be
attempted? - Scope: on which resources/context? - Approval Policy: what
additional authorization is required before effect?

## Roles

Roles are configurable permission bundles.

Starter roles may include: - Sistem Yöneticisi - Kooperatif Yöneticisi -
Muhasebe - Kasa Sorumlusu - Kurul Üyesi - Denetçi - Görüntüleyici

These are templates, not hard-coded business logic.

A User may have multiple Role Assignments.

## Permissions

Prefer atomic permissions.

Examples:

``` text
shareholder.view
shareholder.create
shareholder.update
share.view
share.allocate
share.transfer
share.return
payment.view
payment.create
payment.post
payment.reverse
financial_account.view
policy.view
policy.propose
policy.approve
report.export_pdf
report.export_xlsx
audit.view
```

Final catalog is maintained centrally.

## Scope

A permission may be scoped to: - whole cooperative; - specific Financial
Account; - specific organizational/location context; - specific
module/resource group.

Scope must be evaluated backend-side.

## Read vs write

Viewing sensitive financial/personal data is a separate capability from
modifying it.

Denetçi may have broad read/audit access without transaction creation.

## Temporary assignments

Role/permission assignments may have start/end times.

Expired assignments cease authorization automatically.

## Sensitive permissions

Examples: - reverse posted Payment; - correct locked Period; - close
Share; - crystallize Profit Right; - activate Policy Version; - modify
Roles/Permissions; - bulk export sensitive data.

Sensitive actions may require step-up authentication/2FA in future.

## Approval policies

Approval may depend on: - action type; - amount threshold; - Financial
Account; - domain; - risk classification; - Policy; - Board Decision.

Example only, not a configured default: - small expense: no second
approval; - medium: second approver; - large: management/board approval.

Do not hard-code example thresholds.

## Segregation of duties

Default design should support: `creator != approver`

Whether self-approval is allowed is policy-driven.

## Approval history

Preserve: - requester; - request time; - approver/rejector; - decision
time; - reason/comment; - source action; - policy/rule requiring
approval.

## Authorization evaluation

Avoid scattered:

``` rust
if user.role == "admin"
```

Use centralized authorization service/middleware/policy evaluation.

Conceptual flow:

``` text
Authenticated User
 -> Tenant Context
 -> Role Assignments
 -> Permissions
 -> Scopes
 -> Resource/Action
 -> Approval Requirement
 -> Allowed / Denied / Requires Approval
```

## Frontend

Frontend may hide/disable unauthorized actions for UX.

Backend remains authoritative.

## Audit

Role, permission, scope and approval-policy changes are audited.

A temporary privilege escalation must be historically visible.

## Denial semantics

API should distinguish authentication failure, authorization denial,
resource-not-found/tenant-safe semantics and approval-required state
according to security/API specs without leaking cross-tenant existence.
