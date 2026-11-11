//! OwlPost core: models, local store, IMAP/SMTP engine, snooze,
//! Mailspring-compatible themes and the built-in MCP server.

pub mod activity;
pub mod category;
pub mod credentials;
pub mod demo;
pub mod email_view;
pub mod engine;
pub mod html_clean;
pub mod idle;
pub mod mcp;
pub mod oauth;
pub mod models;
pub mod parse;
pub mod presets;
pub mod smtp;
pub mod snooze;
pub mod snooze_sync;
pub mod store;
pub mod theme;

pub use models::*;
pub use store::Store;

/// Folders chosen by the app that embeds the core (see [`use_folders`]).
struct Folders {
    data: std::path::PathBuf,
    cache: std::path::PathBuf,
}

static FOLDERS: std::sync::OnceLock<Folders> = std::sync::OnceLock::new();

/// Keep the database in `data` and downloaded files in `cache`, instead of
/// the usual per-user folders. For the iPhone app, whose sandbox decides
/// where files go. Call it once, before opening the store; later calls
/// return an error and change nothing, so the database can't move under a
/// running app.
pub fn use_folders(data: std::path::PathBuf, cache: std::path::PathBuf) -> anyhow::Result<()> {
    FOLDERS.set(Folders { data, cache }).map_err(|_| anyhow::anyhow!("OwlPost's folders are already set"))
}

/// The cache folder chosen with [`use_folders`], if any.
pub(crate) fn chosen_cache_dir() -> Option<std::path::PathBuf> {
    FOLDERS.get().map(|f| f.cache.clone())
}

/// OwlPost's data folder (database and installed themes), e.g.
/// `~/Library/Application Support/co.timson.OwlPost`.
///
/// OwlPost used to be called SpringPost: the first time it runs, the old
/// folder is renamed so accounts, mail and themes carry over.
pub fn data_dir() -> std::path::PathBuf {
    if let Some(dir) = FOLDERS.get().map(|f| f.data.clone()) {
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    let Some(dirs) = directories::ProjectDirs::from("co", "timson", "OwlPost") else {
        return std::path::PathBuf::from(".");
    };
    let dir = dirs.data_dir().to_path_buf();
    if !dir.exists() {
        if let Some(old) = directories::ProjectDirs::from("co", "timson", "SpringPost").map(|d| d.data_dir().to_path_buf()) {
            if old.is_dir() && std::fs::rename(&old, &dir).is_ok() {
                for ext in ["db", "db-wal", "db-shm"] {
                    let _ = std::fs::rename(dir.join(format!("springpost.{ext}")), dir.join(format!("owlpost.{ext}")));
                }
                log::info!("moved {} to {}", old.display(), dir.display());
            }
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// OwlPost's log file: `~/Library/Logs/OwlPost/owlpost.log` on a Mac,
/// `~/.local/state/owlpost/owlpost.log` (or under `$XDG_STATE_HOME`) on Linux.
/// `OWLPOST_LOG_FILE` overrides it.
pub fn log_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("OWLPOST_LOG_FILE") {
        return p.into();
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| ".".into());
    if cfg!(target_os = "macos") {
        return home.join("Library/Logs/OwlPost/owlpost.log");
    }
    let state = std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/state"));
    state.join("owlpost/owlpost.log")
}

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
