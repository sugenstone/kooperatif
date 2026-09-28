# STEP-004 — SHAREHOLDER, GUARDIAN & FAMILY IDENTITY FOUNDATION

## Repository

```text
C:\Users\sugen\Documents\PROJE\kooperatif
```

Remote:

```text
https://github.com/sugenstone/kooperatif.git
```

Expected baseline:

```text
main == origin/main == 70a8229
```

Completed milestones:

```text
STEP-001          CLOSED
STEP-002          CLOSED
SPEC-UPGRADE-001  CLOSED
REMOTE-SYNC-001   CLOSED
STEP-003          CLOSED
```

Authoritative specification:

```text
Kooperatif Agent Specification v0.8
```

This STEP introduces the first real cooperative business-domain foundation:

> Shareholder + Guardian + Family identity and relationship model

The objective is to establish a durable identity model before Shares, Periods, Assessments, Payments, Collection Sessions, Ledger, Investments, Governance, or Social Aid are implemented.

---

# 0. CORE BUSINESS CONTEXT

The cooperative may contain many Shareholders.

Important real-world constraints:

1. Multiple Shareholders may have exactly the same first and last name.
2. Shareholders are commonly distinguished operationally by their **guardian/father name**.
3. Every Shareholder belongs to a **family grouping** identified operationally by a **family sequence number**.
4. A Guardian does NOT have to be a Shareholder.
5. A Guardian MAY also independently be a Shareholder.
6. Multiple Shareholders may belong to the same family.
7. One family member may later pay the financial obligations of multiple members of that family.
8. Therefore family membership must be modeled as durable business data now, even though Payment functionality is deferred.
9. Shareholder, User, Guardian and Family are distinct concepts.
10. Person identity must never rely on `first_name + last_name` uniqueness.

This STEP establishes these foundations only.

---

# 1. OPERATING MODE

Follow:

```text
FETCH
→ VERIFY BASELINE
→ READ SPEC
→ INSPECT EXISTING DOMAIN MODEL
→ MODEL
→ THREAT / DATA-INTEGRITY REVIEW
→ PLAN
→ MIGRATE
→ IMPLEMENT BACKEND
→ IMPLEMENT CONTRACTS
→ IMPLEMENT FRONTEND
→ TEST
→ HOSTILE REVIEW
→ FULL GATES
→ DIFF REVIEW
→ COMMIT
→ PUSH
→ VERIFY REMOTE
→ CLOSURE REPORT
→ STOP
```

Do NOT continue into STEP-005.

---

# 2. BASELINE VERIFICATION

Before changing anything run:

```bash
git status --short
git branch --show-current
git remote -v
git fetch --prune origin
git rev-parse HEAD
git rev-parse origin/main
git log --oneline --decorate -n 12
```

Required:

```text
branch = main
HEAD = 70a8229
origin/main = 70a8229
working tree = clean
HEAD == origin/main
```

If remote or local differs:

DO NOT reset.
DO NOT rebase.
DO NOT force-push.
DO NOT discard work.

Investigate and STOP if safety cannot be established.

---

# 3. SPECIFICATION REVIEW

Read all documents relevant to:

```text
Shareholder
Guardian
Family
Person identity
membership
audit
correction
authorization
data model
API contracts
state/lifecycle
privacy
```

At minimum inspect:

```text
AGENTS.md
README.md

docs/00-PROJECT-CHARTER.md
docs/01-DOMAIN-GLOSSARY.md
docs/02-DOMAIN-MODEL.md
docs/03-SHAREHOLDER-FAMILY.md
docs/15-INVARIANTS.md
docs/16-STATE-MACHINES.md
docs/18-AUTHORIZATION-APPROVALS.md
docs/19-AUDIT-REVERSAL-CORRECTION.md
docs/20-DATA-MODEL-DATABASE.md
docs/21-API-CONTRACTS.md
docs/22-SECURITY.md
docs/26-TESTING-QUALITY-GATES.md
docs/27-ARCHITECTURE.md
docs/28-ADR-INDEX.md
```

Also review all Accepted ADRs whose decisions affect this STEP.

If a listed document does not exist, inspect the actual equivalent and report the discrepancy.

Do not invent a new interpretation that contradicts the authoritative spec.

---

# 4. DOMAIN BOUNDARIES

These concepts MUST remain distinct:

```text
User
Shareholder
Guardian
Family
Share
Governance Position
```

A `User` is an application login identity.

A `Shareholder` is a cooperative business entity/person record.

A `Guardian` is a person-reference used for identity/family context.

A `Family` groups related Shareholders for cooperative operational purposes.

A `Share` is a future ownership/participation instrument and is NOT implemented here.

Do NOT create:

```text
shareholder.user_id NOT NULL
```

or otherwise require every Shareholder to have a login.

Do NOT equate:

```text
Guardian == User
Guardian == Shareholder
Family == Household Account
Shareholder == Share
```

---

# 5. PERSON MODEL — IMPORTANT DESIGN DECISION

