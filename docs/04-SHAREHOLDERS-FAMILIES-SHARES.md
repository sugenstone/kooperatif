# 04 --- Shareholders, Families and Shares

## Purpose

Define identity, family grouping, guardian context, Share ownership and
operational behavior.

## Shareholder

A Shareholder is a cooperative record, not a login account.

Candidate attributes: - immutable technical ID, - cooperative ID, -
Party/person reference, - membership status, - family membership, -
guardian association, - notes/metadata, - active/archive state, -
relevant dates.

Do not use name as identity.

## Display identity

Because names may repeat, operational UI should normally display:

``` text
Mehmet Kaya
Vasi: Ahmet Kaya · Aile #047
```

Exact formatting is UI-spec territory, but guardian/family context must
be readily visible on collection and Share workflows.

## Search

Search should support at minimum: - shareholder name, - guardian name, -
family sequence number, - Share number where useful.

## Guardian / Vasi

Guardian is a Party/person relationship.

Rules: - need not be a Shareholder; - may itself be a Shareholder; - do
not duplicate the same real person solely because they act as
Guardian; - guardian relationship history/effective dates should be
supported if operationally required.

Do not automatically grant Guardian financial ownership or legal
authority. Those rules require explicit policy/legal specification.

## Family

Family is a first-class cooperative-scoped entity.

Required concept: - Family Sequence Number.

Family can contain multiple Shareholders.

Family page should aggregate operational views while preserving
individual ownership.

Candidate family view: - members, - guardian/representative context, -
active Share count, - current Period debt, - older outstanding debt, -
excess balances by actual owner, - selected/all-member payment action, -
payment history, - family statement.

Never store one authoritative "family debt" if it merely duplicates
member debts. Derive it.

## Share

Every Share is independently tracked.

Candidate human identifier: `Share #0047`

A Share has: - technical ID, - cooperative ID, - Share Number, -
lifecycle state, - creation/allocation history, - ownership history, -
assessment history, - invested-amount history, - restrictions, -
return/closing history.

## Ownership

At most one active owner per Share at a point in time.

Ownership history must retain: - owner, - start, - end, -
acquisition/transfer source, - related transaction/decision where
applicable.

Changing current owner must not rewrite historical Period Assessments.

## Share lifecycle

Candidate lifecycle states:

``` text
Draft
Active
Suspended
Return Pending
Closed
```

Restriction/blocking may be modeled orthogonally rather than as a
lifecycle state.

Events may include: - create, - allocate, - transfer, - sale-transfer, -
restrict, - lift restriction, - suspend, - reactivate, - return
request, - return approval, - close, - succession/inheritance transfer.

Exact allowed transitions require a later state-machine specification.

## Share allocation

New allocation captures: - Share, - receiving Shareholder, - allocation
date, - allocation classification, - applicable policy version, -
optional acquisition fee, - fee payment/receivable status where
relevant, - board/approval reference if required.

Founder Shareholder and Founder-Period Allocation are separate facts.

## Acquisition fee

Fee is optional unless policy requires it.

Zero fee must be valid.

Fee is cooperative economic income only according to the later
financial/accounting specification; do not hard-code tax/accounting
treatment now.

Fee never silently increases Share Invested Amount.

## Share transfer

Transfer changes owner, not Share identity.

A private sale price between old/new owner is not automatically a
cooperative Financial Account movement.

Cooperative transfer fee is currently **not assumed**. If future policy
adds one, it must be explicit/versioned.

Before transfer, system may need to check: - outstanding debt, -
restrictions, - pending workflows, - minimum holding rules, - required
approvals/documents.

These checks are policy-driven.

## Share return

Return is not deletion.

Return may: - end ownership/economic participation at a defined
cutoff, - calculate refundable invested-amount right, - calculate
separate Profit Right, - create receivables with different due dates, -
close Share, - leave former-owner receivables open.

## Shareholder detail page

Should provide: - identity + guardian + family context, -
active/historical Shares, - current and historical debts, - payments, -
allocations, - excess balances, - receivables, - timeline, -
documents, - "as of date" historical view where supported.

## Share detail page

Should provide: - current status, - current owner, - ownership
history, - allocation information, - Period Assessments, -
payments/allocations affecting it, - invested amount, - Profit Rights, -
restrictions, - return/closing events, - timeline/documents.

## Family payment workflow

User opens Family #047 and sees each member's obligations separately.

User may select: - one member, - several members, - all eligible
members.

The system computes selected obligations and previews the Payment.

Example:

``` text
Family #047
Payer: Ahmet Kaya

[x] Mehmet  15,000
[x] Hasan   10,000
[ ] Ayşe     5,000
[ ] Hüseyin 15,000

Selected obligations: 25,000
```

If payer gives 25,000, one Payment is posted with allocations totaling
25,000.

If payer gives 30,000, the extra 5,000 cannot be silently assigned.
Resolve using explicit policy/user choice.

## Collection-session shareholder card

A collection row/card should make mistaken identity difficult.

Minimum useful context: - shareholder name, - guardian name, - family
sequence number, - active Share count, - current Period obligation, -
older debt, - existing excess, - total outstanding amount, -
latest/recent payment warning where relevant.

## Payment concurrency

After successful posting: - shareholder/family debt display updates, -
collection totals update, - relevant account balance updates, - other
active clients receive event/update.

Backend must revalidate debt/allocation state at posting time to prevent
stale-screen corruption.

## Duplicate warning

When another materially similar recent payment exists, show a clear
warning with: - amount, - time, - operator where allowed, - method, -
payment/receipt reference.

Do not silently block every duplicate-looking payment; legitimate
repeated payments are possible. Exact duplicate policy comes later.

## Statements

Support: - Shareholder Statement, - Family Statement, - Share Statement.

Family Statement aggregates presentation but must preserve member-level
lines and attribution.

## Historical "as of" view

Target capability:

``` text
What was Shareholder X's state on 2030-12-31?
```

Potentially show: - owned Shares, - open debts, - excess balances, -
receivables, - account association.

Design data history so this is possible without mutating old records.

## Open decisions

-   legal/operational meaning and authority of Guardian;
-   family membership effective dating requirements;
-   whether one Shareholder can belong to more than one Family;
-   family overpayment excess ownership;
-   transfer debt prerequisites;
-   succession/inheritance rules;
-   exact Share state machine.
