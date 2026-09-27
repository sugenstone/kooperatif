# 06 --- Payments, Allocations and Collection Sessions

## Purpose

Define the operational money-collection engine for individual and Family
payments, including high-volume collection days.

## Core model

Keep these separate:

1.  **Payment** --- actual value received.
2.  **Financial Account Movement** --- where that value entered.
3.  **Allocation** --- which obligations were satisfied.
4.  **Excess Balance/Position** --- received value not yet allocated.
5.  **Collection Session** --- operational grouping of Payments.

## Payment

Candidate attributes: - technical ID; - cooperative ID; -
receipt/business number; - payer Party/name/context; - payment
date/time; - Payment Method; - destination Financial Account; - gross
amount/value; - currency/value type; - Collection Session if
applicable; - operator User; - state; - idempotency key/reference; -
notes; - source document/attachment; - reversal relationship.

A Payment can exist outside a Collection Session if normal rules permit.

## Payer

Payer may be: - the Shareholder; - another Family member; - Guardian; -
another person.

Payer identity does not transfer ownership of debt, Share or excess
automatically.

## Payment Method

Examples: - Cash; - Bank Transfer; - Card/other future method.

Payment Method describes **how** value arrived.

Destination Financial Account describes **where** it is held.

## Allocation

Allocation links available value to a specific obligation.

Candidate targets: - Period Assessment; - acquisition-fee receivable; -
other explicitly modeled receivable.

Allocation preserves: - Payment/excess source; - obligation target; -
amount; - timestamp; - Policy/selection context; - reversal
relationship.

## Existing excess

Existing excess is cooperative-held value economically attributed to an
owner/context.

When used later: - no new cash movement occurs; - a new Allocation
occurs; - source attribution remains traceable.

## New excess

If Payment exceeds valid selected obligations, remaining value requires
explicit disposition.

For individual payment: - may become that Shareholder's excess according
to policy.

For Family payment: - never infer owner from payer/Guardian/Family
membership; - require explicit attribution or configured Policy.

## Allocation preview

Before posting show: - debtor(s); - payer; - selected obligations; -
current outstanding amounts; - existing excess; - amount received; -
method; - destination account; - proposed allocations; - proposed new
excess; - resulting debt.

Backend recalculates/validates inside transaction.

## Allocation ordering

Not yet globally fixed.

Possible modes: - explicit operator selection; - oldest-first; - current
Period first; - policy-defined order.

No agent may choose a default without approved Policy.

## Family Payment

One person may pay for all or selected members of a Family.

Example:

``` text
Family #047
Payer: Ahmet Kaya

Mehmet  15.000
Hasan   10.000
Ayşe     5.000
Total selected: 30.000
```

If 30.000 is received: - one Payment; - one Financial Account inflow; -
multiple Allocations.

Never create separate cash inflows per member.

## Collection Session

A Collection Session is an operational workspace around a
Period/date/location/account context.

Candidate attributes: - cooperative; - Period; - title; - planned/actual
date; - state; - allowed/default Financial Accounts; - operators; -
notes; - opening/closing timestamps.

Collection Session is not a Financial Account.

## Collection-day workflow

Typical flow:

``` text
Open Collection Session
-> Search Family/Shareholder
-> Verify identity (Name + Guardian + Family No)
-> See debt immediately
-> Select member(s)/obligations
-> Enter received amount
-> Select method/account
-> Preview allocations
-> Post
-> Print receipt if needed
-> Updated debt/account/session totals appear immediately
-> Continue to next payer
```

## Search identity safety

Collection search should support: - Shareholder name; - Guardian name; -
Family Sequence Number; - Share Number where useful.

Results display enough context to distinguish same-name Shareholders.

## Live session header

Show at minimum: - Period; - session state; - Period Assessment Total; -
Prior Excess Applied; - New Cash Collected; - Outstanding Period Debt; -
payment-method breakdown; - relevant Financial Account balances.

Definitions must be explicit.

## Recent transactions

Show recent Payments with: - payer; - debtor/family context; - amount; -
time; - method; - receipt number; - operator where authorized.

Purpose: catch mistakes quickly.

## Duplicate protection

Use both: - UX warning for materially similar recent Payments; - backend
idempotency/concurrency protection.

Legitimate repeated Payments are possible; similarity warning is not
automatic rejection unless exact idempotency conflict exists.

## Concurrency

If another operator changes selected debt after preview: - backend
rejects/recalculates stale proposal; - UI explains what changed; -
operator confirms fresh result.

Never silently over-allocate.

## Posting atomicity

A successful posted Payment must atomically produce/reconcile: - Payment
state; - Financial Account Movement; - Allocations; - excess
disposition; - receipt/document identity where generated
synchronously; - audit event/outbox/event record as architecture
requires.

Partial authoritative posting is unacceptable.

## Reversal

Posted Payment: - is not deleted; - reversal creates linked compensating
effects; - allocations are reversed appropriately; - excess effects are
reversed appropriately; - Financial Account effect is compensated; -
receipt history shows reversal.

## Receipt

One posted Payment produces one logical Collection Receipt identity when
receipts are enabled.

Family allocation lines appear within that receipt.

Reprint is not a new Payment.

## Offline/manual fallback

No offline financial posting behavior is authorized yet.

Do not invent local browser queues that later sync money transactions.

## Permissions

Separate permissions for: - view; - create draft; - post; - reverse; -
operate Collection Session; - close session; - print/export receipt.

## Open decisions

-   allocation priority;
-   exact duplicate heuristic;
-   cash drawer/session reconciliation semantics;
-   whether Collection Session can restrict destination accounts;
-   whether all Payments require a receipt number;
-   approval thresholds;
-   exact real-time transport.