Before coding, explicitly determine the cleanest normalized representation for persons.

Preferred architecture:

```text
Person
├── may act as Shareholder
├── may act as Guardian
└── may potentially acquire other domain roles later
```

This avoids storing the same real person twice merely because they are both:

```text
Guardian
+
Shareholder
```

Strongly prefer a normalized `persons` identity layer unless the existing authoritative specification explicitly requires another model.

If using Person:

```text
Person
  ├── Shareholder profile / relationship
  └── referenced as Guardian
```

Do NOT automatically make every Guardian a Shareholder.

Do NOT automatically make every Shareholder a User.

Explain the final choice in the Closure Report.

---

# 6. PERSON FIELDS

Use only fields justified by the specification/business need.

Potential core identity fields:

```text
id
first_name
last_name
display/search normalization
created_at
updated_at
status if justified
```

Do not prematurely collect excessive personal information.

Do NOT add fields merely because generic CRM systems have them.

Examples requiring explicit justification before introduction:

```text
TC identity number
passport number
birth date
gender
religion
occupation
income
```

Data minimization matters.

If the specification already defines optional contact/address fields, follow it.

---

# 7. SAME NAME IS VALID

This is a hard invariant:

```text
(first_name, last_name)
```

MUST NOT be unique.

The following must be valid:

```text
Mehmet Yılmaz
Mehmet Yılmaz
Mehmet Yılmaz
```

They may represent three different people.

Do not reject legitimate duplicate names.

---

# 8. GUARDIAN / VASİ SEMANTICS

The cooperative operationally identifies Shareholders using the Guardian name.

The Guardian:

```text
may be a Shareholder
may not be a Shareholder
```

Model Guardian as an actual person reference, not merely an uncontrolled string, unless the authoritative spec explicitly says otherwise.

Preferred:

```text
shareholder.guardian_person_id
```

or equivalent normalized relation.

This allows:

```text
Guardian Person A
        │
        ├── Shareholder B
        ├── Shareholder C
        └── Shareholder D
```

and also:

```text
Person A
├── Guardian of B
└── Shareholder himself/herself
```

without duplicating Person A.

---

# 9. GUARDIAN IS NOT REQUIRED TO BE SHAREHOLDER

This invariant MUST be enforced by model design.

Creating:

```text
Guardian: Hasan Kaya
```

must NOT require creating a Shareholder record for Hasan Kaya.

Conversely, if Hasan Kaya already exists as a Person/Shareholder, the existing Person should be selectable as Guardian rather than forcing duplicate person creation.

---

# 10. GUARDIAN OPTIONALITY

Determine from the authoritative specification whether Guardian is mandatory for every Shareholder.

If the business/spec does not explicitly require universal mandatory guardian information, prefer:

```text
guardian_person_id nullable
```

because historical/incomplete records may exist.

The UI should clearly show:

```text
Vasi bilgisi yok
```

when absent.

Do not invent fake values such as:

```text
Bilinmiyor
-
Yok
```

as Person records.

---

# 11. FAMILY MODEL

Introduce a first-class Family aggregate/entity.

Conceptually:

```text
Family
id
sequence_number
display label if needed
status if justified
created_at
updated_at
```

The **family sequence number** is operationally important.

It must be:

```text
stable
human-readable
searchable
unique within the cooperative scope
```

Do not use database UUID as the user-facing family sequence number.

---

# 12. FAMILY SEQUENCE NUMBER

Family sequence number must be treated as a business identifier.

Examples:

```text
1
2
3
125
```

or another documented representation consistent with the spec.

Do not make it silently change when records are sorted or deleted.

It is not:

```text
row number
UI index
array position
```

It is durable business data.

---

# 13. FAMILY MEMBERSHIP

A Shareholder belongs to a Family.

Preferred invariant:

```text
one Shareholder → exactly one current Family
one Family → 1..N Shareholders
```

If historical migration/unknown-family cases require temporary absence, model that deliberately.

Do not allow a Shareholder to silently belong to multiple current Families.

Future family-transfer history may be required.

If the authoritative specification defines historical family membership, implement it.

Otherwise preserve architecture for later history without overbuilding STEP-004.

---

# 14. FAMILY IS NOT A FINANCIAL ACCOUNT

Critical:

```text
Family != FinancialAccount
Family != Cashbox
Family != Payer
```

The Family groups Shareholders.

Future Payment logic may allow:

> one person pays obligations for multiple Shareholders in the same Family.

That future behavior must reference family membership.

Do NOT create family balances in STEP-004.

Do NOT aggregate debt.

Do NOT implement payments.

---

# 15. SHAREHOLDER MODEL

Introduce Shareholder as a distinct business-domain entity linked to Person.

Conceptually:

```text
Shareholder
id
person_id
family_id
guardian_person_id
member_number if specification requires
status
joined_at / left_at only if justified by specification
created_at
updated_at
```

Exact schema must follow the authoritative domain specification.

Do NOT add financial fields such as:

