//! Turning raw RFC 5322 bytes into [`Message`]s.

use crate::models::*;
use chrono::{TimeZone, Utc};
use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders};

fn contacts(addr: Option<&Address>) -> Vec<Contact> {
    let Some(addr) = addr else { return vec![] };
    addr.iter()
        .filter_map(|a| {
            let email = a.address()?.to_string();
            Some(Contact { name: a.name().unwrap_or_default().to_string(), email })
        })
        .collect()
}

fn header_ids(v: &HeaderValue) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(l) => l.iter().map(|s| s.to_string()).collect(),
        _ => vec![],
    }
}

/// Collapse whitespace and trim to a list-view preview.
pub fn make_snippet(text: &str) -> String {
    let mut out = String::with_capacity(200);
    for line in text.lines() {
        let l = line.trim();
        // Skip quoted text and "On ... wrote:" attributions.
        if l.starts_with('>') || (l.starts_with("On ") && l.ends_with("wrote:")) {
            continue;
        }
        for w in l.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(w);
            if out.len() > 180 {
                let mut cut = 180;
                while !out.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.truncate(cut);
                out.push('…');
                return out;
            }
        }
    }
    out
}

pub fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 100).unwrap_or_else(|_| html.to_string())
}

/// Make pasted signature HTML safe to drop into an email.
///
/// Signatures copied from other apps are often whole pages: Canary's include
/// a dark-mode `<style>` that inverts every colour (and would invert the
/// recipient's view too). Keep only what's inside `<body>` — wrapped in a
/// div carrying the body's inline style — and drop `<style>`, `<script>`,
/// `<head>`, `<meta>`, `<title>` and `contenteditable`.
pub fn clean_signature_html(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    // Body content (and its style), if this is a full document.
    let (mut inner, body_style) = match lower.find("<body") {
        Some(b) => {
            let open_end = lower[b..].find('>').map(|e| b + e + 1).unwrap_or(html.len());
            let tag = &html[b..open_end];
            let end = lower[open_end..].find("</body").map(|e| open_end + e).unwrap_or(html.len());
            (html[open_end..end].to_string(), attr(tag, "style"))
        }
        None => (html.to_string(), None),
    };
    for tag in ["style", "script", "head", "title"] {
        inner = strip_element(&inner, tag);
    }
    for tag in ["<meta", "<html", "</html", "<body", "</body", "<!doctype"] {
        inner = strip_tags_starting(&inner, tag);
    }
    inner = inner.replace(" contenteditable=\"true\"", "").replace(" contenteditable", "");
    let inner = inner.trim().to_string();
    match body_style {
        Some(st) if !st.trim().is_empty() && !inner.is_empty() => format!("<div style=\"{}\">{inner}</div>", st.replace('"', "'")),
        _ => inner,
    }
}

/// The value of attribute `name` in an opening tag.
pub fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(name) {
        let at = from + rel;
        let before_ok = at > 0 && lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = lower[at + name.len()..].trim_start();
        if before_ok && rest.starts_with('=') {
            let v = tag[tag.len() - rest.len() + 1..].trim_start();
            let q = v.chars().next()?;
            return Some(if q == '"' || q == '\'' { v[1..].split(q).next().unwrap_or("").to_string() } else { v.split([' ', '>', '/']).next().unwrap_or("").to_string() });
        }
        from = at + name.len();
    }
    None
}

/// Remove `<tag …>…</tag>` blocks entirely (case-insensitive).
fn strip_element(html: &str, tag: &str) -> String {
    let mut out = html.to_string();
    loop {
        let lower = out.to_ascii_lowercase();
        let Some(start) = lower.find(&format!("<{tag}")) else { return out };
        let close = format!("</{tag}>");
        let end = lower[start..].find(&close).map(|e| start + e + close.len()).unwrap_or(out.len());
        out.replace_range(start..end, "");
    }
}

