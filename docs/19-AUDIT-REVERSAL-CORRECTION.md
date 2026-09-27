# 19 --- Audit, Reversal and Correction

## Purpose

Make mistakes correctable without destroying history.

## Record classes

### Draft/effectless

May be edited/deleted when no protected dependency/effect exists.

### Effective non-financial

Prefer archive/deactivate/versioning when referenced.

### Posted financial

Never destructively delete.

## Cancellation

Use before authoritative execution/posting where domain allows.

Cancellation preserves meaningful workflow history when necessary.

## Reversal

Use after a posted financial effect.

Reversal: - creates a new linked event/transaction; - neutralizes
defined effects; - preserves original; - records actor/time/reason; -
updates derived balances/reports; - preserves document/receipt history.

## Correction

For posted finance:

``` text
Original Posted
 -> Reversal
 -> Replacement/Corrective Transaction
```

Correction chain must be navigable in UI/audit.

## Audit event

Candidate fields: - cooperative; - actor User; - timestamp; - action
code; - entity type/id; - correlation/request ID; - relevant
before/after representation or structured diff; - reason; - approval
reference; - Policy/Decision reference where relevant; - source
IP/device metadata only if security/privacy policy permits.

Do not dump secrets or unnecessary sensitive payloads into audit.

## Financial audit

Must make it possible to answer: - who created the Payment? - who
posted/approved it? - which Financial Account moved? - which debts were
allocated? - was it reversed? - why? - what replacement corrected it? -
which receipt/document represents it?

## Locked Period correction

A locked Period must not be reopened casually.

Correction path requires explicit permission/policy and audit.

Exact accounting semantics are specified later.

## Role/policy audit

Audit: - role assignment; - permission/scope changes; - temporary
access; - Policy Version publication; - effective-date changes before
publication; - Board Decision linkage.

Published Policy Versions should be immutable.

## Audit access

Audit UI is read-only.

Audit export is separately permissioned.

## Retention

Audit retention must be defined by legal/operational policy before
production deployment.

Do not implement automatic destructive audit cleanup without approved
retention rules.

## Idempotency

Repeated submission/retry of a reversal/correction command must not
create duplicate compensating effects.

## User-facing language

Avoid misleading "Silindi" for posted finance.

Use explicit Turkish terms such as: - İptal Edildi - Ters Kayıt
Oluşturuldu - Düzeltildi - Arşivlendi
