# 00 --- Project Charter

## Purpose

Build a production-grade cooperative management system for cooperative
economic operations, shareholders, individually tracked shares, period
obligations, collections, financial accounts, investments, assets,
governance, policies, reporting and a financially isolated Social Aid
subsystem.

This is not a CRUD bookkeeping application. The system must preserve
history, reconcile financial effects, explain calculated values and
remain auditable.

## Repository

Canonical repository:

``` text
git@github.com:sugenstone/kooperatif.git
```

## Product principles

1.  Simple UI, strict backend.
2.  Posted financial history is not destructively rewritten.
3.  Every important number must be explainable.
4.  Board-changeable business rules belong in versioned policies, not
    scattered hard-coded conditions.
5.  New policies do not silently rewrite crystallized historical rights.
6.  Cash movement and economic meaning are separate.
7.  Expected, accrued, due, paid and settled are distinct states.
8.  Shareholders are managed records, not application users.
9.  Each Share is an individually tracked entity.
10. Cooperative Finance and Social Aid Finance are separate financial
    contexts.
11. Real-time UI never replaces transactional backend correctness.
12. Reports and PDFs use the same authoritative data as the application.

## Technology direction

-   Frontend: SvelteKit + TypeScript
-   UI: shadcn-svelte
-   Backend: Rust + Axum
-   Database: PostgreSQL
-   Architecture: modular monolith
-   Repository: monorepo
-   Schema changes: migrations
-   Docker-supported development
-   automated backend/frontend/E2E testing
-   CI quality gates

Suggested shape:

``` text
apps/
  web/
  server/
packages/
  ui/
  contracts/
migrations/
docker/
scripts/
docs/
implementation/
```

## Default language and localization

The default application language and locale is **Turkish (`tr-TR`)**.

Requirements: - user-facing application text defaults to Turkish; -
menus, forms, validation/error messages, notifications and operational
warnings default to Turkish; - reports, receipts, PDFs, statements,
printouts and official document templates default to Turkish; - dates,
numbers and currency/value displays use locale-aware formatting, with
`tr-TR` as the default; - technical code identifiers and API/database
concepts should remain English for maintainability; - localization
infrastructure must exist from the beginning so additional languages can
be added later; - user-facing strings should be centralized/localizable
rather than scattered as hard-coded component strings; - localization is
presentation behavior and must not change stored authoritative values or
financial calculations.

## Multi-organization readiness

Design for multiple cooperatives even if the first deployment uses one.
Tenant isolation is backend-enforced. Cooperative-owned records must
carry explicit tenant/cooperative context.

## Major domains

-   Identity/Auth/Sessions
-   Cooperatives
-   Users/RBAC/Scopes
-   Approvals
-   Audit
-   Parties/People
-   Families/Guardians
-   Shareholders
-   Shares/Ownership
-   Periods/Assessments
-   Collection Sessions
-   Payments/Allocations/Excess Balances
-   Financial Accounts
-   Income/Expenses
-   Receivables/Liabilities
-   Investments
-   Business Holdings
-   Real Estate
-   Valuation/Profit Rights/Exit
-   Governance/Policies
-   Documents/Reporting
-   Social Aid
-   Notifications/Tasks
-   Import/Export

## Shareholder identity

A shareholder may have a Family, Family Sequence Number and
Guardian/Vasi context.

Names are not unique. Operational screens must display enough context to
safely distinguish people, especially shareholder name + guardian name +
family sequence number.

Guardian is a person reference and does not need to be a shareholder.

## Family

Family is a first-class operational grouping. Family membership never
merges the legal/economic ownership of shares, debts, rights or excess
balances.

A family may contain multiple shareholders.

A payer may settle obligations for one, some or all members of a family.

## Shares

A shareholder may own zero or more Shares. Every Share has stable
identity, lifecycle, ownership history, period assessments, allocations,
invested-amount history and exit history.

Potential events include allocation, transfer/sale, return,
suspension/reactivation, restriction, succession/inheritance workflow
and closing.

Transfer normally preserves Share identity.

## Periods

A Period is an operating/financial cycle. When a per-share obligation is
created, every eligible Share receives its own Period Assessment.

Three shares at 5,000 TRY means three separate 5,000 TRY assessments,
not only a 15,000 TRY shareholder total.

Periods have a planned collection date, but payments may arrive before,
on or after that date.

## Collection Sessions

High-volume collection is handled through a dedicated **Collection
Session**.

The screen must support fast lookup by: - shareholder name, - guardian
name, - family sequence number.

It should show live: - assessment total, - prior excess applied, - new
cash collected, - cash vs bank/other methods, - remaining debt, -
fully/partially/unpaid counts, - relevant financial-account balances.

