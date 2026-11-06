//! Outgoing mail over SMTP.

use crate::models::{Account, Contact, Security};
use anyhow::{anyhow, Context, Result};
use lettre::message::{header::ContentType, Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
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
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body><div dir=\"auto\">{}<div class=\"springpost-signature\">{}</div>{}</div></body></html>",
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
        .user_agent("SpringPost".into());
    for c in &out.to {
        b = b.to(mailbox(c)?);
    }
    for c in &out.cc {
        b = b.cc(mailbox(c)?);
    }
    for c in &out.bcc {
        b = b.bcc(mailbox(c)?);
    }
    if let Some(irt) = &out.in_reply_to {
        b = b.in_reply_to(format!("<{irt}>"));
    }
    if !out.references.is_empty() {
        b = b.references(out.references.iter().map(|r| format!("<{r}>")).collect::<Vec<_>>().join(" "));
    }
    let email = match html_body(out) {
        Some(html) => b.multipart(MultiPart::alternative_plain_html(out.body.clone(), html))?,
        None => b.header(ContentType::TEXT_PLAIN).body(out.body.clone())?,
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
        let edited = OutgoingMessage { body: "Thanks!".into(), ..out };
        assert!(html_body(&edited).is_none());
        assert!(s.contains("To: Ann <ann@example.com>"));
    }
}
