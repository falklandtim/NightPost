//! Cleaning up email HTML: finding remote content and removing trackers.
//! Shared by the desktop app (`mail_view.rs`), the iPhone app
//! (`nightpost-ffi`'s `reader.rs`) and the MCP server (`email_view.rs`).
//!
//! A tracker is anything an email loads only to tell the sender it was
//! opened (and when, and roughly where): a tiny picture, or one from a known
//! open-tracking path. With remote images on, the page's Content Security
//! Policy lets pictures, styles and fonts load from anywhere
//! ([`crate::mail_html::csp`]), so this file is what keeps trackers out.
//!
//! Every place an email can load something from is checked, by one walk
//! over the HTML (`walk`): `<img src>`, `srcset` (on `<img>` and
//! `<picture>`'s `<source>`), `<input type="image" src>`, `poster`,
//! `background=`, SVG `<image href>`, stylesheet `<link href>`, and any
//! `url(…)` or `@import` in a `style` attribute or a `<style>` block. A
//! tracking `<img>` or `<input>` is removed whole; elsewhere just the
//! tracking address goes and the rest of the tag or CSS stays. Size rules
//! apply only where there's a size (`<img>`, `<input>`); everywhere else
//! it's the known paths.
//!
//! It scans the text rather than parsing HTML, in one pass, so a huge or
//! broken email can't make it hang. Hidden pictures (`display:none`) are
//! kept: newsletters hide their mobile-only pictures that way. Scripts are
//! not removed here: the Content Security Policy stops them (and, in the
//! app, the sandboxed frame).

/// Does this HTML load anything from the internet (pictures, CSS, fonts)?
/// Decides whether "Load images" is offered when remote images are off.
pub fn has_remote_content(html: &str) -> bool {
    let mut found = false;
    walk(html, &mut |url, _| {
        found |= remote_rest(url).is_some();
        true
    });
    found
}

/// Remove trackers from an email (see the file's summary). Returns the
/// cleaned HTML and the hosts the trackers pointed at, each once, in order.
pub fn strip_trackers(html: &str) -> (String, Vec<String>) {
    let mut hosts: Vec<String> = Vec::new();
    let out = walk(html, &mut |url, size| match tracker_host(url, size) {
        Some(host) => {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
            false
        }
        None => true,
    });
    (out, hosts)
}

/// A picture's width and height in pixels, where the tag gives them.
type Size = (Option<f32>, Option<f32>);

/// Asked of every address the page would load, with the picture's size for
/// an `<img>` or `<input>`: keep it (`true`) or remove it (`false`).
type Keep<'a> = dyn FnMut(&str, Option<Size>) -> bool + 'a;

/// Walk every tag in `html`, ask `keep` about each address it would load,
/// and return the HTML with the refused ones removed. Text, comments and
/// untouched tags are copied as they are.
fn walk(html: &str, keep: &mut Keep) -> String {
    // ASCII lowercasing keeps every byte where it was, so offsets carry over.
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let (mut copied, mut i) = (0, 0);
    while let Some(start) = lower.get(i..).and_then(|s| s.find('<')).map(|o| i + o) {
        let after = lower.get(start + 1..).unwrap_or("");
        if after.starts_with("!--") {
            // Browsers load nothing from a comment (Outlook's `[if mso]`
            // blocks included), and a stray quote in one mustn't derail us.
            i = after.find("-->").map_or(html.len(), |e| start + 1 + e + 3);
            continue;
        }
        let name: String = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-').collect();
        if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
            i = start + 1;
            continue;
        }
        let end = tag_end(html, start);
        out.push_str(&html[copied..start]);
        if let Some(tag) = clean_tag(&name, &html[start..end], keep) {
            out.push_str(&tag);
        }
        (copied, i) = (end, end);
        if name == "style" {
            let close = lower.get(end..).and_then(|s| s.find("</style")).map_or(html.len(), |e| end + e);
            out.push_str(&clean_css(&html[end..close], keep));
            (copied, i) = (close, close);
        }
    }
    out.push_str(&html[copied..]);
    out
}

