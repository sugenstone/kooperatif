//! Financial Accounts domain (STEP-008).
//!
//! ONE balance engine behind two operational shapes (`cash` / `bank`):
//! where cooperative-held value is kept. Account Movements are the
//! immutable financial history that derives every balance (ADR-003) —
//! produced only by domain commands (Payment posting, Transfer), never
//! by arbitrary operator writes. Transfers are ONE logical move with
//! two paired legs, atomic and idempotent.
//!
//! Deliberately absent: General Ledger / chart of accounts, Income,
//! Expense, Investment, Social Aid funds, Receipt, bank
//! reconciliation, opening-balance migration, gold/commodity units,
//! cross-currency exchange.

pub mod model;
pub mod repo;
pub mod routes;
