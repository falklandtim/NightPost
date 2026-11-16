//! Keeping attachments from running unchecked.
//!
//! Files from email can be harmful: an `.app` inside a `.zip`, or a
//! `.terminal` or `.fileloc` that launches something. Two guards, used by
//! the app and the MCP server alike:
//!
//! - [`quarantine`] marks a downloaded attachment the way a browser marks a
//!   download (macOS's `com.apple.quarantine`), so Gatekeeper checks it, and
//!   anything unzipped from it, before it runs. The app bundle also sets
//!   `LSFileQuarantineEnabled` (scripts/bundle-macos.sh); this covers
//!   `nightpost mcp`, which runs outside the bundle. Linux has no
//!   equivalent.
//! - [`risky_extension`] names the kinds of file the app asks about before
//!   opening (programs, scripts, installers, archives, documents with
//!   macros).

/// Extensions of files that run something, or may hold something that does.
const RISKY: &[(&str, &str)] = &[
    ("app", "program"),
    ("command", "script"),
    ("terminal", "Terminal file"),
    ("tool", "script"),
    ("sh", "script"),
    ("zsh", "script"),
    ("pkg", "installer"),
    ("mpkg", "installer"),
    ("dmg", "disk image"),
    ("zip", "archive"),
    ("tar", "archive"),
    ("gz", "archive"),
    ("tgz", "archive"),
    ("7z", "archive"),
    ("rar", "archive"),
    ("jar", "program"),
    ("fileloc", "link to a file"),
    ("inetloc", "web link"),
    ("webloc", "web link"),
    ("scpt", "script"),
    ("scptd", "script"),
    ("workflow", "Automator workflow"),
    ("action", "Automator action"),
    ("desktop", "launcher"),
    ("exe", "program"),
    ("msi", "installer"),
    ("bat", "script"),
    ("cmd", "script"),
    ("js", "script"),
    ("vbs", "script"),
    ("ps1", "script"),
    ("docm", "document with macros"),
    ("xlsm", "spreadsheet with macros"),
    ("pptm", "presentation with macros"),
];

/// What kind of risky file `name` is ("archive", "program"…), or `None`.
/// Goes by the last extension, so `report.pdf.app` is a program. Trailing
/// dots and spaces are ignored (`setup.exe .` is still a program).
pub fn risky_kind(name: &str) -> Option<&'static str> {
    let name = name.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let (_, ext) = name.rsplit_once('.')?;
    RISKY.iter().find(|(e, _)| e.eq_ignore_ascii_case(ext.trim())).map(|(_, kind)| *kind)
}

/// Should the app ask before opening an attachment called `name`?
pub fn risky_extension(name: &str) -> bool {
    risky_kind(name).is_some()
}

/// Mark a downloaded attachment as from the internet (macOS), so Gatekeeper
/// checks it before it runs. A failure is logged, without the file's name,
/// and otherwise ignored: the download itself still worked.
pub fn quarantine(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        // Flags 0081: downloaded, not yet opened. Then the time (hex) and the
        // agent's name, as Safari writes it.
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let value = format!("0081;{secs:x};NightPost;");
        let ok = std::process::Command::new("/usr/bin/xattr").args(["-w", "com.apple.quarantine", &value]).arg(path).status().is_ok_and(|s| s.success());
        if !ok {
            log::warn!("couldn't mark an attachment as downloaded from the internet");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_files_are_named() {
        for name in ["invoice.zip", "INVOICE.ZIP", "report.pdf.app", "run.command", "setup.exe .", "Budget.XLSM", "go.desktop", "a.tar.gz"] {
            assert!(risky_extension(name), "{name}");
        }
        for name in ["receipt.pdf", "photo.jpeg", "zip", "notes", "", ".zip-notes", "archive.zipx"] {
            assert!(!risky_extension(name), "{name}");
        }
        assert_eq!(risky_kind("invoice.zip"), Some("archive"));
        assert_eq!(risky_kind("report.pdf.app"), Some("program"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quarantine_marks_the_file() {
        let path = std::env::temp_dir().join(format!("np-quarantine-{}.zip", std::process::id()));
        std::fs::write(&path, b"PK").unwrap();
        quarantine(&path);
        let out = std::process::Command::new("/usr/bin/xattr").args(["-p", "com.apple.quarantine"]).arg(&path).output().unwrap();
        let _ = std::fs::remove_file(&path);
        let value = String::from_utf8_lossy(&out.stdout);
        assert!(value.starts_with("0081;") && value.trim_end().ends_with(";NightPost;"), "{value:?}");
    }
}