/// One tag with its refused addresses removed, or `None` to drop the whole
/// tag (a tracking `<img>` or `<input>`).
fn clean_tag(name: &str, tag: &str, keep: &mut Keep) -> Option<String> {
    let sized = name == "img" || (name == "input" && tag_attr(tag, "type").is_some_and(|t| t.eq_ignore_ascii_case("image")));
    if sized {
        if let Some(src) = tag_attr(tag, "src") {
            if !keep(&src, Some(size(tag))) {
                return None;
            }
        }
    }
    let mut t = tag.to_string();
    let stylesheet = name == "link" && tag_attr(tag, "rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet"));
    let urls: &[&str] = match name {
        "image" => &["background", "href", "xlink:href"],
        "video" => &["background", "poster"],
        "link" if stylesheet => &["background", "href"],
        _ => &["background"],
    };
    for a in urls {
        if tag_attr(&t, a).is_some_and(|v| !keep(&v, None)) {
            t = set_attr(&t, a, None);
        }
    }
    if let Some(v) = tag_attr(&t, "srcset") {
        let kept = clean_srcset(&v, keep);
        if kept != v {
            t = set_attr(&t, "srcset", Some(&kept).filter(|k| !k.is_empty()).map(|k| k.as_str()));
        }
    }
    if let Some(v) = tag_attr(&t, "style") {
        let kept = clean_css(&v, keep);
        if kept != v {
            t = set_attr(&t, "style", Some(&kept));
        }
    }
    Some(t)
}

/// A `srcset` with the refused candidates left out. Each candidate is an
/// address then an optional size (`2x`, `600w`); addresses may hold commas
/// (`data:`), so a candidate ends at a comma after the address.
fn clean_srcset(v: &str, keep: &mut Keep) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut rest = v;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
        if rest.is_empty() {
            break;
        }
        let url_end = rest.find(|c: char| c.is_ascii_whitespace()).unwrap_or(rest.len());
        let url = rest[..url_end].trim_end_matches(',');
        // A trailing comma on the address ends the candidate.
        let end = if rest[..url_end].ends_with(',') { url_end } else { rest[url_end..].find(',').map_or(rest.len(), |e| url_end + e) };
        if keep(url, None) {
            kept.push(rest[..end].trim());
        }
        rest = &rest[end..];
    }
    kept.join(", ")
}

/// CSS with every refused `url(…)` emptied to `url()`, and every refused
/// `@import "…"` to `""`. The rest of the CSS is left as it was.
fn clean_css(css: &str, keep: &mut Keep) -> String {
    let lower = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len());
    let (mut copied, mut i) = (0, 0);
    // The next of each, found again only once passed: searching afresh each
    // time would rescan to the end for every `url(` (slow on huge CSS).
    let find = |what: &str, from: usize| lower.get(from..).and_then(|s| s.find(what)).map(|o| from + o);
    let (mut next_url, mut next_import) = (find("url(", 0), find("@import", 0));
    loop {
        if next_url.is_some_and(|u| u < i) {
            next_url = find("url(", i);
        }
        if next_import.is_some_and(|m| m < i) {
            next_import = find("@import", i);
        }
        // The address's span, from whichever comes first.
        let (from, to) = match (next_url, next_import) {
            (Some(u), imp) if imp.is_none_or(|m| u < m) => (u + 4, lower[u..].find(')').map_or(css.len(), |e| u + e)),
            (_, Some(m)) => {
                let s = m + 7 + (lower[m + 7..].len() - lower[m + 7..].trim_start().len());
                match lower[s..].chars().next() {
                    Some(q @ ('"' | '\'')) => (s, lower[s + 1..].find(q).map_or(css.len(), |e| s + 1 + e + 1)),
                    // `@import url(…)` is picked up as a `url(` next time round.
                    _ => {
                        i = s;
                        continue;
                    }
                }
            }
            _ => break,
        };
        if !keep(unquote(&css[from..to]), None) {
            out.push_str(&css[copied..from]);
            if next_url.is_none_or(|u| from != u + 4) {
                out.push_str("\"\"");
            }
            copied = to;
        }
        i = to.max(from + 1).min(css.len());
        if i >= css.len() {
            break;
        }
    }
    out.push_str(&css[copied..]);
    out
}

/// An address inside `url(…)` or `@import`, without its spaces and quotes
/// (in a `style` attribute the quotes may be written `&quot;`).
fn unquote(s: &str) -> &str {
    let mut s = s.trim();
    for q in ["&quot;", "&#34;", "&#39;", "&apos;", "\"", "'"] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            s = inner.trim();
            break;
        }
    }
    s
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

/// Where an attribute sits in a tag: from its name to the end of its value
/// (closing quote included), and the value itself.
struct Attr {
    start: usize,
    end: usize,
    value: std::ops::Range<usize>,
}

