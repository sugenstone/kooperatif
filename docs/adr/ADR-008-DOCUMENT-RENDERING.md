# ADR-008 --- Document and Report Rendering

**Status:** Accepted

## Decision

Use a backend-owned centralized document service with typed
document/report models and versioned templates.

PDF and XLSX outputs consume the same authoritative report/domain data
model. Renderers must not independently recalculate financial truth.

Evidentiary/official generated documents may be stored immutably in
object storage with template/document version metadata.

## Technology

The concrete rendering library is intentionally not fixed by this ADR.
Select it through an implementation spike that verifies Turkish
typography, multi-page tables, print reliability, maintenance and
deployment constraints.
