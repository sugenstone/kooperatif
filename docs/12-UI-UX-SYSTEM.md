# 12 --- UI/UX System

## Purpose

Define the product-wide interaction and visual rules. Domain
specifications define what is true; this document defines how users
safely and consistently interact with that truth.

## Product UX principles

1.  **Turkish-first (`tr-TR`)**.
2.  **Simple UI, strict backend**.
3.  Optimize for clarity and error prevention before decoration.
4.  Important financial consequences must be visible before
    confirmation.
5.  Same action type should use the same interaction pattern throughout
    the product.
6.  Never rely on color alone to communicate status.
7.  Historical/audit information must be easy to discover.
8.  Dense financial data must remain readable.
9.  High-volume Collection Session screens prioritize speed, identity
    safety and keyboard efficiency.
10. Responsive design is mandatory; desktop is primary for back-office
    work, but core workflows must remain usable on mobile/tablet.

## Component foundation

Use **shadcn-svelte** as the primary UI component foundation from
project start.

Do not create custom primitives when an appropriate shadcn-svelte
primitive exists unless there is a documented reason.

Centralize shared product components in the UI package where
appropriate.

## Application shell

Default authenticated shell: - persistent/collapsible desktop sidebar; -
mobile navigation adapted appropriately; - top header for page
context/actions; - breadcrumb where hierarchy benefits from it; - main
content container with consistent spacing; - optional contextual
right-side panel/drawer only when justified.

Navigation should be domain-oriented, not database-table-oriented.

Candidate main areas: - Ana Sayfa - Hissedarlar - Aileler - Hisseler -
Dönemler - Tahsilat - Finans - Yatırımlar - Gayrimenkuller - İşletme
Ortaklıkları - Yönetim / Kararlar / Politikalar - Raporlar - Sosyal
Yardımlaşma - Sistem / Yetkilendirme

Final navigation is subject to later screen architecture review.

## Page anatomy

Standard page structure: 1. title + concise context; 2. primary action
area; 3. optional summary/KPI strip; 4. filters/search; 5. main content;
6. secondary details/timeline where relevant.

Avoid excessive card nesting.

## Tables

Use tables for scan-heavy structured data.

Requirements: - meaningful column priority; - sortable columns where
useful; - filter state should be visible; - pagination or virtualization
for large data; - row actions consistent; - selection state explicit; -
totals must clearly state what dataset/filter they represent; - mobile
may transform selected tables into cards where necessary.

Never hide critical identity information merely to reduce width.

## Cards

Use cards for summaries, entity overviews and operational grouping---not
as a universal replacement for tables.

Collection/family cards may be specialized for fast recognition.

## Forms

Forms must: - use explicit Turkish labels; - show required vs optional
state; - provide inline validation; - preserve entered data after
recoverable errors; - format money/date inputs carefully; - avoid
ambiguous generic labels like "Tutar" when context needs "Tahsil Edilen
Tutar" or "Dönem Borcu"; - show server validation errors meaningfully.

Do not use placeholder text as the only label.

## Dialog vs Drawer vs Page

### Dialog

Use for focused, short decisions/actions: - confirmation; - small
edit; - reversal reason; - simple approval.

### Drawer/Sheet

Use for contextual detail or medium-complexity actions where preserving
the current page context is valuable.

### Full Page

Use for complex workflows, multi-section entities, detailed financial
operations and long forms.

Do not force a complex payment workflow into a tiny modal merely for
convenience.

## Destructive and corrective actions

Posted financial transactions do not use a generic "Sil" action.

Use explicit actions such as: - Taslağı Sil - İptal Et - Ters Kayıt
Oluştur - Düzeltme İşlemi Başlat - Arşivle

Confirmation UI must explain the consequence.

## Status design

Status must include text/icon semantics, not color alone.

Examples: - Ödendi - Kısmi Ödendi - Ödenmedi - Bekleyen Onay - Ters
Kayıt Oluşturuldu - Kapalı - Askıya Alındı

