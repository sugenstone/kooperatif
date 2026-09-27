# ADR-005 --- Real-Time Collection Transport

**Status:** Accepted

## Decision

Use WebSocket for real-time operational updates.

Primary uses: - Collection Session totals; - live debt/status updates
across operators; - Financial Account/session indicators; - TV/projector
live dashboard.

## Authority

WebSocket is transport, not financial truth. PostgreSQL/domain/ledger
state is authoritative.

After reconnect, clients obtain a fresh authoritative snapshot and then
resume events.

## Privacy

TV/projector mode displays aggregated operational information by default
and must not expose unnecessary Shareholder personal/financial details.
