# 13 --- Screen Specifications

## Purpose

Define screen-level responsibilities and critical information
architecture. Exact visual design may evolve without violating these
responsibilities.

## 1. Dashboard --- Ana Sayfa

Should summarize the cooperative without mixing unrelated financial
meanings.

Candidate areas: - current Period; - current assessment total; -
outstanding debt; - recent/new cash collections; - cooperative
financial-account overview; - pending approvals; - active
investments/alerts; - upcoming receivables/liabilities; - recent
operational activity.

Social Aid metrics should appear as a clearly separate section/context,
never merged into cooperative profit/cash KPIs.

## 2. Shareholders --- Hissedarlar

List/search screen must emphasize safe identity.

Columns/card fields may include: - Hissedar; - Vasi; - Aile No; - aktif
Hisse sayısı; - toplam açık borç; - fazla ödeme; - status.

Search: - name; - guardian; - family number; - share number where
useful.

Actions depend on permissions.

## 3. Shareholder Detail --- Hissedar Detayı

Header: - name; - guardian; - family; - membership status; - active
share count.

Sections/tabs: - Genel Bakış - Hisseler - Borçlar - Ödemeler - Fazla
Ödemeler - Alacaklar - Belgeler - Zaman Çizelgesi

Important actions: - Ödeme Al - Hisse İşlemleri - Ekstre - appropriate
edit/archive actions

Support `... tarihi itibarıyla` historical view when implemented.

## 4. Families --- Aileler

List: - Aile No; - identifying name/guardian context; - member count; -
active share count; - current period debt; - total outstanding debt.

## 5. Family Detail --- Aile Detayı

Must preserve member-level attribution.

Show: - Aile No; - guardian/representative context; - members; - each
member's debt; - each member's active shares; - excess balances by
actual owner; - family-level derived totals; - recent payments.

Actions: - selected-member payment; - all-family payment; - Family
Statement.

Never present a family aggregate in a way that implies debts/excess are
freely interchangeable.

## 6. Shares --- Hisseler

List: - Share number; - current owner; - family context; - lifecycle
state; - relevant debt/financial indicators; - restrictions.

## 7. Share Detail --- Hisse Detayı

Show: - Share number/state; - current owner; - ownership history; -
allocation/acquisition fee; - Period Assessments; -
allocations/payments; - invested amount; - profit/exit rights; -
restrictions; - documents; - timeline.

## 8. Periods --- Dönemler

List: - Period name/number; - lifecycle state; - planned collection
date; - assessment total; - debt satisfied; - outstanding debt; - new
cash collected; - prior excess applied.

Do not show one ambiguous "Tahsil Edilen" metric without definition.

## 9. Period Detail --- Dönem Detayı

Sections: - overview; - assessments; - collections; - outstanding
debts; - collection sessions; - reports; - governing policy references.

Summary must distinguish: - tahakkuk; - önceki fazla ödemeden mahsup; -
yeni nakit tahsilatı; - toplam kapatılan dönem borcu; - kalan dönem
borcu.

## 10. Collection Sessions --- Tahsilat Oturumları

List: - Period; - planned/actual date; - state; - operators if
tracked; - new cash collected; - transaction count; - open/closed state.

## 11. Collection Session Workspace --- Tahsilat Ekranı

This is a specialized operational screen.

### Persistent session header

Show: - Period; - date; - session status; - total assessment; - prior
excess applied; - new cash collected; - remaining Period debt; -
payment-method breakdown; - relevant live account balances.

### Search/lookup

Prominent search input.

Search by: - shareholder name; - guardian; - family sequence number.

Results must show: - shareholder; - guardian; - family no; - share
count; - current Period debt; - older debt; - total debt; - excess
balance; - recent-payment indicator.

### Family result

Expandable family result shows all members with selectable obligations.

Support: - one member; - several members; - all family members.

### Payment action

Payment flow shows: - Payer; - selected debtors/members; - selected
obligations; - existing excess; - amount received; - method; -
destination account; - proposed allocations; - resulting debt; -
resulting excess attribution.

If received amount exceeds selected obligations, require explicit valid
excess handling.

### After posting

Immediately show: - receipt number; - amount; - allocations; - updated
debt; - updated account balance; - updated session totals.

Provide `Makbuz Yazdır` / appropriate output action.

### Recent transactions

Show recent posted payments with enough identity context to catch
mistakes.

### Duplicate warning

A materially similar recent payment should trigger a clear warning
before posting.

### Concurrency

If data changed since preview, do not post stale allocation silently.
Recalculate and tell the operator what changed.

## 12. Financial Accounts --- Finansal Hesaplar

List: - account name; - value/currency type; - current reconciled
balance; - state; - context.

Detail: - balance; - movements; - inflow/outflow classifications; -
transfers; - reconciliation/reporting; - timeline.

Never allow direct balance editing as a normal operation.

## 13. Payment Detail --- Tahsilat Detayı

Show: - receipt/payment number; - payer; - date/time; - operator; -
method; - destination account; - gross amount; - allocations; - excess
disposition; - collection session; - state; - reversal/correction
relationships; - receipt/document.

## 14. Governance/Policies

Policy detail must expose: - policy identity; - version; - effective
interval; - parameters; - linked decision; - affected domain; - history.

Do not overwrite prior versions in-place.

## 15. Approvals

Queue supports: - requested action; - requester; - amount/context; -
age; - required approval level; - reason.

Detail shows source transaction and full consequence before approval.

## 16. Audit

Authorized users can filter by: - actor; - entity; - action; - date; -
module.

Audit UI is read-only.

## 17. Social Aid

Use visibly separate financial context.

Dashboard candidate metrics: - collected support; - aid paid; -
operating expenses; - available fund balance; - restricted fund
balances; - pending aid requests.

Never label surplus as cooperative profit.

## 18. Reports

Report screen supports: - report type; - date/Period; - filters; -
preview; - PDF; - XLSX; - print where applicable.

Export should honor current filters/selection unless the user explicitly
chooses another scope.

## 19. Statements

Shareholder, Family, Share and Financial Account statements should be
chronological and clearly distinguish: - assessment; - payment; -
allocation; - excess; - reversal; - receivable/payment; -
opening/closing relevant balances.

## 20. Global screen rule

Every screen must have a clear answer to: - What am I looking at? -
Which cooperative/context? - What is the current state? - What can I
safely do next? - Where did this number come from?

## 21. Live TV / Projector Dashboard

A dedicated read-only full-screen operational view may be used on a
TV/projector during collection.

Requirements: - WebSocket live updates; - authoritative snapshot on
initial load/reconnect; - large, distance-readable typography; - no
unnecessary navigation; - aggregated Period/session totals; - no
unnecessary Shareholder personal or individual financial details; -
restricted read-only session/identity; - visible connection/reconnecting
state; - stale-data indication if live connection is lost.

Candidate metrics: - Period Assessment Total; - New Cash Collected; -
Prior Excess Applied; - Total Period Debt Satisfied; - Outstanding
Period Debt; - today's/session collection; - transaction count.
