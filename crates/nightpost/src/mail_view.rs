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
use nightpost_core::{Message, MessageId};
pub use nightpost_core::html_clean::{has_remote_content, tag_attr};

/// Something the page asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewEvent {
    /// A link was clicked: open it in the browser.
    OpenLink(String),
    /// Open attachment `usize` of a message.
    Attachment(MessageId, usize),
    /// Save it to Downloads (from its right-click menu).
    SaveAttachment(MessageId, usize),
    /// Save it where you choose (Save As…).
    SaveAttachmentAs(MessageId, usize),
    /// Show remote images in this conversation.
    LoadImages,
    Reply,
    ReplyAll,
    Forward,
    /// A key pressed while the page had focus, for the app's shortcuts:
    /// (key, ⌘ held). Only the Mac's web view sends it: on Linux the page is
    /// a picture inside egui, so keys reach the app directly.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Key(String, bool),
}

/// Map an `nightpost://` link from the page to an event.
pub fn parse_action(url: &str) -> Option<ViewEvent> {
    let rest = url.strip_prefix("nightpost://")?;
    let mut parts = rest.trim_end_matches('/').split('/');
    match parts.next()? {
        "attachment" => Some(ViewEvent::Attachment(parts.next()?.parse().ok()?, parts.next()?.parse().ok()?)),
        "attachment-save" => Some(ViewEvent::SaveAttachment(parts.next()?.parse().ok()?, parts.next()?.parse().ok()?)),
        "attachment-save-as" => Some(ViewEvent::SaveAttachmentAs(parts.next()?.parse().ok()?, parts.next()?.parse().ok()?)),
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

pub use nightpost_core::mail_html::csp;

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
                let (clean, hosts) = nightpost_core::html_clean::clean(h);
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
            "<span class=shield title=\"Tracking pixels report when and where you open an email. NightPost never loads them.\">Blocked {} from {}.</span> ",
            if trackers.len() == 1 { "a tracker" } else { "trackers" },
            escape(&host_list(&trackers))
        )
    };
    if blocked {
        body.push_str(&format!(
            "<div class=banner>{tracked}Remote images are hidden to protect your privacy. <a href=\"nightpost://load-images\">Load images</a></div>"
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
                    "<a class=attachment href=\"nightpost://attachment/{}/{j}\" title=\"Click to open · right-click to save\">📎 {} <span>{}</span></a>",
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
                let inner = nightpost_core::mail_html::email_frame(html, &policy, &colours, false);
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
                "<div class=actions><a class=btn href=\"nightpost://reply\">Reply</a><a class=btn href=\"nightpost://reply-all\">Reply all</a>\
                 <a class=btn href=\"nightpost://forward\">Forward</a></div>",
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
.menu {{ position: fixed; z-index: 10; min-width: 170px; padding: 4px; background: {menu_bg}; border: 1px solid {border}; border-radius: 8px;
  box-shadow: 0 6px 20px rgba(0,0,0,.18); font-size: 13px; }}
.menu a {{ display: block; padding: 5px 10px; border-radius: 5px; color: {text}; text-decoration: none; cursor: default; }}
.menu a:hover {{ background: {link}; color: #fff; }}
.text {{ white-space: pre-wrap; word-wrap: break-word; }}
.quote summary {{ color: {very_subtle}; cursor: pointer; list-style: none; font-size: 12px; }}
.quoted {{ color: {subtle}; white-space: pre-wrap; border-left: 2px solid {hair}; padding-left: 10px; margin-top: 4px; }}
.html iframe {{ border: 0; width: 100%; display: block; min-height: 40px; }}
.html.paper {{ background: #fff; border-radius: 6px; padding: 12px; }}
.actions {{ display: flex; gap: 8px; margin: 4px 0 22px; }}
.btn {{ text-decoration: none; color: {text}; border: 1px solid {border}; border-radius: 16px; padding: 5px 16px; font-size: 13px; background: {bg2}; }}
.btn:hover {{ filter: brightness(0.96); }}
</style></head><body>{body}
<script nonce="{nonce}">var NP_DARK = {dark_js};{script}</script></body></html>"#,
        policy = escape(&policy),
        scheme = if dark { "dark" } else { "light" },
        bg = css(col.bg),
        bg2 = css(col.bg_secondary),
        menu_bg = css(col.bg),
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
  if (!NP_DARK || !d || !d.body || d.npDark) return;
  d.npDark = true;
  var theme = hsl(NP_DARK.bg), bgL = theme[2], textL = hsl(NP_DARK.text)[2];
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
// Right-click an attachment: Open, Save to Downloads, Save As…
var menu = null;
function closeMenu() { if (menu) { menu.remove(); menu = null; } }
document.addEventListener('contextmenu', function (e) {
  closeMenu();
  var a = e.target.closest && e.target.closest('a.attachment');
  if (!a) return;
  e.preventDefault();
  var rest = a.getAttribute('href').replace('nightpost://attachment/', '');
  menu = document.createElement('div');
  menu.className = 'menu';
  [['Open', 'attachment'], ['Save to Downloads', 'attachment-save'], ['Save As\u2026', 'attachment-save-as']].forEach(function (it) {
    var l = document.createElement('a');
    l.textContent = it[0];
    l.href = 'nightpost://' + it[1] + '/' + rest;
    l.addEventListener('click', function () { setTimeout(closeMenu, 0); });
    menu.appendChild(l);
  });
  document.body.appendChild(menu);
  var x = Math.min(e.clientX, window.innerWidth - menu.offsetWidth - 4);
  var y = e.clientY + menu.offsetHeight > window.innerHeight ? e.clientY - menu.offsetHeight : e.clientY;
  menu.style.left = Math.max(4, x) + 'px';
  menu.style.top = Math.max(4, y) + 'px';
});
document.addEventListener('mousedown', function (e) { if (menu && !menu.contains(e.target)) closeMenu(); });
document.addEventListener('keydown', function (e) {
  // Escape closes the menu only (not the conversation too).
  if (menu && e.key === 'Escape') { closeMenu(); e.preventDefault(); e.stopImmediatePropagation(); }
}, true);
window.addEventListener('scroll', closeMenu, true);
window.addEventListener('blur', closeMenu);
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
    use crate::mac_web;
    use crossbeam_channel::Receiver;
    use eframe::egui;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// The web view, laid over the reading pane.
    pub struct MailView {
        web: mac_web::View,
        /// egui's own view, which the web view sits in.
        parent: Retained<NSView>,
        rx: Receiver<ViewEvent>,
        /// Identifies the page now loaded, to reload only when it changes.
        loaded: u64,
        visible: bool,
        bounds: egui::Rect,
    }

    impl MailView {
        pub fn new(window: &impl HasWindowHandle, bg: egui::Color32) -> Option<Self> {
            let mtm = MainThreadMarker::new()?;
            let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
            // SAFETY: the handle's view is egui's live NSView, which outlives
            // this pane (both belong to the main window); retaining keeps it so.
            let parent = unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }?;
            let (tx, rx) = crossbeam_channel::unbounded::<ViewEvent>();
            let nav = tx.clone();
            let web = mac_web::View::new(
                mtm,
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
                mac_web::Options {
                    child: true,
                    background: bg,
                    on_message: Box::new(move |body| {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                            if v.get("ready").is_some() {
                                log::info!("mail view ready: email frame heights {}, email scripts ran: {}", v["ready"], v["scripted"]);
                            }
                            if let Some(k) = v["key"].as_str() {
                                let _ = tx.send(ViewEvent::Key(k.to_string(), v["meta"].as_bool().unwrap_or(false)));
                            }
                        }
                    }),
                    on_navigate: Some(Box::new(move |url, from_page| {
                        // Our own page loads; everything else is ours to handle.
                        if url.starts_with("about:") || url.starts_with("data:") {
                            return true;
                        }
                        // The app's own actions come only from its own page, never
                        // from inside an email (html_clean strips them there too).
                        if !from_page && url.get(..10).is_some_and(|s| s.eq_ignore_ascii_case("nightpost:")) {
                            log::warn!("ignored an app link from inside an email");
                            return false;
                        }
                        let _ = nav.send(parse_action(&url).unwrap_or(ViewEvent::OpenLink(url)));
                        false
                    })),
                },
            );
            web.view.setHidden(true);
            // Keeps its distance from the top when the window's height changes.
            web.view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
            parent.addSubview(&web.view);
            log::debug!("mail view: web view created");
            Some(Self { web, parent, rx, loaded: 0, visible: false, bounds: egui::Rect::NOTHING })
        }

        /// Show `html` over `rect` (in window points). Reloads only when
        /// `key` changes.
        pub fn show(&mut self, _ctx: &egui::Context, rect: egui::Rect, zoom: f32, key: u64, html: impl FnOnce() -> String) {
            if rect != self.bounds {
                self.bounds = rect;
                let (w, h) = (f64::from((rect.width() * zoom).max(1.0)), f64::from((rect.height() * zoom).max(1.0)));
                let (x, top) = (f64::from(rect.left() * zoom), f64::from(rect.top() * zoom));
                // egui measures from the top; an unflipped AppKit view from the bottom.
                let y = if self.parent.isFlipped() { top } else { self.parent.frame().size.height - top - h };
                self.web.view.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)));
            }
            if key != self.loaded {
                self.loaded = key;
                let page = html();
                log::debug!("mail view: loading a {} byte page at {rect:?}", page.len());
                self.web.load_html(&page);
            }
            if !self.visible {
                self.visible = true;
                self.web.view.setHidden(false);
            }
        }

        pub fn hide(&mut self) {
            if self.visible {
                self.visible = false;
                self.web.view.setHidden(true);
                // Keys go back to egui.
                if let Some(w) = self.parent.window() {
                    w.makeFirstResponder(Some(&self.parent));
                }
            }
        }

        pub fn events(&mut self) -> Vec<ViewEvent> {
            self.rx.try_iter().collect()
        }
    }

    impl Drop for MailView {
        fn drop(&mut self) {
            self.web.view.removeFromSuperview();
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
/// WebKitGTK 6.0 renders the page in a GTK 4 window that is never shown, on
/// the GTK thread, and hands back a full-page snapshot (sharp on HiDPI
/// screens), with where the links and folds are. The app paints it, scrolls it, and turns clicks into
/// the same events the macOS view sends. Emails run no scripts, so a picture
/// of the page is the page. (Text can't be selected yet.)
#[cfg(target_os = "linux")]
mod offscreen {
    use super::{parse_action, ViewEvent};
    use eframe::egui;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    pub(super) enum Hit {
        Link(String),
        /// An older message's header: fold it open or shut.
        Fold(usize),
    }

    /// A clickable spot, in page (CSS) pixels.
    #[derive(Clone, Debug)]
    pub(super) struct Target {
        pub(super) rect: egui::Rect,
        pub(super) hit: Hit,
    }

    /// A finished render.
    pub(super) struct Rendered {
        pub(super) key: u64,
        width: u32,
        /// Device pixels per CSS pixel.
        pub(super) scale: f32,
        pub(super) image: egui::ColorImage,
        pub(super) targets: Vec<Target>,
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
        /// An attachment's right-click menu: its "id/index" and where it opened.
        menu: Option<(String, egui::Pos2)>,
        /// Where the menu was drawn last frame (clicks there aren't the page's).
        menu_rect: egui::Rect,
    }

    impl MailView {
        /// The page is a picture here, so an attachment's right-click menu
        /// (Open, Save to Downloads, Save As…) is drawn by the app.
        fn attachment_menu(&mut self, ctx: &egui::Context) {
            let Some((rest, pos)) = self.menu.clone() else { return };
            let area = egui::Area::new(egui::Id::new("mail_view_attachment_menu")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_min_width(160.0);
                    for (label, action) in [("Open", "attachment"), ("Save to Downloads", "attachment-save"), ("Save As…", "attachment-save-as")] {
                        if ui.add(egui::Button::new(label).frame(false)).clicked() {
                            if let Some(e) = parse_action(&format!("nightpost://{action}/{rest}")) {
                                self.events.push(e);
                            }
                            self.menu = None;
                        }
                    }
                });
            });
            self.menu_rect = area.response.rect;
            let away = ctx.input(|i| {
                i.key_pressed(egui::Key::Escape) || (i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)) && !i.pointer.secondary_pressed())
            });
            if away {
                self.menu = None;
            }
        }

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
                menu: None,
                menu_rect: egui::Rect::NOTHING,
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
            if let Some(p) = pointer.filter(|p| !(self.menu.is_some() && self.menu_rect.contains(*p))) {
                let at = (p - origin).to_pos2();
                // A right-click anywhere else closes an open menu.
                if ctx.input(|i| i.pointer.secondary_clicked()) {
                    self.menu = None;
                }
                if let Some(t) = self.targets.iter().find(|t| t.rect.contains(at)).cloned() {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    if ctx.input(|i| i.pointer.primary_clicked()) {
                        match t.hit {
                            Hit::Link(url) => self.events.push(parse_action(&url).unwrap_or(ViewEvent::OpenLink(url))),
                            Hit::Fold(i) => request(Job::Fold(i), self.shared.clone(), ctx.clone()),
                        }
                    } else if ctx.input(|i| i.pointer.secondary_clicked()) {
                        if let Hit::Link(url) = &t.hit {
                            if let Some(rest) = url.strip_prefix("nightpost://attachment/") {
                                self.menu = Some((rest.to_string(), p));
                            }
                        }
                    }
                }
            }
            self.attachment_menu(ctx);
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
    ///
    /// GTK 4 has no off-screen window (GTK 3's `OffscreenWindow` is gone), so
    /// the view sits in an ordinary window that's never shown. A window that's
    /// never shown is never given a size either, so it's realised and the view
    /// sized by hand on every load; otherwise WebKit lays the page out at
    /// 0 × 0. Tested on KDE and Hyprland in trials/email-engines.
    mod gtk_side {
        use super::{Hit, Job, Rendered, Shared, Target};
        use eframe::egui;
        use std::cell::RefCell;
        use std::sync::{Arc, Mutex};
        use webkit6::prelude::*;
        use webkit6::{gdk, gio, glib, gtk, LoadEvent, SnapshotOptions, SnapshotRegion, WebView};

        /// The view's height while laying out, in CSS pixels (the snapshot
        /// takes the whole page; this only has to be something).
        const VIEW_HEIGHT: f32 = 600.0;

        struct Off {
            window: gtk::Window,
            view: WebView,
            key: u64,
            width: u32,
            /// Device pixels per CSS pixel in the snapshot.
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
                            off.view.evaluate_javascript(&js, None, None, None::<&gio::Cancellable>, |_| {});
                            off.gen
                        })
                    });
                    if let Some(gen) = gen {
                        glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || snapshot(gen));
                    }
                }
            }
        }

        /// The hidden window and its view, made once.
        fn make(shared: &Arc<Mutex<Shared>>, ctx: &egui::Context) -> Off {
            let view = WebView::new();
            view.connect_load_changed(|_, ev| {
                if ev == LoadEvent::Finished {
                    let gen = OFF.with(|o| o.borrow().as_ref().map(|off| off.gen));
                    if let Some(gen) = gen {
                        // Once the email frames have sized themselves, then
                        // again after late pictures and fonts arrive.
                        for ms in [250, 1500] {
                            glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || snapshot(gen));
                        }
                    }
                }
            });
            let window = gtk::Window::new();
            window.set_decorated(false);
            window.set_child(Some(&view));
            // `realize` is on both Widget and WebView's own traits.
            WidgetExt::realize(&window);
            WidgetExt::realize(&view);
            Off { window, view, key: 0, width: 0, scale: 1.0, gen: 0, shared: shared.clone(), ctx: ctx.clone() }
        }

        fn load(key: u64, width: u32, scale: f32, html: String, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
            OFF.with(|o| {
                let mut o = o.borrow_mut();
                let off = o.get_or_insert_with(|| make(&shared, &ctx));
                off.key = key;
                off.width = width;
                off.gen += 1;
                off.shared = shared;
                off.ctx = ctx;
                // GTK draws at the screen's scale already (HiDPI); zoom the
                // page for whatever's left, so the picture is sharp but the
                // layout stays `width` CSS pixels wide.
                let gdk_scale = off.view.scale_factor().max(1) as f32;
                let zoom = (scale / gdk_scale).max(0.25);
                off.scale = gdk_scale * zoom;
                off.view.set_zoom_level(f64::from(zoom));
                let (w, h) = ((width as f32 * zoom).round() as i32, (VIEW_HEIGHT * zoom).round() as i32);
                off.window.set_default_size(w, h);
                off.view.set_size_request(w, h);
                off.view.allocate(w, h, -1, None);
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
      // An email's own links never act on the app (html_clean strips them too).
      f.contentDocument.querySelectorAll('a[href]').forEach(function (a) { if (!/^nightpost:/i.test(a.href)) add(a, fr.left, fr.top, { href: a.href }); });
    } catch (e) {}
  });
  var h = Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0);
  return JSON.stringify({ h: h, t: out });
})()"#;

        fn snapshot(gen: u64) {
            let view = OFF.with(|o| o.borrow().as_ref().filter(|off| off.gen == gen).map(|off| off.view.clone()));
            let Some(view) = view else { return };
            let runner = view.clone();
            runner.evaluate_javascript(MEASURE, None, None, None::<&gio::Cancellable>, move |r| {
                let json = r.ok().map(|v| v.to_str().to_string()).unwrap_or_default();
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let height = v["h"].as_f64().unwrap_or(0.0) as f32;
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
                view.snapshot(SnapshotRegion::FullDocument, SnapshotOptions::NONE, None::<&gio::Cancellable>, move |s| {
                    let Ok(texture) = s else { return };
                    OFF.with(|o| {
                        let o = o.borrow();
                        let Some(off) = o.as_ref().filter(|off| off.gen == gen) else { return };
                        // The snapshot covers anything wider than the pane and
                        // is never shorter than the view: keep just the page.
                        let w = (off.width as f32 * off.scale).round() as usize;
                        let h = (height * off.scale).round() as usize;
                        let Some(image) = to_image(&texture, w, h) else { return };
                        if let Ok(mut sh) = off.shared.lock() {
                            sh.rendered = Some(Rendered { key: off.key, width: off.width, scale: off.scale, image, targets });
                        }
                        off.ctx.request_repaint();
                    });
                });
            });
        }

        /// A snapshot as an egui image, cut to at most `max_w` × `max_h`
        /// pixels (`max_h` 0: the whole height).
        fn to_image(texture: &gdk::Texture, max_w: usize, max_h: usize) -> Option<egui::ColorImage> {
            let (tw, th) = (usize::try_from(texture.width()).ok()?, usize::try_from(texture.height()).ok()?);
            let mut down = gdk::TextureDownloader::new(texture);
            down.set_format(gdk::MemoryFormat::R8g8b8a8Premultiplied);
            let (bytes, stride) = down.download_bytes();
            let w = tw.min(max_w.max(1));
            let h = if max_h == 0 { th } else { th.min(max_h) };
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                rgba.extend_from_slice(bytes.get(y * stride..y * stride + w * 4)?);
            }
            Some(egui::ColorImage::from_rgba_premultiplied([w, h], &rgba))
        }
    }

    /// Tests: draw `html` off screen as the reading pane would, and wait for
    /// the first picture. Fails the test after 15 s.
    #[cfg(test)]
    pub(super) fn draw(html: String, width: u32, scale: f32) -> Rendered {
        let ctx = egui::Context::default();
        assert!(crate::web_host::init(&ctx), "GTK didn't start");
        let shared: Arc<Mutex<Shared>> = Arc::default();
        request(Job::Load { key: 7, width, scale, html }, shared.clone(), ctx);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some(r) = shared.lock().ok().and_then(|mut s| s.rendered.take()) {
                return r;
            }
            assert!(std::time::Instant::now() < end, "no picture within 15 s");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightpost_core::{Attachment, Contact};

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
            category: 0,
        }
    }

    fn colors() -> Colors {
        Colors::new(nightpost_core::theme::discover().into_iter().find(|t| t.id == nightpost_core::theme::MOONLIT_LIGHT).unwrap().palette())
    }

    /// Write a dark page for an email (NIGHTPOST_DUMP_IN → NIGHTPOST_DUMP_OUT), to look at.
    #[test]
    #[ignore]
    fn dump_dark_page() {
        let html = std::fs::read_to_string(std::env::var("NIGHTPOST_DUMP_IN").unwrap()).unwrap();
        let col = Colors::new(nightpost_core::theme::discover().into_iter().find(|t| t.id == nightpost_core::theme::MOONLIT_DARK).unwrap().palette());
        let doc = document(&Page { messages: &[msg(1, "x", Some(&html))], expanded: &[true], remote_images: true, nonce: "n", avatar: &|_| None, dark_emails: true }, &col);
        std::fs::write(std::env::var("NIGHTPOST_DUMP_OUT").unwrap(), doc).unwrap();
    }

    /// An email can't carry a link to the app's own actions: only the app's
    /// banner link to load images is left in the page.
    #[test]
    fn email_links_to_app_actions_are_removed() {
        let evil = r#"<a href="nightpost://load-images" style="padding:8px;background:#06c">View in browser</a><a href="nightpost://attachment/9/0">Invoice</a><img src="https://cdn.example/a.png" width="600">"#;
        let doc = document(&Page { messages: &[msg(9, "Hi", Some(evil))], expanded: &[true], remote_images: false, nonce: "n", avatar: &|_| None, dark_emails: true }, &colors());
        assert_eq!(doc.matches("nightpost://load-images").count(), 1, "the banner's only");
        assert!(!doc.contains("nightpost://attachment/9/0"));
        assert!(doc.contains("View in browser") && doc.contains("Invoice"));
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
        assert!(doc.contains("nightpost://load-images"));
        // Attachments link back to the app, names escaped.
        assert!(doc.contains("href=\"nightpost://attachment/7/0\"") && doc.contains("terms &lt;1&gt;.pdf"));
        // Loading images opens up https only for media, still no scripts.
        let open = document(&Page { messages: &[m], expanded: &[true], remote_images: true, nonce: "abc", avatar: &|_| None, dark_emails: true }, &colors());
        assert!(open.contains("img-src data: cid: https: http:") && !open.contains("nightpost://load-images"));
        assert!(open.contains("script-src 'nonce-abc'"));
        // The tracking pixel is gone either way, and the banner says so.
        for d in [&doc, &open] {
            assert!(!d.contains("pixel.gif") && d.contains("Blocked a tracker from track.example."));
        }
    }

    /// The tracker tests live with `strip_trackers`, in `nightpost-core`'s
    /// `html_clean.rs`; this is the banner's list of hosts.
    #[test]
    fn blocked_hosts_are_listed() {
        let hosts = |n: usize| (1..=n).map(|i| format!("t{i}.example")).collect::<Vec<_>>();
        assert_eq!(host_list(&hosts(0)), "");
        assert_eq!(host_list(&hosts(1)), "t1.example");
        assert_eq!(host_list(&hosts(2)), "t1.example and t2.example");
        assert_eq!(host_list(&hosts(4)), "t1.example, t2.example and 2 more");
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
        let (newest, reply, oldest) = (at("second"), at("nightpost://reply"), at("first"));
        assert!(newest < reply && reply < oldest);
        assert!(!doc.contains("nightpost://load-images"), "no banner without remote content");
    }

    /// Writes the demo newsletter conversation to $NIGHTPOST_SHOTS, to look at.
    #[test]
    fn writes_demo_page() {
        let Ok(dir) = std::env::var("NIGHTPOST_SHOTS") else { return };
        let p = std::env::temp_dir().join(format!("sp-mv-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let store = nightpost_core::Store::open(&p).unwrap();
        nightpost_core::demo::seed(&store).unwrap();
        let acc = store.accounts().unwrap().into_iter().next().unwrap();
        let inbox = store.folder_by_role(acc.id, nightpost_core::FolderRole::Inbox).unwrap().unwrap();
        let t = store.threads(&nightpost_core::MailboxView::Folder(inbox.id), "Guji", 5).unwrap().remove(0);
        let mut msgs = store.thread_messages(acc.id, &t.thread_key).unwrap();
        let mut reply = msg(99, "Ooh, ordering two bags!\n\nOn Thu, Kōwhai wrote:\n> November roast", None);
        reply.from = nightpost_core::Contact::new("Alex Rivera", "alex@icloud.demo");
        msgs.insert(0, reply);
        for (name, remote) in [("mail-view", false), ("mail-view-images", true)] {
            let doc = document(&Page { messages: &msgs, expanded: &[true, true], remote_images: remote, nonce: "n1", avatar: &|_| None, dark_emails: true }, &colors());
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.html")), doc).unwrap();
        }
    }

    /// Linux: draws the demo newsletter conversation with WebKitGTK off
    /// screen, as the reading pane shows it, and saves PNGs (light and dark)
    /// to $NIGHTPOST_SHOTS. Needs a desktop session and NIGHTPOST_GTK_TESTS=1.
    #[cfg(target_os = "linux")]
    #[test]
    fn draws_demo_page_on_linux() {
        let (Ok(dir), Some(_)) = (std::env::var("NIGHTPOST_SHOTS"), std::env::var_os("NIGHTPOST_GTK_TESTS")) else { return };
        let p = std::env::temp_dir().join(format!("sp-mv-gtk-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let store = nightpost_core::Store::open(&p).unwrap();
        nightpost_core::demo::seed(&store).unwrap();
        let acc = store.accounts().unwrap().into_iter().next().unwrap();
        let inbox = store.folder_by_role(acc.id, nightpost_core::FolderRole::Inbox).unwrap().unwrap();
        let t = store.threads(&nightpost_core::MailboxView::Folder(inbox.id), "Guji", 5).unwrap().remove(0);
        let msgs = store.thread_messages(acc.id, &t.thread_key).unwrap();
        let expanded = vec![true; msgs.len()];
        for (name, theme) in [("linux-mail-view-light", nightpost_core::theme::MOONLIT_LIGHT), ("linux-mail-view-dark", nightpost_core::theme::MOONLIT_DARK)] {
            let col = Colors::new(nightpost_core::theme::discover().into_iter().find(|t| t.id == theme).unwrap().palette());
            let doc = document(&Page { messages: &msgs, expanded: &expanded, remote_images: false, nonce: "n1", avatar: &|_| None, dark_emails: true }, &col);
            let r = offscreen::draw(doc, 700, 2.0);
            let [w, h] = r.image.size;
            assert!(w == 1400 && h > 400, "{w}×{h}");
            let rgba: Vec<u8> = r.image.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
            image::save_buffer(std::path::Path::new(&dir).join(format!("{name}.png")), &rgba, w as u32, h as u32, image::ColorType::Rgba8).unwrap();
        }
        let _ = std::fs::remove_file(&p);
    }

    /// Linux: draws a page with WebKitGTK in the hidden window and checks the
    /// picture's size and colour and the link's place. It needs a desktop
    /// session, so it runs only with NIGHTPOST_GTK_TESTS=1.
    #[cfg(target_os = "linux")]
    #[test]
    fn draws_a_page_off_screen() {
        if std::env::var_os("NIGHTPOST_GTK_TESTS").is_none() {
            return;
        }
        let html = "<body style='margin:0;background:#fff'><div style='height:900px;background:#c00'>\
                    <a href='https://example.com/x' style='display:block;width:100px;height:20px'>x</a></div></body>";
        let r = offscreen::draw(html.into(), 400, 2.0);
        assert_eq!(r.key, 7);
        // The pane's width and the page's height (not the view's), at the scale asked for.
        assert_eq!(r.scale, 2.0);
        assert_eq!(r.image.size, [800, 1800]);
        let px = r.image.pixels[100 * 800 + 400];
        assert!(px.r() > 150 && px.g() < 60 && px.b() < 60, "not red: {px:?}");
        let link = r.targets.iter().find(|t| matches!(&t.hit, offscreen::Hit::Link(h) if h == "https://example.com/x")).expect("the link");
        assert_eq!((link.rect.width(), link.rect.height()), (100.0, 20.0));
    }

    /// Renders saved raw emails to look at: `NIGHTPOST_EML=a.eml:b.eml` writes
    /// `<name>.html` (images on) to $NIGHTPOST_SHOTS and prints the trackers.
    /// Real mail stays out of the repo this way.
    #[test]
    fn renders_saved_emails() {
        let (Ok(files), Ok(dir)) = (std::env::var("NIGHTPOST_EML"), std::env::var("NIGHTPOST_SHOTS")) else { return };
        for f in files.split(':') {
            let raw = std::fs::read(f).unwrap();
            let m = nightpost_core::parse::parse_message(&raw, 1, 1, 1).unwrap();
            let (_, hosts) = nightpost_core::html_clean::clean(m.body_html.as_deref().unwrap_or(""));
            println!("{f}: snippet {:?}\n  trackers {hosts:?}", m.snippet);
            let name = std::path::Path::new(f).file_stem().unwrap().to_string_lossy().to_string();
            let doc = document(&Page { messages: &[m], expanded: &[true], remote_images: true, nonce: "n1", avatar: &|_| None, dark_emails: true }, &colors());
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.html")), doc).unwrap();
        }
    }

    #[test]
    fn page_links_and_keys() {
        assert_eq!(parse_action("nightpost://attachment/12/3"), Some(ViewEvent::Attachment(12, 3)));
        assert_eq!(parse_action("nightpost://attachment-save/12/3"), Some(ViewEvent::SaveAttachment(12, 3)));
        assert_eq!(parse_action("nightpost://attachment-save-as/12/3"), Some(ViewEvent::SaveAttachmentAs(12, 3)));
        assert_eq!(parse_action("nightpost://reply-all"), Some(ViewEvent::ReplyAll));
        assert_eq!(parse_action("https://x"), None);
        assert!(matches!(key_events("e", false).as_slice(), [egui::Event::Text(t)] if t == "e"));
        assert!(matches!(key_events("z", true).as_slice(), [egui::Event::Key { key: egui::Key::Z, pressed: true, modifiers, .. }, _] if modifiers.command));
        assert!(matches!(key_events("ArrowDown", false).as_slice(), [egui::Event::Key { key: egui::Key::ArrowDown, .. }, _]));
    }
}
