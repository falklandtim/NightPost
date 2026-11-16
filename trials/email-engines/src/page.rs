//! Turns a corpus email into the page the app would show, built by
//! `page_build.rs` with NightPost's own code; or, in the Servo build (which
//! can't link `nightpost-core`), read back from `out/pages/`, where the
//! normal build's `prepare` command wrote it.
//!
//! The corpus holds `.html` bodies, `.eml` messages (for `cid:` pictures,
//! parsed with mail-parser as the app does), and one `.demo` file standing
//! for the demo newsletter in `nightpost_core::demo`. Placeholders:
//! `{{REMOTE}}` becomes the local server's address, and `{{HUGE_IMAGE}}` a
//! 20 MB inline picture made at run time (too big to commit).

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::engine::Page;
use crate::loader::Loader;

/// The "remote" address written into prepared pages, swapped for the local
/// server's real one when a page is loaded.
pub const PREPARE_BASE: &str = "http://127.0.0.1:1";

/// A page as `prepare` writes it: the HTML and its `cid:` parts (Content-ID,
/// MIME type, base64 bytes).
#[derive(Serialize, Deserialize)]
pub struct Prepared {
    pub html: String,
    pub parts: Vec<(String, String, String)>,
}

/// The file name of one prepared page.
pub fn prepared_name(name: &str, dark: bool, remote: bool, raw: bool) -> String {
    format!("{name}-{}-{}{}.json", if dark { "dark" } else { "light" }, if remote { "remote" } else { "local" }, if raw { "-raw" } else { "" })
}

/// Where the corpus lives, in the trial's folder.
pub fn corpus_dir() -> PathBuf {
    crate::root().join("corpus")
}

/// Every email in the corpus by name (file name without extension), sorted.
pub fn list() -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(corpus_dir())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "html" || x == "eml" || x == "demo"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    Ok(names)
}

/// The page for email `name`. `raw` skips `html_clean`, for checking what
/// the engine itself does with scripts and trackers.
#[cfg(not(feature = "servo"))]
pub fn load(name: &str, dark: bool, remote_images: bool, remote_base: &str, raw: bool) -> Result<Page> {
    let (html, parts) = crate::page_build::build(name, dark, remote_images, remote_base, raw)?;
    Ok(Page { html, loader: Loader::new(parts, remote_base.to_string(), remote_images) })
}

/// The page for email `name`, as `prepare` wrote it, pointed at this run's
/// local server.
#[cfg(feature = "servo")]
pub fn load(name: &str, dark: bool, remote_images: bool, remote_base: &str, raw: bool) -> Result<Page> {
    use anyhow::Context;
    use base64::Engine as _;
    let file = crate::out_dir().join("pages").join(prepared_name(name, dark, remote_images, raw));
    let text = std::fs::read_to_string(&file).with_context(|| format!("{} is missing: run `prepare` with the main build first", file.display()))?;
    let p: Prepared = serde_json::from_str(&text)?;
    let parts = p
        .parts
        .into_iter()
        .filter_map(|(id, mime, b64)| Some((id, (mime, base64::engine::general_purpose::STANDARD.decode(b64).ok()?))))
        .collect();
    let html = p.html.replace(&format!("{PREPARE_BASE}/"), &format!("{remote_base}/"));
    Ok(Page { html, loader: Loader::new(parts, remote_base.to_string(), remote_images) })
}

