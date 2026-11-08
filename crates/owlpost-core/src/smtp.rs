//! Outgoing mail over SMTP.

use crate::models::{Account, Contact, Security};
use anyhow::{anyhow, Context, Result};
use lettre::message::{header::ContentType, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::{SmtpTransport, Transport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutgoingMessage {
    pub to: Vec<Contact>,
    pub cc: Vec<Contact>,
    pub bcc: Vec<Contact>,
    pub subject: String,
    pub body: String,
    /// Message-ID being replied to (no angle brackets).
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// An HTML signature: where its plain-text rendering appears in `body`,
    /// the HTML part gets the real markup instead.
    #[serde(default)]
    pub html_signature: Option<HtmlSignature>,
    #[serde(default)]
    pub attachments: Vec<OutgoingAttachment>,
    /// The formatted body as a whole HTML document (from the formatted
    /// composer). Pictures in it as `data:` URLs are sent as inline parts.
    /// `body` stays the plain-text version.
    #[serde(default)]
    pub html: Option<String>,
}

/// A file sent with a message.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutgoingAttachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl OutgoingAttachment {
    pub fn new(filename: impl Into<String>, data: Vec<u8>) -> Self {
        let filename = filename.into();
        Self { content_type: mime_for(&filename).to_string(), filename, data }
    }

    /// Read a file from disk, named after the file.
    pub fn from_path(path: &std::path::Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
        Ok(Self::new(name, data))
    }
}

/// Most providers (Gmail, iCloud) refuse messages over about 25 MB, and
/// attachments grow by a third when encoded for sending.
pub const SIZE_LIMIT: usize = 25 * 1024 * 1024;

/// Roughly how big the message will be on the wire.
pub fn encoded_size(out: &OutgoingMessage) -> usize {
    out.body.len() + out.html.as_ref().map_or(0, String::len) + out.attachments.iter().map(|a| a.data.len() * 4 / 3).sum::<usize>()
}

/// A picture placed in the body: sent as an inline part the HTML refers to.
struct InlineImage {
    cid: String,
    content_type: String,
    data: Vec<u8>,
}

/// Turn `src="data:image/…;base64,…"` pictures into `cid:` references
/// (many mail apps, Gmail among them, don't show data: URLs) and return
/// the pictures to attach inline.
fn inline_images(html: &str, domain: &str) -> (String, Vec<InlineImage>) {
    use base64::Engine;
    let mut out = String::with_capacity(html.len());
    let mut images: Vec<InlineImage> = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("data:image/") {
        let (before, from) = rest.split_at(at);
        // Only inside an attribute value: the URL ends at the closing quote.
        let quote = before.chars().last().filter(|c| *c == '"' || *c == '\'');
        let end = match quote {
            Some(q) => from.find(q).unwrap_or(from.len()),
            None => {
                out.push_str(before);
                out.push_str("data:image/");
                rest = &from["data:image/".len()..];
                continue;
            }
        };
        let url = &from[..end];
        let parsed = url.split_once(',').and_then(|(meta, data)| {
            let ct = meta.strip_prefix("data:")?.strip_suffix(";base64")?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?;
            Some((ct.to_string(), bytes))
        });
        out.push_str(before);
        match parsed {
            Some((ct, bytes)) => {
                let cid = match images.iter().find(|i| i.data == bytes) {
                    Some(i) => i.cid.clone(),
                    None => {
                        let cid = format!("image{}.{}@{domain}", images.len() + 1, chrono::Utc::now().timestamp_millis());
                        images.push(InlineImage { cid: cid.clone(), content_type: ct, data: bytes });
                        cid
                    }
                };
                out.push_str(&format!("cid:{cid}"));
            }
            None => out.push_str(url),
        }
        rest = &from[end..];
    }
    out.push_str(rest);
    (out, images)
}

/// The MIME type for a file name, by extension.
pub fn mime_for(filename: &str) -> &'static str {
    let ext = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pages" => "application/vnd.apple.pages",
        "numbers" => "application/vnd.apple.numbers",
        "key" => "application/vnd.apple.keynote",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HtmlSignature {
    /// The signature as it appears in the plain-text body.
    pub text: String,
    /// The signature's HTML source.
    pub html: String,
}

/// Escape plain text and keep its line breaks, for an HTML part.
pub fn text_to_html(s: &str) -> String {
    let esc = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    esc.replace("\r\n", "\n").replace('\n', "<br>\n")
}

/// The HTML alternative for a body carrying an HTML signature, or `None`
/// when the signature text was edited out of the body (send plain text).
pub fn html_body(out: &OutgoingMessage) -> Option<String> {
    let sig = out.html_signature.as_ref()?;
    if sig.text.trim().is_empty() {
        return None;
    }
    let pos = out.body.find(&sig.text)?;
    let (before, after) = (&out.body[..pos], &out.body[pos + sig.text.len()..]);
    Some(format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body><div dir=\"auto\">{}<div class=\"owlpost-signature\">{}</div>{}</div></body></html>",
        text_to_html(before),
        crate::parse::clean_signature_html(&sig.html),
        text_to_html(after)
    ))
}

