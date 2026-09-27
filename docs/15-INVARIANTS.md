# 15 --- System Invariants

## Purpose

These are rules the system must never violate, regardless of UI, policy
configuration, operator role, or implementation convenience.

Policies may change configurable behavior. Policies may not override
structural financial integrity.

## Identity and tenancy

-   Every tenant-owned record must belong to exactly one
    cooperative/organization context.
-   Backend authorization must enforce tenant isolation.
-   A Shareholder is not an authenticated User by default.
-   Names are not unique identifiers.
-   Family Sequence Number is a business identifier, not the technical
    primary key.
-   Guardian, Payer, Shareholder and User are distinct roles/concepts.

## Shares

-   A Share is an individual entity, not only a count.
-   A Share cannot have two active owners for the same effective
    instant.
-   Share transfer must preserve historical ownership.
-   Historical Period Assessments are not reassigned merely because
    current Share ownership changes.
-   Closing/returning a Share must not erase historical events or
    outstanding former-owner rights.
-   Share Acquisition Fee and Share Invested Amount are distinct.

## Periods and assessments

-   A per-Share Period obligation creates an independently traceable
    assessment for every eligible Share.
-   Aggregate shareholder/family debt is derived from underlying
    obligations; it is not a substitute for them.
-   Closed/locked Period behavior must follow explicit state/policy
    rules.

## Payments

-   One actual receipt of money/value is represented as one
    authoritative Payment/Receipt event.
-   One Payment may have many allocations.
-   One family payment must not create duplicate cash inflows merely
    because it settles multiple debtors.
-   Payer does not imply debtor ownership.
-   Payment Method does not imply destination Financial Account.
-   Posted Payment financial effects must be atomic and reconcilable.
-   A posted Payment must affect its destination Financial Account
    exactly once.
-   No Payment value may disappear during allocation.
-   Allocations cannot exceed available allocatable value.
-   Allocation cannot satisfy more than the eligible outstanding amount
    of an obligation unless the domain explicitly models another
    destination.
-   Excess must have explicit economic attribution; Family membership
    alone does not define ownership.
-   Family overpayment must never be silently assigned to an arbitrary
    member.

## Cash and economic classification

-   Cash inflow is not automatically Income.
-   Cash outflow is not automatically Expense.
-   Transfer between cooperative-controlled Financial Accounts is not
    automatically Income/Expense.
-   Asset acquisition is not automatically Expense.
-   Expected Income is not Realized Income.
-   Accrued Receivable is not Cash Received.
-   Unrealized valuation gain is not cash profit.

## Period reporting

Reports must preserve the distinction between: - Period Assessment
Total; - Prior Excess Applied; - New Cash Collected; - Total Period Debt
Satisfied; - Outstanding Period Debt.

Prior Excess Applied must never be reported as new cash collection for
the current Period.

## Balances

-   Authoritative Financial Account balances must be
    derivable/reconcilable from authoritative movements.
-   Users must not directly edit an authoritative balance as a normal
    operation.
-   Cached/materialized balances, if used, must have a reconciliation
    strategy.

## Posted history

-   Posted financial transactions are not destructively deleted.
-   Reversal preserves the original transaction.
-   Correction of posted finance must preserve the original and
    corrective relationship.
-   Audit history cannot be edited like a normal note.

## Policies

-   Historical calculations retain the Policy Version that governed
    them.
-   A new Policy Version does not silently rewrite crystallized
    historical rights.
-   Decision Date and Effective Date are distinct.
-   Undefined financial rules must not be invented by the
    implementation.

## Rights and exit

-   Share Invested Amount and Profit Right are distinct.
-   Base crystallized right is not overwritten by later Value
    Protection.
-   Reference Share Value is not automatically guaranteed exit payment.
-   Share closure does not imply all former-owner receivables are
    settled.

## Social Aid

-   Cooperative Finance and Social Aid Finance must not be silently
    merged.
-   Social Aid surplus must not be reported as cooperative profit.
-   Restricted Social Aid funds must not be spent outside permitted
    purpose without an explicitly authorized rule.
-   Social Aid supporter/beneficiary need not be Shareholder.

## Documents and reporting

-   PDF/XLSX/print output must not create a separate financial truth.
-   Reprinting a receipt must not create another Payment.
-   Reprinting the same logical official document must not silently
    generate a new document identity.
-   Reversed source transactions must remain discoverable from document
    history/verification.
-   Export permissions/scopes cannot exceed the user's
    backend-authorized data scope.

## Localization

-   Locale formatting changes presentation, not stored authoritative
    values or calculations.

## Concurrency

-   Stale UI state must not permit double allocation, duplicate
    financial movement, or violation of outstanding-debt constraints.
-   Backend/database constraints and transactions are authoritative over
    client previews.

## Testing requirement

Every invariant that can be mechanically tested should have automated
tests at the appropriate layer.
