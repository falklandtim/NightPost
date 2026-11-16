//! `out/report.html`: one page Tim opens in a browser. One row per email,
//! one column per engine (WebKit on the Mac first), light/dark and width on
//! toggles; the numbers under each picture; the checks per engine; and a
//! 0–3 score box under each picture, kept in the browser and exported as
//! JSON with a button.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::Result;
use serde_json::Value;

use crate::{measure, out_dir, Outcome, WIDTHS};

const REFERENCE: &str = "webkit-mac";

/// Build the report from every engine's results in `out/`.
pub fn write() -> Result<()> {
    let out = out_dir();
    let mut engines: Vec<String> = std::fs::read_dir(&out)?
        .flatten()
        .filter(|e| e.path().join("results.jsonl").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    engines.sort_by_key(|e| (e != REFERENCE, e.clone()));
    let mut results: BTreeMap<(String, String, u32, String), Outcome> = BTreeMap::new();
    for e in &engines {
        let text = std::fs::read_to_string(out.join(e).join("results.jsonl"))?;
        for o in text.lines().filter_map(|l| serde_json::from_str::<Outcome>(l).ok()) {
            results.insert((o.engine.clone(), o.email.clone(), o.width, o.theme.clone()), o);
        }
    }
    let emails: Vec<String> = {
        let mut v: Vec<String> = results.keys().map(|k| k.1.clone()).collect();
        v.dedup();
        v.sort();
        v.dedup();
        v
    };
    let mut html = String::from(HEAD);
    summary(&mut html, &engines, &results)?;
    html.push_str("<h2>Pictures</h2><div class=controls>Theme <button data-theme=light class=on>Light</button><button data-theme=dark>Dark</button>\
                   &nbsp; Width <button data-width=600 class=on>600</button><button data-width=900>900</button>\
                   &nbsp; <button id=export>Export scores</button> <span class=muted>Scores: 3 as good as WebKit · 2 small differences · 1 layout visibly off · 0 broken</span></div>");
    let _ = write!(html, "<table class=grid><tr><th>Email</th>{}</tr>", engines.iter().map(|e| format!("<th>{e}</th>")).collect::<String>());
    for email in &emails {
        let _ = write!(html, "<tr><th class=email>{email}</th>");
        for e in &engines {
            html.push_str("<td>");
            for w in WIDTHS {
                for t in ["light", "dark"] {
                    cell(&mut html, results.get(&(e.clone(), email.clone(), w, t.into())), email, e, w, t);
                }
            }
            html.push_str("</td>");
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    html.push_str(TAIL);
    let path = out.join("report.html");
    std::fs::write(&path, html)?;
    println!("wrote {}", path.display());
    Ok(())
}

/// One picture with its numbers and score box.
fn cell(html: &mut String, o: Option<&Outcome>, email: &str, engine: &str, width: u32, theme: &str) {
    let _ = write!(html, "<div class=shot data-w={width} data-t={theme}>");
    let Some(o) = o else {
        html.push_str("<p class=muted>not drawn</p></div>");
        return;
    };
    if let Some(png) = &o.png {
        let _ = write!(html, "<a href=\"{png}\" target=_blank><img loading=lazy src=\"{png}\" style=\"width:{}px\"></a>", width / 2);
    }
    let diff = match (&o.png, engine == REFERENCE) {
        (Some(png), false) => {
            let reference = out_dir().join(REFERENCE).join(format!("{email}-{width}-{theme}.png"));
            reference.exists().then(|| measure::difference(&out_dir().join(png), &reference).ok()).flatten().map(|d| format!(" · differs {:.1}%", d * 100.0))
        }
        _ => None,
    };
    let _ = write!(
        html,
        "<p class=meta>{}{} · {} requests{}</p>",
        o.first_draw_ms.map(|t| format!("{t:.0} ms")).unwrap_or_else(|| "—".into()),
        if o.cold { " (cold)" } else { "" },
        o.requests.len() + o.server.len(),
        diff.unwrap_or_default()
    );
    if let Some(e) = &o.error {
        let _ = write!(html, "<p class=err>{}</p>", esc(e));
    }
    for n in &o.notes {
        let _ = write!(html, "<p class=muted>{}</p>", esc(n));
    }
    let key = format!("{engine}|{email}|{width}|{theme}");
    let _ = write!(html, "<p class=score data-key=\"{key}\">Score {}</p></div>", (0..=3).map(|s| format!("<button>{s}</button>")).collect::<String>());
}

/// The checks, timings and memory per engine.
fn summary(html: &mut String, engines: &[String], results: &BTreeMap<(String, String, u32, String), Outcome>) -> Result<()> {
    html.push_str("<h2>Engines</h2><table class=sum><tr><th></th>");
    for e in engines {
        let _ = write!(html, "<th>{e}</th>");
    }
    html.push_str("</tr>");
    let checks: Vec<Option<Value>> = engines
        .iter()
        .map(|e| std::fs::read_to_string(out_dir().join(e).join("checks.json")).ok().and_then(|s| serde_json::from_str(&s).ok()))
        .collect();
    let times = |e: &str, cold: bool| -> Vec<f64> {
        results.values().filter(|o| o.engine == e && o.cold == cold).filter_map(|o| o.first_draw_ms).collect()
    };
    row(html, "Warm draw (median)", engines.iter().map(|e| measure::median(times(e, false)).map(|t| format!("{t:.0} ms")).unwrap_or_default()));
    row(html, "Cold draw", engines.iter().map(|e| times(e, true).first().map(|t| format!("{t:.0} ms")).unwrap_or_default()));
    row(html, "Peak memory (long digest)", checks.iter().map(|c| match c {
        Some(c) => format!("{} <span class=muted>{}</span>", c["memory_mb"].as_f64().map(|m| format!("{m:.0} MB")).unwrap_or("—".into()), esc(c["memory_note"].as_str().unwrap_or(""))),
        None => String::new(),
    }));
    let builds: Vec<Value> = std::fs::read_to_string(out_dir().join("build-cost.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let host = std::process::Command::new("uname").arg("-sm").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    // The latest measurement on this machine wins.
    let build = |e: &str| builds.iter().rev().find(|b| b["engine"] == e && b["host"] == host.as_str());
    let base = build("none").and_then(|b| b["bytes"].as_f64());
    row(html, &format!("Clean release build ({host})"), engines.iter().map(|e| match build(e) {
        Some(b) if b["error"].is_string() => format!("<span class=fail>{}</span>", esc(b["error"].as_str().unwrap_or(""))),
        Some(b) => format!("{} s", b["seconds"].as_f64().unwrap_or(0.0)),
        None => "<span class=muted>not measured</span>".into(),
    }));
    row(html, "Binary size added", engines.iter().map(|e| match (build(e).and_then(|b| b["bytes"].as_f64()), base) {
        (Some(b), Some(n)) => format!("{:.1} MB", (b - n) / 1_048_576.0),
        _ => String::new(),
    }));
    let names = ["privacy", "no scripts", "no crashes", "links", "text", "sharp at 2×", "licences"];
    for n in names {
        row(html, n, checks.iter().map(|c| {
            let found = c.as_ref().and_then(|c| c["checks"].as_array()).and_then(|a| a.iter().find(|x| x["name"] == n));
            match found {
                Some(x) => {
                    let mark = match x["pass"].as_bool() {
                        Some(true) => "<b class=pass>PASS</b>",
                        Some(false) => "<b class=fail>FAIL</b>",
                        None => "<b class=muted>?</b>",
                    };
                    format!("{mark} <span class=muted>{}</span>", esc(x["detail"].as_str().unwrap_or("")))
                }
                None => "<span class=muted>not run</span>".into(),
            }
        }));
    }
    html.push_str("</table>");
    Ok(())
}

fn row(html: &mut String, label: &str, cells: impl Iterator<Item = String>) {
    let _ = write!(html, "<tr><th>{label}</th>{}</tr>", cells.map(|c| format!("<td>{c}</td>")).collect::<String>());
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

const HEAD: &str = r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Email engine trial</title>
<style>
:root { --bg:#f6f7f9; --card:#fff; --text:#1d1f24; --muted:#6b7280; --line:#e3e6ea; --accent:#3557c9; --pass:#16794a; --fail:#b42318; }
@media (prefers-color-scheme: dark) { :root { --bg:#14161a; --card:#1d2026; --text:#e6e8ec; --muted:#9aa1ad; --line:#2d323a; --accent:#8fa7ff; --pass:#5fd39a; --fail:#ff8a7a; } }
body { margin:0; padding:20px 16px 60px; background:var(--bg); color:var(--text); font:14px/1.45 -apple-system, system-ui, sans-serif; }
h1 { font-size:22px; margin:0 0 4px; } h2 { font-size:17px; margin:28px 0 10px; }
.muted { color:var(--muted); font-size:12px; }
table { border-collapse:collapse; } th, td { text-align:left; vertical-align:top; }
.sum th, .sum td { padding:6px 10px; border-bottom:1px solid var(--line); max-width:520px; }
.pass { color:var(--pass); } .fail { color:var(--fail); } .err { color:var(--fail); font-size:12px; }
.controls { position:sticky; top:0; background:var(--bg); padding:8px 0; z-index:2; border-bottom:1px solid var(--line); }
button { font:inherit; padding:3px 10px; border:1px solid var(--line); background:var(--card); color:var(--text); border-radius:6px; cursor:pointer; }
button.on { background:var(--accent); color:#fff; border-color:var(--accent); }
.grid th, .grid td { padding:10px; border-bottom:1px solid var(--line); }
.grid th.email { position:sticky; left:0; background:var(--bg); white-space:nowrap; }
.shot { display:none; } .shot.show { display:block; }
.shot img { display:block; border:1px solid var(--line); background:#fff; max-height:1400px; object-fit:cover; object-position:top; }
.meta { margin:6px 0 2px; font-size:12px; color:var(--muted); }
.score button { padding:1px 8px; margin-left:3px; }
.wrap { overflow-x:auto; }
</style></head><body>
<h1>Email engine trial</h1>
<p class=muted>Made-up emails drawn by each engine at 2×. WebKit on the Mac (today's look) is the reference. Pictures are drawn with remote images on (as after “Load images”), all served locally; the privacy check runs separately with them off. Dark pictures use the dark theme's colours only: the app's recolouring script (which turns an email's dark text light) is left out for every engine, so emails that set their own dark text look worse here than in the app. Click a picture to see it full size.</p>
<div class=wrap>"#;

const TAIL: &str = r#"</div>
<script>
const KEY = "email-engine-trial-scores";
let scores = {};
try { scores = JSON.parse(localStorage.getItem(KEY) || "{}"); } catch (e) {}
function save() { try { localStorage.setItem(KEY, JSON.stringify(scores)); } catch (e) {} }
let theme = "light", width = "600";
function show() {
  document.querySelectorAll(".shot").forEach(s => s.classList.toggle("show", s.dataset.t === theme && s.dataset.w === width));
  document.querySelectorAll("[data-theme]").forEach(b => b.classList.toggle("on", b.dataset.theme === theme));
  document.querySelectorAll("[data-width]").forEach(b => b.classList.toggle("on", b.dataset.width === width));
}
document.querySelectorAll("[data-theme]").forEach(b => b.onclick = () => { theme = b.dataset.theme; show(); });
document.querySelectorAll("[data-width]").forEach(b => b.onclick = () => { width = b.dataset.width; show(); });
document.querySelectorAll(".score").forEach(p => {
  const key = p.dataset.key;
  p.querySelectorAll("button").forEach(b => {
    b.classList.toggle("on", String(scores[key]) === b.textContent);
    b.onclick = () => { scores[key] = Number(b.textContent); save(); p.querySelectorAll("button").forEach(x => x.classList.toggle("on", x === b)); };
  });
});
document.getElementById("export").onclick = () => {
  const blob = new Blob([JSON.stringify(scores, null, 2)], { type: "application/json" });
  const a = document.createElement("a"); a.href = URL.createObjectURL(blob); a.download = "engine-scores.json"; a.click();
};
show();
</script></body></html>"#;
