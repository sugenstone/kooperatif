//! Governance, decisions & voting foundation (STEP-014, docs/09).
//!
//! Governance is EVIDENCE, not money: bodies, memberships, decisions
//! and votes record WHO decided WHAT — no command writes
//! account_movements or touches a financial aggregate.
//!
//! `RBAC != GOVERNANCE != APPROVAL` (docs/18): `governance.read/manage`
//! grant API operation only; membership is a `persons` fact; business
//! approval policy (thresholds, which action needs which body) is an
//! open decision in docs/09 and is deliberately NOT implemented —
//! nothing is fabricated and no financial command is gated.

pub mod model;
pub mod repo;
pub mod routes;
