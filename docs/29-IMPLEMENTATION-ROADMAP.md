# 29 --- Implementation Roadmap

## Status

This is the roadmap framework, not authorization to start coding all
steps.

Implementation begins only after the remaining domain specifications
required by a step are complete and relevant ADRs are accepted.

## Phase 0 --- Repository/Foundation

Potential steps: - repository baseline; - monorepo; - SvelteKit; -
shadcn-svelte; - Rust/Axum; - PostgreSQL; - Docker; - CI; - i18n
`tr-TR`; - shared contracts; - health checks.

## Phase 1 --- Identity and Tenant Foundation

-   Users/Sessions;
-   Cooperatives/Organizations;
-   tenant context;
-   memberships;
-   RBAC/Scopes;
-   Audit foundation.

## Phase 2 --- Parties, Families, Shareholders

-   Party;
-   Family;
-   Guardian association;
-   Shareholder;
-   search/disambiguation;
-   detail/timeline foundation.

## Phase 3 --- Shares

-   Share identity;
-   ownership;
-   allocation;
-   optional acquisition fee foundation;
-   transfer;
-   restrictions/lifecycle.

## Phase 4 --- Financial Foundation

Blocked until Financial Ledger + Money Representation ADRs are accepted.

-   Financial Accounts;
-   movements/ledger;
-   Receivables/Liabilities foundation;
-   reconciliation.

## Phase 5 --- Periods and Assessments

-   Period lifecycle;
-   Policy-driven assessment generation;
-   per-Share assessments;
-   Period reporting semantics.

## Phase 6 --- Payments and Collection

-   Payment;
-   Payment Method;
-   allocations;
-   excess;
-   individual payment;
-   Family Payment;
-   Collection Session;
-   duplicate/idempotency/concurrency;
-   real-time updates;
-   Collection Receipt.

## Phase 7 --- Governance and Policy Engine

-   Boards;
-   meetings;
-   decisions;
-   votes;
-   Policy/PolicyVersion;
-   effective-date handling;
-   approvals.

Some policy infrastructure may need to be introduced earlier in minimal
form; roadmap steps should avoid circular dependencies.

## Phase 8 --- Reporting/Documents

-   statements;
-   PDF;
-   XLSX;
-   templates;
-   numbering;
-   verification where approved.

## Phase 9 --- Investments/Assets

-   Investments;
-   Business Holdings;
-   Real Estate;
-   contracts;
-   expected/actual income;
-   valuations.

## Phase 10 --- Exit/Profit Rights

Blocked until calculation policies are finalized.

-   Share Return;
-   invested-amount right;
-   Profit Right;
-   crystallization;
-   Value Protection;
-   delayed settlements.

## Phase 11 --- Social Aid

-   isolated financial context;
-   supporters;
-   beneficiaries;
-   funds;
-   aid requests/decisions/payments;
-   Social Aid reporting.

## Phase 12 --- Notifications/Files/Operational Hardening

-   notifications/tasks;
-   attachment management;
-   backups/recovery;
-   security hardening;
-   observability;
-   performance.

## STEP files

Actual coding instructions live under:

``` text
implementation/
  STEP-001-....md
  STEP-002-....md
```

Each STEP: - has one bounded scope; - names required specs/ADRs; -
states exclusions; - defines acceptance criteria; - defines
tests/gates; - requires closure report; - ends with STOP.

## Rule

Never give an agent "build the whole application."

Implement one reviewed STEP at a time.
