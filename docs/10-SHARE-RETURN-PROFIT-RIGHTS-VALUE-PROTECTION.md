# 10 --- Share Return, Profit Rights and Value Protection

## Purpose

Define the separation between Share closure/return, refundable invested
amount, Profit Right and delayed settlement.

## Core principle

Returning a Share does not mean one immediate payment.

At least two economically distinct rights may arise:

1.  **Refundable Invested Amount Right**
2.  **Profit Right**

They may have different formulas, due dates, waiting periods, approvals
and value-protection rules.

## Share Return

A Shareholder may return a Share to the cooperative under applicable
Policy.

Return: - ends active ownership/economic participation at a defined
cutoff; - does not erase history; - may close/cancel the Share according
to domain rule; - may create one or more cooperative
Liabilities/Receivables payable to former owner.

## Cutoff date

Return workflow must establish an authoritative economic cutoff date.

The cutoff determines which: - Period obligations; - profits/losses; -
valuations; - investments; - ownership rights

are included according to Policy.

Exact cutoff rule is unresolved.

## Refundable Invested Amount Right

This is not automatically equal to every historical cash payment.

Formula is Policy-driven.

When crystallized preserve: - base amount; - calculation date/cutoff; -
inputs; - Policy Version; - due date; - waiting period; - approvals; -
settlements.

Example policy may make it payable after 6 months, but no fixed duration
is hard-coded.

## Profit Right

Separate right calculated under approved Profit Policy.

When crystallized preserve: - base Profit Right; - calculation/cutoff
date; - valuation inputs; - Policy Version; - due date; - settlements; -
Value Protection method if applicable.

Profit Right may be payable years later.

Example 4--5 years is possible but not hard-coded.

## Delayed settlement

If a crystallized right is paid later, nominal amount may lose value.

Therefore each right can reference a **Value Protection Policy**.

Potential modes: - none; - inflation/index; - gold; - fixed annual
adjustment; - other approved formula.

The cooperative chooses the applicable model through Policy.

## Base amount immutability

Never overwrite:

``` text
Base Right: 50.000 TL
```

with an adjusted value.

Preserve:

``` text
Base Right
Crystallization Date
Value Protection Policy Version
Adjustment(s)
Current Payable Equivalent
Settlements
Remaining Payable
```

This makes history explainable.

## Due date vs value-protection start

These may be different concepts.

Policy must define: - crystallization date; - protection start date; -
due date; - settlement date behavior.

Do not assume adjustment starts only when overdue.

## Partial settlement

Rights may be partially paid.

Each settlement: - references Liability/Right; - creates Financial
Account outflow; - reduces remaining payable; - preserves
value-protection semantics.

Exact treatment of future adjustment after partial payment must be
Policy-defined.

## Profit calculation

Still `UNRESOLVED / POLICY_DRIVEN`.

Potential inputs: - Financial Accounts; - realized Income/Expense; -
Investments; - Business Holdings; - Real Estate; -
Receivables/Liabilities; - valuation rules; - Share ownership/economic
participation period.

No implementation may invent a NAV/profit formula.

## Share transfer vs Share return

Private transfer: - Share continues; - owner changes; - cooperative does
not automatically pay old owner.

Return: - cooperative closes/recovers Share according to rule; - former
owner may receive cooperative payables.

## Transfer of economic balance and exit

A Shareholder may transfer an attributable balance/right to another
Shareholder and exit if policy permits.

This is distinct from transferring the Share itself.

Such assignment must explicitly identify: - right being assigned; -
assignor; - assignee; - amount/value; - effective date; -
approval/document; - whether Share closes.

Do not merge these concepts.

## Statements

Former Shareholder detail remains accessible historically with: -
returned Shares; - crystallized rights; - due dates; - value
protection; - settlements; - outstanding cooperative liabilities.

## Open decisions

-   refundable invested amount formula;
-   Profit Right formula;
-   cutoff rule;
-   waiting periods;
-   Value Protection models and data sources;
-   treatment of expenses/unrealized valuation;
-   partial settlement adjustment;
-   right assignment rules;
-   tax/legal/accounting treatment.