Status color usage must remain consistent product-wide.

## Money and values

Default locale: `tr-TR`.

Presentation examples: - `12.500,00 TL` - dates in Turkish locale; -
percentages localized consistently.

Stored authoritative values are not locale-formatted strings.

For dense screens, decimal precision may be reduced only where
domain-safe and explicitly defined.

## Search

Global/entity search should tolerate operational identifiers.

Shareholder/family search must support: - hissedar adı; - vasi adı; -
aile sıra no; - relevant share/business numbers.

Search results must include disambiguating identity context.

## Empty states

Empty states should explain: - what is missing; - why the page may be
empty; - what action is available if the user has permission.

Do not show meaningless decorative empty-state copy.

## Loading

Use skeleton/loading states for meaningful waits.

Avoid blocking the entire app for a local panel refresh.

Financial posting actions must visibly prevent accidental repeated
submission while pending.

## Error states

Errors must be actionable and Turkish by default.

Do not expose raw Rust/database/internal stack errors to users.

Preserve a trace/reference ID for support when useful.

## Toasts and notifications

Toasts are for transient confirmation, not for critical information that
disappears.

A successful financial posting should have a persistent resulting state
on screen even if a success toast is also shown.

## Accessibility

-   keyboard navigability;
-   visible focus;
-   semantic labels;
-   accessible dialogs;
-   no color-only meaning;
-   adequate target sizes;
-   form errors connected to fields;
-   screen-reader-friendly status/action labels.

## Permission-aware UI

Hide/disable actions appropriately based on resolved permissions, but
frontend permission behavior is not security.

When an action is visible but unavailable due to workflow state, explain
why.

## Approval UX

Critical actions should show: - requester; - action; -
amount/resource; - reason; - required approval state; - approval
history.

Self-approval restrictions should be clearly communicated.

## Timeline pattern

Important entities should expose a chronological timeline.

Examples: - Hissedar oluşturuldu - Hisse tahsis edildi - Dönem borcu
oluştu - Tahsilat yapıldı - Hisse devredildi - Ters kayıt oluşturuldu

Timeline items link to source entities/documents where permitted.

## Explainability pattern

Key calculated totals should offer `Nasıl hesaplandı?`

The explanation surface may be a drawer/dialog/page depending on
complexity and should show source components and governing
policy/version.

## Collection Session --- special operational mode

Collection Session is allowed a specialized high-throughput layout.

Priorities: 1. identify the correct person/family; 2. see debt
immediately; 3. enter payment with minimal interaction; 4. preview
allocation; 5. post safely; 6. immediately see updated
debt/account/session totals; 7. move to next payer quickly.

Recommended features: - autofocus search where safe; - keyboard
shortcuts; - large, obvious primary action; - recent-payment warning; -
live session totals; - live relevant account balance; - latest
transactions panel; - fast family expansion; - selected-member
payment; - all-family payment; - clear cash/bank method selection.

Never sacrifice confirmation of identity and amount merely to save one
click.

## Real-time UX

When another operator posts a payment: - update affected session
totals; - update affected family/shareholder debt; - update relevant
account balance; - show subtle activity feedback if useful.

If the user's currently open payment preview becomes stale, require
recalculation/revalidation before posting.

## Responsive strategy

Desktop: - optimized for dense back-office workflows.

Tablet: - preserve operational collection usability.

Mobile: - core lookup, entity detail, approval and basic collection
workflows; - avoid horizontally unusable financial tables.

## Localization

All user-facing strings must be localizable.

Default is Turkish (`tr-TR`).

Technical code identifiers remain English.

Do not concatenate translated fragments in ways that make future
localization difficult.

## UX consistency gate

A feature is not UI-complete if it introduces: - a new unapproved
primitive/pattern; - inconsistent status language; - inconsistent
destructive action semantics; - hard-coded non-localizable copy; -
inaccessible interaction; - financial consequence hidden from
confirmation.
