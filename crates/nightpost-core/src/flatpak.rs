//! Running as a Flatpak (docs/flatpak.md).
//!
//! The sandbox keeps NightPost's files under `~/.var/app/co.timson.NightPost/`,
//! apart from a copy installed with `scripts/install-linux.sh`, which uses
//! `~/.local/share/nightpost/`. The first time the Flatpak runs, it copies
//! that copy's database and window settings in, once, so the accounts, mail
//! and preferences carry over. Passwords need no copying: both find them in
//! the same keyring, by address.
//!
//! The copy is made in a side folder and checked by SQLite before it's put
//! in place, so a failed or torn copy (the other NightPost writing while it's
//! copied) is never opened as the real database. After that, the two copies
//! keep separate databases and each syncs with the server.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The app's Flatpak ID, the same as its desktop file and window app ID.
pub const APP_ID: &str = "co.timson.NightPost";

/// Is NightPost running inside a Flatpak? Flatpak sets `FLATPAK_ID` in
/// every sandboxed process.
pub fn running_in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

/// The program and arguments that start NightPost's MCP server from outside
/// the app, for `claude mcp add` or an MCP client's config. Inside a
/// Flatpak, the program's own path (`/app/bin/nightpost`) only exists in the
/// sandbox, so it's `flatpak run` with the app's ID instead.
pub fn mcp_launch(exe: &str, flatpak_id: Option<&str>) -> (String, Vec<String>) {
    match flatpak_id {
        Some(id) => ("flatpak".to_string(), vec!["run".to_string(), id.to_string(), "mcp".to_string()]),
        None => (exe.to_string(), vec!["mcp".to_string()]),
    }
}

/// Where the installed copy keeps its files, and where the Flatpak keeps its own.
#[derive(Debug, Clone)]
pub struct ImportPaths {
    /// The installed copy's database folder.
    pub old_data: PathBuf,
    /// The installed copy's window settings folder (eframe's).
    pub old_settings: PathBuf,
    /// The Flatpak's database folder.
    pub new_data: PathBuf,
    /// The Flatpak's window settings folder.
    pub new_settings: PathBuf,
}

impl ImportPaths {
    /// The usual places for this user. Inside the sandbox `HOME` is still the
    /// real home, while `XDG_DATA_HOME` points into `~/.var/app/…`, so the
    /// installed copy's folders are found from `HOME`. Its settings folder is
    /// named after the window's app ID, as eframe names it.
    pub fn for_this_user() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let base = directories::BaseDirs::new()?;
        let old = home.join(".local/share");
        Some(Self {
            old_data: old.join("nightpost"),
            old_settings: old.join("co.timson.nightpost"),
            new_data: base.data_dir().join("nightpost"),
            new_settings: base.data_dir().join("co.timson.nightpost"),
        })
    }
}

/// Copy the installed copy's database and window settings into the
/// Flatpak's own folders, once. Returns whether it copied anything: nothing
/// when the Flatpak already has a database (so this only ever happens on
/// its first run) or when there's no installed copy. An error leaves the
/// Flatpak's folders as they were, so it starts empty.
pub fn import_installed_copy(p: &ImportPaths) -> Result<bool> {
    let new_db = p.new_data.join("nightpost.db");
    let old_db = p.old_data.join("nightpost.db");
    if new_db.exists() || !old_db.is_file() {
        return Ok(false);
    }
    // A side folder per process, in case the app and the MCP server both
    // start first.
    let side = p.new_data.with_file_name(format!("nightpost-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&side);
    let result = copy_and_check(&p.old_data, &side);
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&side);
        return Err(e);
    }
    std::fs::create_dir_all(&p.new_data).with_context(|| format!("making {}", p.new_data.display()))?;
    // The other process may have finished first: keep its copy.
    if new_db.exists() {
        let _ = std::fs::remove_dir_all(&side);
        return Ok(false);
    }
    std::fs::rename(side.join("nightpost.db"), &new_db).context("putting the copied database in place")?;
    let _ = std::fs::remove_dir_all(&side);
    // Window size, theme and list options. Not having them is no loss.
    let (old_ron, new_ron) = (p.old_settings.join("app.ron"), p.new_settings.join("app.ron"));
    if old_ron.is_file() && !new_ron.exists() && std::fs::create_dir_all(&p.new_settings).is_ok() {
        let _ = std::fs::copy(&old_ron, &new_ron);
    }
    Ok(true)
}

