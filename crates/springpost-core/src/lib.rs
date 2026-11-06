//! SpringPost core: models, local store, IMAP/SMTP engine, snooze,
//! Mailspring-compatible themes and the built-in MCP server.

pub mod credentials;
pub mod demo;
pub mod engine;
pub mod mcp;
pub mod models;
pub mod parse;
pub mod presets;
pub mod smtp;
pub mod snooze;
pub mod store;
pub mod theme;

pub use models::*;
pub use store::Store;

/// A fresh, unique temp database path for tests (timestamps alone collide
/// on macOS, whose clock only has microsecond resolution).
#[cfg(test)]
pub(crate) fn test_db_path(prefix: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("{prefix}-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}
