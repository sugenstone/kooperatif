//! Build dependencies that Cargo cannot infer on its own.
//!
//! `sqlx::migrate!("../../migrations")` (src/db.rs) embeds migration
//! files at compile time, but Cargo only tracks files already seen —
//! adding a NEW migration file invalidates nothing, so a "fresh" stale
//! binary can silently embed an older migration set (observed during
//! the M1-P2-C1 shadow test). Watching the directory forces the crate
//! to rebuild whenever a migration is added or changed.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
