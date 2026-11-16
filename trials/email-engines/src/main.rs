//! The email engine trial: draws a corpus of made-up emails with each HTML
//! engine built in, measures them, checks NightPost's privacy promises, and
//! writes a report Tim can score. Not part of the app; see
//! `docs/tasks/email-engine-trial.md` and `docs/gtk4-upgrade.md`.
//!
//! Commands:
//! - `list`: the corpus and the engines built in.
//! - `make-corpus`: rewrite the corpus's `.eml` files.
//! - `prepare`: write every page to `out/pages/` for the Servo build.
//! - `render [--engine E] [--only TEXT]`: draw every email at 600 and 900
//!   points, light and dark, into `out/<engine>/`.
//! - `check [--engine E]`: the must-pass checks, into `out/<engine>/checks.json`.
//! - `report`: build `out/report.html` from whatever is in `out/`.
//! - `render-one …`: one drawing in this process (used by `check` to run
//!   risky cases in a child process).

mod checks;
mod corpus_gen;
mod engine;
mod engines;
mod images;
mod loader;
mod measure;
mod page;
#[cfg(not(feature = "servo"))]
mod page_build;
mod report;
mod server;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use engine::{Engine, RenderOpts};
use server::Server;

/// The widths drawn, in points.
pub const WIDTHS: [u32; 2] = [600, 900];
/// Pixels per point.
pub const SCALE: f32 = 2.0;
/// How long one drawing may take.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// One drawing, as a line of `results.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub engine: String,
    pub email: String,
    pub width: u32,
    pub theme: String,
    pub remote_images: bool,
    /// The first drawing since the engine started.
    pub cold: bool,
    pub first_draw_ms: Option<f64>,
    pub height_pt: Option<f32>,
    /// What the engine asked for (through the loader, or its `cid:` handler).
    pub requests: Vec<String>,
    /// What reached the local "remote" server during this drawing.
    pub server: Vec<String>,
    pub notes: Vec<String>,
    pub error: Option<String>,
    /// The picture, relative to `out/`.
    pub png: Option<String>,
}

/// The trial's folder (`trials/email-engines`), which holds `corpus/` and
/// `out/`. Servo is built from `servo-build/` inside it (its copy of Stylo
/// can't share a lock file with Blitz's), so step out of that.
pub fn root() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if dir.ends_with("servo-build") {
        dir.parent().unwrap_or(dir).to_path_buf()
    } else {
        dir.to_path_buf()
    }
}

/// `out/`, in the trial's folder.
pub fn out_dir() -> PathBuf {
    root().join("out")
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    match args.first().map(String::as_str) {
        Some("list") => {
            println!("engines built in: {:?}", engine::available());
            for name in page::list()? {
                println!("{name}");
            }
            Ok(())
        }
        Some("make-corpus") => corpus_gen::make(),
        #[cfg(not(feature = "servo"))]
        Some("prepare") => page_build::prepare_all(),
        Some("render") => render_all(&engines_wanted(flag("--engine"))?, flag("--only").as_deref()),
        Some("check") => {
            for name in engines_wanted(flag("--engine"))? {
                checks::run(&name)?;
            }
            Ok(())
        }
        Some("report") => report::write(),
        Some("render-one") => render_one_cli(&flag),
        _ => bail!("usage: email-engines list | make-corpus | render [--engine E] [--only TEXT] | check [--engine E] | report"),
    }
}

fn engines_wanted(asked: Option<String>) -> Result<Vec<String>> {
    let built: Vec<String> = engine::available().into_iter().map(String::from).collect();
    match asked {
        Some(e) if built.contains(&e) => Ok(vec![e]),
        Some(e) => bail!("{e} isn't built in (built in: {built:?}); add --features {e}"),
        None if built.is_empty() => bail!("no engine built in: run with --features <engine>"),
        None => Ok(built),
    }
}