/// Copy the database and its write-ahead log (which holds the newest
/// changes) into `side`, then have SQLite check the copy. Opening it folds
/// the log into the database, so one self-contained file is left. The
/// shared-memory file isn't copied: SQLite rebuilds it from the log.
fn copy_and_check(old: &Path, side: &Path) -> Result<()> {
    std::fs::create_dir_all(side).with_context(|| format!("making {}", side.display()))?;
    for ext in ["db", "db-wal"] {
        let from = old.join(format!("nightpost.{ext}"));
        if from.is_file() {
            std::fs::copy(&from, side.join(format!("nightpost.{ext}"))).with_context(|| format!("copying {}", from.display()))?;
        }
    }
    let db = side.join("nightpost.db");
    let conn = rusqlite::Connection::open(&db).context("opening the copied database")?;
    let verdict: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).context("checking the copied database")?;
    if verdict != "ok" {
        bail!("the copy of the installed NightPost's database is damaged ({verdict}); quit that NightPost and start this one again");
    }
    // Fold the log in and close, so only the database file is left.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").context("finishing the copied database")?;
    drop(conn);
    for ext in ["db-wal", "db-shm"] {
        let _ = std::fs::remove_file(side.join(format!("nightpost.{ext}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{demo, Store};

    /// Made-up folders for an "installed copy" and a "Flatpak", under the
    /// temp folder. The installed copy holds the demo accounts.
    fn setup(name: &str) -> (ImportPaths, PathBuf) {
        let root = std::env::temp_dir().join(format!("np-flatpak-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = ImportPaths {
            old_data: root.join("old/nightpost"),
            old_settings: root.join("old/co.timson.nightpost"),
            new_data: root.join("new/nightpost"),
            new_settings: root.join("new/co.timson.nightpost"),
        };
        std::fs::create_dir_all(&p.old_data).unwrap();
        std::fs::create_dir_all(&p.old_settings).unwrap();
        std::fs::write(p.old_settings.join("app.ron"), "(theme: \"moonlit-dark\")").unwrap();
        (p, root)
    }

    #[test]
    fn copies_the_installed_copy_once() {
        let (p, root) = setup("once");
        // The installed copy is still open, so its newest changes are in
        // the write-ahead log, not yet in the database file.
        let old = Store::open(&p.old_data.join("nightpost.db")).unwrap();
        demo::seed(&old).unwrap();
        let accounts = old.accounts().unwrap().len();
        assert!(accounts > 0);

        assert!(import_installed_copy(&p).unwrap(), "the first run copies");
        drop(old);
        let new = Store::open(&p.new_data.join("nightpost.db")).unwrap();
        assert_eq!(new.accounts().unwrap().len(), accounts, "accounts and mail come across, including what was in the log");
        assert_eq!(std::fs::read_to_string(p.new_settings.join("app.ron")).unwrap(), "(theme: \"moonlit-dark\")");
        let leftovers: Vec<_> = std::fs::read_dir(root.join("new")).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert!(leftovers.iter().all(|n| !n.to_string_lossy().starts_with("nightpost-import")), "no side folder left: {leftovers:?}");

        // Never again, even after the Flatpak's own changes.
        let mut extra = new.accounts().unwrap()[0].clone();
        extra.email = "added-in-flatpak@example.com".into();
        new.add_account(&extra).unwrap();
        drop(new);
        assert!(!import_installed_copy(&p).unwrap());
        assert_eq!(Store::open(&p.new_data.join("nightpost.db")).unwrap().accounts().unwrap().len(), accounts + 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nothing_to_copy_without_an_installed_copy() {
        let (p, root) = setup("none");
        assert!(!import_installed_copy(&p).unwrap());
        assert!(!p.new_data.join("nightpost.db").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// A damaged copy is thrown away, and the Flatpak starts empty.
    #[test]
    fn a_damaged_copy_is_not_used() {
        let (p, root) = setup("damaged");
        std::fs::write(p.old_data.join("nightpost.db"), b"SQLite format 3\0 but not really a database").unwrap();
        assert!(import_installed_copy(&p).is_err());
        assert!(!p.new_data.join("nightpost.db").exists(), "no half database to open");
        assert!(std::fs::read_dir(root.join("new")).map(|d| d.count()).unwrap_or(0) <= 1, "no side folder left");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_mcp_command_works_from_outside_the_sandbox() {
        assert_eq!(mcp_launch("/home/tim/.local/bin/nightpost", None), ("/home/tim/.local/bin/nightpost".to_string(), vec!["mcp".to_string()]));
        assert_eq!(mcp_launch("/app/bin/nightpost", Some(APP_ID)), ("flatpak".to_string(), vec!["run".to_string(), APP_ID.to_string(), "mcp".to_string()]));
    }
}
