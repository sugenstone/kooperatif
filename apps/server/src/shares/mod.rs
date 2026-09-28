//! Shares / Hisse domain (STEP-005): first-class Share entities with
//! temporal ownership (`share_ownerships`) and append-oriented business
//! history (`share_events`). Acquisition fees and sale prices are
//! recorded as agreed-value metadata only — they never create Payment,
//! Cashbox, or Ledger state (docs/04, STEP-005 §18–§19).

pub mod model;
pub mod repo;
pub mod routes;
