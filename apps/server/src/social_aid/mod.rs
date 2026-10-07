//! Social Aid, donation & restricted fund foundation (STEP-013, docs/11).
//!
//! Social Aid is a SEPARATE financial context (docs/11, docs/15): its
//! donations and aid disbursements never create Payment / Allocation /
//! Shareholder Credit / Income / Expense / Transfer / Share Return /
//! Investment rows. `Financial Account != Fund` is a canonical
//! distinction (docs/01): the account answers WHERE money sits, the
//! fund answers WHAT PURPOSE it is restricted to.

pub mod model;
pub mod repo;
pub mod routes;
