//! Income & Expense management (STEP-010, docs/07).
//!
//! Income and Expense are BUSINESS EVENTS: each posted entry is bound
//! 1:1 to exactly one authoritative `account_movements` row. They are
//! economic classification, never a second balance source — account
//! balances stay movement-derived, and Payments / Transfers /
//! Shareholder Credits are never Income or Expense.

pub mod model;
pub mod repo;
pub mod routes;