```text
balance
debt
credit
period_debt
share_count
paid_amount
```

Those belong to future modules.

---

# 16. SHAREHOLDER STATUS

Implement only lifecycle states supported by specification.

Likely examples may include:

```text
active
inactive
```

but DO NOT invent a large state machine.

If the spec defines states such as:

```text
candidate
active
left
deceased
removed
```

follow the specification exactly.

Do not infer sensitive status without source support.

Lifecycle transitions must preserve history.

---

# 17. NO HARD DELETE FOR SHAREHOLDERS

Shareholders will later be referenced by:

```text
Shares
Periods
Assessments
Payments
Ledger
Governance
Documents
Audit
```

Therefore destructive deletion is dangerous.

Default policy:

> Shareholder records are not physically deleted once established as durable business records.

For mistakes, distinguish:

```text
mistakenly created unused record
```

from:

```text
real historical Shareholder
```

If safe void/cancel semantics are already specified, use them.

Otherwise implement status-based deactivation and leave destructive correction for an explicit controlled rule.

Do not create casual `DELETE /shareholders/:id`.

---

# 18. MISTAKEN RECORD CREATION

We explicitly discussed accidental records.

The system must support correcting an accidentally created Shareholder without corrupting history.

Preferred conceptual distinction:

```text
created by mistake + no downstream references
→ controlled void/cancel path

real historical record
→ deactivate/status transition
```

If implementing `voided` in this STEP is consistent with the spec, use it.

Otherwise document/defer the exact correction workflow.

Do not silently hard-delete audited records.

---

# 19. MERGE DUPLICATE PEOPLE — DO NOT OVERBUILD

A future operator may accidentally create:

```text
Person A = Mehmet Kaya
Person B = Mehmet Kaya
```

for the same real person.

Do NOT implement a complex Person merge engine unless already specified.

But design FKs so a future controlled merge is possible.

Document this as deferred work.

---

# 20. DISPLAY IDENTITY

Because names may duplicate, every Shareholder-facing UI must present sufficient context.

Preferred display identity:

```text
Ad Soyad
Vasi: Ad Soyad
Aile No: 123
```

Example:

```text
Mehmet Yılmaz
Vasi: Hasan Yılmaz
Aile No: 47
```

Do NOT show only:

```text
Mehmet Yılmaz
```

in ambiguous selectors, lists, payment-preparation surfaces, or search results.

This is a major business requirement.

---

# 21. REUSABLE SHAREHOLDER LABEL

Create a consistent backend/API/frontend representation/helper for Shareholder identity.

Conceptually:

```text
shareholder display label/context
```

should make it difficult for future modules to accidentally render only the person's name.

Do not duplicate formatting rules independently across many components.

---

# 22. SEARCH

Shareholder search must support at minimum:

```text
first name
last name
full name
guardian first/last/full name
family sequence number
```

Search should be case-insensitive and usable with Turkish names.

Respect the repository's existing PostgreSQL normalization strategy.

Do not rely solely on client-side filtering.

---

# 23. TURKISH NAME HANDLING

STEP-002 already encountered Turkish `İ/ı` normalization concerns.

Do not casually apply ASCII normalization to human names.

User login handles and human display names have different requirements.

Human names must preserve original Turkish characters:

```text
İ
ı
Ş
ş
Ğ
ğ
Ç
ç
Ö
ö
Ü
ü
```

Search normalization must not mutate stored display values.

Test representative Turkish names.

---

# 24. FAMILY SEARCH / IDENTIFICATION

Family lookup should support:

```text
family sequence number
member name
guardian context where useful
```

Family list/detail should make it easy to understand who belongs to the family.

---

# 25. FAMILY DETAIL PAGE

Create a family detail experience showing at minimum:

```text
Aile sıra numarası
Aile üyeleri
Her üyenin Ad Soyad
Vasi
Hissedar durumu
```

Do NOT show financial totals yet.

Future:

```text
family collection/payment view
```

will build on this page/model.

---

# 26. SHAREHOLDER DETAIL PAGE

This is important because later this page becomes the Shareholder's full history.

Create the foundation now.

At minimum:

```text
Kimlik
Ad Soyad
Vasi
Aile sıra numarası
Durum
Temel kayıt bilgileri
```

Provide future-compatible section/tab structure if appropriate.

Possible future sections:

```text
Genel
Hisseler
Borçlar
Ödemeler
Hareketler
Belgeler
Geçmiş
```

BUT:

Do NOT create fake data or fake APIs for those modules.

If showing future tabs harms UX, omit them now.

The current page should be useful with real STEP-004 data only.

---

# 27. FAMILY MEMBER NAVIGATION

From Shareholder detail:

```text
Aile No
```

should allow navigation to Family detail if UX permits.

From Family detail:

each Shareholder should navigate to Shareholder detail.

This establishes a coherent identity graph.

---

# 28. CREATE SHAREHOLDER WORKFLOW

The create flow should handle:

```text
Person
Guardian
Family
Shareholder
```

without forcing operators to understand database normalization.

The operator should be able to:

1. enter/select the Shareholder person;
2. select an existing Guardian OR create a Guardian Person;
3. select an existing Family OR create a Family if authorized;
4. review identity context;
5. create the Shareholder.

Avoid a confusing technical multi-table UI.

---

# 29. DUPLICATE WARNING — NOT NAME BLOCKING

When entering a person/shareholder with a name matching existing records:

DO NOT block solely because the name exists.

Instead surface potential matches such as:

```text
Benzer kayıtlar bulundu
```

with context:

```text
Mehmet Yılmaz — Vasi Hasan Yılmaz — Aile 47
Mehmet Yılmaz — Vasi Ahmet Yılmaz — Aile 81
```

The operator may confirm that this is a different person.

Do not silently auto-merge.

---

# 30. GUARDIAN SELECTION UX

Guardian selection/search must clearly distinguish same-name Persons.

Show useful context if available.

The operator must be able to:

```text
select existing Person
```

or:

```text
create new Person as Guardian
```

without creating a Shareholder for that Guardian.

---

# 31. FAMILY CREATION UX

Family creation must make the family sequence number clear.

If sequence numbers are manually assigned:

validate uniqueness.

If automatically assigned:

use a concurrency-safe database mechanism.

Do not implement:

```text
SELECT MAX(sequence_number) + 1
```

without proper serialization/sequence semantics.

Determine the correct strategy from specification/business expectations.

Document the decision.

---

# 32. AUTHORIZATION — NEW PERMISSIONS

STEP-003 established application-owned permission catalog.

Add only permissions required for STEP-004.

Preferred minimal set:

```text
shareholders.read
shareholders.manage
families.read
families.manage
```

Evaluate whether separate `persons.*` permissions are actually necessary.

Prefer NOT exposing Person administration as an unrelated generic module if Persons exist only to support Shareholder/Guardian workflows.

Do not add:

```text
shares.*
payments.*
ledger.*
social_aid.*
```

yet.

---

# 33. SYSTEM ADMIN ROLE DOES NOT AUTO-INHERIT

Critical STEP-003 rule:

There are no wildcards.

Therefore when STEP-004 introduces:

```text
shareholders.*
families.*
```

the existing `Sistem Yöneticisi` Role will NOT automatically receive them.

Create an explicit migration/catalog update that deliberately grants the new STEP-004 administrative permissions to the system-seeded administrative Role if that is the intended bootstrap behavior.

Do not rely on role name at runtime.

Migration-time deterministic seed relationship is acceptable.

Document this.

Future custom Roles must NOT automatically receive new permissions.

---

# 34. SERVER-SIDE AUTHORIZATION

All APIs must enforce permissions server-side.

Examples:

```text
GET shareholder/family data
→ *.read

create/update/status mutations
→ *.manage
```

Do not rely on frontend hiding.

401:

```text
not authenticated
```

403:

```text
authenticated but not authorized
```

Preserve STEP-003 semantics.

---

# 35. CSRF

Every authenticated state-changing STEP-004 endpoint must use the full STEP-002 CSRF architecture:

```text
Origin validation
+
synchronizer token
```

No exceptions.

Add HTTP-level proof.

---

# 36. AUDIT

Shareholder and Family identity changes are business-significant.

Audit at minimum:

```text
person created
person identity updated
family created
family sequence changed if allowed
shareholder created
shareholder updated
shareholder status changed
shareholder family changed if allowed
shareholder guardian changed
accidental-record void/cancel if implemented
```

Capture:

```text
actor
target
time
meaningful before/after state
```

Do not log secrets.

Avoid unnecessarily duplicating sensitive personal data in audit payloads.

---

# 37. CHANGE HISTORY

The future Shareholder page must eventually show historical changes.

Ensure audit/history data created now can support questions like:

```text
Bu hissedar ne zaman oluşturuldu?
Vasisi ne zaman değişti?
Hangi aileye bağlıydı?
Kim değiştirdi?
Durumu ne zaman değişti?
```

Do not build the full history UI unless scope remains reasonable.

But do not design an audit trail incapable of answering these questions.

---

# 38. FAMILY CHANGE

Changing a Shareholder's Family is significant.

If supported:

```text
old_family
new_family
actor
timestamp
reason if specification requires
```

must be auditable.

Do not silently overwrite family membership with no trace.

---

# 39. GUARDIAN CHANGE

Changing Guardian is also identity-significant.

Audit:

```text
old guardian
new guardian
actor
timestamp
```

Do not rewrite old audit events.

---

# 40. API DESIGN

Implement REST APIs consistent with existing conventions.

Likely capabilities:

```text
GET    /api/shareholders
POST   /api/shareholders
GET    /api/shareholders/:id
PATCH  /api/shareholders/:id

GET    /api/families
POST   /api/families
GET    /api/families/:id
PATCH  /api/families/:id
```

Additional focused endpoints may be used for:

