# 14 --- Reporting and Document Design

## Purpose

Define consistent printable/exportable output behavior.

## Output classes

### Transactional documents

Evidence of a specific authoritative event.

Examples: - Tahsilat Makbuzu - Hisse Devir Belgesi - Hisse İade
Belgesi - approval/decision-linked documents where required.

### Statements

Chronological account/history outputs: - Hissedar Ekstresi - Aile
Ekstresi - Hisse Ekstresi - Finansal Hesap Ekstresi

### Analytical reports

Filtered/aggregated reporting: - Dönem Tahsilat Raporu - Borç Raporu -
Gelir/Gider Raporu - Investment/asset reports - Social Aid reports

## Formats

### PDF

For stable, printable, presentable output.

### XLSX

For analysis and structured data work.

### Print

Browser/print workflow may use the same document rendering semantics
where appropriate.

CSV may be added for technical/data exchange needs but is not the
primary business output.

## Turkish-first output

Default output language/locale: `tr-TR`.

Document labels, dates, numbers and monetary values default to Turkish
conventions.

## Authoritative source rule

A report/document must use authoritative application data.

Never maintain a separate manually edited PDF total.

Where snapshot semantics are legally/operationally required, create an
explicit immutable snapshot model rather than silently freezing rendered
text.

## Receipt semantics

One posted Payment produces one logical Collection Receipt identity
where receipts are enabled.

A receipt can contain many allocations.

Reprinting does not: - create another Payment; - change the original
receipt number; - duplicate cash movement.

If the Payment is reversed, document verification/history must indicate
reversal.

## Document numbering

Official documents may use configurable cooperative-scoped numbering.

Example patterns:

``` text
THS-2030-000001
DEV-2030-000014
IADE-2030-000008
```

Exact prefixes/formats are configurable/specifiable later.

Number generation must be concurrency-safe.

## Document templates

Templates are versioned.

Template can define: - cooperative logo/identity; - header; - footer; -
typography; - signature areas; - labels; - table layout; - optional
verification area.

Historical generated documents should retain which template version
rendered them where required.

## Collection Receipt minimum content

Candidate content: - cooperative identity; - receipt/document number; -
date/time; - payer; - family context where relevant; - payment method; -
destination account where appropriate; - gross received amount; -
allocation breakdown; - operator; - Payment reference; - reversal status
if applicable.

Do not imply payer is necessarily the debtor.

## Family receipt

For one family payment, show one gross receipt followed by
member/share/debt allocation lines.

Do not print multiple cash receipts unless multiple Payments actually
occurred.

## Shareholder Statement

Should support date range and include chronological: - Period
Assessments; - Payments; - Allocations; - Excess effects; - Share events
where relevant; - Receivables/settlements; - reversals/corrections.

Summary may include: - outstanding debt; - excess balance; - open
receivables; - active Share count.

## Family Statement

Aggregates family presentation while preserving each member's
attribution.

Never collapse member excess/debt ownership into an ambiguous shared
family balance.

## Period Collection Report

Must distinguish at minimum: - Period Assessment Total; - Prior Excess
Applied; - New Cash Collected; - Total Period Debt Satisfied; -
Outstanding Period Debt; - payment-method breakdown.

## Filter/export rule

Default export should reflect current filters and selected records.

UI must clearly state export scope, e.g.: -
`Mevcut görünümü dışa aktar — 37 kayıt` - `Seçili 3 kaydı dışa aktar`

Do not unexpectedly export the entire database when the user is viewing
a filtered subset.

## PDF design

Documents should be professional and readable: - consistent margins; -
stable header/footer; - page numbers; - repeated table headers across
pages; - no clipped rows; - clear totals; - sensible whitespace; -
Turkish character support; - print-safe layout.

Avoid decorative dashboard-card design in formal multi-page reports.

## XLSX design

Exports should: - use meaningful Turkish column headings by default; -
preserve machine-usable numeric/date cells; - avoid exporting formatted
currency as text when numeric data is appropriate; - include
filter/report metadata where useful; - use separate sheets when a report
naturally has summary + detail.

## Verification

Selected official documents may support: - verification code; - QR; -
public or authenticated verification endpoint depending on security
decision.

Verification should reveal only approved information.

Possible statuses: - Geçerli - Ters Kayıt Oluşturulmuş - İptal Edilmiş -
Belge Bulunamadı

Do not expose sensitive shareholder data through a public verification
endpoint by default.

## Permissions

Output actions use backend-enforced permissions/scopes.

Candidate permissions: - `report.view` - `report.export_pdf` -
`report.export_xlsx` - `receipt.print` - `document.verify` -
sensitive-report-specific permissions where needed.

A user cannot bypass data scope by exporting.

## Audit

Important exports/documents may be audited, especially: - official
receipt generation; - sensitive shareholder reports; - bulk exports; -
document verification actions where justified.

## Rendering architecture

Document generation must be centralized enough to guarantee consistency.

Do not implement unrelated one-off PDF rendering logic inside every page
component.

The exact rendering technology is an architecture decision.

## Open decisions

-   exact numbering schemes;
-   which documents require immutable snapshots;
-   which documents get QR verification;
-   public vs authenticated verification;
-   signature/e-signature requirements;
-   exact report retention policy.
