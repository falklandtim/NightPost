//! Turning raw RFC 5322 bytes into [`Message`]s.

use crate::models::*;
use chrono::{TimeZone, Utc};
use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders};

pub(crate) fn contacts(addr: Option<&Address>) -> Vec<Contact> {
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

/// Bump when [`make_snippet`] changes, so stored previews are rebuilt.
pub const SNIPPET_VERSION: i64 = 2;

/// Collapse whitespace and trim to a list-view preview.
///
/// Newsletters' text parts are full of things nobody wants to read in a
/// list: link targets (`<https://…>`, `[https://…]`, Markdown `[text](url)`),
/// invisible padding characters that keep Gmail from showing the body as
/// a preview (`&zwnj;`, U+034F, which draw as boxes), and rows of `****`.
pub fn make_snippet(text: &str) -> String {
    let mut kept = String::with_capacity(text.len().min(8192));
    for line in text.lines() {
        let l = line.trim();
        // Skip quoted text and "On ... wrote:" attributions.
        if l.starts_with('>') || (l.starts_with("On ") && l.ends_with("wrote:")) {
            continue;
        }
        kept.push_str(l);
        kept.push('\n');
        if kept.len() > 8000 {
            break;
        }
    }
    let clean = strip_links(&strip_footnote_refs(&decode_entities(&kept)));
    let mut out = String::with_capacity(200);
    let mut last = String::new();
    for w in clean.split_whitespace() {
        let w: String = w.chars().filter(|c| !invisible(*c) && !('\u{2500}'..='\u{257F}').contains(c)).collect();
        let lower = w.to_ascii_lowercase();
        if w.is_empty()
            || w == last
            || lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("www.")
            || matches!(w.as_str(), "()" | "[]" | "<>" | "[[]]")
            || (w.len() >= 3 && w.chars().all(|c| c.is_ascii_punctuation()))
        {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&w);
        if out.len() > 180 {
            let mut cut = 180;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push('…');
            return out;
        }
        last = w;
    }
    out
}

/// Zero-width and other characters that only pad or steer text.
fn invisible(c: char) -> bool {
    matches!(c, '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}' | '\u{180B}'..='\u{180F}'
        | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FEFF}' | '\u{FFA0}')
}

/// HTML entities that leak into text parts (`&zwnj;`, `&nbsp;`, `&#847;`).
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).map(|e| e + 1);
        let decoded = match end {
            Some(e) if rest[e..].starts_with(';') && e > 1 => {
                let name = &rest[1..e];
                let c = match name {
                    "nbsp" | "ensp" | "emsp" | "thinsp" => Some(' '),
                    "zwnj" | "zwj" | "shy" | "lrm" | "rlm" => Some('\u{200B}'),
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "hellip" => Some('…'),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "rsquo" => Some('’'),
                    "lsquo" => Some('‘'),
                    _ => name
                        .strip_prefix("#x")
                        .or_else(|| name.strip_prefix("#X"))
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                        .and_then(char::from_u32),
                };
                c.map(|c| (c, e + 1))
            }
            _ => None,
        };
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_link(s: &str) -> bool {
    let s = s.trim_start_matches(|c: char| c == '[' || c.is_whitespace()).to_ascii_lowercase();
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with("mailto:") || s.starts_with("www.")
}

/// Drop link targets, keeping link text: `[text](url)` → `text`, and
/// `<url>`, `<[[url]]>`, `[url]`, `(url)` → nothing.
/// Undo html2text's link footnotes for a preview: `[text][3]` becomes
/// `text`, and a linked picture `[ [Logo] ][1]` goes (its alt text isn't
/// something you'd read).
fn strip_footnote_refs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find("][") {
        let after = &rest[at + 2..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || !after[digits..].starts_with(']') {
            out.push_str(&rest[..at + 2]);
            rest = after;
            continue;
        }
        // The '[' that opens the link text (pictures nest one more).
        let head = &rest[..at];
        let mut depth = 0;
        let open = head.char_indices().rev().find(|&(_, c)| {
            match c {
                ']' => depth += 1,
                '[' if depth == 0 => return true,
                '[' => depth -= 1,
                _ => {}
            }
            false
        });
        match open {
            Some((o, _)) => {
                out.push_str(&head[..o]);
                let inner = head[o + 1..].trim();
                if !(inner.starts_with('[') && inner.ends_with(']')) {
                    out.push_str(inner);
                }
            }
            None => out.push_str(head),
        }
        rest = &after[digits + 1..];
    }
    out.push_str(rest);
    out
}