fn mailbox(c: &Contact) -> Result<Mailbox> {
    let addr = c.email.parse().with_context(|| format!("invalid address: {}", c.email))?;
    Ok(Mailbox::new((!c.name.trim().is_empty()).then(|| c.name.trim().to_string()), addr))
}

/// Build the RFC 5322 message. Returns `(message_id, raw bytes)`.
pub fn build(account: &Account, out: &OutgoingMessage) -> Result<(String, Vec<u8>)> {
    if out.to.is_empty() && out.cc.is_empty() && out.bcc.is_empty() {
        return Err(anyhow!("add at least one recipient"));
    }
    build_with(account, out, false)
}

/// Build a draft for the Drafts folder: recipients are optional and Bcc is
/// kept, so the draft can be finished later (here or on another device).
pub fn build_draft(account: &Account, out: &OutgoingMessage) -> Result<(String, Vec<u8>)> {
    build_with(account, out, true)
}

fn build_with(account: &Account, out: &OutgoingMessage, draft: bool) -> Result<(String, Vec<u8>)> {
    let domain = account.email.rsplit('@').next().unwrap_or("localhost");
    let message_id = format!(
        "{}.{}@{}",
        chrono::Utc::now().timestamp_millis(),
        std::process::id(),
        domain
    );
    let from = Contact::new(account.display_name.clone(), account.email.clone());
    let mut b = lettre::Message::builder()
        .from(mailbox(&from)?)
        .subject(out.subject.clone())
        .message_id(Some(format!("<{message_id}>")))
        .user_agent("OwlPost".into());
    for c in &out.to {
        b = b.to(mailbox(c)?);
    }
    for c in &out.cc {
        b = b.cc(mailbox(c)?);
    }
    for c in &out.bcc {
        b = b.bcc(mailbox(c)?);
    }
    if draft {
        b = b.keep_bcc();
        // Nothing is sent, but lettre wants an envelope even with no recipients.
        let me: lettre::Address = account.email.parse().with_context(|| format!("invalid address: {}", account.email))?;
        b = b.envelope(lettre::address::Envelope::new(Some(me.clone()), vec![me])?);
    }
    if let Some(irt) = &out.in_reply_to {
        b = b.in_reply_to(format!("<{irt}>"));
    }
    if !out.references.is_empty() {
        b = b.references(out.references.iter().map(|r| format!("<{r}>")).collect::<Vec<_>>().join(" "));
    }
    // The formatted composer's HTML, or the plain text with an HTML signature.
    let html = out.html.clone().or_else(|| html_body(out));
    // Text and HTML versions; pictures in the body ride along inline.
    let alternative = |html: String| -> MultiPart {
        let (html, images) = inline_images(&html, domain);
        if images.is_empty() {
            return MultiPart::alternative_plain_html(out.body.clone(), html);
        }
        let mut related = MultiPart::related().singlepart(SinglePart::html(html));
        for img in images {
            let ct = ContentType::parse(&img.content_type).unwrap_or_else(|_| ContentType::parse("application/octet-stream").expect("valid"));
            related = related.singlepart(lettre::message::Attachment::new_inline(img.cid).body(img.data, ct));
        }
        MultiPart::alternative().singlepart(SinglePart::plain(out.body.clone())).multipart(related)
    };
    let email = match (html, out.attachments.is_empty()) {
        (Some(html), true) => b.multipart(alternative(html))?,
        (None, true) => b.header(ContentType::TEXT_PLAIN).body(out.body.clone())?,
        // With attachments: the text (and HTML) first, then each file.
        (html, false) => {
            let mut mixed = match html {
                Some(html) => MultiPart::mixed().multipart(alternative(html)),
                None => MultiPart::mixed().singlepart(SinglePart::plain(out.body.clone())),
            };
            for a in &out.attachments {
                let ct = ContentType::parse(&a.content_type).unwrap_or_else(|_| ContentType::parse("application/octet-stream").expect("valid"));
                mixed = mixed.singlepart(lettre::message::Attachment::new(a.filename.clone()).body(a.data.clone(), ct));
            }
            b.multipart(mixed)?
        }
    };
    Ok((message_id, email.formatted()))
}