Successful posted payments should propagate to other active collection
clients without manual refresh. SSE vs WebSocket is an ADR decision.

## Payer and multi-debtor payments

Payer, shareholder, debtor and guardian are distinct concepts.

One payer may pay for an entire family.

Example:

``` text
Receipt: 45,000 TRY
Payer: Ahmet Kaya
Family #047

Allocations:
Mehmet / Share #12 -> 15,000
Hasan / Share #31 -> 10,000
Ayşe / Share #48 -> 5,000
Hüseyin / Share #61 -> 15,000

Actual financial-account inflow: +45,000
```

This is one receipt and one actual inflow, not four inflows.

If payment exceeds selected obligations, excess must not be silently
assigned to an arbitrary family member. Policy or explicit user choice
must determine ownership/destination.

## Payment methods

Payments may use configured methods such as cash or bank transfer/EFT.

Payment Method and destination Financial Account are distinct.

## Excess payments

A shareholder may pay more than due. Actual money enters a cooperative
financial account immediately. Unallocated value becomes an Excess
Payment Balance attributable according to explicit rules.

Previously received excess may later satisfy new debt according to
allocation policy. Applying old excess is not new cash collection.

## Period reporting

Always distinguish: - Period Assessment Total - Prior Excess Balance
Applied - New Cash Collected - Total Period Debt Satisfied - Outstanding
Period Debt

These must reconcile.

## Optional Share acquisition fee

A newly allocated Share may have an optional acquisition/allocation fee.

Founder-period allocations may default to zero; later allocations may
have a default fee. Fee is separate from Share Invested Amount and does
not automatically become refundable principal.

Founder shareholder status and founder-period Share allocation are
distinct.

## Financial accounts

The cooperative may have multiple accounts/value types, e.g. Istanbul
TRY, Village TRY, Istanbul Gold.

Historical transactions retain their original account context.

## Investments/assets

Support investments, business holdings and real estate. Expected income
is not realized income. Asset purchase is not automatically expense.
Unrealized valuation gain is not cash profit.

## Share return and exit

Share return may create at least two independent rights: 1. refundable
invested amount/principal right; 2. profit right.

They may have different calculation, eligibility, due date, payment date
and value-protection policies.

A Share may close while former-owner receivables remain unpaid for
years.

## Value protection

Delayed receivables may use versioned policies such as none,
inflation/index, gold, fixed rate or approved formula.

Never overwrite the original base right. Preserve base amount,
crystallization date, policy/version, current payable equivalent, due
date and settlement.

## Governance/policies

Board-changeable rules are versioned Policies tied where relevant to
Board Decisions, effective dates and scope.

Historical calculations retain the Policy Version used.

## Record mutation

-   Draft/effectless records may be deletable.
-   Referenced non-financial records normally archive/deactivate.
-   Posted financial events are not destructively deleted.
-   Corrections use cancellation-before-posting or
    reversal-after-posting, plus replacement when needed.

## Authorization

Use **RBAC + Scoped Permissions + Approval Policies**.

Distinguish: - what a user can do, - where they can do it, - whether
extra approval is required.

Backend is authoritative. Critical actions may require separation of
duties.

## Audit

Sensitive actions must preserve actor, timestamp, entity, action,
relevant before/after context, approval, reversal/correction
relationships, reason and linked decision/document where applicable.

## Social Aid

Social Aid is a separate non-profit financial domain in the same
application.

It may share auth, RBAC, approvals, audit, documents and notifications,
but has separate: - financial accounts, - support/income, - expenses/aid
payments, - funds, - reporting/dashboard.

Do not report Social Aid surplus as cooperative profit.

Supporters and beneficiaries do not need to be shareholders.
Restricted-purpose funds must be supported.

## Reporting/output

Output is a platform capability, not ad-hoc screen code.

Support: - print, - PDF, - XLSX, - filtered export, - selected-record
export, - statements, - operational receipts, - versioned document
templates, - stable business document numbering where needed.

Examples: - Collection Receipt - Shareholder Statement - Family
Statement - Share Statement - Period Collection Report - Financial
Account Statement - Share Transfer/Return documents - Social Aid reports

Reprinting a document must not create a new financial transaction. If
its source transaction is later reversed, verification/history must
expose that fact.

QR/document verification may be added for selected official documents.

## Explainability

Important calculated values should expose **How was this calculated?**
with source values, formula/policy version, valuation date and decision
reference where applicable.

## Intentionally unresolved

Do not invent: - exact profit formula, - exact exit formula, - exact
allocation ordering, - exact excess ownership rule for family
overpayments, - exact valuation methodology, - exact approval
thresholds, - exact value-protection index, - Turkish
legal/tax/accounting classification, - exact inheritance rules.

Dedicated specifications/policies must settle these before affected
behavior is implemented.
