//! Kooperatif backend library.
//!
//! Library crate shared by every process binary of the modular monolith
//! (ADR-001). `main.rs` (HTTP API) is the first binary; the future Rust
//! worker of ADR-009 will be added as another binary sharing these modules
//! without duplicating business logic.
//!
//! STEP-001 scope: infrastructure only. No domain, no financial logic.

pub mod config;
pub mod db;
pub mod http;
pub mod observability;
