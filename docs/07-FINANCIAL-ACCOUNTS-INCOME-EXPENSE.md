# 07 --- Financial Accounts, Income, Expense and Transfers

## Purpose

Define cooperative-held value locations and separate cash movement from
economic classification.

## Financial Account

The cooperative may have multiple Financial Accounts and multiple value
types.

Examples: - İstanbul TL Kasası; - Köy TL Kasası; - İstanbul Altın
Kasası; - Köy Altın Kasası; - Banka A TL Hesabı.

A Financial Account represents where cooperative-controlled value is
held/recorded.

## Account dimensions

Candidate attributes: - cooperative; - account name; - account type; -
location/context; - value type/currency/commodity; - lifecycle state; -
opening date; - optional bank/cash metadata; - access scope; -
reconciliation configuration.

Do not encode all dimensions into one enum.

## Shareholder default account association

A Shareholder may have a default/preferred cooperative Financial Account
for normal collection.

Example: - initially Köy TL Kasası; - later İstanbul TL Kasası.

Changing this association: - affects future default routing; - does not
rewrite historical Payments; - does not move money automatically; - must
be effective-dated/auditable if historical reconstruction requires it.

The operator may select another authorized destination account where
policy permits.

## Balance

Authoritative balance is derived/reconciled from Financial Account
Movements.

Do not expose a normal `balance = ...` edit.

## Financial Account Movement

Every authoritative movement records: - account; - direction/effect; -
amount/value; - value type; - timestamp/business date; - source
transaction; - classification/context; - reversal relationship.

Movement sources may include: - Payment; - transfer; - expense
payment; - income receipt; - asset acquisition/disposal; - receivable
settlement; - liability settlement; - Social Aid only in its separate
financial context.

## Transfer

Moving value between cooperative-controlled accounts:

``` text
Source Account: -X
Destination Account: +X
```

Transfer is not automatically Income or Expense.

Transfer must be linked as one logical operation.

Cross-value/currency transfer/exchange is unresolved and requires
explicit conversion rules.

## Income

Income is an economic classification, not synonymous with cash receipt.

Potential examples: - rental income; - investment income; - business
distribution; - Share acquisition fee if accounting/policy classifies it
as income; - other operating income.

Income recognition basis requires accounting/domain policy.

## Expense

Expense is an economic classification, not synonymous with cash outflow.

Potential examples: - operating expense; - rent paid by cooperative; -
service expense; - maintenance; - management/administrative expense.

Buying an asset is not automatically Expense.

## Receivable

A recognized right to receive value.

Examples: - rent receivable; - investment distribution receivable; -
Shareholder acquisition-fee receivable; - returned-Shareholder payable
is not a receivable; it is cooperative liability.

Receivable tracks recognition, due date and settlement separately.

## Liability/Payable

A cooperative obligation to pay value.

Examples: - Share return invested-amount payable; - Profit Right
payable; - rent payable; - supplier liability.

A liability may exist long before cash leaves an account.

## Cash basis vs accrual

The application should be capable of distinguishing: - recognized
Income/Expense; - Receivable/Liability; - actual cash receipt/payment.

Exact accounting/reporting basis is Policy/configuration/legal-design
territory.

## Expense workflow

Candidate flow:

``` text
Draft Expense
-> Approval if required
-> Recognized/Approved
-> Payment from Financial Account
-> Settled/Partially Settled
```

Do not force every Expense to equal one cash payment.

## Income workflow

Candidate flow:

``` text
Expected/Contractual
-> Recognized Receivable/Income
-> Due
-> Cash Receipt
-> Settled
```

Exact recognition varies by source domain.

## Account reconciliation

Support future reconciliation: - expected system balance; - counted/bank
statement balance; - difference; - reconciliation record; -
explanation/correction workflow.

Manual reconciliation adjustment must be explicit and audited, not
direct balance overwrite.

## Account access

Roles/scopes may restrict: - viewing; - receiving into; - paying from; -
transferring; - reconciling; - exporting.

## Reporting

At minimum: - account statement; - opening/closing balance for
interval; - inflows; - outflows; - transfers; - classified
Income/Expense; - reconciliation differences.

## Open decisions

-   exact account types;
-   gold/commodity unit model;
-   exchange/conversion;
-   accounting recognition basis;
-   reconciliation frequency;
-   opening-balance migration rules;
-   whether shareholder default account is mandatory.
