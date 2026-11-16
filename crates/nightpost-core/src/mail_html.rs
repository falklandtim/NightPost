//! HTML for reading and writing email, shared by the desktop and the phone.
//!
//! Reading: each email's HTML goes in a page of its own ([`email_frame`])
//! under a strict Content Security Policy ([`csp`]): no scripts but the
//! app's own (by nonce), no forms, and nothing remote until the reader asks
//! for images. Tracking pixels are stripped before that (see
//! [`crate::html_clean`]).
//!
//! Writing: a draft reopened for editing is defanged first
//! ([`editor_html`]); replies and forwards carry the original as text
//! ([`quote_html`], [`forward_html`]), so someone else's HTML never goes
//! into the editor; and the edited HTML is wrapped as a whole email
//! ([`email_document`]) to send.

use crate::models::{Contact, Message};

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The Content Security Policy for a page showing email. `nonce` admits the
/// app's own script; `remote` lets images, styles and fonts load from the
/// web (after "Load images"). Scripts in the email never run.
pub fn csp(nonce: &str, remote: bool) -> String {
    let net = if remote { " https: http:" } else { "" };
    format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'unsafe-inline'{net}; img-src data: cid:{net}; font-src data:{net}; \
         frame-src 'self' about: data:; form-action 'none'; base-uri 'none'"
    )
}

/// One email's HTML as a page of its own, under `policy` (from [`csp`]).
/// Links target the top page, so the app decides where they go. `css` adds
/// colours (e.g. for a dark theme); `viewport` adds the phone's
/// `width=device-width`, so a narrow screen isn't treated as a desktop.
pub fn email_frame(html: &str, policy: &str, css: &str, viewport: bool) -> String {
    let viewport = if viewport { "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">" } else { "" };
    format!(
        "<meta charset=utf-8>{viewport}<meta http-equiv=\"Content-Security-Policy\" content=\"{}\"><base target=\"_top\">\
         <style>html,body{{margin:0;padding:0;}} body{{font:14px/1.45 -apple-system,system-ui,sans-serif;word-wrap:break-word;}} img{{max-width:100%;height:auto;}} {css}</style>{html}",
        escape(policy)
    )
}

/// A draft's HTML made safe to edit: no scripts, styles, frames, forms or
/// event handlers, and no `javascript:` links. (The editor blocks scripts
/// anyway; this also keeps a draft from restyling the editor.)
pub fn editor_html(html: &str) -> String {
    let mut out = crate::parse::clean_signature_html(html);
    for tag in ["iframe", "object", "embed", "form"] {
        // Every one of them, not just the first.
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(i) = lower.find(&format!("<{tag}")) else { break };
            let end = lower[i..].find(&format!("</{tag}>")).map(|e| i + e + tag.len() + 3).or_else(|| lower[i..].find('>').map(|e| i + e + 1)).unwrap_or(out.len());
            out.replace_range(i..end, "");
        }
    }
    strip_event_handlers(&out)
}

/// Remove `on…="…"` attributes and `javascript:` links.
fn strip_event_handlers(html: &str) -> String {
    let chars: Vec<char> = html.chars().collect();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    let mut in_tag = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        }
        // An attribute starting with "on" after whitespace, inside a tag.
        if in_tag && c.is_whitespace() && chars.get(i + 1).is_some_and(|x| x.eq_ignore_ascii_case(&'o')) && chars.get(i + 2).is_some_and(|x| x.eq_ignore_ascii_case(&'n')) {
            let mut j = i + 3;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '-') {
                j += 1;
            }
            let mut k = j;
            while k < chars.len() && chars[k].is_whitespace() {
                k += 1;
            }
            if k < chars.len() && chars[k] == '=' && j > i + 3 {
                k += 1;
                while k < chars.len() && chars[k].is_whitespace() {
                    k += 1;
                }
                i = match chars.get(k) {
                    Some(&q @ ('"' | '\'')) => chars[k + 1..].iter().position(|&x| x == q).map(|e| k + 1 + e + 1).unwrap_or(chars.len()),
                    _ => chars[k..].iter().position(|x| x.is_whitespace() || *x == '>').map(|e| k + e).unwrap_or(chars.len()),
                };
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out.replace("javascript:", "")
}

fn text_block(s: &str) -> String {
    crate::smtp::text_to_html(s.trim_end())
}