fn strip_links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let b = s.as_bytes();
    // The closing byte for an opener, searched no further than `max` bytes.
    let close = |from: usize, open: u8, shut: u8, max: usize| -> Option<usize> {
        let stop = (from + max).min(b.len());
        (from + 1..stop).find(|&j| b[j] == shut || (b[j] == open && open != shut)).filter(|&j| b[j] == shut)
    };
    while i < b.len() {
        let c = b[i];
        let skip = match c {
            b'[' => {
                // [[url]] or [url]
                let inner_start = i;
                let mut j = i;
                while j < b.len() && b[j] == b'[' {
                    j += 1;
                }
                match (j..(j + 2000).min(b.len())).find(|&k| b[k] == b']') {
                    Some(k) if is_link(&s[j..k]) => {
                        let mut e = k;
                        while e < b.len() && b[e] == b']' {
                            e += 1;
                        }
                        Some(e)
                    }
                    Some(k) if j == inner_start + 1 && b.get(k + 1) == Some(&b'(') => {
                        // [text](url)
                        match close(k + 1, b'(', b')', 2000) {
                            Some(p) if is_link(&s[k + 2..p]) || s[k + 2..p].starts_with('/') => {
                                out.push_str(&s[j..k]);
                                Some(p + 1)
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
            b'<' => close(i, b'<', b'>', 4000).filter(|&k| is_link(&s[i + 1..k])).map(|k| k + 1),
            b'(' => close(i, b'(', b')', 2000).filter(|&k| is_link(&s[i + 1..k])).map(|k| k + 1),
            _ => None,
        };
        match skip {
            Some(e) => {
                out.push(' ');
                i = e;
            }
            None => {
                let ch = s[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// Readable text from email HTML. Email layouts are built from tables, so
/// they're read as plain lines (raw mode), never drawn with box borders
/// that previews would show as rows of ─ and │.
pub fn html_to_text(html: &str) -> String {
    html2text::config::plain().raw_mode(true).string_from_read(html.as_bytes(), 100).unwrap_or_else(|_| html.to_string())
}

/// Text made by older versions, which drew table borders.
pub fn has_table_borders(text: &str) -> bool {
    text.contains("──") || text.contains('│')
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

    let category = crate::category::classify(&msg);
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
        category,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_only_mail_reads_without_table_borders() {
        let html = "<table><tr><td><a href=\"https://x.example\"><img src=\"https://x.example/logo.png\" alt=\"Council Logo\"></a></td></tr>\
<tr><td>Kia ora,<br>Your latest rates notice is attached. If you have any questions, <a href=\"https://x.example/c\">contact us</a>.</td></tr>\
<tr><td>Amount due</td><td>$1,176.24</td></tr></table>";
        let text = html_to_text(html);
        assert!(!has_table_borders(&text), "{text}");
        assert!(text.contains("Amount due") && text.contains("$1,176.24"));
        let snip = make_snippet(&text);
        assert!(snip.starts_with("Kia ora, Your latest rates notice is attached. If you have any questions, contact us."), "{snip}");
        assert_eq!(strip_footnote_refs("see [docs][12] and [ [Pay Now] ][3] now [x] a][b"), "see docs and  now [x] a][b");
    }

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

    #[test]
    fn newsletter_previews_read_cleanly() {
        // Marketo: links in angle brackets, some wrapped over several lines.
        assert_eq!(
            make_snippet("View in your browser <[[https://go.example.nz/v/NzI3]]>\n\nYou last purchased from\n  \nLyall Bay\nFind your store here <[[\n\n   \n https://www.example.nz/store/x]]>\nFind Parts Fast <https://go.example.nz/dc/abc>"),
            "View in your browser You last purchased from Lyall Bay Find your store here Find Parts Fast"
        );
        // Plain-text conversions with [url] after the link text.
        assert_eq!(make_snippet("Hi there, Beam [https://ollama.com/library/beam] from Reflection AI"), "Hi there, Beam from Reflection AI");
        // Markdown links keep their text; repeats collapse.
        assert_eq!(make_snippet("[BLUNT](https://x.nz) [Shop All](https://x.nz/all) [GWP](https://x.nz) [GWP](https://x.nz)"), "BLUNT Shop All GWP");
        // Preview padding: literal entities and invisible characters.
        assert_eq!(make_snippet("&zwnj; &zwnj; &nbsp; &#847; Your gift of NZ$20 is waiting"), "Your gift of NZ$20 is waiting");
        assert_eq!(make_snippet("Onions, radishes and more! \u{34F} \u{34F} \u{200C}\u{34F} \u{34F}"), "Onions, radishes and more!");
        // Rows of asterisks, and "( url )".
        assert_eq!(make_snippet("**********\nLittle Farms ( https://example.com?id=1 )\nView in the browser ( https://t.io/d/x )"), "Little Farms View in the browser");
        // Ordinary text is untouched.
        assert_eq!(make_snippet("Fish & chips (with peas) at 6 <maybe> [sic]"), "Fish & chips (with peas) at 6 <maybe> [sic]");
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