/// Remove single tags beginning with `prefix` (e.g. `<meta`), keeping content.
fn strip_tags_starting(html: &str, prefix: &str) -> String {
    let mut out = html.to_string();
    loop {
        let lower = out.to_ascii_lowercase();
        let Some(start) = lower.find(prefix) else { return out };
        let end = lower[start..].find('>').map(|e| start + e + 1).unwrap_or(out.len());
        out.replace_range(start..end, "");
    }
}

/// Plain-text rendering of an HTML signature: no wrapping, links inline
/// rather than as footnotes, trailing whitespace trimmed.
pub fn signature_text(html: &str) -> String {
    let html = inline_links(&clean_signature_html(html));
    let text = html2text::config::plain_no_decorate()
        .link_footnotes(false)
        .no_link_wrapping()
        .string_from_read(html.as_bytes(), 10_000)
        .unwrap_or_else(|_| html.clone());
    // Collapse runs of spaces (signatures are full of &nbsp; padding).
    text.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Replace `<a href=URL>TEXT</a>` with `TEXT`, or `TEXT <URL>` when the text
/// doesn't already say where the link goes (so plain-text readers keep it).
fn inline_links(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(rel) = lower[i..].find("<a") {
        let start = i + rel;
        let next = lower.as_bytes().get(start + 2).copied().unwrap_or(b'>');
        let (Some(open_end), true) = (lower[start..].find('>').map(|e| start + e), next == b' ' || next == b'>' || next == b'\n') else {
            out.push_str(&html[i..start + 2]);
            i = start + 2;
            continue;
        };
        let Some(close) = lower[open_end..].find("</a>").map(|c| open_end + c) else { break };
        let tag = &html[start..open_end];
        let href = tag
            .to_ascii_lowercase()
            .find("href=")
            .map(|h| {
                let v = tag[h + 5..].trim_start();
                let q = v.chars().next().unwrap_or(' ');
                if q == '"' || q == '\'' { v[1..].split(q).next().unwrap_or("").to_string() } else { v.split([' ', '>']).next().unwrap_or("").to_string() }
            })
            .unwrap_or_default();
        let inner = &html[open_end + 1..close];
        let inner_text = html2text::config::plain_no_decorate().string_from_read(inner.as_bytes(), 10_000).unwrap_or_default();
        let bare = |s: &str| {
            let s = s.trim().trim_end_matches('/').to_ascii_lowercase();
            ["https://", "http://", "mailto:", "tel:", "www."].iter().fold(s, |s, p| s.strip_prefix(p).map(str::to_string).unwrap_or(s))
        };
        out.push_str(&html[i..start]);
        out.push_str(inner);
        let (bh, bt) = (bare(&href), bare(&inner_text));
        // The text already is the address (e.g. "timson.co" → https://timson.co).
        let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
        let says_where = bh == bt
            || (bt.contains(['.', '@']) && bh.starts_with(&bt))
            || (href.to_ascii_lowercase().starts_with("tel:") && !digits(&bt).is_empty() && digits(&bh.replace("%20", "")) == digits(&bt));
        if !href.is_empty() && !says_where {
            out.push_str(&format!(" &lt;{}&gt;", href.replace(['<', '>'], "")));
        }
        i = close + 4;
    }
    out.push_str(&html[i..]);
    out
}

/// Parse a raw message. `thread_key` is filled with the root reference and
/// should be refined with [`crate::store::Store::resolve_thread_key`].
pub fn parse_message(raw: &[u8], account_id: AccountId, folder_id: FolderId, uid: u32) -> Option<Message> {
    let msg = MessageParser::default().parse(raw)?;
    let from = contacts(msg.from()).into_iter().next().unwrap_or_default();
    let to = contacts(msg.to());
    let cc = contacts(msg.cc());
    let date = msg
        .date()
        .and_then(|d| Utc.timestamp_opt(d.to_timestamp(), 0).single())
        .unwrap_or_else(Utc::now);
    let message_id = msg
        .message_id()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("owlpost-{account_id}-{folder_id}-{uid}@local"));
    let in_reply_to = header_ids(msg.in_reply_to()).into_iter().next().unwrap_or_default();
    let references = header_ids(msg.references());

    let body_html = msg.body_html(0).map(|s| s.to_string());
    let body_text = match msg.body_text(0) {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => body_html.as_deref().map(html_to_text).unwrap_or_default(),
    };
    let attachments = msg
        .attachments()
        .map(|a| Attachment {
            filename: a.attachment_name().unwrap_or("attachment").to_string(),
            mime_type: a
                .content_type()
                .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: a.contents().len(),
        })
        .collect();
    let thread_key = references.first().cloned().unwrap_or_else(|| message_id.clone());
    Some(Message {
        id: 0,
        account_id,
        folder_id,
        uid,
        snippet: make_snippet(&body_text),
        message_id,
        thread_key,
        in_reply_to,
        references,
        subject: msg.subject().unwrap_or("(no subject)").to_string(),
        from,
        to,
        cc,
        date,
        body_text,
        body_html,
        attachments,
        unread: true,
        starred: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_message() {
        let raw = b"From: Ann Lee <ann@example.com>\r\nTo: bob@example.com\r\nSubject: Re: Lunch\r\n\
Message-ID: <abc@example.com>\r\nIn-Reply-To: <root@example.com>\r\nReferences: <root@example.com>\r\n\
Date: Tue, 3 Nov 2026 10:00:00 +0000\r\nContent-Type: text/plain\r\n\r\nSounds good!\r\n\r\n> earlier\r\n";
        let m = parse_message(raw, 1, 2, 3).unwrap();
        assert_eq!(m.from.email, "ann@example.com");
        assert_eq!(m.from.name, "Ann Lee");
        assert_eq!(m.message_id, "abc@example.com");
        assert_eq!(m.in_reply_to, "root@example.com");
        assert_eq!(m.thread_key, "root@example.com");
        assert_eq!(m.snippet, "Sounds good!");
    }
}

#[cfg(test)]
mod signature_tests {
    use super::signature_text;

    #[test]
    fn html_signatures_read_cleanly() {
        let t = signature_text("<p><b>Tim</b> · Photographer<br><a href=\"https://timson.co\">timson.co</a> · <a href='https://instagram.com/timson'>Instagram</a> · <a href=\"mailto:tim@x.co\">tim@x.co</a></p>");
        assert_eq!(t, "Tim · Photographer\ntimson.co · Instagram <https://instagram.com/timson> · tim@x.co");
    }

    #[test]
    fn cleans_pasted_canary_signature() {
        let raw = include_str!("../tests/fixtures/canary-signature.html");
        let clean = super::clean_signature_html(raw);
        let lower = clean.to_lowercase();
        assert!(!lower.contains("<style") && !lower.contains("invert") && !lower.contains("<html") && !lower.contains("<head") && !lower.contains("<body"));
        assert!(!lower.contains("contenteditable"));
        assert!(clean.starts_with("<div style=\"font-family:Helvetica;font-size:13px;\">"), "{}", &clean[..80]);
        assert!(clean.contains("<img src=\"https://www.dropbox.com/") && clean.contains("<b>Tim Morrison</b>"));
        // Cleaning twice changes nothing.
        assert_eq!(super::clean_signature_html(&clean), clean);
        let text = signature_text(raw);
        assert_eq!(
            text,
            "Tim Morrison\nfounder & creative\n\ntimson.co\nt. 02102 764 486 | tim@timson.co | www.timson.co\n\n\nHEADSHOTS THAT MAKE YOU LOOK AWESOME FOR LESS\nLEARN MORE: https://headshotparty.co"
        );
        assert!(text.starts_with("Tim Morrison"), "{text}");
        assert!(!text.contains("invert") && !text.contains("filter"), "{text}");
    }
}