```text
person lookup
guardian lookup
duplicate candidates
status actions
```

Do not expose arbitrary generic CRUD merely because a table exists.

---

# 41. LIST API

Shareholder list API should return enough context to render:

```text
Ad Soyad
Vasi
Aile No
Durum
```

without N+1 requests.

Support:

```text
search
pagination
stable sorting
```

Do not return an unbounded entire cooperative registry.

---

# 42. PAGINATION

Use server-side pagination from the start.

Define reasonable:

```text
page/pageSize
```

or cursor semantics consistent with project conventions.

Validate maximum page size.

Stable ordering required.

Do not create future scalability debt with:

```text
SELECT * FROM shareholders
```

for every list screen.

---

# 43. FAMILY LIST API

Return:

```text
family sequence
member count
useful member summary if appropriate
status if implemented
```

Avoid N+1 queries.

Use pagination.

---

# 44. CONTRACTS

Add shared TypeScript/Rust-compatible API contracts following existing architecture.

Do not expose raw DB rows.

Separate:

```text
ShareholderListItem
ShareholderDetail
FamilyListItem
FamilyDetail
Create/Update requests
Search results
```

as justified.

---

# 45. DATABASE CONSTRAINTS

Enforce invariants in PostgreSQL where possible.

Potential constraints:

```text
Family sequence unique
Shareholder.person_id uniqueness if one Person may have only one current Shareholder identity
valid FKs
valid statuses
non-empty normalized names
```

Do NOT make names unique.

Do NOT use application validation alone for hard invariants.

---

# 46. PERSON / SHAREHOLDER UNIQUENESS

Decide explicitly:

Can one Person have multiple Shareholder records simultaneously?

Preferred:

```text
one Person → at most one current Shareholder entity
```

unless the authoritative specification requires historical multiple memberships.

If historical re-entry is needed later, distinguish:

```text
Shareholder identity
```

from:

```text
membership episode
```

Do not create duplicate Shareholder rows to represent leaving and rejoining unless specified.

---

# 47. TRANSACTIONS

Creating a Shareholder may involve:

```text
Person
Guardian Person
Family
Shareholder
Audit
```

These must not leave partial state if an operation fails.

Use PostgreSQL transactions.

Example failure:

```text
Person created
Family creation fails
Shareholder not created
```

must not accidentally leave garbage records unless intentionally reusable.

Design transaction boundaries deliberately.

---

# 48. CONCURRENCY

Test important races:

```text
two operators create same family sequence
two operators request next automatic family sequence
two operators update same Shareholder
duplicate Person candidate does not create false uniqueness errors
```

If family sequence is generated automatically, prove concurrency safety.

Use PostgreSQL mechanisms, not process-local mutexes.

---

# 49. OPTIMISTIC CONCURRENCY

Evaluate lost-update protection for Shareholder/Family edits.

Two admins should not unknowingly overwrite each other's changes.

Use the project's existing concurrency strategy or introduce a minimal explicit version/update timestamp precondition if justified.

If deferred, explain why.

Do not silently accept lost updates without analysis.

---

# 50. PRIVACY / DATA MINIMIZATION

Person data is personal data.

Only expose it to authenticated authorized users.

Do not:

```text
put Person data in public endpoints
log entire Person payloads
return unnecessary fields
cache sensitive lists publicly
```

Preserve `no-store` behavior for authenticated business data where appropriate.

---

# 51. FRONTEND — NAVIGATION

Add authorization-aware navigation:

```text
Hissedarlar
Aileler
```

Visibility:

```text
shareholders.read
families.read
```

respectively.

Frontend hiding remains UX only.

---

# 52. FRONTEND — SHAREHOLDER LIST

Create a polished Turkish-first page.

Suggested route:

```text
/hissedarlar
```

Each row/card should prominently show:

```text
Ad Soyad
Vasi
Aile No
Durum
```

Provide:

```text
search
pagination
create action if authorized
detail navigation
```

Same-name Shareholders must be visually distinguishable.

---

# 53. FRONTEND — SHAREHOLDER DETAIL

Suggested:

```text
/hissedarlar/[id]
```

Display:

```text
Ad Soyad
Vasi
Aile No
Durum
created/updated information where useful
```

Allow edit only with:

```text
shareholders.manage
```

Use a professional information hierarchy.

This page is the future anchor for the Shareholder's full cooperative history.

---

# 54. FRONTEND — CREATE / EDIT

Use shadcn-svelte.

Forms must support:

```text
Shareholder Person
Guardian selection/create
Family selection/create
Status if appropriate
```

Use autocomplete/combobox UX for Person/Family selection rather than giant dropdowns.

Do not require operators to know UUIDs.

---

# 55. FRONTEND — FAMILY LIST

Suggested:

```text
/aileler
```

Display:

```text
Aile No
Üye Sayısı
useful member preview
```

Search and pagination required.

---

# 56. FRONTEND — FAMILY DETAIL

Suggested:

```text
/aileler/[id]
```

