//! Shareholder Credit / "Fazla Ödeme" domain (STEP-009).
//!
//! A Shareholder Credit is an Excess Payment Balance attributable to
//! exactly ONE explicitly chosen Shareholder — backed only by a posted
//! Payment's unassigned remainder. A Credit Application settles an
//! Assessment from held credit without being a Payment and without
//! creating an Account Movement. Balances are always derived; all
//! corrections are status-based with actor/time/reason.
//!
//! Deliberately absent: refund/disbursement, credit transfer between
//! shareholders, family-owned credit, interest/indexation, expiry.

pub mod model;
pub mod repo;
pub mod routes;
