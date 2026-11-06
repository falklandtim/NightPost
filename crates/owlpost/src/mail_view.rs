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
}

/// The whole conversation as one HTML document.
pub fn document(page: &Page, col: &Colors) -> String {
    let policy = csp(page.nonce, page.remote_images);
    let dark = col.p.dark;
    let blocked = !page.remote_images && page.messages.iter().any(|m| m.body_html.as_deref().is_some_and(has_remote_content));
    let mut body = String::new();
    if blocked {
        body.push_str(
            "<div class=banner>Remote images are hidden to protect your privacy. <a href=\"owlpost://load-images\">Load images</a></div>",
        );
    }
    let n = page.messages.len();
    for (i, m) in page.messages.iter().enumerate() {
        let open = page.expanded.get(i).copied().unwrap_or(true) || i + 1 == n;
        let who = if m.from.name.is_empty() { m.from.email.clone() } else { m.from.name.clone() };
        let initial = who.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let mut to = format!("to {}", m.to.iter().map(|c| c.display()).collect::<Vec<_>>().join(", "));
        if !m.cc.is_empty() {
            to.push_str(&format!(" · cc {}", m.cc.iter().map(|c| c.display()).collect::<Vec<_>>().join(", ")));
        }
        let email = if m.from.name.is_empty() { String::new() } else { format!("<span class=addr>&lt;{}&gt;</span>", escape(&m.from.email)) };
        body.push_str(&format!(
            "<details class=card{open}><summary><div class=avatar style=\"background:{bg}\">{initial}</div>\
             <div class=who><div><b>{name}</b> {email}</div><div class=to>{to}</div><div class=snippet>{snippet}</div></div>\
             <div class=date>{date}{star}</div></summary><div class=content>",
            open = if open { " open" } else { "" },
            bg = css(style::avatar_color(if m.from.email.is_empty() { &who } else { &m.from.email })),
            initial = escape(&initial),
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
        match m.body_html.as_deref().filter(|h| !h.trim().is_empty()) {
            Some(html) => {
                // Each email in its own sandbox: no scripts, no forms, its
                // CSS kept to itself. Links go to the top page (and so to
                // the browser).
                let inner = format!(
                    "<meta charset=utf-8><meta http-equiv=\"Content-Security-Policy\" content=\"{}\"><base target=\"_top\">\
                     <style>html,body{{margin:0;padding:0;}} body{{font:14px/1.45 -apple-system,system-ui,sans-serif;color:#1d1d1f;word-wrap:break-word;}} img{{max-width:100%;height:auto;}}</style>{}",
                    escape(&policy),
                    html
                );
                body.push_str(&format!(
                    "<div class=\"html{}\"><iframe sandbox=\"allow-same-origin allow-popups allow-top-navigation-by-user-activation\" srcdoc=\"{}\"></iframe></div>",
                    if dark { " paper" } else { "" },
                    escape(&inner)
                ));
            }
            None => body.push_str(&text_html(&m.body_text)),
        }
        body.push_str("</div></details>");
    }
    body.push_str(
        "<div class=actions><a class=btn href=\"owlpost://reply\">Reply</a><a class=btn href=\"owlpost://reply-all\">Reply all</a>\
         <a class=btn href=\"owlpost://forward\">Forward</a></div>",
    );
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="{policy}">
<style>
:root {{ color-scheme: {scheme}; }}
html, body {{ margin: 0; background: {bg2}; color: {text}; font: 14px/1.5 -apple-system, system-ui, sans-serif; }}
body {{ padding: 6px 36px 36px; }}
a {{ color: {link}; }}
.banner {{ background: {bg}; border: 1px solid {border}; border-radius: 8px; padding: 9px 14px; margin-bottom: 14px; color: {subtle}; font-size: 13px; }}
.card {{ background: {bg}; border: 1px solid {border}; border-radius: 10px; box-shadow: 0 2px 10px rgba(0,0,0,.055); margin-bottom: 16px; }}
.card > summary {{ list-style: none; display: flex; gap: 12px; align-items: flex-start; padding: 20px 26px 0; cursor: default; }}
.card:not([open]) > summary {{ padding-bottom: 18px; }}
.card > summary::-webkit-details-marker {{ display: none; }}
.avatar {{ flex: none; width: 36px; height: 36px; border-radius: 50%; color: #fff; display: grid; place-items: center; font-weight: 600; font-size: 15px; }}
.who {{ flex: 1; min-width: 0; }}
.who b {{ font-weight: 600; }}
.addr, .date {{ color: {very_subtle}; font-size: 12px; }}
.date {{ flex: none; white-space: nowrap; padding-top: 2px; }}
.star {{ color: {star}; }}
.to {{ color: {subtle}; font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
.snippet {{ display: none; color: {subtle}; font-size: 12.5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
.card:not([open]) .snippet {{ display: block; }}
.card:not([open]) .to {{ display: none; }}
.content {{ padding: 0 26px 24px; }}
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
.actions {{ display: flex; gap: 8px; }}
.btn {{ text-decoration: none; color: {text}; border: 1px solid {border}; border-radius: 16px; padding: 5px 16px; font-size: 13px; background: {bg2}; }}
.btn:hover {{ filter: brightness(0.96); }}
</style></head><body>{body}
<script nonce="{nonce}">{script}</script></body></html>"#,
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
        script = SCRIPT,
    )
}

/// The page's only script: size each email frame to its content, and pass
/// shortcut keys to the app (the web view has keyboard focus once clicked).
const SCRIPT: &str = r#"
function fit(f) {
  try {
    var d = f.contentDocument; if (!d || !d.documentElement) return;
    f.style.height = Math.max(d.documentElement.scrollHeight, d.body ? d.body.scrollHeight : 0) + 'px';
  } catch (e) {}
}
document.querySelectorAll('.html iframe').forEach(function (f) {
  f.addEventListener('load', function () {
    fit(f);
    try {
      new ResizeObserver(function () { fit(f); }).observe(f.contentDocument.body);
      f.contentDocument.addEventListener('keydown', forward);
    } catch (e) {}
    setTimeout(function () { fit(f); }, 300);
  });
});
// Size them now too, and once everything (fonts, images) has loaded.
document.querySelectorAll('.html iframe').forEach(fit);
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

#[cfg(not(target_os = "macos"))]
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
        pub fn show(&mut self, rect: egui::Rect, zoom: f32, key: u64, html: impl FnOnce() -> String) {
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

        pub fn events(&self) -> Vec<ViewEvent> {
            self.rx.try_iter().collect()
        }

    }
}

#[cfg(not(target_os = "macos"))]
mod fallback {
    use super::ViewEvent;
    use eframe::egui;

    /// No web view on this platform yet: messages are shown as text.
    pub struct MailView;

    impl MailView {
        pub fn new<W>(_window: &W, _bg: egui::Color32) -> Option<Self> {
            None
        }
        pub fn show(&mut self, _rect: egui::Rect, _zoom: f32, _key: u64, _html: impl FnOnce() -> String) {}
        pub fn hide(&mut self) {}
        pub fn events(&self) -> Vec<ViewEvent> {
            vec![]
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

    #[test]
    fn html_bodies_are_sandboxed_and_locked_down() {
        let evil = r#"<p style="color:red">Sale!</p><img src="https://track.example/pixel.gif"><script>alert(1)</script><form action="https://x"></form>"#;
        let mut m = msg(7, "Sale!", Some(evil));
        m.attachments = vec![Attachment { filename: "terms <1>.pdf".into(), mime_type: "application/pdf".into(), size: 2048 }];
        let doc = document(&Page { messages: &[m.clone()], expanded: &[true], remote_images: false, nonce: "abc" }, &colors());
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
        let open = document(&Page { messages: &[m], expanded: &[true], remote_images: true, nonce: "abc" }, &colors());
        assert!(open.contains("img-src data: cid: https: http:") && !open.contains("owlpost://load-images"));
        assert!(open.contains("script-src 'nonce-abc'"));
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
        let doc = document(&Page { messages: &msgs, expanded: &[false, true], remote_images: false, nonce: "n" }, &colors());
        assert_eq!(doc.matches("<details class=card open>").count(), 1);
        assert_eq!(doc.matches("<details class=card>").count(), 1);
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
            let doc = document(&Page { messages: &msgs, expanded: &[true, true], remote_images: remote, nonce: "n1" }, &colors());
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