/// Find attribute `name` (preceded by whitespace or `/`) in a single tag.
fn find_attr(tag: &str, name: &str) -> Option<Attr> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower.get(from..).and_then(|s| s.find(name)).map(|i| from + i) {
        from = i + name.len();
        let before_ok = lower[..i].ends_with(|c: char| c.is_ascii_whitespace() || c == '/');
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let eq = tag.len() - rest.len();
        let vstart = eq + 1 + (tag[eq + 1..].len() - tag[eq + 1..].trim_start().len());
        return Some(match tag[vstart..].chars().next() {
            Some(q @ ('"' | '\'')) => {
                let close = tag[vstart + 1..].find(q).map(|e| vstart + 1 + e);
                Attr { start: i, end: close.map_or(tag.len(), |c| c + 1), value: vstart + 1..close.unwrap_or(tag.len()) }
            }
            _ => {
                let e = tag[vstart..].find(|c: char| c.is_ascii_whitespace() || c == '>').map_or(tag.len(), |e| vstart + e);
                let v = tag[vstart..e].trim_end_matches('/');
                Attr { start: i, end: vstart + v.len(), value: vstart..vstart + v.len() }
            }
        });
    }
    None
}

/// An attribute's value from a single tag (`name=` preceded by whitespace).
pub fn tag_attr(tag: &str, name: &str) -> Option<String> {
    find_attr(tag, name).map(|a| tag[a.value].to_string())
}