/// Send via the account's SMTP server. Returns the generated Message-ID and
/// the raw message (so callers can APPEND it to Sent where needed).
pub fn send(account: &Account, password: &str, out: &OutgoingMessage) -> Result<(String, Vec<u8>)> {
    let (id, raw) = build(account, out)?;
    let envelope = {
        let from = account.email.parse()?;
        let rcpts = out
            .to
            .iter()
            .chain(&out.cc)
            .chain(&out.bcc)
            .map(|c| c.email.parse())
            .collect::<Result<Vec<_>, _>>()?;
        lettre::address::Envelope::new(Some(from), rcpts)?
    };
    let creds = Credentials::new(account.username.clone(), password.to_string());
    let s = &account.smtp;
    let transport = match s.security {
        Security::Tls => SmtpTransport::relay(&s.host)?,
        Security::StartTls => SmtpTransport::starttls_relay(&s.host)?,
        Security::None => SmtpTransport::builder_dangerous(&s.host),
    }
    .port(s.port)
    .credentials(creds)
    .authentication(match account.auth {
        crate::models::Auth::Google => vec![Mechanism::Xoauth2],
        crate::models::Auth::Password => vec![Mechanism::Plain, Mechanism::Login],
    })
    .timeout(Some(std::time::Duration::from_secs(60)))
    .build();
    transport.send_raw(&envelope, &raw).context("SMTP send failed")?;
    Ok((id, raw))
}

/// Gmail and Microsoft 365 file sent mail automatically; others need an APPEND.
pub fn server_saves_sent_copy(account: &Account) -> bool {
    let h = account.smtp.host.to_ascii_lowercase();
    h.ends_with("gmail.com") || h.ends_with("office365.com") || h.ends_with("outlook.com")
}

