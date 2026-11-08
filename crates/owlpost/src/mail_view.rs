//! HTML email in the system web view (WebKit), as Mail, Canary and
//! Mailspring do it.
//!
//! The whole conversation is one page: a card per message (a `<details>`, so
//! folding needs no reload), each HTML body in its own sandboxed `<iframe>`
//! so a newsletter's CSS can't touch the rest. A strict Content Security
//! Policy blocks scripts, forms and, until "Load images", anything remote.
//! Links open in the browser; attachment clicks and keyboard shortcuts come
//! back to the app.

use crate::style::{self, Colors};
use eframe::egui;
use owlpost_core::{Message, MessageId};
pub use owlpost_core::html_clean::{has_remote_content, strip_trackers, tag_attr};

/// Something the page asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewEvent {
    /// A link was clicked: open it in the browser.
    OpenLink(String),
    /// Open (or right-click: save) attachment `usize` of a message.
    Attachment(MessageId, usize),
    /// Show remote images in this conversation.
    LoadImages,
    Reply,
    ReplyAll,
    Forward,
    /// A key pressed while the page had focus, for the app's shortcuts:
    /// (key, ⌘ held).
    Key(String, bool),
}

/// Map an `owlpost://` link from the page to an event.
pub fn parse_action(url: &str) -> Option<ViewEvent> {
    let rest = url.strip_prefix("owlpost://")?;
    let mut parts = rest.trim_end_matches('/').split('/');
    match parts.next()? {
        "attachment" => Some(ViewEvent::Attachment(parts.next()?.parse().ok()?, parts.next()?.parse().ok()?)),
        "load-images" => Some(ViewEvent::LoadImages),
        "reply" => Some(ViewEvent::Reply),
        "reply-all" => Some(ViewEvent::ReplyAll),
        "forward" => Some(ViewEvent::Forward),
        _ => None,
    }
}

fn css(c: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// "go.example.com", "a.com and b.com", "a.com, b.com and 1 more".
fn host_list(hosts: &[String]) -> String {
    match hosts {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
    }
}

/// The Content Security Policy for the page and every email in it.
pub fn csp(nonce: &str, remote: bool) -> String {
    let net = if remote { " https: http:" } else { "" };
    format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'unsafe-inline'{net}; img-src data: cid:{net}; font-src data:{net}; \
         frame-src 'self' about: data:; form-action 'none'; base-uri 'none'"
    )
}

/// Plain text as HTML: escaped, links made clickable, quoted lines folded.
pub fn text_html(text: &str) -> String {
    let mut out = String::new();
    let mut quote = String::new();
    let flush = |out: &mut String, quote: &mut String| {
        if !quote.is_empty() {
            out.push_str(&format!("<details class=quote><summary>•••</summary><div class=quoted>{}</div></details>", linkify(quote.trim_end())));
            quote.clear();
        }
    };
    for line in text.lines() {
        if line.trim_start().starts_with('>') || (line.starts_with("On ") && line.trim_end().ends_with("wrote:")) {
            quote.push_str(line);
            quote.push('\n');
        } else {
            flush(&mut out, &mut quote);
            out.push_str(&linkify(line));
            out.push('\n');
        }
    }
    flush(&mut out, &mut quote);
    format!("<div class=text>{}</div>", out.trim_end())
}

fn linkify(s: &str) -> String {
    let mut out = String::new();
    for (i, word) in s.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let trimmed = word.trim_end_matches(['.', ',', ')', ';', ':']);
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            let tail = &word[trimmed.len()..];
            out.push_str(&format!("<a href=\"{0}\">{0}</a>{1}", escape(trimmed), escape(tail)));
        } else {
            out.push_str(&escape(word));
        }
    }
    out
}

/// What to show and how.
pub struct Page<'a> {
    pub messages: &'a [Message],
    /// Which messages start unfolded.
    pub expanded: &'a [bool],
    pub remote_images: bool,
    pub nonce: &'a str,
    /// A sender's picture as a data URI (and whether it fills the circle).
    pub avatar: &'a dyn Fn(&str) -> Option<(String, bool)>,
    /// In a dark theme, recolour emails to match (light backgrounds go
    /// dark, dark text goes light) instead of showing them on white paper.
    pub dark_emails: bool,
}