Show:

```text
Aile No
Üyeler
each member's full identity context
```

Link members to Shareholder detail.

Do NOT display fake debt/payment totals.

---

# 57. DEFAULT LANGUAGE

Default:

```text
Turkish / tr-TR
```

All new user-facing text through existing i18n.

English scaffold should remain coherent where existing architecture requires it.

---

# 58. EMPTY / LOADING / ERROR STATES

Implement intentional UX for:

```text
no Shareholders
no Families
no search results
loading
403
404
validation errors
conflict
```

Do not leave raw API errors in the UI.

---

# 59. ACCESSIBILITY

Forms and lists must maintain:

```text
labels
keyboard access
focus behavior
semantic controls
clear validation messages
```

Do not sacrifice accessibility for visual polish.

---

# 60. TEST — SAME NAME

This scenario is mandatory:

Create:

```text
Shareholder 1:
Mehmet Yılmaz
Guardian: Hasan Yılmaz
Family: 47

Shareholder 2:
Mehmet Yılmaz
Guardian: Ahmet Yılmaz
Family: 81
```

Both must coexist.

Search/list/detail must distinguish them.

No uniqueness conflict on the names.

---

# 61. TEST — GUARDIAN NOT SHAREHOLDER

Create:

```text
Guardian Person:
Hasan Yılmaz
```

without a Shareholder record.

Then create a Shareholder referencing Hasan as Guardian.

Must succeed.

Verify Hasan did NOT automatically become a Shareholder.

---

# 62. TEST — GUARDIAN ALSO SHAREHOLDER

Create Person A as Shareholder.

Then use the SAME Person A as Guardian of Shareholder B.

Must succeed without duplicating Person A.

This proves the normalized identity model.

---

# 63. TEST — FAMILY MEMBERS

Create:

```text
Family 100
├── Shareholder A
├── Shareholder B
└── Shareholder C
```

Family detail must return all three.

Each Shareholder must resolve to Family 100.

---

# 64. TEST — FAMILY SEQUENCE

Prove:

```text
duplicate family sequence → rejected
```

If automatic generation:

prove concurrent creation produces unique stable numbers.

---

# 65. TEST — SEARCH

Test Turkish and duplicate-name searches.

Representative data:

```text
İsmail
Şahin
Çağrı
Öztürk
Gül
Işık
```

Test:

```text
shareholder name
guardian name
family number
```

Stored display values must remain unchanged.

---

# 66. TEST — AUTHORIZATION

HTTP matrix:

```text
unauthenticated → 401

authenticated without shareholders.read
→ shareholder read → 403

authenticated with shareholders.read
→ read success

without shareholders.manage
→ mutation → 403

same for families.*
```

Also prove custom Roles can receive the new permissions through the STEP-003 RBAC system.

---

# 67. TEST — SYSTEM ADMIN MIGRATION

After migration:

Existing system administrative Role must receive intended new STEP-004 permissions explicitly.

Existing custom Roles must NOT silently receive them.

Test this.

No wildcard behavior.

---

# 68. TEST — CSRF

Representative state-changing endpoints must reject:

```text
missing CSRF token
invalid CSRF token
disallowed Origin
```

and succeed with valid proof.

---

# 69. TEST — AUDIT

Verify actor/target/change semantics for:

```text
Shareholder create
Shareholder update
Guardian change
Family change
Family create/update
status transition
```

where implemented.

---

# 70. TEST — PAGINATION

Test:

```text
page boundaries
stable ordering
maximum page size
search + pagination combination
```

No duplicates/missing records caused by unstable ordering.

---

# 71. TEST — TRANSACTION ROLLBACK

Force a mid-operation failure in a multi-entity creation path.

Verify no invalid partial Shareholder aggregate remains.

---

# 72. TEST — DIRECT API / IDOR

Attempt:

```text
unauthorized UUID access
unauthorized mutation
nonexistent UUID
cross-user crafted requests
```

Permission checks must remain authoritative.

Expected semantics:

```text
401 / 403 / 404
```

according to existing architecture.

---

# 73. HOSTILE DATA-INTEGRITY REVIEW

Before closure actively review:

### Duplicate names
Did any uniqueness assumption sneak in?

### Guardian coupling
Does Guardian accidentally require Shareholder status?

### Person duplication
Can selecting an existing Person still create a duplicate?

### Family sequence race
Can concurrent requests produce duplicate sequence numbers?

### Hard deletion
Can historical Shareholder identity disappear?

### Authorization
Can direct API bypass UI restrictions?

### CSRF
Are both layers applied?

### Audit
Can guardian/family changes happen without trace?

### Search
Does Turkish text behave incorrectly?

### N+1
Do list pages issue one query per row?

### Overexposure
Are unnecessary personal fields returned?

### Lost update
Can simultaneous edits silently overwrite each other?

Fix defects before closure.

---

# 74. DO NOT IMPLEMENT SHARES

Even though the word Shareholder exists, do NOT implement:

