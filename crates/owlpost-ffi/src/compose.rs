//! Writing email on the phone: replies and forwards set up as the desktop
//! sets them up, drafts saved to the server's Drafts folder (so the Mac sees
//! them), and sending through the core's SMTP.
//!
//! The phone's editor works in HTML. A [`Draft`] carries that HTML, the
//! addresses as typed ("Ann <ann@x.com>, bob@y.com"), and attachments as
//! files on the phone; [`outgoing`] turns it into the message the core
//! sends, with a plain-text version alongside.
//!
//! Only Tim sends, from the composer. Nothing here is reachable from the
//! MCP server.

use super::{fail, Mail, Result};
use owlpost_core::smtp::{OutgoingAttachment, OutgoingMessage, SIZE_LIMIT};
use owlpost_core::{engine, mail_html, mcp, parse, Contact, FolderRole, Message};

/// A message being written.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Draft {
    /// The account it's sent from.
    pub account_id: i64,
    /// Addresses as typed, comma-separated.
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    /// The editor's HTML, quoted text included.
    pub html: String,
    /// The Message-ID it replies to, without angle brackets.
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Files on the phone to send with it.
    pub attachments: Vec<String>,
    /// The saved draft (an email id) this one replaces: deleted from the
    /// server once the new one is saved, or once it's sent.
    pub replaces: Option<i64>,
}

/// A reply or a forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ComposeKind {
    Reply,
    ReplyAll,
    Forward,
}

/// Someone to suggest in an address field.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ContactSuggestion {
    pub name: String,
    pub email: String,
}

/// The empty line to type on, above any quoted text.
const TYPE_HERE: &str = "<div><br></div>";

/// The draft as the core sends it: HTML plus a plain-text version, the
/// attachments read from disk. Errors name what's wrong in words to show.
pub(crate) fn outgoing(d: &Draft) -> anyhow::Result<OutgoingMessage> {
    let parse = |field: &str, s: &str| -> anyhow::Result<Vec<Contact>> {
        let list = Contact::parse_list(s);
        if let Some(bad) = list.iter().find(|c| !c.email.contains('@') || c.email.contains(char::is_whitespace)) {
            anyhow::bail!("{field}: \"{}\" isn't an email address", bad.email);
        }
        Ok(list)
    };
    let attachments = d.attachments.iter().map(|p| OutgoingAttachment::from_path(std::path::Path::new(p))).collect::<anyhow::Result<Vec<_>>>()?;
    let out = OutgoingMessage {
        to: parse("To", &d.to)?,
        cc: parse("Cc", &d.cc)?,
        bcc: parse("Bcc", &d.bcc)?,
        subject: d.subject.trim().to_string(),
        body: parse::html_to_text(&d.html),
        in_reply_to: d.in_reply_to.clone().filter(|s| !s.is_empty()),
        references: d.references.clone(),
        html_signature: None,
        attachments,
        html: Some(mail_html::email_document(&d.html)),
    };
    if owlpost_core::smtp::encoded_size(&out) > SIZE_LIMIT {
        anyhow::bail!("this message is over 25 MB, more than most mail servers accept: remove an attachment or two");
    }
    Ok(out)
}

/// A reply or forward of `m`, from `account`, with the original quoted as
/// text below an empty line. Forwards keep the attachments, given as files.
pub(crate) fn template(account: &owlpost_core::Account, m: &Message, kind: ComposeKind, attachments: Vec<String>) -> Draft {
    let date = m.date.with_timezone(&chrono::Local).format("%a, %e %b %Y at %H:%M").to_string();
    let (to, cc, subject, in_reply_to, references, quote) = match kind {
        ComposeKind::Forward => {
            let subject = if m.subject.to_ascii_lowercase().starts_with("fwd:") { m.subject.clone() } else { format!("Fwd: {}", m.subject) };
            (String::new(), String::new(), subject, None, vec![], mail_html::forward_html(m, &date))
        }
        _ => {
            let r = mcp::reply_draft(account, m, kind == ComposeKind::ReplyAll);
            (Contact::join(&r.to), Contact::join(&r.cc), r.subject, r.in_reply_to, r.references, mail_html::quote_html(m))
        }
    };
    Draft {
        account_id: account.id,
        to,
        cc,
        bcc: String::new(),
        subject,
        html: format!("{TYPE_HERE}{quote}"),
        in_reply_to,
        references,
        attachments: if kind == ComposeKind::Forward { attachments } else { vec![] },
        replaces: None,
    }
}

