# ADR-003 --- Financial Ledger Architecture

**Status:** Accepted

## Decision

Use a **double-entry-inspired immutable ledger** as the authoritative
financial movement architecture.

User-facing screens remain business-oriented and do not require
accounting terminology.

Posted financial entries are immutable. Corrections use
reversal/compensating entries and, where required, replacement
transactions.

## Consequences

Financial Account balances are derived/reconciled from ledger movements.
Transfers, asset purchases, investments, receivables, liabilities and
payments preserve both sides of the economic event. A
mutable-balance-only design is prohibited.
