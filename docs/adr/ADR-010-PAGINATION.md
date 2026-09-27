# ADR-010 --- API Pagination Strategy

**Status:** Accepted

## Decision

Use standardized **cursor-first pagination** for large, changing
operational datasets such as Payments, ledger movements, audit events,
timelines and notifications.

Offset/page pagination is permitted for genuinely small/stable
administrative datasets.

Filtering and sorting are backend-driven through typed/allowlisted
contracts.

Financial UI must distinguish totals for the complete filtered dataset
from the subset currently loaded on screen.