#[uniffi::export]
impl Mail {
    /// Set up a reply, reply-all or forward of an email. A forward fetches
    /// the original's attachments to send on. Blocks on the network for
    /// forwards with attachments.
    pub fn start_compose(&self, message_id: i64, kind: ComposeKind) -> Result<Draft> {
        let store = self.worker()?;
        let m = store.message(message_id)?.ok_or_else(|| fail("that email no longer exists"))?;
        let acc = self.account(&store, m.account_id)?;
        let files = if kind == ComposeKind::Forward {
            (0..m.attachments.len()).map(|i| engine::attachment_file(&store, &acc, &m, i).map(|p| p.to_string_lossy().into_owned())).collect::<anyhow::Result<Vec<_>>>()?
        } else {
            vec![]
        };
        Ok(template(&acc, &m, kind, files))
    }

    /// Reopen a saved draft to carry on writing it, with its Bcc and
    /// attachments fetched from the server. Blocks on the network.
    pub fn open_draft(&self, message_id: i64) -> Result<Draft> {
        let store = self.worker()?;
        let m = store.message(message_id)?.ok_or_else(|| fail("that draft no longer exists"))?;
        let acc = self.account(&store, m.account_id)?;
        let parts = engine::open_draft(&store, &acc, &m)?;
        let html = match m.body_html.as_deref() {
            Some(h) if !h.trim().is_empty() => mail_html::editor_html(h),
            _ => owlpost_core::smtp::text_to_html(m.body_text.trim_end()),
        };
        Ok(Draft {
            account_id: acc.id,
            to: Contact::join(&m.to),
            cc: Contact::join(&m.cc),
            bcc: Contact::join(&parts.bcc),
            subject: if m.subject == "(no subject)" { String::new() } else { m.subject.clone() },
            html,
            in_reply_to: (!m.in_reply_to.is_empty()).then(|| m.in_reply_to.clone()),
            references: m.references.clone(),
            attachments: parts.attachments.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            replaces: Some(m.id),
        })
    }

    /// Save the draft in its account's Drafts folder on the server,
    /// replacing the one it was opened from. Returns the saved draft's id:
    /// pass it as `replaces` next time. Blocks on the network.
    pub fn save_draft(&self, draft: Draft) -> Result<i64> {
        let store = self.worker()?;
        let acc = self.account(&store, draft.account_id)?;
        let out = outgoing(&draft)?;
        let old = match draft.replaces {
            Some(id) => store.message(id)?,
            None => None,
        };
        // Moved to another account: save it there, then remove the old one.
        let same = old.as_ref().is_some_and(|o| o.account_id == acc.id);
        let saved = engine::save_draft(&store, &acc, &out, if same { old.as_ref() } else { None })?;
        if let (Some(o), false) = (&old, same) {
            if let Ok(oa) = self.account(&store, o.account_id) {
                let _ = engine::delete_draft(&store, &oa, o);
            }
        }
        let m = store.copy_in_role(acc.id, &saved, FolderRole::Drafts)?.ok_or_else(|| fail("the draft was saved, but didn't sync back: pull to refresh Drafts"))?;
        Ok(m.id)
    }

    /// Delete a saved draft from the server (the composer was discarded).
    pub fn delete_draft(&self, message_id: i64) -> Result<()> {
        let store = self.worker()?;
        let Some(m) = store.message(message_id)? else { return Ok(()) };
        let acc = self.account(&store, m.account_id)?;
        Ok(engine::delete_draft(&store, &acc, &m)?)
    }

    /// Send the message, file it in Sent, and remove the draft it came
    /// from. Blocks on the network.
    pub fn send(&self, draft: Draft) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, draft.account_id)?;
        let out = outgoing(&draft)?;
        if out.to.is_empty() && out.cc.is_empty() && out.bcc.is_empty() {
            return Err(fail("add someone to send it to"));
        }
        engine::send(&store, &acc, &out)?;
        if let Some(id) = draft.replaces {
            if let Some(old) = store.message(id)? {
                let oa = self.account(&store, old.account_id)?;
                engine::delete_draft(&store, &oa, &old).map_err(|e| fail(&format!("Sent, but the draft couldn't be removed: {e:#}")))?;
            }
        }
        Ok(())
    }

    /// People you've written to or heard from whose name or address starts
    /// with `prefix`, best first.
    pub fn suggest_contacts(&self, prefix: String) -> Result<Vec<ContactSuggestion>> {
        let prefix = prefix.trim();
        if prefix.chars().count() < 2 {
            return Ok(vec![]);
        }
        Ok(self.store()?.suggest_contacts(prefix, 8)?.into_iter().map(|c| ContactSuggestion { name: c.name, email: c.email }).collect())
    }
}
