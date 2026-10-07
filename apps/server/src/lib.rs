//! Kooperatif backend library.
//!
//! Library crate shared by every process binary of the modular monolith
//! (ADR-001). `main.rs` (HTTP API) is the first binary; the future Rust
//! worker of ADR-009 will be added as another binary sharing these modules
//! without duplicating business logic.
//!
//! STEP-001: infrastructure baseline. STEP-002: identity, authentication
//! and server-side sessions. No other domain functionality exists yet.

pub mod auth;
pub mod clock;
pub mod config;
pub mod credits;
pub mod db;
pub mod financial_accounts;
pub mod governance;
pub mod http;
pub mod income_expense;
pub mod investments;
pub mod observability;
pub mod parties;
pub mod payments;
pub mod periods;
pub mod reports;
pub mod share_returns;
pub mod shares;
pub mod social_aid;
