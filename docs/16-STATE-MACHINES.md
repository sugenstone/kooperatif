# 16 --- State Machines

## Purpose

Centralize lifecycle semantics. Agents must not invent ad-hoc state
transitions in screens or endpoints.

Exact transition matrices will be finalized with each domain. This
document defines the canonical direction.

## General rules

-   State transitions are commands/events, not arbitrary field edits.
-   Invalid transitions are rejected by backend.
-   Transition actor/time/reason are audited where relevant.
-   Posted/effective states are not moved backward by deleting history.
-   Reversal/correction is modeled explicitly.

## Period

Candidate lifecycle:

``` text
Draft -> Open -> Closed/Locked
```

Possible future operational substate such as collection-active must not
be added without specification.

Draft: - configuration may change according to permissions; - no
assumption that obligations are authoritative yet.

Open: - assessments/collections operate under defined policy.

Closed/Locked: - ordinary mutation restricted; - corrections use
controlled processes.

## Payment

Candidate lifecycle:

``` text
Draft -> Pending Approval -> Approved -> Posted
Draft/Pending -> Cancelled
Posted -> Reversed
Reversed -> [terminal historical state]
```

Not every Payment must require approval; Approval Policy decides.

Posted Payment cannot return to Draft.

Correction of a Posted Payment is normally:
`Original Posted -> Reversed` plus a new replacement Payment.

## Period Assessment

Candidate lifecycle:

``` text
Open -> Partially Satisfied -> Satisfied
```

Additional states such as cancelled/reversed require explicit financial
semantics.

Assessment status should preferably derive from authoritative
amount/settlement state rather than be independently editable.

## Share

Candidate lifecycle:

``` text
Draft -> Active
Active -> Suspended -> Active
Active -> Return Pending -> Closed
```

Transfer normally changes ownership while Share remains Active.

Restrictions may be orthogonal flags/relationships rather than lifecycle
states.

Succession/inheritance transitions require later specification.

## Share Ownership

Candidate lifecycle: `Scheduled/Pending -> Active -> Ended`

At most one active ownership interval per Share at an instant.

## Share Return

Candidate workflow:

``` text
Draft Request -> Pending Approval -> Approved -> Rights Calculated/Crystallized -> Share Closed
```

Payment of resulting receivables may occur much later and is not part of
Share's active lifecycle.

## Receivable

Candidate lifecycle:

``` text
Recognized -> Not Yet Due -> Due -> Partially Settled -> Settled
```

Cancellation/reversal only through explicit domain rules.

## Financial Account

Candidate lifecycle:

``` text
Draft -> Active -> Inactive/Closed
```

Closing an account does not delete movements.

## Investment / Asset

Candidate lifecycle varies by asset type but must distinguish active
ownership from disposed/closed historical state.

## Policy Version

Candidate lifecycle:

``` text
Draft -> Approved -> Scheduled/Effective -> Superseded/Expired
```

Published historical versions are immutable.

## Board Decision

Candidate lifecycle:

``` text
Draft -> Voting/Review -> Approved | Rejected
Approved -> Effective (when effective date reached/conditions met)
```

Exact governance semantics require dedicated governance specification.

## Approval Request

Candidate lifecycle:

``` text
Pending -> Approved | Rejected | Cancelled | Expired
```

Approval does not automatically mean execution unless the source
workflow defines it.

## Collection Session

Candidate lifecycle:

``` text
Planned -> Open -> Closed
```

Payments outside an Open Collection Session may still be valid if normal
Payment rules permit them.

Closing a Collection Session does not close the Period.

## Generated official document

Candidate status is derived from source: - Valid; - Source Reversed; -
Source Cancelled; - Superseded where document semantics allow.

Do not delete historical document identity to represent reversal.

## Social Aid request

Candidate lifecycle:

``` text
Draft -> Submitted -> Under Review -> Approved | Rejected
Approved -> Payment Pending -> Partially Paid/Paid -> Closed
```

Exact states depend on Social Aid specification.

## Rule for implementation

Before implementing a stateful domain, convert its candidate lifecycle
into an explicit transition table with: - from state; - command; -
required permission; - guards; - side effects; - to state; - audit
event; - idempotency behavior.

If this information is missing for a critical financial transition, stop
and request specification.
