//! Reporting & traceable analytics foundation (STEP-015, docs/14,
//! docs/15 §reporting invariants, docs/17).
//!
//! Reporting is a READ-ONLY projection layer: every endpoint derives
//! its metrics from the authoritative domain tables at query time and
//! no report request ever mutates domain state. There is deliberately
//! NO second financial truth — balances, debts, credits and restricted
//! availability reuse the exact canonical derivations of their owning
//! modules (ADR-003/ADR-004).
//!
//! Boundaries held by construction:
//!   * every cash number is a SUM over `account_movements` (never a
//!     stored/edited column);
//!   * assessment remaining = amount − active payment allocations −
//!     active credit applications (the canonical settled formula);
//!   * credit, entitlement, valuation and restricted-fund figures are
//!     classifications of state, never additional money;
//!   * reversed/cancelled records contribute nothing to current totals
//!     while remaining traceable in drill-downs;
//!   * NULL is preserved as NULL (an undetermined Profit Right is never
//!     rendered as 0.00).
//!
//! Explicitly NOT implemented (authoritative policy undefined, docs/17):
//! Net Worth, NAV, Profit/Loss, Balance Sheet, realized/unrealized
//! gain, shareholder netting, "free cash", as-of point-in-time history.

pub mod repo;
pub mod routes;
