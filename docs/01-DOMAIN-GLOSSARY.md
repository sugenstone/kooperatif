# 01 --- Domain Glossary

Canonical vocabulary. Use consistently in database/API/code/tests/UI
unless an approved terminology decision changes it.

## Core people and grouping

### Party / Taraf

A generic person or organization that may participate in transactions or
relationships.

### Shareholder / Hissedar

A managed cooperative participant capable of owning Shares. Not an
application User.

### Family / Aile

A first-class operational grouping of related Shareholders. Family
membership does not merge individual debts, Shares, rights or balances.

### Family Sequence Number / Aile Sıra No

A cooperative-scoped business identifier used to locate/distinguish a
Family. It is not the database primary key.

### Guardian / Vasi

A person associated with a Shareholder/Family for identification and
operational context. Need not be a Shareholder.

### Payer / Ödeyen

The person/entity providing money/value. May differ from the
Shareholder/Debtor whose obligations are settled.

### User

An authenticated application operator. User != Shareholder.

## Share concepts

### Share / Hisse

A uniquely identified cooperative participation unit with its own
lifecycle, ownership history, assessments, allocations, invested amount
and exit history.

### Share Ownership

A dated relationship between a Share and an owner.

### Share Allocation

Initial/new assignment of a Share under allocation policy.

### Share Transfer

Transfer of an existing Share to another owner, normally preserving
Share identity.

### Share Sale

A Share Transfer involving consideration between parties.

### Share Return

Return of a Share to the cooperative, potentially triggering exit
rights.

### Share Acquisition Fee

Optional cooperative fee for a new Share allocation. Separate from Share
Invested Amount.

### Share Invested Amount

Qualifying contributed principal attributed to a Share under governing
rules.

### Profit Right

A separately calculated right under an applicable policy. Distinct from
Share Invested Amount.

### Reference Share Value

A calculated reference economic value. Not automatically the exit
payment.

## Period/debt concepts

### Period / Dönem

An operating/financial cycle with lifecycle and planned collection date.

### Period Assessment / Dönem Borcu

A debt created for one eligible Share for one Period.

### Period Assessment Total

Sum of all assessments created for the Period.

### Outstanding Debt

Unpaid remainder after valid allocations/settlements.

### Due Date

Date an obligation becomes payable.

## Payment concepts

### Payment / Receipt / Tahsilat

One authoritative receipt of value into the relevant financial context.

### Payment Method

How value was delivered, e.g. Cash, Bank Transfer/EFT. Not the
destination Financial Account.

### Allocation / Mahsup

Application of available payment/excess value to a specific debt.
Allocation is not new cash.

### Excess Payment Balance

Already-received shareholder money not currently allocated to eligible
debt.

### Prior Excess Applied

Previously received excess used to satisfy a current/new debt. Not new
cash collection.

### New Cash Collected

Actual new shareholder-payment value entering Financial Accounts in the
report period.

### Family Payment

One Payment allocated across obligations of selected members of one
Family.

### Multi-Debtor Payment

One Payment allocated across obligations belonging to more than one
debtor.

## Collection operations

### Collection Session / Tahsilat Oturumu

High-volume operational session normally associated with a Period and
planned collection date. Distinct from Period.

### Collection Receipt / Tahsilat Makbuzu

Printable/verifiable document representing one posted Payment/Receipt,
potentially with many allocations.

## Finance

### Financial Account

A controlled store/account of financial value, e.g. Istanbul TRY,
Village TRY, Istanbul Gold.

### Financial Account Movement

An authoritative value movement affecting a Financial Account.

### Income

Economic income. Not synonymous with every account inflow.

### Expense

Economic cost/consumption. Not synonymous with every account outflow.

### Receivable

Amount owed to a party.

### Liability

Obligation reducing the cooperative's economic position.

### Fund

Economic designation/earmark. Not necessarily the same as a Financial
Account.

### Restricted Fund

A Fund whose resources may only be used for specified purposes.

## Assets/investments

### Investment

Cooperative resource allocation intended to create economic
value/income.

### Expected Investment Income

Forecast/expectation only; not realized income.

### Business Holding

Cooperative ownership interest in an external business.

### Real Estate Asset

Owned/part-owned/leased-out/leased property tracked by the system.

### Valuation

Dated value assessment using a recorded source/method.

### Unrealized Valuation Change

Estimated value change not realized by qualifying event.

## Rights/policies/governance

### Crystallized Right

A right whose base amount and governing policy are fixed at a defined
event/date.

### Value Protection

Policy-driven adjustment of delayed receivable payable equivalent
without overwriting base right.

### Policy

Configurable business rule.

### Policy Version

Versioned immutable rule definition for an effective interval.

### Board Decision

Governance decision that may authorize actions or policy versions.

### Decision Date

Date decision was made.

### Effective Date

Date behavior becomes operative.

## Integrity

### Posted Transaction

Transaction that has taken authoritative effect.

### Reversal

New event neutralizing a posted transaction while preserving original
history.

### Correction

Controlled fix, normally reversal + replacement for posted finance.

### Audit Record

Protected evidence of who did what, when, and to what.

## Reporting/documents

### Statement / Ekstre

Chronological/period presentation of authoritative events and resulting
balances for an entity.

### Document Template

Versioned layout/configuration for consistent document rendering.

### Document Number

Stable business-facing identifier for an official document where
required.

### Report Snapshot

A deliberately persisted historical report state only when the reporting
specification requires snapshot semantics. Do not create snapshots
casually.

## Social Aid

### Social Aid

Separate non-profit operational/financial domain.

### Supporter

Shareholder, non-shareholder person or organization providing value to
Social Aid.

### Beneficiary

Person/entity receiving Social Aid; need not be a Shareholder.

### Social Aid Support/Income

Value received by Social Aid.

### Social Aid Expense/Aid Payment

Value used for aid or Social Aid operating costs.

### Social Aid Surplus

Period excess of relevant inflows over outflows. Do not call it
cooperative profit.

## Canonical distinctions

``` text
Shareholder != User
Family != Shareholder
Guardian != Shareholder
Payer != Debtor
Payer != Guardian
Share != Shareholder
Share Count != Share Records
Share Allocation != Share Transfer
Share Acquisition Fee != Share Invested Amount
Payment != Allocation
Payment != Period Assessment
Payment Method != Financial Account
One Payment != One Allocation
Cash Inflow != Income
Cash Outflow != Expense
Asset Purchase != Expense
Expected Income != Realized Income
Accrued Receivable != Cash Received
Crystallized Right != Paid Right
Base Right != Value-Protected Payable Amount
Reference Share Value != Guaranteed Exit Payment
Current Owner != Historical Owner
Decision Date != Effective Date
Financial Account != Fund
Collection Session != Period
Cooperative Finance != Social Aid Finance
Social Aid Surplus != Cooperative Profit
Reversal != Deletion
PDF/Export != Separate Financial Truth
```

## Naming rule

Avoid ambiguous unqualified fields like `balance`, `amount`, `status`,
`type`.

Prefer names such as:

``` text
period_assessment_amount
outstanding_period_debt
new_cash_collected
prior_excess_applied
share_invested_amount
share_acquisition_fee
profit_right_base_amount
value_protection_adjustment
reference_share_value
financial_account_balance
social_aid_fund_balance
family_sequence_number
payer_party_id
payment_method_id
```

Ambiguous financial naming is a design defect.
