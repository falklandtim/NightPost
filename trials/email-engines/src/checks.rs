//! The must-pass checks, one engine at a time, written to
//! `out/<engine>/checks.json`: privacy, no scripts, no crashes or hangs,
//! links, text, sharpness at 2×, and licences. Plus the peak memory of
//! drawing the long digest, measured in a child process.
//!
//! The broken emails and the memory figure each run in a child process
//! (`render-one`), so a crash or a runaway engine can't take the trial down
//! or skew another engine's numbers.

use std::collections::BTreeSet;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;

use crate::engine::{self, Engine};
use crate::loader::Loader;
use crate::server::Server;
use crate::{out_dir, page, render_once, Outcome};

/// One check's result. `pass` is `None` when it couldn't be decided.
#[derive(Serialize)]
pub struct Check {
    pub name: &'static str,
    pub pass: Option<bool>,
    pub detail: String,
}

#[derive(Serialize)]
struct Checks {
    engine: String,
    checks: Vec<Check>,
    /// Peak resident memory drawing `12-long-digest`, in megabytes.
    memory_mb: Option<f64>,
    memory_note: String,
}

fn check(name: &'static str, pass: bool, detail: impl Into<String>) -> Check {
    Check { name, pass: Some(pass), detail: detail.into() }
}

/// Run every check for engine `name` and save them.
pub fn run(name: &str) -> Result<()> {
    let server = Server::start(crate::images::all()?)?;
    let mut eng = engine::create(name)?;
    let mut checks = vec![privacy(eng.as_mut(), &server)?, scripts(eng.as_mut(), &server)?];
    checks.push(crashes(name));
    checks.push(links(eng.as_mut(), &server)?);
    checks.push(text(eng.as_mut(), &server)?);
    checks.push(sharp(eng.as_mut(), &server)?);
    checks.push(licences(name));
    let (memory_mb, memory_note) = match crate::measure::child_with_memory(&[
        "render-one", "--engine", name, "--email", "12-long-digest", "--width", "600", "--theme", "light", "--remote", "1",
    ]) {
        Ok((_, mb)) => (Some(mb), memory_note(name)),
        Err(e) => (None, format!("couldn't measure: {e:#}")),
    };
    for c in &checks {
        let mark = match c.pass {
            Some(true) => "PASS",
            Some(false) => "FAIL",
            None => "----",
        };
        println!("{name} {mark} {}: {}", c.name, c.detail);
    }
    if let Some(mb) = memory_mb {
        println!("{name} memory: {mb:.0} MB peak ({memory_note})");
    }
    let path = out_dir().join(name).join("checks.json");
    std::fs::create_dir_all(out_dir().join(name))?;
    std::fs::write(&path, serde_json::to_string_pretty(&Checks { engine: name.into(), checks, memory_mb, memory_note })?)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn memory_note(name: &str) -> String {
    match name {
        "webkit-mac" | "webkit-gtk4" => "this process only: WebKit draws in its own web process, which isn't counted".into(),
        _ => "the whole drawing process".into(),
    }
}

/// With remote images off, nothing reaches the server: an engine may ask
/// the loader for a remote picture (that's how the setting is enforced),
/// but the loader must refuse it, and nothing may go round the loader.
/// With images on, trackers still never reach the server.
fn privacy(eng: &mut dyn Engine, server: &Server) -> Result<Check> {
    let mut problems = Vec::new();
    let mut refused = 0;
    for email in page::list()? {
        if email.starts_with("13") {
            continue;
        }
        let o = render_once(eng, server, &email, 600, false, false, false, false)?;
        refused += o.requests.iter().filter(|r| !Loader::is_local(r)).count();
        if !o.server.is_empty() {
            problems.push(format!("{email} reached the server: {:?}", o.server));
        }
    }
    let o = render_once(eng, server, "08-trackers", 600, false, true, false, false)?;
    let tracked: Vec<&String> = o.server.iter().chain(o.requests.iter()).filter(|r| ["/track/", "/wf/open", "/pixel.png"].iter().any(|t| r.contains(t))).collect();
    if !tracked.is_empty() {
        problems.push(format!("08-trackers with images on reached trackers: {tracked:?}"));
    }
    // Not a pass condition, but worth confirming the set-up works: with
    // images on, the remote picture in 07 does arrive.
    let o = render_once(eng, server, "07-inline-pictures", 600, false, true, false, false)?;
    let remote_ok = o.server.iter().any(|r| r.contains("remote-banner"));
    let cids = o.requests.iter().filter(|r| r.starts_with("cid:")).count();
    let detail = if problems.is_empty() {
        format!(
            "nothing reached the server with images off ({refused} remote requests asked of the loader and refused); trackers never fetched. \
             (Sanity: {cids} cid: loads in 07; its remote picture {} with images on.)",
            if remote_ok { "loaded" } else { "did NOT load" }
        )
    } else {
        problems.join("; ")
    };
    Ok(check("privacy", problems.is_empty(), detail))
}

/// The raw 14-scripts (no `html_clean`), images on so a script that ran
/// could reach the server: no `/marker/` request may appear.
fn scripts(eng: &mut dyn Engine, server: &Server) -> Result<Check> {
    let o = render_once(eng, server, "14-scripts", 600, false, true, true, false)?;
    if let Some(e) = &o.error {
        return Ok(Check { name: "no scripts", pass: None, detail: format!("didn't draw: {e}") });
    }
    let ran: Vec<&String> = o.server.iter().chain(o.requests.iter()).filter(|r| r.contains("/marker/")).collect();
    let js_link = eng.element_box("jslink").and_then(|b| eng.link_at(b.centre().0, b.centre().1)).unwrap_or_default();
    let note = if js_link.starts_with("javascript:") { " The javascript: link is reported as a link; the app must refuse to open it." } else { "" };
    Ok(if ran.is_empty() {
        check("no scripts", true, format!("no script ran (seven ways tried).{note}"))
    } else {
        check("no scripts", false, format!("scripts ran: {ran:?}"))
    })
}

/// Each broken email in a child process: a picture or an error within
/// 10 seconds, and no crash.
fn crashes(name: &str) -> Check {
    let mut problems = Vec::new();
    let mut fine = Vec::new();
    let cases: Vec<String> = page::list().unwrap_or_default().into_iter().filter(|e| e.starts_with("13")).collect();
    for email in &cases {
        match child(&["render-one", "--engine", name, "--email", email, "--width", "600", "--theme", "light", "--remote", "0"], Duration::from_secs(10)) {
            Ok((out, took)) => match serde_json::from_str::<Outcome>(out.trim()) {
                Ok(o) => {
                    let what = o.error.map(|e| format!("error: {e}")).unwrap_or_else(|| "drew".into());
                    fine.push(format!("{email} {what} ({:.1} s)", took.as_secs_f64()));
                }
                Err(_) => problems.push(format!("{email}: no result ({})", out.lines().last().unwrap_or("").trim())),
            },
            Err(e) => problems.push(format!("{email}: {e}")),
        }
    }
    if cases.is_empty() {
        return Check { name: "no crashes", pass: None, detail: "no 13… emails in the corpus".into() };
    }
    let detail = [problems.join("; "), fine.join("; ")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" | ");
    check("no crashes", problems.is_empty(), detail)
}

/// Run this program with `args`, killing it after `limit` (plus start-up
/// time). Returns its output and how long the drawing took.
fn child(args: &[&str], limit: Duration) -> Result<(String, Duration), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut c = Command::new(me).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    // A little more than the limit: the child also starts its engine.
    let deadline = started + limit + Duration::from_secs(5);
    loop {
        match c.try_wait() {
            Ok(Some(status)) => {
                let out = c.wait_with_output().map_err(|e| e.to_string())?;
                let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                if !status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    return Err(format!("crashed ({status}): {}", err.lines().last().unwrap_or("").trim()));
                }
                return Ok((stdout, started.elapsed()));
            }
            Ok(None) if Instant::now() > deadline => {
                let _ = c.kill();
                let _ = c.wait();
                return Err(format!("hung: killed after {:.0} s", started.elapsed().as_secs_f64()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The link under the middle of the button in 02 is its address.
fn links(eng: &mut dyn Engine, server: &Server) -> Result<Check> {
    let o = render_once(eng, server, "02-table-newsletter", 600, false, true, false, false)?;
    if let Some(e) = o.error {
        return Ok(Check { name: "links", pass: None, detail: format!("didn't draw: {e}") });
    }
    let want = "https://harbourside.example/autumn";
    let Some(b) = eng.element_box("cta") else {
        return Ok(check("links", false, "couldn't find the button's position"));
    };
    let (x, y) = b.centre();
    let got = eng.link_at(x, y);
    Ok(check("links", got.as_deref() == Some(want), format!("at ({x:.0}, {y:.0}): {got:?}")))
}

/// The text across the known paragraph in 06 comes back.
fn text(eng: &mut dyn Engine, server: &Server) -> Result<Check> {
    let o = render_once(eng, server, "06-quoted-reply", 600, false, false, false, false)?;
    if let Some(e) = o.error {
        return Ok(Check { name: "text", pass: None, detail: format!("didn't draw: {e}") });
    }
    let want = "Sounds good to me. Let's book the bach for the long weekend and split it three ways.";
    let Some(b) = eng.element_box("known") else {
        return Ok(check("text", false, "couldn't find the paragraph's position"));
    };
    let got = eng.text_between((b.x + 1.0, b.y + 2.0), (b.x + b.w - 1.0, b.y + b.h - 2.0));
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let ok = got.as_deref().map(norm).is_some_and(|g| g == want || (want.starts_with(&g) && g.len() + 2 >= want.len()));
    Ok(check("text", ok, format!("{got:?}")))
}

/// The 1-point rule in 05 is exactly one dark pixel row per point of scale
/// (two at 2×), with no grey rows either side (which would mean it was
/// blurred). The scale is the picture's own: a hidden GTK window draws at
/// its display's scale, which may be 1×.
fn sharp(eng: &mut dyn Engine, server: &Server) -> Result<Check> {
    let o = render_once(eng, server, "05-receipt", 600, false, false, false, false)?;
    let Some(png) = o.png else {
        return Ok(Check { name: "sharp at 2×", pass: None, detail: format!("didn't draw: {:?}", o.error) });
    };
    let Some(b) = eng.element_box("rule") else {
        return Ok(check("sharp at 2×", false, "couldn't find the rule's position"));
    };
    let img = image::open(out_dir().join(png))?.to_rgba8();
    let scale = img.width() as f32 / 600.0;
    let x = ((b.x + b.w / 2.0) * scale) as u32;
    let (from, to) = (((b.y - 3.0) * scale).max(0.0) as u32, ((b.y + 4.0) * scale) as u32);
    let (mut dark, mut grey) = (0, 0);
    for y in from..to.min(img.height()) {
        let Some(p) = img.get_pixel_checked(x.min(img.width().saturating_sub(1)), y) else { continue };
        let l = (p.0[0] as u32 + p.0[1] as u32 + p.0[2] as u32) / 3;
        if l < 60 {
            dark += 1;
        } else if l < 200 {
            grey += 1;
        }
    }
    let want = scale.round() as u32;
    let at = if want == 2 { String::new() } else { format!(" (drawn at {want}×)") };
    Ok(check("sharp at 2×", dark == want && grey == 0, format!("{dark} dark rows, {grey} grey rows (want {want} and 0){at}")))
}

/// Every crate the engine adds fits GPL-3.0-or-later.
fn licences(name: &str) -> Check {
    let tree = |features: &[&str]| -> Result<BTreeSet<String>, String> {
        let mut c = Command::new("cargo");
        c.current_dir(env!("CARGO_MANIFEST_DIR")).args(["tree", "-e", "normal", "--prefix", "none", "--format", "{p}|{l}"]);
        for f in features {
            c.args(["--features", f]);
        }
        let out = c.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("cargo tree failed").to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim_end_matches(" (*)").to_string()).collect())
    };
    let (with, without) = match (tree(&[name]), tree(&[])) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Check { name: "licences", pass: None, detail: e },
    };
    let added: Vec<&String> = with.difference(&without).collect();
    let bad: Vec<&&String> = added.iter().filter(|l| !licence_ok(l.split('|').nth(1).unwrap_or(""))).collect();
    let list = added.iter().map(|l| l.replace('|', ": ")).collect::<Vec<_>>().join(", ");
    if bad.is_empty() {
        check("licences", true, format!("{} crates added, all fit GPL-3.0-or-later: {list}", added.len()))
    } else {
        check("licences", false, format!("check by hand: {bad:?}"))
    }
}

/// Does an SPDX expression fit GPL-3.0-or-later? `OR` needs one fitting
/// choice; `AND` (or `/`, an old spelling of OR) is handled conservatively.
fn licence_ok(expr: &str) -> bool {
    const FIT: &[&str] = &[
        "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0", "Unicode-DFS-2016",
        "MPL-2.0", "CC0-1.0", "BSL-1.0", "0BSD", "Unlicense", "LGPL-2.1-or-later", "LGPL-2.1", "GPL-3.0-or-later", "MIT-0",
    ];
    let expr = expr.replace(['(', ')'], "");
    if expr.trim().is_empty() {
        return false;
    }
    expr.split(" AND ").all(|part| part.split(" OR ").flat_map(|p| p.split('/')).any(|l| FIT.contains(&l.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licence_expressions() {
        assert!(licence_ok("MIT OR Apache-2.0"));
        assert!(licence_ok("MIT/Apache-2.0"));
        assert!(licence_ok("(MIT OR Apache-2.0) AND Unicode-3.0"));
        assert!(!licence_ok("SSPL-1.0"));
        assert!(!licence_ok(""));
        assert!(!licence_ok("MIT AND SSPL-1.0"));
    }
}