/// The whole conversation as one HTML document.
pub fn document(page: &Page, col: &Colors) -> String {
    let policy = csp(page.nonce, page.remote_images);
    let dark = col.p.dark;
    let recolor = dark && page.dark_emails;
    // Tracking pixels never load, whatever the image setting.
    let mut trackers: Vec<String> = Vec::new();
    let bodies: Vec<Option<String>> = page
        .messages
        .iter()
        .map(|m| {
            m.body_html.as_deref().filter(|h| !h.trim().is_empty()).map(|h| {
                let (clean, hosts) = strip_trackers(h);
                for host in hosts {
                    if !trackers.contains(&host) {
                        trackers.push(host);
                    }
                }
                clean
            })
        })
        .collect();
    let blocked = !page.remote_images && bodies.iter().flatten().any(|h| has_remote_content(h));
    let mut body = String::new();
    let tracked = if trackers.is_empty() {
        String::new()
    } else {
        format!(
            "<span class=shield title=\"Tracking pixels report when and where you open an email. OwlPost never loads them.\">Blocked {} from {}.</span> ",
            if trackers.len() == 1 { "a tracker" } else { "trackers" },
            escape(&host_list(&trackers))
        )
    };
    if blocked {
        body.push_str(&format!(
            "<div class=banner>{tracked}Remote images are hidden to protect your privacy. <a href=\"owlpost://load-images\">Load images</a></div>"
        ));
    } else if !tracked.is_empty() {
        body.push_str(&format!("<div class=\"banner quiet\">{tracked}</div>"));
    }
    let n = page.messages.len();
    // Newest first; Reply etc. sit right under the newest message.
    for i in (0..n).rev() {
        let m = &page.messages[i];
        let open = page.expanded.get(i).copied().unwrap_or(true) || i + 1 == n;
        let who = if m.from.name.is_empty() { m.from.email.clone() } else { m.from.name.clone() };
        let initial = who.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let mut to = format!("to {}", m.to.iter().map(|c| c.display()).collect::<Vec<_>>().join(", "));
        if !m.cc.is_empty() {
            to.push_str(&format!(" · cc {}", m.cc.iter().map(|c| c.display()).collect::<Vec<_>>().join(", ")));
        }
        let email = if m.from.name.is_empty() { String::new() } else { format!("<span class=addr>&lt;{}&gt;</span>", escape(&m.from.email)) };
        body.push_str(&format!(
            "<details class=card{open}><summary>{face}\
             <div class=who><div><b>{name}</b> {email}</div><div class=to>{to}</div><div class=snippet>{snippet}</div></div>\
             <div class=date>{date}{star}</div></summary><div class=content>",
            open = if open { " open" } else { "" },
            face = match (page.avatar)(&m.from.email) {
                Some((uri, true)) => format!("<img class=\"avatar photo\" alt=\"\" src=\"{}\">", escape(&uri)),
                Some((uri, false)) => format!("<div class=\"avatar icon\"><img alt=\"\" src=\"{}\"></div>", escape(&uri)),
                None => format!(
                    "<div class=avatar style=\"background:{}\">{}</div>",
                    css(style::avatar_color(if m.from.email.is_empty() { &who } else { &m.from.email })),
                    escape(&initial)
                ),
            },
            name = escape(&who),
            to = escape(&to),
            snippet = escape(&m.snippet),
            date = escape(&style::long_date(m.date)),
            star = if m.starred { " <span class=star>★</span>" } else { "" },
        ));
        body.push_str("<hr>");
        if !m.attachments.is_empty() {
            body.push_str("<div class=attachments>");
            for (j, a) in m.attachments.iter().enumerate() {
                body.push_str(&format!(
                    "<a class=attachment href=\"owlpost://attachment/{}/{j}\" title=\"Open\">📎 {} <span>{}</span></a>",
                    m.id,
                    escape(&a.filename),
                    escape(&crate::attachments::human_size(a.size))
                ));
            }
            body.push_str("</div><hr>");
        }
        match bodies[i].as_deref() {
            Some(html) => {
                // Each email in its own sandbox: no scripts, no forms, its
                // CSS kept to itself. Links go to the top page (and so to
                // the browser).
                let colours = if recolor {
                    format!(":root{{color-scheme:dark;}} html{{background:{};}} body{{color:{};}} a{{color:{};}}", css(col.bg), css(col.text), css(col.link))
                } else {
                    "body{color:#1d1d1f;}".to_string()
                };
                let inner = format!(
                    "<meta charset=utf-8><meta http-equiv=\"Content-Security-Policy\" content=\"{}\"><base target=\"_top\">\
                     <style>html,body{{margin:0;padding:0;}} body{{font:14px/1.45 -apple-system,system-ui,sans-serif;word-wrap:break-word;}} img{{max-width:100%;height:auto;}} {colours}</style>{}",
                    escape(&policy),
                    html
                );
                body.push_str(&format!(
                    "<div class=\"html{}\"><iframe sandbox=\"allow-same-origin allow-popups allow-top-navigation-by-user-activation\" srcdoc=\"{}\"></iframe></div>",
                    if dark && !recolor { " paper" } else { "" },
                    escape(&inner)
                ));
            }
            None => body.push_str(&text_html(&m.body_text)),
        }
        body.push_str("</div></details>");
        if i + 1 == n {
            body.push_str(
                "<div class=actions><a class=btn href=\"owlpost://reply\">Reply</a><a class=btn href=\"owlpost://reply-all\">Reply all</a>\
                 <a class=btn href=\"owlpost://forward\">Forward</a></div>",
            );
        }
    }
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="{policy}">
<style>
:root {{ color-scheme: {scheme}; }}
html, body {{ margin: 0; background: {bg}; color: {text}; font: 14px/1.5 -apple-system, system-ui, sans-serif; }}
body {{ padding: 2px 48px 40px; max-width: 860px; }}
a {{ color: {link}; }}
.banner {{ background: {bg2}; border: 1px solid {border}; border-radius: 8px; padding: 9px 14px; margin-bottom: 14px; color: {subtle}; font-size: 13px; }}
.banner.quiet {{ padding: 6px 14px; font-size: 12px; }}
.shield::before {{ content: ""; display: inline-block; width: 11px; height: 13px; margin-right: 6px; vertical-align: -2px; background: currentColor; opacity: .7;
  -webkit-mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 11 13'%3E%3Cpath d='M5.5 0 11 2v4.2C11 9.5 8.7 12 5.5 13 2.3 12 0 9.5 0 6.2V2z'/%3E%3C/svg%3E") no-repeat center / contain; }}
.card {{ border-top: 1px solid {hair}; margin-bottom: 8px; }}
.card > summary {{ list-style: none; display: flex; gap: 12px; align-items: flex-start; padding: 18px 0 0; cursor: default; }}
.card:not([open]) > summary {{ padding-bottom: 18px; }}
.card > summary::-webkit-details-marker {{ display: none; }}
.avatar {{ flex: none; width: 36px; height: 36px; border-radius: 50%; color: #fff; display: grid; place-items: center; font-weight: 600; font-size: 15px; }}
.avatar.photo {{ object-fit: cover; background: #fff; }}
.avatar.icon {{ background: #fff; box-shadow: inset 0 0 0 1px rgba(0,0,0,.07); }}
.avatar.icon img {{ width: 22px; height: 22px; object-fit: contain; }}
.who {{ flex: 1; min-width: 0; }}
.who b {{ font-weight: 600; }}
.addr, .date {{ color: {very_subtle}; font-size: 12px; }}
.date {{ flex: none; white-space: nowrap; padding-top: 2px; }}
.star {{ color: {star}; }}
.to {{ color: {subtle}; font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
.snippet {{ display: none; color: {subtle}; font-size: 12.5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
.card:not([open]) .snippet {{ display: block; }}
.card:not([open]) .to {{ display: none; }}
.content {{ padding: 0 0 24px; }}
hr {{ border: 0; border-top: 1px solid {hair}; margin: 12px 0; }}
.content > hr:last-of-type {{ margin-bottom: 20px; }}
.attachments {{ display: flex; flex-wrap: wrap; gap: 8px; }}
.attachment {{ text-decoration: none; color: {text}; background: {bg2}; border: 1px solid {border}; border-radius: 6px; padding: 6px 10px; font-size: 13px; }}
.attachment span {{ color: {subtle}; font-size: 11.5px; margin-left: 4px; }}
.attachment:hover {{ filter: brightness(0.96); }}
.text {{ white-space: pre-wrap; word-wrap: break-word; }}
.quote summary {{ color: {very_subtle}; cursor: pointer; list-style: none; font-size: 12px; }}
.quoted {{ color: {subtle}; white-space: pre-wrap; border-left: 2px solid {hair}; padding-left: 10px; margin-top: 4px; }}
.html iframe {{ border: 0; width: 100%; display: block; min-height: 40px; }}
.html.paper {{ background: #fff; border-radius: 6px; padding: 12px; }}
.actions {{ display: flex; gap: 8px; margin: 4px 0 22px; }}
.btn {{ text-decoration: none; color: {text}; border: 1px solid {border}; border-radius: 16px; padding: 5px 16px; font-size: 13px; background: {bg2}; }}
.btn:hover {{ filter: brightness(0.96); }}
</style></head><body>{body}
<script nonce="{nonce}">var OWL_DARK = {dark_js};{script}</script></body></html>"#,
        policy = escape(&policy),
        scheme = if dark { "dark" } else { "light" },
        bg = css(col.bg),
        bg2 = css(col.bg_secondary),
        text = css(col.text),
        subtle = css(col.subtle),
        very_subtle = css(col.very_subtle),
        link = css(col.link),
        border = css(style::blend(col.border, col.bg, 0.6)),
        hair = css(style::blend(col.border, col.bg, 0.7)),
        star = css(col.star),
        nonce = page.nonce,
        dark_js = if recolor { format!("{{bg:{},text:{}}}", rgb_js(col.bg), rgb_js(col.text)) } else { "null".into() },
        script = SCRIPT,
    )
}

/// A colour as a JS `[r,g,b]` array.
fn rgb_js(c: egui::Color32) -> String {
    format!("[{},{},{}]", c.r(), c.g(), c.b())
}

/// The page's only script: size each email frame to its content, and pass
/// shortcut keys to the app (the web view has keyboard focus once clicked).
const SCRIPT: &str = r#"
// Dark theme: recolour an email to match. Light backgrounds are darkened
// and dark text lightened, keeping each colour's hue, so a white email
// becomes the theme's background with light text, coloured bands and
// buttons keep their colour, and anything already dark is left alone.
// Pictures are untouched.
function hsl(c) {
  var r = c[0] / 255, g = c[1] / 255, b = c[2] / 255;
  var mx = Math.max(r, g, b), mn = Math.min(r, g, b), l = (mx + mn) / 2, h = 0, s = 0;
  if (mx !== mn) {
    var d = mx - mn; s = l > 0.5 ? d / (2 - mx - mn) : d / (mx + mn);
    h = mx === r ? (g - b) / d + (g < b ? 6 : 0) : mx === g ? (b - r) / d + 2 : (r - g) / d + 4; h /= 6;
  }
  return [h, s, l];
}
function css(h, s, l, a) {
  return 'hsla(' + (h * 360).toFixed(1) + ',' + (s * 100).toFixed(1) + '%,' + (l * 100).toFixed(1) + '%,' + a + ')';
}
function parse(v) {
  var m = /rgba?\(([\d.]+),\s*([\d.]+),\s*([\d.]+)(?:,\s*([\d.]+))?/.exec(v || '');
  return m ? [+m[1], +m[2], +m[3], m[4] === undefined ? 1 : +m[4]] : null;
}
function recolor(d) {
  if (!OWL_DARK || !d || !d.body || d.owlDark) return;
  d.owlDark = true;
  var theme = hsl(OWL_DARK.bg), bgL = theme[2], textL = hsl(OWL_DARK.text)[2];
  var els = [d.documentElement].concat(Array.prototype.slice.call(d.body.querySelectorAll('*')));
  els.splice(1, 0, d.body);
  // Read everything first, then write, so the page lays out once.
  var todo = [];
  els.forEach(function (el) {
    if (/^(IMG|SVG|VIDEO|PICTURE|CANVAS)$/i.test(el.tagName)) return;
    var cs = d.defaultView.getComputedStyle(el);
    var bg = parse(cs.backgroundColor), fg = parse(cs.color), set = {};
    var picture = cs.backgroundImage && cs.backgroundImage !== 'none';
    if (bg && bg[3] > 0 && !picture) {
      var b = hsl(bg);
      // Light (L 0.5–1) maps onto theme background–mid grey; white is the theme's own.
      var L = bgL + (1 - b[2]) * (0.5 - bgL) / 0.5;
      // Near-white (white, cream, pale grey) becomes the theme's own
      // background, a touch lighter the less white it was; paler colours
      // keep their hue.
      if (b[2] > 0.85) set.bg = css(theme[0], theme[1], L, bg[3]);
      else if (b[2] > 0.5) set.bg = css(b[0], b[1] * 0.6, L, bg[3]);
    }
    if (fg && fg[3] > 0) {
      var t = hsl(fg);
      // Dark (L 0–0.5) maps onto theme text–mid grey; black is the theme's text.
      if (t[2] < 0.5) set.fg = css(t[0], t[1], textL - t[2] * (textL - 0.6) / 0.5, fg[3]);
    }
    var bc = parse(cs.borderTopColor);
    if (bc && bc[3] > 0 && parseFloat(cs.borderTopWidth) > 0) {
      var k = hsl(bc);
      if (k[2] > 0.6) set.border = css(k[0], k[1] * 0.8, bgL + (1 - k[2]) * (0.6 - bgL) / 0.4 + 0.08, bc[3]);
    }
    if (set.bg || set.fg || set.border) todo.push([el, set]);
  });
  todo.forEach(function (x) {
    var st = x[0].style, set = x[1];
    if (set.bg) st.setProperty('background-color', set.bg, 'important');
    if (set.fg) st.setProperty('color', set.fg, 'important');
    if (set.border) st.setProperty('border-color', set.border, 'important');
  });
}
function fit(f) {
  try {
    var d = f.contentDocument; if (!d || !d.documentElement) return;
    f.style.height = Math.max(d.documentElement.scrollHeight, d.body ? d.body.scrollHeight : 0) + 'px';
  } catch (e) {}
}
document.querySelectorAll('.html iframe').forEach(function (f) {
  f.addEventListener('load', function () {
    try { recolor(f.contentDocument); } catch (e) {}
    fit(f);
    try {
      new ResizeObserver(function () { fit(f); }).observe(f.contentDocument.body);
      f.contentDocument.addEventListener('keydown', forward);
    } catch (e) {}
    setTimeout(function () { fit(f); }, 300);
  });
});
// Size them now too, and once everything (fonts, images) has loaded.
document.querySelectorAll('.html iframe').forEach(function (f) {
  try { if (f.contentDocument && f.contentDocument.readyState === 'complete') recolor(f.contentDocument); } catch (e) {}
  fit(f);
});
window.addEventListener('load', function () {
  var frames = document.querySelectorAll('.html iframe');
  frames.forEach(fit);
  // Report how the page came out (logged by the app, for diagnosis).
  setTimeout(function () {
    var report = { ready: [], scripted: false };
    frames.forEach(function (f) {
      report.ready.push(f.offsetHeight);
      // Ran = the email's script replaced its whole body (as the demo's tries to).
      try { if (f.contentDocument.body.innerHTML.trim() === 'hacked') report.scripted = true; } catch (e) {}
    });
    window.ipc.postMessage(JSON.stringify(report));
  }, 500);
});
document.querySelectorAll('details.card').forEach(function (d) {
  d.addEventListener('toggle', function () { d.querySelectorAll('.html iframe').forEach(fit); });
});
function forward(e) {
  var t = e.target; if (t && (t.isContentEditable || /INPUT|TEXTAREA|SELECT/.test(t.tagName))) return;
  // The page keeps copy, select-all and find.
  if (e.metaKey && /^[cafgv]$/i.test(e.key)) return;
  if (e.altKey || e.ctrlKey) return;
  var keys = ['ArrowUp','ArrowDown','Escape','Delete','Backspace','Enter'];
  if (e.key.length === 1 || keys.indexOf(e.key) >= 0) {
    window.ipc.postMessage(JSON.stringify({ key: e.key, meta: e.metaKey }));
    e.preventDefault();
  }
}
document.addEventListener('keydown', forward);
"#;

/// Turn a key from the page into egui input, so the app's shortcuts work
/// while the message has focus.
pub fn key_events(key: &str, command: bool) -> Vec<egui::Event> {
    let modifiers = egui::Modifiers { command, mac_cmd: command, ..Default::default() };
    let named = match key {
        "ArrowUp" => Some(egui::Key::ArrowUp),
        "ArrowDown" => Some(egui::Key::ArrowDown),
        "Escape" => Some(egui::Key::Escape),
        "Delete" => Some(egui::Key::Delete),
        "Backspace" => Some(egui::Key::Backspace),
        "Enter" => Some(egui::Key::Enter),
        k => egui::Key::from_name(&k.to_ascii_uppercase()).filter(|_| command),
    };
    let mut out = vec![];
    if let Some(k) = named {
        for pressed in [true, false] {
            out.push(egui::Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers });
        }
    } else if !command && key.chars().count() == 1 {
        out.push(egui::Event::Text(key.to_string()));
    }
    out
}

#[cfg(target_os = "macos")]
pub use native::MailView;

#[cfg(target_os = "linux")]
pub use offscreen::MailView;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub use fallback::MailView;

#[cfg(target_os = "macos")]
mod native {
    use super::{parse_action, ViewEvent};
    use crossbeam_channel::{Receiver, Sender};
    use eframe::egui;
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::raw_window_handle::HasWindowHandle;

    /// The web view, laid over the reading pane.
    pub struct MailView {
        view: wry::WebView,
        rx: Receiver<ViewEvent>,
        /// Identifies the page now loaded, to reload only when it changes.
        loaded: u64,
        visible: bool,
        bounds: egui::Rect,
    }

    impl MailView {
        pub fn new(window: &impl HasWindowHandle, bg: egui::Color32) -> Option<Self> {
            let (tx, rx) = crossbeam_channel::unbounded::<ViewEvent>();
            let (nav, ipc, popup): (Sender<_>, Sender<_>, Sender<_>) = (tx.clone(), tx.clone(), tx);
            let view = wry::WebViewBuilder::new()
                .with_bounds(wry::Rect { position: LogicalPosition::new(0.0, 0.0).into(), size: LogicalSize::new(1.0, 1.0).into() })
                .with_visible(false)
                .with_background_color((bg.r(), bg.g(), bg.b(), 255))
                .with_accept_first_mouse(true)
                .with_focused(false)
                .with_navigation_handler(move |url| {
                    // Our own page loads; everything else is ours to handle.
                    if url.starts_with("about:") || url.starts_with("data:") {
                        return true;
                    }
                    let ev = parse_action(&url).unwrap_or(ViewEvent::OpenLink(url));
                    let _ = nav.send(ev);
                    false
                })
                .with_new_window_req_handler(move |url, _| {
                    let _ = popup.send(parse_action(&url).unwrap_or(ViewEvent::OpenLink(url)));
                    wry::NewWindowResponse::Deny
                })
                .with_ipc_handler(move |req| {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(req.body()) {
                        if v.get("ready").is_some() {
                            log::info!("mail view ready: email frame heights {}, email scripts ran: {}", v["ready"], v["scripted"]);
                        }
                        if let Some(k) = v["key"].as_str() {
                            let _ = ipc.send(ViewEvent::Key(k.to_string(), v["meta"].as_bool().unwrap_or(false)));
                        }
                    }
                })
                .build_as_child(window)
                .map_err(|e| log::warn!("couldn't create the mail web view: {e}"))
                .ok()?;
            log::debug!("mail view: web view created");
            Some(Self { view, rx, loaded: 0, visible: false, bounds: egui::Rect::NOTHING })
        }

        /// Show `html` over `rect` (in window points). Reloads only when
        /// `key` changes.
        pub fn show(&mut self, _ctx: &egui::Context, rect: egui::Rect, zoom: f32, key: u64, html: impl FnOnce() -> String) {
            if rect != self.bounds {
                self.bounds = rect;
                let _ = self.view.set_bounds(wry::Rect {
                    position: LogicalPosition::new((rect.left() * zoom) as f64, (rect.top() * zoom) as f64).into(),
                    size: LogicalSize::new((rect.width() * zoom).max(1.0) as f64, (rect.height() * zoom).max(1.0) as f64).into(),
                });
            }
            if key != self.loaded {
                self.loaded = key;
                let page = html();
                log::debug!("mail view: loading a {} byte page at {rect:?}", page.len());
                if let Err(e) = self.view.load_html(&page) {
                    log::warn!("mail view: couldn't load the page: {e}");
                }
            }
            if !self.visible {
                self.visible = true;
                let _ = self.view.set_visible(true);
            }
        }

        pub fn hide(&mut self) {
            if self.visible {
                self.visible = false;
                let _ = self.view.set_visible(false);
                let _ = self.view.focus_parent();
            }
        }

        pub fn events(&mut self) -> Vec<ViewEvent> {
            self.rx.try_iter().collect()
        }

    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod fallback {
    use super::ViewEvent;
    use eframe::egui;

    /// No web view on this platform yet: messages are shown as text.
    pub struct MailView;

    impl MailView {
        pub fn new<W>(_window: &W, _bg: egui::Color32) -> Option<Self> {
            None
        }
        pub fn show(&mut self, _ctx: &egui::Context, _rect: egui::Rect, _zoom: f32, _key: u64, _html: impl FnOnce() -> String) {}
        pub fn hide(&mut self) {}
        pub fn events(&mut self) -> Vec<ViewEvent> {
            vec![]
        }
    }
}

/// Linux: the conversation drawn by WebKitGTK off screen, shown as a picture.
///
/// Wayland has no way to put another toolkit's view inside our window, so
/// WebKitGTK renders the page in an offscreen window on the GTK thread and
/// hands back a full-page snapshot (sharp on HiDPI screens), with where the
/// links and folds are. The app paints it, scrolls it, and turns clicks into
/// the same events the macOS view sends. Emails run no scripts, so a picture
/// of the page is the page. (Text can't be selected yet.)
#[cfg(target_os = "linux")]
mod offscreen {
    use super::{parse_action, ViewEvent};
    use eframe::egui;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    enum Hit {
        Link(String),
        /// An older message's header: fold it open or shut.
        Fold(usize),
    }

    /// A clickable spot, in page (CSS) pixels.
    #[derive(Clone, Debug)]
    struct Target {
        rect: egui::Rect,
        hit: Hit,
    }

    /// A finished render.
    struct Rendered {
        key: u64,
        width: u32,
        /// Device pixels per CSS pixel.
        scale: f32,
        image: egui::ColorImage,
        targets: Vec<Target>,
    }

    #[derive(Default)]
    struct Shared {
        rendered: Option<Rendered>,
    }

    pub struct MailView {
        shared: Arc<Mutex<Shared>>,
        /// What was last asked for: page key and width.
        requested: (u64, u32),
        /// Picture tiles: texture, top (in points), height (in points).
        tiles: Vec<(egui::TextureHandle, f32, f32)>,
        /// The shown page's size in points, and its targets.
        size: egui::Vec2,
        targets: Vec<Target>,
        scroll: f32,
        events: Vec<ViewEvent>,
        bg: egui::Color32,
        shown_key: u64,
    }

    impl MailView {
        pub fn new<W>(_window: &W, bg: egui::Color32) -> Option<Self> {
            if !crate::web_host::available() {
                return None;
            }
            Some(Self {
                shared: Arc::default(),
                requested: (0, 0),
                tiles: vec![],
                size: egui::Vec2::ZERO,
                targets: vec![],
                scroll: 0.0,
                events: vec![],
                bg,
                shown_key: 0,
            })
        }

        pub fn show(&mut self, ctx: &egui::Context, rect: egui::Rect, _zoom: f32, key: u64, html: impl FnOnce() -> String) {
            let width = rect.width().round().max(1.0) as u32;
            if (key, width) != self.requested {
                if key != self.requested.0 {
                    self.scroll = 0.0;
                }
                self.requested = (key, width);
                let scale = ctx.pixels_per_point();
                request(Job::Load { key, width, scale, html: html() }, self.shared.clone(), ctx.clone());
            }
            self.take_render(ctx);
            self.paint_and_handle(ctx, rect);
        }

        /// A new picture from the GTK thread, cut into texture-sized tiles.
        fn take_render(&mut self, ctx: &egui::Context) {
            let Some(r) = self.shared.lock().ok().and_then(|mut s| s.rendered.take()) else { return };
            if r.key != self.requested.0 {
                return;
            }
            let max = ctx.input(|i| i.max_texture_side).max(1024);
            let [w, h] = r.image.size;
            self.tiles.clear();
            let mut top = 0;
            while top < h {
                let rows = (h - top).min(max);
                let mut tile = egui::ColorImage::new([w, rows], vec![egui::Color32::TRANSPARENT; w * rows]);
                tile.pixels.copy_from_slice(&r.image.pixels[top * w..(top + rows) * w]);
                let tex = ctx.load_texture(format!("mail-view-{top}"), tile, egui::TextureOptions::LINEAR);
                self.tiles.push((tex, top as f32 / r.scale, rows as f32 / r.scale));
                top += rows;
            }
            self.size = egui::vec2(w as f32 / r.scale, h as f32 / r.scale);
            self.targets = r.targets;
            self.shown_key = r.key;
            let _ = r.width;
        }

        fn paint_and_handle(&mut self, ctx: &egui::Context, rect: egui::Rect) {
            let max_scroll = (self.size.y - rect.height()).max(0.0);
            // Input: only when nothing (a menu, a popup) is above the pane.
            let pointer = ctx.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p) && ctx.layer_id_at(*p).is_none_or(|l| l.order == egui::Order::Background));
            if pointer.is_some() {
                let dy = ctx.input(|i| i.smooth_scroll_delta.y);
                self.scroll = (self.scroll - dy).clamp(0.0, max_scroll);
            }
            self.scroll = self.scroll.clamp(0.0, max_scroll);
            let origin = rect.min - egui::vec2(0.0, self.scroll);
            if let Some(p) = pointer {
                let at = (p - origin).to_pos2();
                if let Some(t) = self.targets.iter().find(|t| t.rect.contains(at)).cloned() {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    if ctx.input(|i| i.pointer.primary_clicked()) {
                        match t.hit {
                            Hit::Link(url) => self.events.push(parse_action(&url).unwrap_or(ViewEvent::OpenLink(url))),
                            Hit::Fold(i) => request(Job::Fold(i), self.shared.clone(), ctx.clone()),
                        }
                    }
                }
            }
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("mail_view"))).with_clip_rect(rect);
            painter.rect_filled(rect, 0.0, self.bg);
            if self.tiles.is_empty() || self.shown_key != self.requested.0 {
                painter.text(rect.center_top() + egui::vec2(0.0, 40.0), egui::Align2::CENTER_TOP, "Loading…", egui::FontId::proportional(14.0), egui::Color32::GRAY);
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                if self.tiles.is_empty() {
                    return;
                }
            }
            for (tex, top, height) in &self.tiles {
                let r = egui::Rect::from_min_size(origin + egui::vec2(0.0, *top), egui::vec2(self.size.x, *height));
                if r.intersects(rect) {
                    painter.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
            }
            // A thin scroll indicator when the page is taller than the pane.
            if max_scroll > 0.0 {
                let frac = rect.height() / self.size.y;
                let h = (rect.height() * frac).max(24.0);
                let y = rect.top() + (rect.height() - h) * (self.scroll / max_scroll);
                let bar = egui::Rect::from_min_size(egui::pos2(rect.right() - 6.0, y), egui::vec2(4.0, h));
                painter.rect_filled(bar, 2.0, egui::Color32::from_gray(128).gamma_multiply(0.5));
            }
        }

        pub fn hide(&mut self) {}

        pub fn events(&mut self) -> Vec<ViewEvent> {
            std::mem::take(&mut self.events)
        }
    }

    enum Job {
        Load { key: u64, width: u32, scale: f32, html: String },
        Fold(usize),
    }

    /// Hand a job to the GTK thread.
    fn request(job: Job, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
        crate::web_host::on_gtk_thread(move || gtk_side::run(job, shared, ctx));
    }

    /// Runs on the GTK thread.
    mod gtk_side {
        use super::{Hit, Job, Rendered, Shared, Target};
        use eframe::egui;
        use gtk::prelude::*;
        use std::cell::RefCell;
        use std::sync::{Arc, Mutex};
        use webkit2gtk::{LoadEvent, SnapshotOptions, SnapshotRegion, WebViewExt};

        struct Off {
            _window: gtk::OffscreenWindow,
            view: webkit2gtk::WebView,
            key: u64,
            width: u32,
            scale: f32,
            /// Bumped per load, so a late callback from an older page is ignored.
            gen: u64,
            shared: Arc<Mutex<Shared>>,
            ctx: egui::Context,
        }

        thread_local! {
            static OFF: RefCell<Option<Off>> = const { RefCell::new(None) };
        }

        pub fn run(job: Job, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
            match job {
                Job::Load { key, width, scale, html } => load(key, width, scale, html, shared, ctx),
                Job::Fold(i) => {
                    let gen = OFF.with(|o| {
                        o.borrow().as_ref().map(|off| {
                            let js = format!("(function(){{var d=document.querySelectorAll('details.card')[{i}]; if(d) d.open=!d.open; return '';}})()");
                            off.view.run_javascript(&js, None::<&gtk::gio::Cancellable>, |_| {});
                            off.gen
                        })
                    });
                    if let Some(gen) = gen {
                        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || snapshot(gen));
                    }
                }
            }
        }

        fn load(key: u64, width: u32, scale: f32, html: String, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
            OFF.with(|o| {
                let mut o = o.borrow_mut();
                if o.is_none() {
                    let window = gtk::OffscreenWindow::new();
                    let view = webkit2gtk::WebView::new();
                    view.connect_load_changed(|_, ev| {
                        if ev == LoadEvent::Finished {
                            let gen = OFF.with(|o| o.borrow().as_ref().map(|off| off.gen));
                            if let Some(gen) = gen {
                                // Once the email frames have sized themselves, then
                                // again after late pictures and fonts arrive.
                                for ms in [250, 1500] {
                                    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || snapshot(gen));
                                }
                            }
                        }
                    });
                    window.add(&view);
                    window.show_all();
                    *o = Some(Off { _window: window, view, key: 0, width: 0, scale: 1.0, gen: 0, shared: shared.clone(), ctx: ctx.clone() });
                }
                let off = o.as_mut().expect("just made");
                off.key = key;
                off.width = width;
                off.scale = scale;
                off.gen += 1;
                off.shared = shared;
                off.ctx = ctx;
                // GTK may already draw at the screen's scale (HiDPI); zoom the
                // page for whatever's left, so the picture is sharp but the
                // layout stays `width` CSS pixels wide.
                let gdk = off.view.scale_factor().max(1) as f32;
                let zoom = (scale / gdk).max(0.25);
                off.view.set_zoom_level(zoom as f64);
                off.view.set_size_request((width as f32 * zoom).round() as i32, (600.0 * zoom) as i32);
                off.view.load_html(&html, None);
            });
        }

        /// Where the page's links and folds are (CSS pixels), and its height.
        const MEASURE: &str = r#"(function () {
  var out = [];
  function add(el, ox, oy, extra) {
    var r = el.getBoundingClientRect();
    if (r.width > 0 && r.height > 0) out.push(Object.assign({ x: r.left + ox + scrollX, y: r.top + oy + scrollY, w: r.width, h: r.height }, extra));
  }
  document.querySelectorAll('a[href]').forEach(function (a) { add(a, 0, 0, { href: a.href }); });
  document.querySelectorAll('details.card').forEach(function (d, i) { var s = d.querySelector('summary'); if (s) add(s, 0, 0, { fold: i }); });
  document.querySelectorAll('.html iframe').forEach(function (f) {
    try {
      var fr = f.getBoundingClientRect();
      if (fr.height === 0) return;
      f.contentDocument.querySelectorAll('a[href]').forEach(function (a) { add(a, fr.left, fr.top, { href: a.href }); });
    } catch (e) {}
  });
  var h = Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0);
  return JSON.stringify({ h: h, t: out });
})()"#;

        fn snapshot(gen: u64) {
            let view = OFF.with(|o| o.borrow().as_ref().filter(|off| off.gen == gen).map(|off| off.view.clone()));
            let Some(view) = view else { return };
            let runner = view.clone();
            runner.run_javascript(MEASURE, None::<&gtk::gio::Cancellable>, move |r| {
                use javascriptcore::ValueExt;
                let json = r.ok().and_then(|r| r.js_value()).map(|v| v.to_str().to_string()).unwrap_or_default();
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let targets: Vec<Target> = v["t"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| {
                        let rect = egui::Rect::from_min_size(egui::pos2(t["x"].as_f64()? as f32, t["y"].as_f64()? as f32), egui::vec2(t["w"].as_f64()? as f32, t["h"].as_f64()? as f32));
                        let hit = match (t["href"].as_str(), t["fold"].as_u64()) {
                            (Some(h), _) => Hit::Link(h.to_string()),
                            (None, Some(i)) => Hit::Fold(i as usize),
                            _ => return None,
                        };
                        Some(Target { rect, hit })
                    })
                    .collect();
                view.snapshot(SnapshotRegion::FullDocument, SnapshotOptions::NONE, None::<&gtk::gio::Cancellable>, move |s| {
                    let Ok(surface) = s else { return };
                    let Some(image) = to_image(surface) else { return };
                    OFF.with(|o| {
                        if let Some(off) = o.borrow().as_ref().filter(|off| off.gen == gen) {
                            if let Ok(mut sh) = off.shared.lock() {
                                // Device pixels per CSS pixel, as it actually came out.
                                let scale = image.size[0] as f32 / off.width.max(1) as f32;
                                let _ = off.scale;
                                sh.rendered = Some(Rendered { key: off.key, width: off.width, scale, image, targets });
                            }
                            off.ctx.request_repaint();
                        }
                    });
                });
            });
        }

        /// A cairo snapshot (premultiplied BGRA) as an egui image.
        fn to_image(surface: gtk::cairo::Surface) -> Option<egui::ColorImage> {
            let mut img = gtk::cairo::ImageSurface::try_from(surface).ok()?;
            img.flush();
            let (w, h, stride) = (img.width() as usize, img.height() as usize, img.stride() as usize);
            let data = img.data().ok()?;
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                let row = &data[y * stride..y * stride + w * 4];
                for px in row.chunks_exact(4) {
                    rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            }
            Some(egui::ColorImage::from_rgba_premultiplied([w, h], &rgba))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use owlpost_core::{Attachment, Contact};

    fn msg(id: i64, text: &str, html: Option<&str>) -> Message {
        Message {
            id,
            account_id: 1,
            folder_id: 1,
            uid: id as u32,
            message_id: format!("m{id}@x"),
            thread_key: "t".into(),
            in_reply_to: String::new(),
            references: vec![],
            subject: "Hello".into(),
            from: Contact::new("Maya Chen", "maya@example.com"),
            to: vec![Contact::new("Alex", "alex@example.com")],
            cc: vec![],
            date: chrono::Utc::now(),
            snippet: text.chars().take(40).collect(),
            body_text: text.into(),
            body_html: html.map(str::to_string),
            attachments: vec![],
            unread: false,
            starred: false,
        }
    }

    fn colors() -> Colors {
        Colors::new(owlpost_core::theme::discover().into_iter().find(|t| t.id == "owlpost-light").unwrap().palette())
    }

    /// Write a dark page for an email (OWLPOST_DUMP_IN → OWLPOST_DUMP_OUT), to look at.
    #[test]
    #[ignore]
    fn dump_dark_page() {
        let html = std::fs::read_to_string(std::env::var("OWLPOST_DUMP_IN").unwrap()).unwrap();
        let col = Colors::new(owlpost_core::theme::discover().into_iter().find(|t| t.id == owlpost_core::theme::MOONLIT_DARK).unwrap().palette());
        let doc = document(&Page { messages: &[msg(1, "x", Some(&html))], expanded: &[true], remote_images: true, nonce: "n", avatar: &|_| None, dark_emails: true }, &col);
        std::fs::write(std::env::var("OWLPOST_DUMP_OUT").unwrap(), doc).unwrap();
    }

    #[test]
    fn html_bodies_are_sandboxed_and_locked_down() {
        let evil = r#"<p style="color:red">Sale!</p><img src="https://cdn.example/sale.jpg" width="600"><img src="https://track.example/pixel.gif"><script>alert(1)</script><form action="https://x"></form>"#;
        let mut m = msg(7, "Sale!", Some(evil));
        m.attachments = vec![Attachment { filename: "terms <1>.pdf".into(), mime_type: "application/pdf".into(), size: 2048 }];
        let doc = document(&Page { messages: &[m.clone()], expanded: &[true], remote_images: false, nonce: "abc", avatar: &|_| None, dark_emails: true }, &colors());
        // The email is inside a sandboxed frame without allow-scripts or allow-forms.
        assert!(doc.contains("<iframe sandbox=\"allow-same-origin allow-popups allow-top-navigation-by-user-activation\""));
        assert!(!doc.contains("allow-scripts") && !doc.contains("allow-forms"));
        // Its markup is escaped into srcdoc, never live in the page.
        assert!(!doc.contains("<script>alert(1)"));
        assert!(doc.contains("&lt;script&gt;alert(1)"));
        // Scripts only by nonce; nothing remote; and a banner offers images.
        assert!(doc.contains("script-src 'nonce-abc'") && doc.contains("img-src data: cid:;"));
        assert!(doc.contains("owlpost://load-images"));
        // Attachments link back to the app, names escaped.
        assert!(doc.contains("href=\"owlpost://attachment/7/0\"") && doc.contains("terms &lt;1&gt;.pdf"));
        // Loading images opens up https only for media, still no scripts.
        let open = document(&Page { messages: &[m], expanded: &[true], remote_images: true, nonce: "abc", avatar: &|_| None, dark_emails: true }, &colors());
        assert!(open.contains("img-src data: cid: https: http:") && !open.contains("owlpost://load-images"));
        assert!(open.contains("script-src 'nonce-abc'"));
        // The tracking pixel is gone either way, and the banner says so.
        for d in [&doc, &open] {
            assert!(!d.contains("pixel.gif") && d.contains("Blocked a tracker from track.example."));
        }
    }

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
        assert_eq!(host_list(&hosts), "go.repco.example, c.vialoops.example and 2 more");
        // Nothing to do: returned unchanged.
        assert_eq!(strip_trackers("<p>Hi <b>there</b></p>"), ("<p>Hi <b>there</b></p>".to_string(), vec![]));
    }

    #[test]
    fn plain_text_is_escaped_linked_and_quotes_fold() {
        let h = text_html("Hi <b>you</b>, see https://timson.co.\n\nOn Mon, Ann wrote:\n> earlier\n> stuff");
        assert!(h.contains("Hi &lt;b&gt;you&lt;/b&gt;"));
        assert!(h.contains("<a href=\"https://timson.co\">https://timson.co</a>."));
        assert!(h.contains("<details class=quote><summary>•••</summary><div class=quoted>On Mon, Ann wrote:\n&gt; earlier\n&gt; stuff</div></details>"));
    }

    #[test]
    fn older_messages_start_folded() {
        let msgs = [msg(1, "first", None), msg(2, "second", None)];
        let doc = document(&Page { messages: &msgs, expanded: &[false, true], remote_images: false, nonce: "n", avatar: &|_| None, dark_emails: true }, &colors());
        assert_eq!(doc.matches("<details class=card open>").count(), 1);
        assert_eq!(doc.matches("<details class=card>").count(), 1);
        // Newest on top, with the reply buttons under it, before older mail.
        let b = doc.find("<body").unwrap();
        let at = |s: &str| b + doc[b..].find(s).unwrap();
        let (newest, reply, oldest) = (at("second"), at("owlpost://reply"), at("first"));
        assert!(newest < reply && reply < oldest);
        assert!(!doc.contains("owlpost://load-images"), "no banner without remote content");
    }

    /// Writes the demo newsletter conversation to $OWLPOST_SHOTS, to look at.
    #[test]
    fn writes_demo_page() {
        let Ok(dir) = std::env::var("OWLPOST_SHOTS") else { return };
        let p = std::env::temp_dir().join(format!("sp-mv-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let store = owlpost_core::Store::open(&p).unwrap();
        owlpost_core::demo::seed(&store).unwrap();
        let acc = store.accounts().unwrap().into_iter().next().unwrap();
        let inbox = store.folder_by_role(acc.id, owlpost_core::FolderRole::Inbox).unwrap().unwrap();
        let t = store.threads(&owlpost_core::MailboxView::Folder(inbox.id), "Guji", 5).unwrap().remove(0);
        let mut msgs = store.thread_messages(acc.id, &t.thread_key).unwrap();
        let mut reply = msg(99, "Ooh, ordering two bags!\n\nOn Thu, Kōwhai wrote:\n> November roast", None);
        reply.from = owlpost_core::Contact::new("Alex Rivera", "alex@icloud.demo");
        msgs.insert(0, reply);
        for (name, remote) in [("mail-view", false), ("mail-view-images", true)] {
            let doc = document(&Page { messages: &msgs, expanded: &[true, true], remote_images: remote, nonce: "n1", avatar: &|_| None, dark_emails: true }, &colors());
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.html")), doc).unwrap();
        }
    }

    /// Renders saved raw emails to look at: `OWLPOST_EML=a.eml:b.eml` writes
    /// `<name>.html` (images on) to $OWLPOST_SHOTS and prints the trackers.
    /// Real mail stays out of the repo this way.
    #[test]
    fn renders_saved_emails() {
        let (Ok(files), Ok(dir)) = (std::env::var("OWLPOST_EML"), std::env::var("OWLPOST_SHOTS")) else { return };
        for f in files.split(':') {
            let raw = std::fs::read(f).unwrap();
            let m = owlpost_core::parse::parse_message(&raw, 1, 1, 1).unwrap();
            let (_, hosts) = strip_trackers(m.body_html.as_deref().unwrap_or(""));
            println!("{f}: snippet {:?}\n  trackers {hosts:?}", m.snippet);
            let name = std::path::Path::new(f).file_stem().unwrap().to_string_lossy().to_string();
            let doc = document(&Page { messages: &[m], expanded: &[true], remote_images: true, nonce: "n1", avatar: &|_| None, dark_emails: true }, &colors());
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.html")), doc).unwrap();
        }
    }

    #[test]
    fn page_links_and_keys() {
        assert_eq!(parse_action("owlpost://attachment/12/3"), Some(ViewEvent::Attachment(12, 3)));
        assert_eq!(parse_action("owlpost://reply-all"), Some(ViewEvent::ReplyAll));
        assert_eq!(parse_action("https://x"), None);
        assert!(matches!(key_events("e", false).as_slice(), [egui::Event::Text(t)] if t == "e"));
        assert!(matches!(key_events("z", true).as_slice(), [egui::Event::Key { key: egui::Key::Z, pressed: true, modifiers, .. }, _] if modifiers.command));
        assert!(matches!(key_events("ArrowDown", false).as_slice(), [egui::Event::Key { key: egui::Key::ArrowDown, .. }, _]));
        assert!(has_remote_content("<img src=\"https://x/y.png\">") && !has_remote_content("<a href=\"https://x\">x</a>"));
    }
}
