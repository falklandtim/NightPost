//! NightPost — a fast, native email client written in Rust.

mod app;
mod attachments;
mod avatars;
mod composer;
mod contacts;
mod html_view;
mod mail_view;
mod icons;
mod logging;
#[cfg(target_os = "macos")]
mod mac_web;
mod onboarding;
mod prefs;
mod style;
mod web_host;
mod worker;

use anyhow::Result;
use nightpost_core::{mcp, store, theme, Store};

const HELP: &str = "NightPost — native email client

USAGE:
    nightpost [--demo]              Launch the app (--demo adds sample accounts)
    nightpost mcp [--demo]          Run the built-in MCP server on stdio
    nightpost themes                List the themes
    nightpost themes inspect <dir>  Show how a Mailspring-style theme package resolves
    nightpost --db <path> …         Use a different database (or set NIGHTPOST_DB)

LOGS:
    NightPost keeps a log of syncing, push and errors (also shown in Sync activity):
    macOS  ~/Library/Logs/NightPost/nightpost.log
    Linux  ~/.local/state/nightpost/nightpost.log
Set RUST_LOG=info to see it in the terminal too.

Register with Claude Code:
    claude mcp add nightpost -- /path/to/nightpost mcp
";

/// Linux: is the window a native Wayland one? `NIGHTPOST_X11=1` runs it
/// through XWayland instead.
#[cfg(target_os = "linux")]
fn linux_wayland() -> bool {
    if std::env::var_os("NIGHTPOST_X11").is_some_and(|v| v == "1") && std::env::var_os("DISPLAY").is_some() {
        // Before any threads start, so changing the environment is safe.
        std::env::remove_var("WAYLAND_DISPLAY");
        return false;
    }
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn main() -> Result<()> {
    // Before anything reads the environment, or starts a thread.
    nightpost_core::carry_legacy_env();
    #[cfg(target_os = "linux")]
    let wayland = linux_wayland();
    #[cfg(not(target_os = "linux"))]
    let wayland = false;

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--db") {
        if i + 1 < args.len() {
            std::env::set_var("NIGHTPOST_DB", &args[i + 1]);
            args.drain(i..=i + 1);
        }
    }
    let demo = args.iter().any(|a| a == "--demo");
    args.retain(|a| a != "--demo");
    // Logs go to the log file, and to stderr (never stdout, the MCP stream).
    logging::init(match args.first().map(String::as_str) {
        Some("mcp") => "mcp",
        Some(_) => "cli",
        None => "app",
    });

    // A Flatpak's first run brings the installed copy's mail and settings
    // across, once. Never for a demo (demo runs mustn't read real mail) or a
    // database chosen with --db.
    if nightpost_core::flatpak::running_in_flatpak() && !demo && std::env::var_os("NIGHTPOST_DB").is_none() {
        if let Some(paths) = nightpost_core::flatpak::ImportPaths::for_this_user() {
            match nightpost_core::flatpak::import_installed_copy(&paths) {
                Ok(true) => log::info!("copied the installed NightPost's mail and settings into the Flatpak"),
                Ok(false) => {}
                Err(e) => log::warn!("couldn't copy the installed NightPost's mail into the Flatpak, so it starts empty: {e:#}"),
            }
        }
    }

    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") | Some("help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("mcp") => {
            let store = Store::open(&store::default_db_path())?;
            if demo {
                nightpost_core::demo::seed(&store)?;
            }
            mcp::Server::new(store).run_stdio()
        }
        Some("themes") => {
            if args.get(1).map(String::as_str) == Some("inspect") {
                let dir = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: nightpost themes inspect <dir>"))?;
                let t = theme::Theme::load_package(std::path::Path::new(dir), theme::ThemeSource::User)?;
                println!("{} ({}) — {} variables declared", t.display_name, t.id, t.overrides.len());
                for (role, hex) in t.palette().entries() {
                    println!("  {role:<22} {hex}");
                }
                return Ok(());
            }
            for t in theme::discover() {
                let p = t.palette();
                println!(
                    "{:<28} {:<22} {:?}  accent {}  bg {}  {}",
                    t.id,
                    t.display_name,
                    t.source,
                    p.accent.to_hex(),
                    p.background.to_hex(),
                    if p.dark { "dark" } else { "light" }
                );
            }
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n\n{HELP}");
            std::process::exit(2);
        }
        None => run_gui(demo, wayland),
    }
}

/// The app icon. macOS shows it in the Dock, so it gets the padded artwork
/// that lines up with other apps; elsewhere it's the window icon.
fn app_icon() -> std::sync::Arc<eframe::egui::IconData> {
    let png: &[u8] = if cfg!(target_os = "macos") {
        include_bytes!("../assets/icon-macos-1024.png")
    } else {
        include_bytes!("../assets/icon-1024.png")
    };
    let icon = image::load_from_memory(png)
        .map(|img| {
            let rgba = img.to_rgba8();
            eframe::egui::IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
        })
        .unwrap_or_default();
    std::sync::Arc::new(icon)
}

/// Window size and settings (theme, list options) live in eframe's own folder,
/// named after the app. Bring the newest old one across after a rename
/// (OwlPost, or SpringPost before that). The keys inside keep their old
/// names; `NightPostApp::new` reads those as a fallback.
fn migrate_window_settings() {
    let Some(base) = directories::BaseDirs::new() else { return };
    let new = base.data_dir().join("NightPost");
    if new.exists() {
        return;
    }
    for name in ["OwlPost", "SpringPost"] {
        let old = base.data_dir().join(name);
        if old.is_dir() {
            // Copy, not move: an older OwlPost may still run on this machine.
            if copy_dir(&old, &new).is_ok() {
                log::info!("copied window settings from {}", old.display());
            }
            return;
        }
    }
}

/// Copy a folder of plain files (eframe's settings folder has no subfolders
/// worth keeping), creating `to`.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn run_gui(demo: bool, wayland: bool) -> Result<()> {
    migrate_window_settings();
    let store = Store::open(&store::default_db_path())?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("NightPost")
            // Linux: matches the launcher (co.timson.NightPost.desktop), so the
            // window gets its icon and groups with it in the taskbar.
            .with_app_id("co.timson.NightPost")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0])
            // On macOS the toolbar sits in the title bar, beside the traffic lights.
            .with_fullsize_content_view(cfg!(target_os = "macos"))
            .with_titlebar_shown(!cfg!(target_os = "macos"))
            .with_title_shown(!cfg!(target_os = "macos"))
            .with_icon(app_icon()),
        // Wayland: don't wait for the compositor's frame callback before
        // drawing. While waiting, the window couldn't answer the compositor's
        // "are you alive?" check (KDE showed Not Responding, and the first
        // frame could stall). Wayland never tears, and NightPost only draws
        // when something changes.
        vsync: !wayland,
        ..Default::default()
    };
    eframe::run_native(
        "NightPost",
        options,
        Box::new(move |cc| Ok(Box::new(app::NightPostApp::new(cc, store, demo)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
