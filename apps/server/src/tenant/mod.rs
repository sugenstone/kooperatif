//! Cooperative tenant kernel (M1-P0, REQ-024.1/024.3, PD-01/PD-02).
//!
//! Scope of this module in P0:
//! - canonical `cooperatives` + `cooperative_memberships` model;
//! - per-request tenant context resolution (`TenantCtx`) that always
//!   re-validates membership server-side and fails closed;
//! - the `business_enabled` safety gate that keeps not-yet-migrated
//!   cooperatives out of business traffic;
//! - CLI-only provisioning entry points (M1-K7): cooperative creation,
//!   membership grants and the one-time bootstrap of the initial
//!   cooperative.
//!
//! Deliberately NOT here: cooperative-scoped roles/permissions (P1),
//! `cooperative_id` on business tables (P2), RLS policies (P4).

pub mod extractor;
pub mod gate;
pub mod repo;
pub mod routes;

pub use extractor::TenantCtx;