/// The tag with attribute `name` given a new value, or removed (`None`).
fn set_attr(tag: &str, name: &str, value: Option<&str>) -> String {
    let Some(a) = find_attr(tag, name) else { return tag.to_string() };
    let new = value.map(|v| format!("{name}=\"{}\"", v.replace('"', "&quot;"))).unwrap_or_default();
    format!("{}{new}{}", &tag[..a.start], &tag[a.end..])
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

/// A picture's size from its `width`/`height` attributes or its style.
fn size(tag: &str) -> Size {
    let style = tag_attr(tag, "style").unwrap_or_default();
    let w = tag_attr(tag, "width").as_deref().and_then(px).or_else(|| css_prop(&style, "width").as_deref().and_then(px));
    let h = tag_attr(tag, "height").as_deref().and_then(px).or_else(|| css_prop(&style, "height").as_deref().and_then(px));
    (w, h)
}

/// Open-tracking URLs of common email services.
const TRACKER_PATHS: &[&str] = &[
    "/trk?", "/track/open", "/open.php", "/open.aspx", "/open.gif", "/wf/open", "/e/o/", "/o.gif", "/pixel.gif", "/pixel.png", "/beacon",
    "/e2t/to/", "/imp?", "/ci0/", "/oo/", "list-manage.com/track", "mandrillapp.com/track", "/mo/", "/t.gif",
];

/// A remote address in lower case without its `https://`, `http://` or
/// `//`; `None` for anything local (`cid:`, `data:`, relative).
fn remote_rest(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    ["https://", "http://", "//"].iter().find_map(|p| lower.strip_prefix(p)).map(str::to_string)
}

/// The host an address reports opens to, if it's a tracker: tiny (when
/// `size` is known) or on a known tracking path.
fn tracker_host(url: &str, size: Option<Size>) -> Option<String> {
    let rest = remote_rest(url)?;
    let host = rest.split(['/', '?', ':', '#']).next().unwrap_or("").to_string();
    let tiny = match size {
        Some((Some(w), Some(h))) => w <= 3.0 && h <= 3.0,
        Some((Some(d), None) | (None, Some(d))) => d <= 1.0,
        _ => false,
    };
    let path = rest.get(host.len()..).unwrap_or("");
    let known = TRACKER_PATHS.iter().any(|t| if t.starts_with('/') { path.contains(t) } else { rest.contains(t) });
    (tiny || known).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_pixels_are_removed() {
        // Marketo (Repco), Loops (Ollama), Mailchimp, SendGrid; and images to keep.
        let html = r#"<td><img src="https://info.example.com/images/logo.png" width="170" style="width:170px" alt="Logo" />
<img width="660" style="display:inline-block; width:660px;" src="https://info.example.com/images/Header Mobile.jpg" alt="x">
<img style="display:none; max-height:0" src="https://info.example.com/images/mobile-only.jpg" alt="mobile">
<img src="https://go.repco.example/trk?t=1&mid=abc" width="1" height="1" style="display:none !important;" alt="" />
<img alt="" src="https://c.vialoops.example/CI0/0100/xyz=452" style="display: none; width: 1px; height: 1px;">
<IMG SRC='https://shop.us1.list-manage.com/track/open.php?u=1&id=2' height=1 width=1>
<img src="https://u123.ct.sendgrid.net/wf/open?upn=abc" alt="" style="height:1px !important;width:1px !important;border-width:0 !important"></td>"#;
        let (clean, hosts) = strip_trackers(html);
        assert_eq!(hosts, ["go.repco.example", "c.vialoops.example", "shop.us1.list-manage.com", "u123.ct.sendgrid.net"]);
        assert!(clean.contains("logo.png") && clean.contains("Header Mobile.jpg") && clean.contains("mobile-only.jpg"));
        assert!(!clean.contains("trk?") && !clean.contains("CI0") && !clean.contains("track/open") && !clean.contains("wf/open"));
        assert!(clean.starts_with("<td>") && clean.ends_with("</td>"));
        // Nothing to do: returned unchanged.
        assert_eq!(strip_trackers("<p>Hi <b>there</b></p>"), ("<p>Hi <b>there</b></p>".to_string(), vec![]));
    }

    /// Strip, and check the tracker's gone, its host is reported once, and
    /// everything in `kept` is still there.
    fn blocks(html: &str, tracker: &str, host: &str, kept: &[&str]) {
        let (clean, hosts) = strip_trackers(html);
        assert!(!clean.contains(tracker), "tracker left in: {clean}");
        assert_eq!(hosts, [host], "for {html}");
        for k in kept {
            assert!(clean.contains(k), "{k:?} lost: {clean}");
        }
    }

    #[test]
    fn gap1_css_in_style_attributes() {
        blocks(
            r#"<td style="color:red; background-image:url(https://t.example/open.gif?u=1); padding:4px">Hi</td>"#,
            "t.example",
            "t.example",
            &["color:red", "padding:4px", ">Hi</td>"],
        );
        // Quoted, with entities, and in capitals.
        blocks(r#"<div style="BACKGROUND: URL(&quot;https://t.example/o.gif&quot;) no-repeat">x</div>"#, "t.example", "t.example", &["no-repeat", ">x</div>"]);
        // An ordinary background picture stays.
        let html = r#"<td style="background:url('https://cdn.example/hero.jpg')">x</td>"#;
        assert_eq!(strip_trackers(html), (html.to_string(), vec![]));
    }

    #[test]
    fn gap2_css_in_style_blocks() {
        blocks(
            "<style>.x{background:url('https://t.example/o.gif')} .y{color:blue}</style><p class=x>Hi</p>",
            "t.example",
            "t.example",
            &[".y{color:blue}", "<p class=x>Hi</p>", "</style>"],
        );
        blocks("<STYLE>@import \"https://t.example/track/open?u=1\"; p{margin:0}</STYLE>", "t.example", "t.example", &["p{margin:0}"]);
        blocks("<style>@import url( https://t.example/e/o/abc ); p{margin:0}</style>", "t.example", "t.example", &["p{margin:0}"]);
    }

    #[test]
    fn gap3_background_attribute() {
        blocks(r#"<table background="https://t.example/pixel.png" width="600"><tr><td>Hi</td></tr></table>"#, "t.example", "t.example", &["width=\"600\"", "<td>Hi</td>"]);
        let html = r#"<table background="https://cdn.example/paper.jpg">"#;
        assert_eq!(strip_trackers(html), (html.to_string(), vec![]));
    }

    #[test]
    fn gap4_srcset() {
        blocks(r#"<img src="data:image/gif;base64,R0lGOD,lh" srcset="https://t.example/open.gif 1x" alt="logo">"#, "t.example", "t.example", &["data:image/gif;base64,R0lGOD,lh", "alt=\"logo\""]);
        // Only the tracking candidate goes; the others stay.
        blocks(
            r#"<img src="https://cdn.example/a.png" srcset="https://cdn.example/a.png 1x, https://t.example/o.gif 2x, https://cdn.example/a@3x.png 3x">"#,
            "t.example",
            "t.example",
            &["https://cdn.example/a.png 1x", "https://cdn.example/a@3x.png 3x"],
        );
    }

    #[test]
    fn gap5_picture_sources() {
        blocks(
            r#"<picture><source srcset="https://t.example/wf/open?upn=1" media="(min-width:600px)"><img src="https://cdn.example/b.png"></picture>"#,
            "t.example",
            "t.example",
            &["<picture>", "https://cdn.example/b.png", "</picture>"],
        );
    }

    #[test]
    fn gap6_other_picture_tags() {
        blocks(r#"<input type="image" src="https://t.example/p.png" width="1" height="1"><p>Hi</p>"#, "t.example", "t.example", &["<p>Hi</p>"]);
        blocks(r#"<video poster="https://t.example/open.gif" width="300"></video>"#, "t.example", "t.example", &["<video", "width=\"300\""]);
        blocks(r#"<svg width="10" height="10"><image href="https://t.example/o.gif" width="10"/></svg>"#, "t.example", "t.example", &["<svg", "</svg>"]);
        blocks(r#"<svg><image xlink:href="https://t.example/beacon?x=1"/></svg>"#, "t.example", "t.example", &["</svg>"]);
        blocks(r#"<link rel="stylesheet" href="https://t.example/track/open.css"><p>Hi</p>"#, "t.example", "t.example", &["<p>Hi</p>"]);
        // An ordinary input picture and video poster stay.
        let html = r#"<input type="image" src="https://cdn.example/go.png" width="120" height="40"><video poster="https://cdn.example/still.jpg"></video>"#;
        assert_eq!(strip_trackers(html), (html.to_string(), vec![]));
    }

    #[test]
    fn gap7_known_path_without_a_size() {
        blocks(r#"<p>Hi</p><img src="https://x.example/wf/open?upn=1">"#, "x.example", "x.example", &["<p>Hi</p>"]);
    }

    #[test]
    fn a_host_is_reported_once() {
        let html = r#"<img src="https://t.example/o.gif"><td style="background:url(https://t.example/o.gif)" background="https://t.example/pixel.png">"#;
        assert_eq!(strip_trackers(html).1, ["t.example"]);
    }

    #[test]
    fn comments_are_left_alone() {
        let html = r#"<!--[if mso]><img src="https://cdn.example/a.png" title='it"s'><![endif]--><p>Hi</p>"#;
        assert_eq!(strip_trackers(html), (html.to_string(), vec![]));
    }

    #[test]
    fn remote_content_is_found() {
        assert!(has_remote_content("<img src=\"https://x.example/y.png\">"));
        assert!(!has_remote_content("<a href=\"https://x.example\">x</a>"));
        assert!(!has_remote_content("<img src=\"cid:logo\"><img src=\"data:image/png;base64,AA\">"));
        // Gap 8.
        assert!(has_remote_content(r#"<img src="data:image/gif;base64,AA" srcset="https://x.example/a.png 2x">"#));
        assert!(has_remote_content(r#"<img src="//x.example/a.png">"#));
        assert!(has_remote_content(r#"<div style="background:url(//x.example/a.png)">"#));
        assert!(has_remote_content(r#"<style>@import "https://x.example/a.css";</style>"#));
        assert!(has_remote_content(r#"<div style="background: url( 'https://x.example/a.png' )">"#));
        assert!(has_remote_content(r#"<picture><source srcset="https://x.example/a.webp"><img src="cid:a"></picture>"#));
        assert!(has_remote_content(r#"<video poster="https://x.example/a.jpg"></video>"#));
        assert!(has_remote_content(r#"<link rel="stylesheet" href="https://x.example/a.css">"#));
    }

    #[test]
    fn broken_emails_finish_quickly() {
        let mut html = String::new();
        for i in 0..20_000 {
            html.push_str(&format!("<td style=\"background:url('https://cdn.example/{i}.png\"><img src='https://t.example/o.gif?{i} <style>p{{ <img srcset=\"é, ,, 1x"));
        }
        html.push_str("<img src=\"https://t.example/o.gif");
        let started = std::time::Instant::now();
        let _ = strip_trackers(&html);
        let _ = has_remote_content(&html);
        let _ = strip_trackers("<");
        let _ = strip_trackers("<img src=");
        let _ = strip_trackers("<style>url(");
        let _ = strip_trackers("<p style=\"url(é");
        // Many addresses in one style block, none of them an `@import`.
        let css = format!("<style>{}</style>", "p{background:url(a.png)}".repeat(100_000));
        let _ = strip_trackers(&css);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "took {:?}", started.elapsed());
    }
}
