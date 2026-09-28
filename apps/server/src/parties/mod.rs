//! Parties domain: persons, shareholders, guardians, families and
//! temporal family memberships (STEP-004, docs/04).
//!
//! Boundaries: Person ≠ User ≠ Shareholder; Guardian is a person
//! reference that may or may not be a Shareholder; Family groups
//! shareholders and is NOT a financial account. No Share, payment,
//! ledger or Social Aid concepts exist here.

pub mod model;
pub mod repo;
pub mod routes;
