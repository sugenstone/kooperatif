# 05 --- Periods and Assessments

## Purpose

Define recurring cooperative Periods, per-Share obligations, collection
dates, assessment generation, debt visibility and Period-level
reporting.

## Core business rule

The cooperative may create recurring **Periods**. For each Period, a
payable amount/rule is defined for every eligible Share.

The Period obligation is created **per Share**, not merely as one
aggregate Shareholder debt.

Example:

``` text
Period: 2030 / 04
Assessment per eligible Share: 5.000 TL

Shareholder A
  Share #001 -> 5.000 TL
  Share #002 -> 5.000 TL
  Share #003 -> 5.000 TL

Shareholder total for this Period: 15.000 TL
```

The aggregate is derived from the underlying Share Assessments.

## Period

Candidate attributes: - technical ID; - cooperative ID; - human
name/number; - sequence/order; - start/end or accounting/reference
interval where relevant; - planned collection date; - due date if
different; - lifecycle state; - governing Policy Version; - notes; -
created/opened/closed timestamps.

## Planned collection date

A Period has a date on which collection is normally expected to occur.

This date: - supports operational planning; - does not imply every
Shareholder will pay that day; - does not prevent earlier/later Payment
if policy permits; - can be used to create/open Collection Sessions.

Do not mark unpaid obligations as paid merely because collection day
ended.

## Assessment

A Period Assessment is an authoritative obligation attributed to: - one
Period; - one Share; - the economically responsible Shareholder/owner
according to effective ownership/policy; - one value type/currency; -
one governing Policy Version.

Candidate fields: - original amount; - outstanding amount derived from
settlements; - due date; - creation source/batch; - status derived from
settlement state; - cancellation/reversal relationship if later
specified.

## Eligibility

Eligibility is Policy-driven.

Potential dimensions: - Share lifecycle state; - ownership effective
date; - founder/special classification; - suspension/restriction; -
newly allocated Share; - returned Share; - explicit Board Decision.

No implementation may guess eligibility rules.

## Assessment generation

Assessment generation must be: - deterministic; - idempotent; -
auditable; - safe under retry/concurrency.

Running generation twice for the same Period/eligible Share must not
create duplicate authoritative Assessments.

Before posting/generating, preview should show: - eligible Share
count; - amount/rule; - expected total assessment; - exclusions; -
governing Policy Version.

## Historical ownership

A later Share transfer must not rewrite which Shareholder was
responsible for a historical Assessment.

If policy permits debt transfer/assumption, that is a separate explicit
domain event.

## Automatic use of existing excess

When a new Period is created/opened and a Shareholder has eligible
excess value, policy may allow automatic allocation to new Assessments.

This must preserve: - original Payment/excess source; - allocation
timestamp; - Assessment target; - Policy Version; - amount applied.

This is **not new cash collection**.

If automatic application is enabled, it must be deterministic and
explainable.

## Debt views

At minimum expose: - current Period debt; - older outstanding debt; -
total outstanding debt; - debt by Share; - debt by Period; - satisfied
amount; - existing excess separately.

## Period metrics

Every Period report/dashboard must distinguish:

``` text
Period Assessment Total
Prior Excess Applied
New Cash Collected
Total Period Debt Satisfied
Outstanding Period Debt
```

Do not use an ambiguous single metric named only `Tahsil Edilen`.

## Period opening

Opening a Period may: - freeze/attach governing Policy Version; -
generate Assessments; - apply eligible prior excess if configured; -
make collection available.

Exact command boundaries are implementation/Policy decisions, but
effects must be atomic or recoverably staged.

## Period closing

Closing/locking a Period: - does not delete unpaid debt; - does not
erase later Payments; - does not convert outstanding debt into zero; -
restricts ordinary configuration mutation; - preserves historical
reporting.

Late payment against a closed Period may still be allowed if policy
permits.

## Reversal/correction

Incorrect Assessment generation must not be fixed by arbitrary database
deletion after financial effects exist.

Use explicit cancellation/reversal/correction semantics depending on
whether allocations/payments already reference the Assessment.

## UI requirements

Period list/detail should show: - Period state; - planned collection
date; - assessment total; - prior excess applied; - new cash; -
satisfied debt; - outstanding debt; - collection sessions; -
report/export actions.

Assessment/payment modal must show the person's current relevant debt
before confirmation.

## Audit

Audit: - Period creation/open/close; - assessment generation; - Policy
Version; - manual exception; - assessment correction/reversal; - excess
auto-application.

## Open decisions

-   exact Period cadence;
-   assessment eligibility rules;
-   amount formula;
-   due-date semantics;
-   allocation priority;
-   whether opening and assessment generation are one command;
-   exact locked-Period correction policy.
