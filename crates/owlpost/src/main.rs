//! OwlPost — a fast, native email client written in Rust.

mod app;
mod attachments;
mod composer;
mod html_view;
mod mail_view;
mod icons;
mod onboarding;
mod prefs;
mod style;
mod worker;

use anyhow::Result;
use owlpost_core::{mcp, store, theme, Store};

const HELP: &str = "OwlPost — native email client

USAGE:
    owlpost [--demo]              Launch the app (--demo adds sample accounts)
    owlpost mcp [--demo]          Run the built-in MCP server on stdio
    owlpost themes                List available themes (incl. Mailspring themes)
    owlpost themes install <dir>  Install a Mailspring theme package
    owlpost themes inspect <dir>  Show how a theme package resolves
    owlpost --db <path> …         Use a different database (or set OWLPOST_DB)

Register with Claude Code:
    claude mcp add owlpost -- /path/to/owlpost mcp
";

fn main() -> Result<()> {
    // Logs go to stderr so they never corrupt the MCP stdio stream.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .init();

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--db") {
        if i + 1 < args.len() {
            std::env::set_var("OWLPOST_DB", &args[i + 1]);
            args.drain(i..=i + 1);
        }
    }
    let demo = args.iter().any(|a| a == "--demo");
    args.retain(|a| a != "--demo");

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
            if args.get(1).map(String::as_str) == Some("install") {
                let dir = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: owlpost themes install <dir>"))?;
                let t = theme::install(std::path::Path::new(dir))?;
                println!("Installed {} ({}) — pick it in Preferences → Appearance", t.display_name, t.id);
                return Ok(());
            }
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
        None => run_gui(demo),
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

fn run_gui(demo: bool) -> Result<()> {
    migrate_window_settings();
    let store = Store::open(&store::default_db_path())?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("OwlPost")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0])
            // On macOS the toolbar sits in the title bar, beside the traffic lights.
            .with_fullsize_content_view(cfg!(target_os = "macos"))
            .with_titlebar_shown(!cfg!(target_os = "macos"))
            .with_title_shown(!cfg!(target_os = "macos"))
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "OwlPost",
        options,
        Box::new(move |cc| Ok(Box::new(app::OwlPostApp::new(cc, store, demo)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