/// Draw the corpus with each engine. The broken emails (13…) are left to
/// `check`, which runs each in a child process so a crash can't end the run.
fn render_all(engines: &[String], only: Option<&str>) -> Result<()> {
    let server = Server::start(images::all()?)?;
    for name in engines {
        let dir = out_dir().join(name);
        std::fs::create_dir_all(&dir)?;
        let mut eng = engine::create(name)?;
        let mut lines = Vec::new();
        let mut cold = true;
        for email in page::list()? {
            if email.starts_with("13") || only.is_some_and(|o| !email.contains(o)) {
                continue;
            }
            for width in WIDTHS {
                for dark in [false, true] {
                    // Pictures on, as after "Load images", so the pictures
                    // compare like for like; privacy is tested by `check`.
                    let o = render_once(eng.as_mut(), &server, &email, width, dark, true, false, cold)?;
                    cold = false;
                    println!("{name} {email} {width} {}: {}", o.theme, o.error.as_deref().map(|e| format!("error: {e}")).unwrap_or_else(|| format!("{:.0} ms", o.first_draw_ms.unwrap_or(0.0))));
                    lines.push(serde_json::to_string(&o)?);
                }
            }
        }
        let path = dir.join("results.jsonl");
        if only.is_some() {
            // A partial run keeps the other emails' lines.
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            let keep: Vec<&str> = old.lines().filter(|l| !lines.iter().any(|n| same_case(l, n))).collect();
            lines.splice(0..0, keep.into_iter().map(String::from));
        }
        std::fs::write(&path, lines.join("\n") + "\n")?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn same_case(a: &str, b: &str) -> bool {
    let key = |s: &str| serde_json::from_str::<Outcome>(s).ok().map(|o| (o.email, o.width, o.theme));
    key(a).is_some() && key(a) == key(b)
}

/// Draw one email once, save its picture, and describe what happened.
#[allow(clippy::too_many_arguments)]
pub fn render_once(eng: &mut dyn Engine, server: &Server, email: &str, width: u32, dark: bool, remote: bool, raw: bool, cold: bool) -> Result<Outcome> {
    let theme = if dark { "dark" } else { "light" };
    let mut o = Outcome {
        engine: eng.name().into(),
        email: email.into(),
        width,
        theme: theme.into(),
        remote_images: remote,
        cold,
        first_draw_ms: None,
        height_pt: None,
        requests: vec![],
        server: vec![],
        notes: vec![],
        error: None,
        png: None,
    };
    let page = page::load(email, dark, remote, &server.base(), raw)?;
    server.take_log();
    let opts = RenderOpts { width_pt: width, scale: SCALE, dark, remote_images: remote, timeout: TIMEOUT };
    match eng.render(&page, &opts) {
        Ok(r) => {
            let rel = format!("{}/{email}-{width}-{theme}{}.png", eng.name(), if raw { "-raw" } else { "" });
            let path = out_dir().join(&rel);
            r.image.save(&path).with_context(|| path.display().to_string())?;
            o.first_draw_ms = Some(r.first_draw.as_secs_f64() * 1000.0);
            o.height_pt = Some(r.height_pt);
            o.requests = r.requests;
            o.notes = r.notes;
            o.png = Some(rel);
        }
        Err(e) => o.error = Some(format!("{e:#}")),
    }
    // Give stragglers (WebKit fetches in parallel) a moment to arrive.
    std::thread::sleep(Duration::from_millis(50));
    o.server = server.take_log();
    Ok(o)
}

/// `render-one --engine E --email X --width W --theme light|dark --remote 0|1 --raw 0|1`:
/// print the outcome as JSON. Run as a child process by `check`.
fn render_one_cli(flag: &dyn Fn(&str) -> Option<String>) -> Result<()> {
    let name = flag("--engine").context("--engine")?;
    let email = flag("--email").context("--email")?;
    let width: u32 = flag("--width").and_then(|w| w.parse().ok()).unwrap_or(600);
    let dark = flag("--theme").as_deref() == Some("dark");
    let remote = flag("--remote").as_deref() == Some("1");
    let raw = flag("--raw").as_deref() == Some("1");
    std::fs::create_dir_all(out_dir().join(&name))?;
    let server = Server::start(images::all()?)?;
    let mut eng = engine::create(&name)?;
    let o = render_once(eng.as_mut(), &server, &email, width, dark, remote, raw, true)?;
    println!("{}", serde_json::to_string(&o)?);
    Ok(())
}
