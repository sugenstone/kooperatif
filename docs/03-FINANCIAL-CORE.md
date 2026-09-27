# 03 --- Financial Core

## Purpose

Define financial semantics that every module must respect.

## Core rule: separate event, cash and allocation

For a shareholder payment:

1.  **Payment/Receipt** = actual value received.
2.  **Financial Account Movement** = actual effect on destination
    account.
3.  **Allocation** = how received/available value satisfies debts.
4.  **Excess Payment Balance** = received value still attributable but
    not allocated.

These are related but not interchangeable.

## Example: exact payment

Shareholder owes: - Period 27 / Share A: 5,000 - Period 28 / Share A:
10,000

Payment received: 15,000.

Expected semantic result:

``` text
Payment: +15,000 received
Financial account movement: +15,000
Allocations:
  Debt P27/A: 5,000
  Debt P28/A: 10,000
Remaining new excess: 0
```

## Example: overpayment

Debt total: 20,000. Payment: 30,000.

``` text
Payment: +30,000
Financial account movement: +30,000
Debt allocations: 20,000
New excess attribution: 10,000
```

The 10,000 is already cooperative-held cash. It is not "outside the
cashbox."

## Example: old excess used in new period

Prior excess: 10,000. New Period Assessment: 10,000.

``` text
New cash collected now: 0
Prior excess applied: 10,000
Period debt satisfied: 10,000
Outstanding period debt: 0
```

Do not report 10,000 as new cash collection.

## Example: family payment

One payer provides 45,000 for four family members.

``` text
One Payment: 45,000
One account inflow: 45,000
Four or more allocations: total 45,000
```

Do not create one account inflow per family member.

## Reconciliation invariants

For a posted receipt denominated in one unit/currency, the financial
design must prove a reconciliation equivalent to:

``` text
gross_received
=
allocated_to_obligations
+ attributable_unallocated/excess disposition
+ other explicitly modeled disposition
```

No value may disappear.

Account movements must also reconcile to the posted Payment exactly
once.

Exact multi-currency/commodity rules will be specified separately.

## Period metrics

For each Period expose at least:

``` text
period_assessment_total
prior_excess_applied
new_cash_collected
total_period_debt_satisfied
outstanding_period_debt
```

Where applicable:

``` text
total_period_debt_satisfied
=
prior_excess_applied
+ current/new payment value allocated to this period
+ other explicitly allowed settlement sources
```

Do not assume every new cash payment belongs only to the current Period.

## Payment preview

Before confirmation, collection/payment UI should show: - selected
shareholder/family, - payer, - total outstanding debt, - debt breakdown
by Share/Period, - existing excess balances, - entered payment amount, -
payment method, - destination Financial Account, - proposed
allocations, - post-payment outstanding debt, - proposed excess result.

Preview is advisory. Backend recomputes/validates authoritative result
inside the posting transaction.

## Allocation policy

Exact ordering is not yet decided.

Possible dimensions include: - oldest debt first, - selected Period
first, - explicit user selection, - Share ordering, - policy-based
automatic allocation.

Agent must not choose one until specified.

## Excess ownership

Excess must belong to an explicit economic owner/context.

For an individual shareholder payment this may be straightforward.

For a Family Payment that exceeds selected obligations, ownership is
ambiguous. Therefore the system must: - require explicit
allocation/ownership choice; or - apply a formally configured policy.

Never split family excess equally or assign it to payer/guardian by
assumption.

## Payment methods and accounts

Payment Method answers **how value arrived**.

Financial Account answers **where value is held/recorded**.

Examples:

``` text
Method: Cash
Account: Istanbul TRY Cash

Method: Bank Transfer
Account: Bank A - TRY
```

Do not encode account identity into payment-method enums.

## Duplicate protection

High-volume collection creates duplicate risk.

UI should warn on materially similar recent payments using relevant
context such as: - payer, - shareholder/family, - amount, - time
proximity, - payment method, - account.

Backend must also provide idempotency/concurrency safeguards where
appropriate.

A warning alone is not sufficient protection.

## Posting and reversal

Draft payment: no authoritative financial effect.

Posted payment: authoritative financial effect exists.

Posted payments are not deleted.

Reversal: - references original Payment, - creates compensating
authoritative effects, - preserves original, - updates receipt
verification/history to show reversal.

Correction normally means reversal + replacement Payment.

## Financial-account balance

Do not treat a manually editable `balance` field as authoritative truth.

Balance must be derivable/reconcilable from authoritative movements.
Cached/materialized balances may be used for performance only with
strict reconciliation strategy.

## Transfers

Transfer between cooperative-owned accounts: - outflow from source
account, - inflow to destination account, - not automatically
Income/Expense.

## Asset acquisition

Buying a property/investment: - reduces financial account, -
creates/increases another asset, - is not automatically an Expense.

## Income/expense vs cash

Income may exist before receipt if accrual rules recognize it. Cash may
arrive without being Income, e.g. account transfer or shareholder
principal contribution.

Keep classifications explicit.

## Receivables/liabilities

Rights and obligations require lifecycle states. Candidate concepts: -
recognized, - not yet due, - due, - partially settled, - settled, -
cancelled/reversed where legally/domain valid.

## Value protection

Never mutate the base right amount.

Store/calculably preserve: - base amount, - crystallization date, -
policy version, - adjustment, - current payable equivalent, - due
date, - settlements.

## Ledger architecture decision

Before implementing the financial core, produce an ADR evaluating a
ledger/double-entry-inspired internal model.

The UI need not expose accounting jargon.

The selected architecture must support: - reconciliation, - reversals, -
traceability, - asset conversions, - transfers, -
receivables/liabilities, - future valuation/reporting.

Do not implement an ad-hoc mutable-balance system before this ADR is
approved.

## Transactionality

Posting a payment must not leave partial state such as: - account
increased but allocations missing, - allocations posted but payment
failed, - duplicate account movement, - excess created twice.

Use database transactions and constraints/idempotency as appropriate.

## Explainability

Every financial total shown to users should be traceable to source
records.

"How calculated?" should be possible for key totals.

## Unresolved financial decisions

Do not implement assumptions for: - exact allocation priority, - family
overpayment excess ownership, - profit formula, - exit formula, -
valuation formula, - value-protection index, - commodity/gold unit
conversion, - accounting/tax classification.
