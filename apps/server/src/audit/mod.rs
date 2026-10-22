//! Audit read surface (M1-P5 B4, discovery: M1-P5-B4-AUDIT-READ-DESIGN).
//!
//! Two lanes, both credential-bound and database-enforced:
//!
//! - `GET /api/audit/events` — cooperative lane (`BusinessAuth`,
//!   `audit.read` in the verified scope). Rows are hard-restricted to
//!   the verified cooperative inside `app_audit_read_scoped`.
//! - `GET /api/platform/audit/events` — platform lane (decision D4).
//!   Runs OUTSIDE the business membership gate, same as
//!   `/api/platform/users/*`: `app_audit_read_platform` re-verifies a
//!   NULL-scope `audit.read` grant inside the database. A
//!   cooperative-scoped grant can never reach this lane.
//!
//! Reads never mutate: both definers are STABLE, return an explicit
//! column allowlist (no session_id), and `kooperatif_app` still holds
//! zero direct privileges on `security_events`. Metadata passes a
//! per-event-type key allowlist (D1) — unknown event types degrade to
//! an empty object.

pub mod routes;
