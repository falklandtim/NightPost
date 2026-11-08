//! Cleaning up email HTML: finding remote content and removing tracking
//! pixels. Shared by the app's web view and the MCP server.

/// Does this HTML load anything from the internet (images, CSS, fonts)?
pub fn has_remote_content(html: &str) -> bool {
    let h = html.to_ascii_lowercase();
    ["src=\"http", "src='http", "src=http", "url(http", "url('http", "url(\"http", "background=\"http", "href=\"http"]
        .iter()
        .any(|p| {
            if *p == "href=\"http" {
                // Only stylesheets count, not ordinary links.
                h.contains("<link") && h.contains("stylesheet") && h.contains(p)
            } else {
                h.contains(p)
            }
        })
}

/// Remove tracking pixels from an email: images whose only job is to tell
/// the sender you opened it (and when, and roughly where). They're tiny
/// (1×1) or come from known open-tracking paths. Returns the cleaned HTML
/// and the hosts the trackers pointed at.
pub fn strip_trackers(html: &str) -> (String, Vec<String>) {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut hosts: Vec<String> = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<img").map(|i| at + i) {
        let end = tag_end(html, i);
        let tag = &html[i..end];
        out.push_str(&html[at..i]);
        match tracker_host(tag) {
            Some(host) => {
                if !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
            None => out.push_str(tag),
        }
        at = end;
    }
    out.push_str(&html[at..]);
    (out, hosts)
}

/// Where a tag starting at `start` ends (after its `>`), skipping quoted `>`.
fn tag_end(html: &str, start: usize) -> usize {
    let mut quote = None;
    for (j, c) in html[start..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return start + j + 1,
            _ => {}
        }
    }
    html.len()
}

/// An attribute's value from a single tag (`name=` preceded by whitespace).
pub fn tag_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name).map(|i| from + i) {
        from = i + name.len();
        let before_ok = lower[..i].ends_with(|c: char| c.is_ascii_whitespace() || c == '/');
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let vstart = tag.len() - rest.len() + 1;
        let v = tag[vstart..].trim_start();
        return Some(match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_string(),
            _ => v.split(|c: char| c.is_ascii_whitespace() || c == '>').next().unwrap_or("").trim_end_matches('/').to_string(),
        });
    }
    None
}

/// A pixel size from `1`, `1px` or a CSS declaration's value.
fn px(v: &str) -> Option<f32> {
    v.trim().trim_end_matches("!important").trim().trim_end_matches("px").trim().parse().ok()
}

fn css_prop(style: &str, prop: &str) -> Option<String> {
    style.split(';').find_map(|d| {
        let (k, v) = d.split_once(':')?;
        k.trim().eq_ignore_ascii_case(prop).then(|| v.trim().to_ascii_lowercase())
    })
}

/// Open-tracking URLs of common email services.
const TRACKER_PATHS: &[&str] = &[
    "/trk?", "/track/open", "/open.php", "/open.aspx", "/open.gif", "/wf/open", "/e/o/", "/o.gif", "/pixel.gif", "/pixel.png", "/beacon",
    "/e2t/to/", "/imp?", "/ci0/", "/oo/", "list-manage.com/track", "mandrillapp.com/track", "/mo/", "/t.gif",
];

/// The host an `<img>` tag reports opens to, if it's a tracker.
fn tracker_host(tag: &str) -> Option<String> {
    let src = tag_attr(tag, "src")?;
    let lower = src.to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://")).or_else(|| lower.strip_prefix("//"))?;
    let host = rest.split(['/', '?', ':', '#']).next().unwrap_or("").to_string();
    let style = tag_attr(tag, "style").unwrap_or_default();
    let w = tag_attr(tag, "width").as_deref().and_then(px).or_else(|| css_prop(&style, "width").as_deref().and_then(px));
    let h = tag_attr(tag, "height").as_deref().and_then(px).or_else(|| css_prop(&style, "height").as_deref().and_then(px));
    let tiny = match (w, h) {
        (Some(w), Some(h)) => w <= 3.0 && h <= 3.0,
        (Some(d), None) | (None, Some(d)) => d <= 1.0,
        (None, None) => false,
    };
    let path = &rest[host.len()..];
    let known = TRACKER_PATHS.iter().any(|t| if t.starts_with('/') { path.contains(t) } else { lower.contains(t) });
    (tiny || known).then_some(host)
}
