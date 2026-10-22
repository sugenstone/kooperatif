//! Regression guard for the embedded sqlx migration set.
//!
//! `sqlx::migrate!` embeds `migrations/` at COMPILE time; without
//! `build.rs` watching the directory, a fresh-looking stale binary ran
//! an older migration set during the M1-P2-C1 shadow test. This test
//! compares the embedded set against the filesystem so any future
//! staleness fails loudly instead of silently skipping migrations.

use std::path::Path;

/// Number of `*.sql` migration files on disk for this package.
fn migration_file_count() -> usize {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter(|e| {
            e.as_ref()
                .map(|e| e.path().extension().is_some_and(|x| x == "sql"))
                .unwrap_or(false)
        })
        .count()
}

#[test]
fn embedded_migrations_cover_every_file_on_disk() {
    let migrator = sqlx::migrate!("../../migrations");
    let embedded = migrator.migrations.len();
    let on_disk = migration_file_count();
    assert_eq!(
        embedded, on_disk,
        "embedded migration set ({embedded}) diverges from migrations/ ({on_disk}) — \
         stale build; rebuild the kooperatif-server crate"
    );
}