/// Standard reply quoting: `On <date>, <name> wrote:` followed by `> ` lines.
pub fn quote(original: &crate::models::Message) -> String {
    let when = original.date.with_timezone(&chrono::Local).format("%a, %e %b %Y at %H:%M");
    let mut s = format!("\n\nOn {when}, {} wrote:\n", original.from.display());
    for line in original.body_text.lines() {
        s.push_str("> ");
        s.push_str(line);
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatted_html_with_pictures() {
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG fake");
        let account = Account {
            id: 1,
            display_name: "Me".into(),
            email: "me@example.com".into(),
            username: "me@example.com".into(),
            imap: crate::models::ServerConfig { host: "imap.example.com".into(), port: 993, security: Security::Tls },
            smtp: crate::models::ServerConfig { host: "smtp.example.com".into(), port: 465, security: Security::Tls },
            color: "#000".into(),
            signature: String::new(),
            demo: false,
            auth: crate::models::Auth::Password,
        };
        let out = OutgoingMessage {
            to: vec![Contact::new("Ann", "ann@example.com")],
            subject: "Look".into(),
            body: "Hello there".into(),
            html: Some(format!("<p><b>Hello</b> there</p><img src=\"data:image/png;base64,{png}\"><img src='data:image/png;base64,{png}'>")),
            ..Default::default()
        };
        let (_, raw) = build(&account, &out).unwrap();
        let raw = String::from_utf8_lossy(&raw);
        assert!(raw.contains("multipart/alternative") && raw.contains("multipart/related"), "{raw}");
        assert!(raw.contains("Content-ID: <image1.") && !raw.contains("Content-ID: <image2."), "the same picture is sent once");
        assert!(!raw.contains("data:image/png"), "no data: URLs left in the HTML");
        let parsed = crate::parse::parse_message(raw.as_bytes(), 1, 1, 1).unwrap();
        assert!(parsed.body_html.unwrap().contains("<b>Hello</b>"));
        assert_eq!(parsed.body_text.trim(), "Hello there");
    }
    use crate::models::*;

    #[test]
    fn builds_reply_headers() {
        let acc = Account {
            id: 1,
            display_name: "Tim".into(),
            email: "tim@icloud.com".into(),
            username: "tim@icloud.com".into(),
            imap: ServerConfig { host: "imap.mail.me.com".into(), port: 993, security: Security::Tls },
            smtp: ServerConfig { host: "smtp.mail.me.com".into(), port: 587, security: Security::StartTls },
            color: "#000".into(),
            signature: String::new(),
            demo: false,
            auth: Auth::Password,
        };
        let out = OutgoingMessage {
            to: vec![Contact::new("Ann", "ann@example.com")],
            subject: "Re: Hi".into(),
            body: "Thanks!".into(),
            in_reply_to: Some("root@example.com".into()),
            references: vec!["root@example.com".into()],
            ..Default::default()
        };
        let (_, raw) = build(&acc, &out).unwrap();
        let s = String::from_utf8(raw).unwrap();
        assert!(s.contains("In-Reply-To: <root@example.com>"));
        assert!(!s.contains("multipart"), "plain message stays plain");

        // An HTML signature turns it into text + HTML alternatives.
        let sig = HtmlSignature { text: "Tim | timson.co".into(), html: "<b>Tim</b> | <a href=\"https://timson.co\">timson.co</a>".into() };
        let out = OutgoingMessage { body: "Thanks!\n\nTim | timson.co\n\nOn Mon, Ann wrote:\n> <hi>".into(), html_signature: Some(sig.clone()), ..out };
        let html = html_body(&out).unwrap();
        assert!(html.contains("Thanks!<br>"));
        assert!(html.contains("<b>Tim</b> | <a href=\"https://timson.co\">"));
        assert!(html.contains("&gt; &lt;hi&gt;"), "quoted text is escaped");
        let (_, raw) = build(&acc, &out).unwrap();
        let s = String::from_utf8(raw).unwrap();
        assert!(s.contains("multipart/alternative") && s.contains("text/html"));
        // Edited out of the body: falls back to plain text.
        let edited = OutgoingMessage { body: "Thanks!".into(), ..out.clone() };
        assert!(html_body(&edited).is_none());
        assert!(s.contains("To: Ann <ann@example.com>"));

        // Attachments: the body (with its HTML signature) then the files,
        // which come back intact when the message is read.
        let pdf = b"%PDF-1.4 not really".to_vec();
        let out = OutgoingMessage {
            attachments: vec![OutgoingAttachment::new("Quote 2026.pdf", pdf.clone()), OutgoingAttachment::new("photo.JPG", vec![0xff, 0xd8, 0xff, 0])],
            ..out
        };
        let (_, raw) = build(&acc, &out).unwrap();
        let s = String::from_utf8(raw.clone()).unwrap();
        assert!(s.contains("multipart/mixed") && s.contains("multipart/alternative"));
        let m = crate::parse::parse_message(&raw, 1, 1, 1).unwrap();
        assert!(m.body_text.starts_with("Thanks!"));
        let names: Vec<_> = m.attachments.iter().map(|a| (a.filename.as_str(), a.mime_type.as_str(), a.size)).collect();
        assert_eq!(names, [("Quote 2026.pdf", "application/pdf", pdf.len()), ("photo.JPG", "image/jpeg", 4)]);
        // Plain text with an attachment.
        let plain = OutgoingMessage { html_signature: None, body: "See attached".into(), ..out };
        let raw = build(&acc, &plain).unwrap().1;
        let m = crate::parse::parse_message(&raw, 1, 1, 1).unwrap();
        assert_eq!((m.body_text.trim(), m.attachments.len()), ("See attached", 2));
    }
}
