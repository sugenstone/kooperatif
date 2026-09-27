# 02 --- Domain Model

## Goal

Define bounded contexts, core entities and relationships without
prematurely fixing database tables.

## Bounded contexts

### Identity & Access

User, Session, Role, Permission, Scope, RoleAssignment, ApprovalRule.

### Cooperative Core

Cooperative/Organization and tenant configuration.

### Parties & Families

Party, Shareholder, Family, FamilyMembership, GuardianAssociation.

### Shares

Share, ShareOwnership, ShareAllocation, ShareRestriction,
ShareLifecycleEvent, ShareReturn.

### Periods & Assessments

Period, PeriodAssessment, assessment eligibility/calculation references.

### Collections

CollectionSession, Payment, PaymentMethod, PaymentAllocation,
ExcessBalance effects, CollectionReceipt.

### Finance

FinancialAccount, FinancialAccountMovement, Income/Expense
classifications, Receivable, Liability, Transfer.

### Assets & Investments

Investment, InvestmentIncome, BusinessHolding, RealEstateAsset,
Contract, Valuation.

### Governance

Board/Body, Meeting, AgendaItem, Decision, Vote, Policy, PolicyVersion.

### Reporting/Documents

DocumentTemplate, GeneratedDocument identity, DocumentNumber,
ExportJob/Report definition where needed.

### Social Aid

Supporter, Beneficiary, SocialAidFinancialAccount, Fund, SupportReceipt,
AidRequest, AidDecision, AidPayment.

## Relationship rules

### Party / Shareholder / Guardian / Payer

Use a reusable Party/person concept where appropriate so one real person
can participate in multiple roles without duplicate identity records.

A Shareholder references a Party.

Guardian association references a Party and does not require that Party
to have a Shareholder record.

A Payment's Payer references a Party when known; controlled free-text
fallback may be specified later if operationally required.

### Family

Family belongs to one Cooperative.

Family has a cooperative-scoped Family Sequence Number.

FamilyMembership links Shareholders to Family with effective history if
membership changes must be historically reconstructed.

Do not store family total debt as independent authoritative debt. Derive
family views from member obligations.

### Share

Share belongs to one Cooperative.

Share ownership is time-bounded.

Invariant target: at most one active owner at a time.

Share history survives transfer and return.

### Period Assessment

Each assessment references exactly one Period and one Share.

The debtor/shareholder context must be historically reconstructible from
Share Ownership at assessment creation, and the assessment should
preserve the relevant ownership reference/snapshot needed for immutable
interpretation.

Do not reassign historical assessments merely because a Share is later
transferred.

### Payment

Payment represents one actual receipt of value.

Payment may have zero or many allocations during its lifecycle, subject
to posting rules.

Payment may settle debts belonging to multiple Shareholders/Shares.

Payment has: - cooperative, - payer, - payment method, - destination
financial account, - receipt timestamp/business date, - gross received
value, - lifecycle status, - collection-session reference when
applicable.

### Payment Allocation

Allocation references one Payment and one eligible obligation/debt.

Sum of allocations plus explicitly owned unallocated/excess disposition
must reconcile to payment value under the financial specification.

### Excess

Excess must have an explicit owner/accounting attribution. Family
membership alone is insufficient ownership.

Family overpayment behavior is intentionally policy-driven and must not
be guessed.

### Collection Session

CollectionSession normally references one Period and a planned/actual
collection date.

Payments can reference a CollectionSession, but Period payments outside
a session remain valid.

A CollectionSession is operational aggregation, not the source of
financial truth.

### Financial Account

Every posted Payment must create/associate the corresponding
authoritative account movement exactly once.

Do not infer actual cash solely from allocations.

### Documents

A GeneratedDocument references its authoritative source entity/entities
and template version.

Reprint does not create another Payment.

## Aggregate guidance

Do not create one giant Cooperative aggregate.

Use transactional boundaries appropriate to invariants. For example,
posting a Payment and its financial-account movement/allocations must be
atomic enough to prevent partial financial state.

## Historical reconstruction

The system should be able to answer questions such as: - Who owned Share
#47 on date X? - What debts did Shareholder Y have at end of Period Z? -
Which policy version calculated a right? - Which account received
Payment P? - Which allocations were produced by Payment P? - Who paid
for Family #047 during Collection Session S? - Was a receipt later
reversed?

Design history explicitly rather than reconstructing it from mutable
current-state fields.

## IDs and business numbers

Use technical immutable IDs separately from human business identifiers.

Examples: - UUID/internal ID for entity identity; - Family Sequence
Number for operations; - Share Number for human reference; -
Payment/Receipt Number; - Document Number; - Board Decision Number.

Business identifiers are cooperative-scoped unless explicitly specified
otherwise.

## Deletion

Hard deletion is acceptable only for safe drafts/unreferenced setup
records according to domain rules.

Historical/posted/effective entities require archive, close, cancel,
reversal or correction semantics.

## Open design decisions

Must be resolved later: - exact Party deduplication rules; - whether
FamilyMembership requires full effective dating from v1; - exact excess
attribution policy for family overpayment; - exact payment allocation
ordering; - exact ledger architecture; - SSE vs WebSocket; - exact
business-number formats.
