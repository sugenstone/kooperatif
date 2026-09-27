# AGENTS.md --- Kooperatif

Repository: `git@github.com:sugenstone/kooperatif.git`

## Mandatory reading order

Before any implementation: 1. Read `docs/00-PROJECT-CHARTER.md`. 2. Read
`docs/01-DOMAIN-GLOSSARY.md`. 3. Read `docs/15-INVARIANTS.md`. 4. Read
`docs/27-ARCHITECTURE.md`. 5. Read `docs/28-ADR-INDEX.md` and any
Accepted ADR relevant to the step. 6. Read the domain specifications
relevant to the step. 7. For user-facing work, read
`docs/12-UI-UX-SYSTEM.md` and `docs/13-SCREEN-SPECIFICATIONS.md`. 8. For
reports/documents, read `docs/14-REPORTING-AND-DOCUMENT-DESIGN.md`. 9.
For lifecycle work, read `docs/16-STATE-MACHINES.md`. 10. For
calculations, read `docs/17-CALCULATION-RULES.md`. 11. For
permissions/approvals, read `docs/18-AUTHORIZATION-APPROVALS.md`. 12.
For correction/audit, read `docs/19-AUDIT-REVERSAL-CORRECTION.md`. 13.
For schema work, read `docs/20-DATA-MODEL-DATABASE.md`. 14. For API
work, read `docs/21-API-CONTRACTS.md`. 15. For security-sensitive work,
read `docs/22-SECURITY.md`. 16. For file/notification/operations work,
read the corresponding specification. 17. Read
`docs/26-TESTING-QUALITY-GATES.md`. 18. Read only the approved
implementation STEP and its explicit dependencies. 19. Do not proceed to
another STEP without explicit instruction.

### Authority order when documents appear to conflict

1.  Explicit approved corrective instruction for the current STEP.
2.  Accepted ADR for architecture-specific decisions.
3.  System Invariants.
4.  Domain-specific specification.
5.  Project Charter.
6.  UI/UX and screen specifications.
7.  Roadmap.

If a conflict remains meaningful: **STOP and report it.**

## Non-negotiable rules

-   Never invent unspecified financial behavior.
-   Never silently choose a profit-sharing, exit, valuation, indexation,
    allocation, or excess-payment rule.
-   Never destructively delete a posted financial transaction.
-   Never silently rewrite historical ownership, debt, rights, policy
    versions, or financial movements.
-   Never treat every cash inflow as income or every cash outflow as
    expense.
-   Never merge Cooperative Finance and Social Aid Finance.
-   Never model multiple shares only as `share_count`.
-   Never assume payer == shareholder == debtor == guardian.
-   Never assign an excess family payment to an arbitrary family member.
-   Never rely on frontend authorization or tenant filtering for
    security.
-   Never hide a specification conflict by choosing an interpretation.

If a required rule is undefined or documents conflict: **STOP and report
the exact ambiguity.**

## Implementation discipline

For each approved step: 1. Verify repository baseline. 2. State exact
scope and exclusions. 3. Identify relevant invariants. 4. Implement only
that scope. 5. Add tests proving behavior and failure cases. 6. Run all
required gates. 7. Inspect final diff for scope creep. 8. Produce a
closure report with evidence. 9. Stop.

## Technology direction

-   SvelteKit + TypeScript
-   shadcn-svelte from project start
-   Rust + Axum
-   PostgreSQL
-   modular monolith
-   monorepo
-   migration-driven schema
-   Docker-supported development
-   backend-authoritative authorization
-   CI + automated tests + E2E

Generic starter infrastructure may be reused. Domain-specific code from
another application must not leak into this repository.

## Language and localization

-   Default product language and locale: **Turkish (`tr-TR`)**.
-   All user-facing UI copy, validation/errors, notifications, reports,
    receipts, PDFs, printouts and document templates must default to
    Turkish.
-   Technical identifiers in Rust/TypeScript/database/API code should
    remain English unless there is a strong reason otherwise.
-   The frontend must be i18n-ready from the beginning; do not scatter
    hard-coded user-facing strings across components.
-   Future languages such as English may be added without redesigning
    domain logic.
-   Dates, numbers and monetary/value formatting must use locale-aware
    formatting. Default presentation uses `tr-TR`.
-   Localization must never alter authoritative stored financial values;
    it affects presentation only.

## UI/UX discipline

-   Use shadcn-svelte as the default component foundation.
-   Do not invent a new interaction pattern when the UI/UX specification
    already defines one.
-   Do not optimize visual minimalism at the cost of financial meaning
    or identity safety.
-   Collection Session is a specialized high-throughput operational UI
    and must follow its dedicated rules.
-   Do not use color as the only status signal.
-   Reports/PDF/XLSX are governed by the reporting/document
    specification, not ad-hoc page code.

## Financial integrity

High-level user actions may atomically produce several authoritative
effects.

Example: one shareholder/family payment can produce: - one
receipt/payment, - one financial-account inflow, - many debt
allocations, - a remaining excess-payment effect.

Do not make users manually create these effects separately.

## UI principle

**Simple UI, strict backend.**

Financial action screens should preview consequences before confirmation
where practical.

## Completion

Compiling is not closure. A completed step must pass the project's
formatting, linting, typechecking, testing, migration, build, E2E and
repository-cleanliness gates applicable to that step.
