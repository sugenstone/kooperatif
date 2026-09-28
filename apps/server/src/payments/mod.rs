//! Payments / Tahsilat domain (STEP-007).
//!
//! Durable records of money actually RECEIVED: the `posted` Payment
//! (one receipt = one row, immutable, idempotency-keyed) and its
//! Allocation lines applying received value to Assessments. Payer is a
//! Person — never implicitly the debtor; a Family is never a debtor.
//!
//! Deliberately absent: Cashbox, Bank Account, Ledger, Receipt,
//! Collection Session and refund/disbursement domains.

pub mod model;
pub mod repo;
pub mod routes;
