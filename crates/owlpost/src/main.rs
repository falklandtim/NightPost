//! OwlPost — a fast, native email client written in Rust.

mod app;
mod attachments;
mod avatars;
mod composer;
mod contacts;
mod html_view;
mod mail_view;
mod icons;
mod logging;
mod onboarding;
mod prefs;
mod style;
mod web_host;
mod worker;

use anyhow::Result;
use owlpost_core::{mcp, store, theme, Store};

const HELP: &str = "OwlPost — native email client

USAGE:
    owlpost [--demo]              Launch the app (--demo adds sample accounts)
    owlpost mcp [--demo]          Run the built-in MCP server on stdio
    owlpost themes                List the themes
    owlpost themes inspect <dir>  Show how a Mailspring-style theme package resolves
    owlpost --db <path> …         Use a different database (or set OWLPOST_DB)

LOGS:
    OwlPost keeps a log of syncing, push and errors (also shown in Sync activity):
    macOS  ~/Library/Logs/OwlPost/owlpost.log
    Linux  ~/.local/state/owlpost/owlpost.log
Set RUST_LOG=info to see it in the terminal too.

Register with Claude Code:
    claude mcp add owlpost -- /path/to/owlpost mcp
";

/// Linux: is the window a native Wayland one? `OWLPOST_X11=1` runs it
/// through XWayland instead.
#[cfg(target_os = "linux")]
fn linux_wayland() -> bool {
    if std::env::var_os("OWLPOST_X11").is_some_and(|v| v == "1") && std::env::var_os("DISPLAY").is_some() {
        // Before any threads start, so changing the environment is safe.
        std::env::remove_var("WAYLAND_DISPLAY");
        return false;
    }
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn main() -> Result<()> {
    #[cfg(target_os = "linux")]
    let wayland = linux_wayland();
    #[cfg(not(target_os = "linux"))]
    let wayland = false;

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--db") {
        if i + 1 < args.len() {
            std::env::set_var("OWLPOST_DB", &args[i + 1]);
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

    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") | Some("help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("mcp") => {
            let store = Store::open(&store::default_db_path())?;
            if demo {
                owlpost_core::demo::seed(&store)?;
            }
            mcp::Server::new(store).run_stdio()
        }
        Some("themes") => {
            if args.get(1).map(String::as_str) == Some("inspect") {
                let dir = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: owlpost themes inspect <dir>"))?;
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
/// named after the app. Bring SpringPost's across after the rename.
fn migrate_window_settings() {
    let Some(base) = directories::BaseDirs::new() else { return };
    let (old, new) = (base.data_dir().join("SpringPost"), base.data_dir().join("OwlPost"));
    if old.is_dir() && !new.exists() {
        let _ = std::fs::rename(old, new);
    }
}

fn run_gui(demo: bool, wayland: bool) -> Result<()> {
    migrate_window_settings();
    let store = Store::open(&store::default_db_path())?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("OwlPost")
            // Linux: matches the launcher (co.timson.OwlPost.desktop), so the
            // window gets its icon and groups with it in the taskbar.
            .with_app_id("co.timson.OwlPost")
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
        // frame could stall). Wayland never tears, and OwlPost only draws
        // when something changes.
        vsync: !wayland,
        ..Default::default()
    };
    eframe::run_native(
        "OwlPost",
        options,
        Box::new(move |cc| Ok(Box::new(app::OwlPostApp::new(cc, store, demo)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