```text
Share
Hisse
Kumbara
Share acquisition
Share sale
Share transfer
Founder share
Paid share acquisition
Share fee
Share history
```

Those belong to the next dedicated domain STEP.

---

# 75. DO NOT IMPLEMENT FINANCE

Do NOT implement:

```text
Period
Assessment
Debt
Payment
Family payment
Collection screen
Cashbox
Bank account
Ledger
Balance
Extra payment
Opening balance
```

The Family model is being established partly to support those future workflows, but they are not part of STEP-004.

---

# 76. DO NOT IMPLEMENT SOCIAL AID

Social Aid remains a separate bounded context.

Do not attach Social Aid donor/beneficiary behavior to Person/Shareholder merely because Person now exists.

Future Social Aid may reference Persons where appropriate through explicit design.

---

# 77. BACKUP READINESS GATE

STEP-004 introduces durable cooperative master data but not material financial production data.

Therefore ADR-013's financial-production Backup Readiness Gate does not yet require production backup tooling for STEP closure.

However:

- do not weaken ADR-013;
- migrations must remain backup-compatible;
- do not introduce restore permissions into RBAC.

Report consistency.

---

# 78. QUALITY GATES

Run all established gates.

At minimum:

```text
git diff --check

frontend:
format
lint
svelte-check/typecheck
tests
production build

contracts:
typecheck

backend:
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

database:
clean migration apply
idempotent migration verification where established
real PostgreSQL integration suite

Docker:
existing compose/smoke gate

RBAC regression:
STEP-003 tests

Auth regression:
STEP-002 tests
```

Do not hide skipped DB tests.

---

# 79. E2E

STEP-003 deferred E2E until the first multi-screen business workflow.

STEP-004 is that point.

Add a focused deterministic browser E2E if the repository's existing test infrastructure can support it reasonably.

Preferred scenario:

```text
authorized admin logs in
→ opens Hissedarlar
→ creates/selects Family
→ creates/selects Guardian
→ creates Shareholder
→ sees Shareholder in list
→ opens detail
→ navigates to Family
→ sees Shareholder in Family
```

Include a same-name distinction if practical.

If E2E cannot reasonably be introduced, give a concrete technical reason in the Closure Report.

Do not simply say "deferred".

---

# 80. DOCUMENTATION

Update documentation with:

```text
Person model
Shareholder model
Guardian semantics
Family model
family sequence semantics
same-name rule
display identity rule
search behavior
lifecycle/deletion policy
permissions introduced
audit behavior
future relationship to Share/Payment domains
```

Do not rewrite unrelated specification.

If implementation reveals a contradiction requiring an ADR:

STOP and report it instead of silently deciding architecture.

---

# 81. DIFF REVIEW

Before commit:

```bash
git status --short
git diff --stat
git diff
git diff --check
```

Review all changes.

Reject:

```text
unrelated domains
financial implementation
Share implementation
Social Aid implementation
generated junk
logs
database dumps
.env
credentials
temporary files
```

---

# 82. COMMIT

After all gates pass:

```text
feat: add shareholder family identity foundation
```

One coherent STEP-004 commit.

Do not amend prior commits.

---

# 83. PRE-PUSH SAFETY

Before push:

```bash
git fetch --prune origin
git rev-parse origin/main
```

Expected pre-push:

```text
origin/main = 70a8229
```

If changed unexpectedly:

STOP.

Do not force-push.

If safe:

```bash
git push origin main
```

---

# 84. POST-PUSH VERIFICATION

Run:

```bash
git fetch origin
git rev-parse HEAD
git rev-parse origin/main
git status
git branch -vv
```

Required:

```text
HEAD == origin/main
working tree clean
main tracks origin/main
```

---

# 85. REQUIRED CLOSURE REPORT

Produce:

```text
# STEP-004 — SHAREHOLDER, GUARDIAN & FAMILY IDENTITY FOUNDATION
# CLOSURE REPORT
```

with ALL sections below.

## A. Baseline

Report starting:

```text
branch
HEAD
origin/main
working tree
```

## B. Specification Review

Documents read, discrepancies and binding rules.

## C. Person Model

Explain the chosen identity model and why.

## D. Shareholder Model

Schema and lifecycle.

## E. Guardian Model

Explain how:

```text
Guardian may be non-Shareholder
Guardian may also be Shareholder
```

without duplication.

## F. Family Model

Explain:

```text
Family
sequence number
membership
stability
```

## G. Same-Name Handling

Prove same-name Shareholders coexist.

## H. Display Identity

Explain the canonical UI/API identity context:

```text
Ad Soyad
Vasi
Aile No
```

## I. Search

Explain name/guardian/family search and Turkish handling.

## J. Permissions

List EXACT permission keys added.

Confirm no Share/Finance/Social Aid permissions.

## K. System Administrator Permission Upgrade

Explain how the existing seeded administrative Role received the new permissions without wildcard behavior.

## L. APIs

List endpoints and permission requirements.

## M. Frontend

