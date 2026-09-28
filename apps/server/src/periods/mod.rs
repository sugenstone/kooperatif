//! Periods & Assessments domain (STEP-006): first-class Period / Dönem
//! entities, an explicit durable Assessment Rule per Period, and durable
//! obligation records (`assessments`) with per-Share provenance
//! (`assessment_share_sources`). Assessments are obligations only — no
//! Payment/Cashbox/Ledger state exists here (§42–§44, docs/05).

pub mod model;
pub mod repo;
pub mod routes;