/// The quoted message under a reply, as text.
pub fn quote_html(m: &Message) -> String {
    let when = m.date.with_timezone(&chrono::Local).format("%a, %e %b %Y at %H:%M");
    format!(
        "<div class=\"nightpost-quote\"><div>On {when}, {} wrote:</div><blockquote type=\"cite\">{}</blockquote></div>",
        escape(&m.from.display()),
        text_block(&m.body_text)
    )
}

/// The forwarded message under a forward, as text. `date` is the original's
/// date as the app writes dates.
pub fn forward_html(m: &Message, date: &str) -> String {
    format!(
        "<div class=\"nightpost-quote\"><div>---------- Forwarded message ---------</div><div>From: {}</div><div>Date: {}</div><div>Subject: {}</div><div>To: {}</div><br>{}</div>",
        escape(&m.from.to_header()),
        escape(date),
        escape(&m.subject),
        escape(&Contact::join(&m.to)),
        text_block(&m.body_text)
    )
}

/// The editor's HTML as a whole email, ready to send.
pub fn email_document(inner: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
<style>blockquote{{margin:6px 0 6px 2px;padding-left:12px;border-left:3px solid #d0d4da;color:#555}}img{{max-width:100%;height:auto}}</style></head>\
<body><div class=\"nightpost-body\" style=\"font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;font-size:14px;line-height:1.45\">{inner}</div></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_html_is_defanged() {
        let h = editor_html(r#"<html><head><style>body{color:red}</style></head><body><p onclick="x()" class=a>Hi</p><img src=x onerror=alert(1)><a href="javascript:evil()">l</a><script>bad()</script><iframe src=x></iframe></body></html>"#);
        assert!(!h.contains("onclick") && !h.contains("onerror") && !h.contains("javascript:") && !h.contains("<script") && !h.contains("<iframe") && !h.contains("color:red"), "{h}");
        assert!(h.contains("<p class=a>Hi</p>"), "{h}");
        assert_eq!(editor_html("<p onmouseover='x'>Kia ōra</p>"), "<p>Kia ōra</p>");
    }

    #[test]
    fn editor_html_removes_every_frame_and_form() {
        let h = editor_html("<p>a</p><iframe src=x></iframe><p>b</p><iframe src=y></iframe><form><input></form><form></form><p>c</p>");
        assert!(!h.contains("<iframe") && !h.contains("<form"), "{h}");
        assert!(h.contains("<p>a</p>") && h.contains("<p>b</p>") && h.contains("<p>c</p>"), "{h}");
    }

    #[test]
    fn email_document_wraps_the_editor() {
        let d = email_document("<b>Hi</b>");
        assert!(d.starts_with("<!DOCTYPE html>") && d.contains("<b>Hi</b>") && d.contains("nightpost-body"));
    }

    #[test]
    fn frames_carry_the_policy_and_keep_links_out() {
        let policy = csp("n1", false);
        assert!(policy.contains("script-src 'nonce-n1'") && policy.contains("img-src data: cid:;") && policy.contains("form-action 'none'"));
        assert!(csp("n1", true).contains("img-src data: cid: https: http:"));
        let f = email_frame("<p>Hi</p>", &policy, "body{color:#fff}", true);
        assert!(f.contains("Content-Security-Policy\" content=\"default-src 'none'"));
        assert!(f.contains("<base target=\"_top\">") && f.contains("width=device-width") && f.contains("body{color:#fff}") && f.ends_with("<p>Hi</p>"));
        assert!(!email_frame("", &policy, "", false).contains("viewport"));
    }

    #[test]
    fn quotes_are_text_not_html() {
        let raw = b"From: Ann <ann@example.com>\r\nTo: tim@example.com\r\nSubject: Q&A\r\nMessage-ID: <q1@example.com>\r\nDate: Mon, 2 Nov 2026 10:00:00 +0000\r\n\r\n<b>not bold</b>\r\nline 2\r\n";
        let m = crate::parse::parse_message(raw, 1, 1, 1).unwrap();
        let q = quote_html(&m);
        assert!(q.contains("&lt;b&gt;not bold&lt;/b&gt;<br>") && q.contains("wrote:"), "{q}");
        let f = forward_html(&m, "Mon 1 Jan");
        assert!(f.contains("Subject: Q&amp;A") && f.contains("Date: Mon 1 Jan") && f.contains("Forwarded message"), "{f}");
    }
}