Describe:

```text
Hissedarlar
Hissedar detay
Aileler
Aile detay
create/edit flows
Guardian selection
Family selection
duplicate warnings
```

## N. Audit / History

List events and before/after semantics.

## O. Database

Tables, FKs, indexes, uniqueness constraints, migration number.

Explicitly confirm names are NOT unique.

## P. Concurrency

Family sequence and edit concurrency decisions/tests.

## Q. Security Review

Report:

```text
401/403
CSRF
IDOR
data exposure
hard-delete behavior
permission enforcement
```

## R. Data-Integrity Review

Report findings for:

```text
duplicate names
Guardian coupling
Person duplication
Family sequence race
partial transaction failure
lost updates
```

## S. Tests

Exact counts grouped by:

```text
Rust unit
HTTP
PostgreSQL
frontend
E2E
regression
```

## T. Quality Gates

Every gate and result.

## U. ADR Consistency

Report relevant Accepted ADR consistency including ADR-013.

## V. Backup Readiness

Confirm whether the gate is triggered and why.

## W. Deferred Work

Explicitly include:

```text
Share/Hisse/Kumbara lifecycle
founder/non-founder acquisition rules
Share fees
Share transfer/sale
Periods
Assessments
Debt
Payments
family collection
Cashbox
Ledger
Governance
Investments
Social Aid
Person merge
advanced history UI
```

## X. Files Changed

Every file grouped by purpose.

## Y. Repository Hygiene

Report:

```text
git diff --check
secret scan
temporary artifacts
working tree
```

## Z. Commit & Remote

Report:

```text
commit hash
pre-push origin/main
push result
post-push HEAD
post-push origin/main
HEAD == origin/main
```

## AA. Final Result

End with exactly:

```text
READY FOR STEP-004 REVIEW
```

or:

```text
STEP-004 BLOCKED
```

---

# 86. STOP CONDITION

After the Closure Report:

**STOP.**

Do NOT begin STEP-005.

Do NOT implement:

```text
Shares
Kumbara
Share acquisition
Share transfer
Share sale
Periods
Assessments
Debt
Payments
Collection Sessions
Cashbox
Ledger
Governance
Investments
Social Aid
Backup tooling
```

# STEP-004 AMENDMENT — TEMPORAL FAMILY MEMBERSHIP

The Family relationship MUST preserve historical membership.

A Shareholder:

- MUST have at most one current/active Family membership;
- MAY belong to different Families at different times;
- MAY leave an existing Family and move into an existing Family;
- MAY leave an existing Family and create/be assigned to a newly created Family;
- MUST retain all previous Family membership history.

Do NOT model Family membership only as a destructive mutable `shareholders.family_id` relationship if that would erase history.

Prefer a dedicated temporal relationship equivalent to:

```text
shareholder_family_memberships

id
shareholder_id
family_id
started_at
ended_at
reason (optional if justified)
created_by / changed_by as appropriate
created_at
```

Current membership is the membership whose validity interval is active.

Required invariant:

```text
one Shareholder
→ at most one active Family membership at any moment
```

Historical memberships may be unlimited but MUST NOT overlap.

Example:

```text
Shareholder Abdullah

Family 47
2024-01-01 → 2026-09-15

Family 126
2026-09-15 → current
```

Moving a Shareholder between Families MUST be transactional:

```text
BEGIN

close current Family membership
create new Family membership
write durable audit/history

COMMIT
```

If the destination Family is created during the same operation:

```text
create Family
close old membership
create new membership
write audit

```

must occur within one transaction.

The operation must not produce a state where the Shareholder belongs to two current Families.

Concurrency must be handled at PostgreSQL level.

The UI must support an authorized operator action equivalent to:

```text
Aile Değiştir
```

with:

- current Family clearly displayed;
- destination existing Family selection; OR
- creation of a new Family;
- new Family sequence number;
- effective date/time as defined by the domain model;
- optional/required reason according to specification;
- confirmation before mutation.

Shareholder Detail must expose at least the current Family.

The backend/data model MUST already preserve full Family history so a future history surface can show:

```text
AİLE GEÇMİŞİ

Aile No 47
01.01.2024 → 15.09.2026

Aile No 126
15.09.2026 → Devam ediyor
```

Do not rewrite previous financial/domain history when Family changes.

Future Period, Assessment, Debt, Payment and Family Collection modules must be able to determine historical Family membership according to the relevant business date rather than assuming the Shareholder's current Family was always their Family.

Add tests proving:

1. Shareholder starts in Family A.
2. Shareholder moves to Family B.
3. Family A membership remains historically queryable.
4. Family B becomes the only current membership.
5. Shareholder can move from Family B into a newly created Family C.
6. Historical sequence A → B → C remains intact.
7. Two active Family memberships are impossible.
8. Concurrent Family-transfer requests cannot create overlapping active memberships.
9. Failed transfer rolls back both membership changes.
10. Family change is durably audited.

Wait for reviewer authorization.
