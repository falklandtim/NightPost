//! Builds each page with NightPost's own code (`nightpost-core`): tracking
//! pixels stripped (`html_clean::strip_trackers`), then wrapped by
//! `mail_html::email_frame` under the app's Content Security Policy, with
//! the theme's colours as in `mail_view.rs`.
//!
//! Not built into the Servo build, which can't link `nightpost-core` (both
//! bring their own SQLite); `prepare` writes every page to `out/pages/` for
//! it instead (see `page.rs`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use base64::Engine as _;
use mail_parser::MimeHeaders;
use nightpost_core::{html_clean, mail_html, theme};

use crate::page::{corpus_dir, Prepared, PREPARE_BASE};

/// The nonce in the page's policy. The trial adds no script of its own.
const NONCE: &str = "trial";

/// The page for email `name` and its `cid:` parts. `raw` skips
/// `html_clean`, for checking what the engine itself does with scripts.
pub fn build(name: &str, dark: bool, remote_images: bool, remote_base: &str, raw: bool) -> Result<(String, Parts)> {
    let (body, parts) = read(name)?;
    let body = fill(&body, remote_base)?;
    let html = if raw { body } else { html_clean::strip_trackers(&body).0 };
    let policy = mail_html::csp(NONCE, remote_images);
    Ok((mail_html::email_frame(&html, &policy, &theme_css(dark), false), parts))
}

/// Write every page variant the Servo build asks for into `out/pages/`.
pub fn prepare_all() -> Result<()> {
    let dir = crate::out_dir().join("pages");
    std::fs::create_dir_all(&dir)?;
    let mut n = 0;
    for name in crate::page::list()? {
        for dark in [false, true] {
            for remote in [false, true] {
                for raw in [false, true] {
                    // Only what the checks use: raw for the scripts email,
                    // and the 20 MB picture once.
                    if raw && !name.starts_with("14") || name.starts_with("13c") && (dark || remote || raw) {
                        continue;
                    }
                    let (html, parts) = build(&name, dark, remote, PREPARE_BASE, raw)?;
                    let parts = parts
                        .into_iter()
                        .map(|(id, (mime, bytes))| (id, mime, base64::engine::general_purpose::STANDARD.encode(bytes)))
                        .collect();
                    let file = dir.join(crate::page::prepared_name(&name, dark, remote, raw));
                    std::fs::write(file, serde_json::to_string(&Prepared { html, parts })?)?;
                    n += 1;
                }
            }
        }
    }
    println!("wrote {n} pages to {}", dir.display());
    Ok(())
}

/// `cid:` parts by Content-ID: MIME type and bytes.
pub type Parts = HashMap<String, (String, Vec<u8>)>;

/// The email's HTML body and its `cid:` parts.
fn read(name: &str) -> Result<(String, Parts)> {
    let dir = corpus_dir();
    if dir.join(format!("{name}.demo")).exists() {
        return Ok((demo_newsletter()?, Parts::new()));
    }
    let html = dir.join(format!("{name}.html"));
    if html.exists() {
        return Ok((std::fs::read_to_string(&html).with_context(|| html.display().to_string())?, Parts::new()));
    }
    let eml = dir.join(format!("{name}.eml"));
    let raw = std::fs::read(&eml).with_context(|| format!("no corpus email called {name}"))?;
    let msg = mail_parser::MessageParser::default().parse(&raw).context("the .eml doesn't parse")?;
    let body = msg.body_html(0).map(|b| b.into_owned()).context("the .eml has no HTML part")?;
    let mut parts = Parts::new();
    for part in msg.parts.iter() {
        let Some(id) = part.content_id() else { continue };
        let mime = part
            .content_type()
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
            .unwrap_or_else(|| "application/octet-stream".into());
        parts.insert(id.trim_matches(['<', '>']).to_string(), (mime, part.contents().to_vec()));
    }
    Ok((body, parts))
}

/// Swap the placeholders for this run's values.
fn fill(body: &str, remote_base: &str) -> Result<String> {
    let mut out = body.replace("{{REMOTE}}", remote_base);
    if out.contains("{{HUGE_IMAGE}}") {
        let png = crate::images::huge_png(20_000_000)?;
        let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
        out = out.replace("{{HUGE_IMAGE}}", &uri);
    }
    Ok(out)
}

/// The demo newsletter, read back from a scratch store seeded with the demo
/// accounts (`demo.rs` keeps its HTML private). Its picture points at a
/// made-up `.demo` host; that becomes the local server, so nothing leaves
/// the machine when remote images are on.
fn demo_newsletter() -> Result<String> {
    let dir = crate::out_dir().join("demo-store");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("demo.db");
    let _ = std::fs::remove_file(&path);
    let store = nightpost_core::Store::open(&path)?;
    nightpost_core::demo::seed(&store)?;
    for id in 1..500 {
        let Some(m) = store.message(id)? else { continue };
        if let Some(html) = m.body_html.filter(|_| m.message_id == "nov-roast@kowhai.demo") {
            return Ok(html.replace("https://images.kowhai.demo/", "{{REMOTE}}/images/").replace("guji-hero.jpg", "guji-hero.jpg.png"));
        }
    }
    anyhow::bail!("the demo newsletter wasn't in the demo store")
}

/// The colours `mail_view::document` gives an email: in the dark theme,
/// Moonlit Dark's background, text and link colours (what the app does with
/// "recolour emails" on, minus its recolouring script, which engines without
/// JavaScript can't run); in light, just the text colour.
pub fn theme_css(dark: bool) -> String {
    if !dark {
        return "body{color:#1d1d1f;}".into();
    }
    let themes = theme::discover();
    let Some(p) = themes.iter().find(|t| t.id == theme::MOONLIT_DARK).map(|t| t.palette()) else {
        return ":root{color-scheme:dark;}".into();
    };
    format!(
        ":root{{color-scheme:dark;}} html{{background:{};}} body{{color:{};}} a{{color:{};}}",
        p.background.to_hex(),
        p.text.to_hex(),
        p.link.to_hex()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{list, load};

    #[test]
    fn every_corpus_email_builds() {
        for name in list().unwrap() {
            if name.starts_with("13c") {
                continue; // 20 MB; built by the crash check instead
            }
            let page = load(&name, false, false, "http://127.0.0.1:9", false).unwrap();
            assert!(page.html.contains("Content-Security-Policy"), "{name}");
            assert!(!page.html.contains("{{"), "{name} has a placeholder left");
        }
    }

    #[test]
    fn trackers_go_unless_raw() {
        let clean = load("08-trackers", false, true, "http://127.0.0.1:9", false).unwrap();
        let raw = load("08-trackers", false, true, "http://127.0.0.1:9", true).unwrap();
        assert!(!clean.html.contains("/track/open.gif") && raw.html.contains("/track/open.gif"));
    }

    #[test]
    fn eml_parts_load_by_cid() {
        let page = load("07-inline-pictures", false, false, "http://127.0.0.1:9", false).unwrap();
        assert!(page.loader.fetch("cid:photo-1@harbourside.example").is_some());
    }

    #[test]
    fn dark_uses_moonlit_colours() {
        let css = theme_css(true);
        assert!(css.contains("color-scheme:dark") && css.contains("background:#"), "{css}");
    }
}
