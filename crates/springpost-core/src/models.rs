//! Plain data types shared between the sync engine, the store and the UI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type AccountId = i64;
pub type FolderId = i64;
pub type MessageId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Security {
    /// Implicit TLS (IMAPS 993 / SMTPS 465).
    Tls,
    /// Plain connection upgraded with STARTTLS (IMAP 143 / SMTP 587).
    StartTls,
    /// No encryption. Only for local test servers.
    None,
}

impl Security {
    pub fn as_str(self) -> &'static str {
        match self {
            Security::Tls => "tls",
            Security::StartTls => "starttls",
            Security::None => "none",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "starttls" => Security::StartTls,
            "none" => Security::None,
            _ => Security::Tls,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Security::Tls => "SSL/TLS",
            Security::StartTls => "STARTTLS",
            Security::None => "None",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub display_name: String,
    pub email: String,
    /// Login username (usually the email address).
    pub username: String,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    /// Hex colour used to tag the account in unified views.
    pub color: String,
    /// The text of this account's default signature (see [`Signature`]);
    /// empty when it has none. Read-only: change it via the signatures API.
    pub signature: String,
    /// Demo accounts are seeded locally and never touch the network.
    pub demo: bool,
}

impl Account {
    /// Gmail organises mail with labels (a message can be in several
    /// "folders" at once); other servers use plain folders.
    pub fn is_gmail(&self) -> bool {
        let h = self.imap.host.to_ascii_lowercase();
        h.ends_with("gmail.com") || h.ends_with("googlemail.com")
    }
}

/// A named, plain-text signature. Each account can pick one as its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Signature {
    pub id: i64,
    pub title: String,
    /// Plain text, or HTML source when `html` is set.
    pub body: String,
    #[serde(default)]
    pub html: bool,
}

impl Signature {
    /// How the signature reads as plain text (what the composer shows and
    /// what goes in the text/plain part).
    pub fn text(&self) -> String {
        if self.html {
            crate::parse::signature_text(&self.body)
        } else {
            self.body.trim_end().to_string()
        }
    }
}

/// The role a folder plays, independent of its server-side name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum FolderRole {
    Inbox,
    Starred,
    Drafts,
    Sent,
    Archive,
    Spam,
    Trash,
    All,
    Other,
}

impl FolderRole {
    pub fn as_str(self) -> &'static str {
        match self {
            FolderRole::Inbox => "inbox",
            FolderRole::Starred => "starred",
            FolderRole::Drafts => "drafts",
            FolderRole::Sent => "sent",
            FolderRole::Archive => "archive",
            FolderRole::Spam => "spam",
            FolderRole::Trash => "trash",
            FolderRole::All => "all",
            FolderRole::Other => "other",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "inbox" => FolderRole::Inbox,
            "starred" => FolderRole::Starred,
            "drafts" => FolderRole::Drafts,
            "sent" => FolderRole::Sent,
            "archive" => FolderRole::Archive,
            "spam" => FolderRole::Spam,
            "trash" => FolderRole::Trash,
            "all" => FolderRole::All,
            _ => FolderRole::Other,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            FolderRole::Inbox => "Inbox",
            FolderRole::Starred => "Starred",
            FolderRole::Drafts => "Drafts",
            FolderRole::Sent => "Sent",
            FolderRole::Archive => "Archive",
            FolderRole::Spam => "Spam",
            FolderRole::Trash => "Trash",
            FolderRole::All => "All Mail",
            FolderRole::Other => "Folder",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            FolderRole::Inbox => "📥",
            FolderRole::Starred => "⭐",
            FolderRole::Drafts => "📝",
            FolderRole::Sent => "📤",
            FolderRole::Archive => "🗄",
            FolderRole::Spam => "⚠",
            FolderRole::Trash => "🗑",
            FolderRole::All => "📚",
            FolderRole::Other => "📁",
        }
    }
    /// Roles shown in the unified "all accounts" section of the sidebar.
    pub const UNIFIED: [FolderRole; 6] = [
        FolderRole::Inbox,
        FolderRole::Starred,
        FolderRole::Drafts,
        FolderRole::Sent,
        FolderRole::Archive,
        FolderRole::Trash,
    ];

    /// Guess a role from IMAP SPECIAL-USE attributes and/or the folder name.
    pub fn detect(name: &str, attributes: &[String]) -> Self {
        for a in attributes {
            match a.to_ascii_lowercase().as_str() {
                "\\sent" => return FolderRole::Sent,
                "\\drafts" => return FolderRole::Drafts,
                "\\trash" => return FolderRole::Trash,
                "\\junk" => return FolderRole::Spam,
                "\\archive" => return FolderRole::Archive,
                "\\all" => return FolderRole::All,
                "\\flagged" => return FolderRole::Starred,
                _ => {}
            }
        }
        let leaf = name.rsplit(['/', '.']).next().unwrap_or(name).to_ascii_lowercase();
        match leaf.as_str() {
            "inbox" => FolderRole::Inbox,
            "sent" | "sent mail" | "sent items" | "sent messages" => FolderRole::Sent,
            "drafts" | "draft" => FolderRole::Drafts,
            "trash" | "deleted" | "deleted items" | "deleted messages" | "bin" => FolderRole::Trash,
            "spam" | "junk" | "junk e-mail" | "junk email" | "bulk mail" => FolderRole::Spam,
            "archive" | "archives" => FolderRole::Archive,
            "all mail" => FolderRole::All,
            "starred" | "flagged" => FolderRole::Starred,
            _ if name.eq_ignore_ascii_case("inbox") => FolderRole::Inbox,
            _ => FolderRole::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub account_id: AccountId,
    /// Full IMAP path, e.g. `[Gmail]/Sent Mail`.
    pub path: String,
    /// Human friendly name (last path segment).
    pub name: String,
    pub role: FolderRole,
    pub uid_validity: u32,
    pub last_uid: u32,
    pub unread: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Contact {
    pub name: String,
    pub email: String,
}

impl Contact {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self { name: name.into(), email: email.into() }
    }
    /// The name if known, otherwise the local part of the address.
    pub fn short(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.split_whitespace().next().unwrap_or("").to_string();
        }
        self.email.split('@').next().unwrap_or(&self.email).to_string()
    }
    pub fn display(&self) -> String {
        if self.name.trim().is_empty() {
            self.email.clone()
        } else {
            self.name.trim().to_string()
        }
    }
    pub fn to_header(&self) -> String {
        if self.name.trim().is_empty() {
            self.email.clone()
        } else {
            format!("\"{}\" <{}>", self.name.replace('"', ""), self.email)
        }
    }
    /// Parse a comma separated list like `Ann <a@x.com>, b@y.com`.
    pub fn parse_list(input: &str) -> Vec<Contact> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_quotes = false;
        let mut in_angle = false;
        for ch in input.chars() {
            match ch {
                '"' => in_quotes = !in_quotes,
                '<' => in_angle = true,
                '>' => in_angle = false,
                ',' | ';' if !in_quotes && !in_angle => {
                    if let Some(c) = Self::parse_one(&cur) {
                        out.push(c);
                    }
                    cur.clear();
                    continue;
                }
                _ => {}
            }
            cur.push(ch);
        }
        if let Some(c) = Self::parse_one(&cur) {
            out.push(c);
        }
        out
    }
    fn parse_one(s: &str) -> Option<Contact> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let (Some(l), Some(r)) = (s.find('<'), s.rfind('>')) {
            if l < r {
                let email = s[l + 1..r].trim().to_string();
                let name = s[..l].trim().trim_matches('"').trim().to_string();
                return Some(Contact { name, email });
            }
        }
        Some(Contact { name: String::new(), email: s.trim_matches('"').to_string() })
    }
    pub fn join(list: &[Contact]) -> String {
        list.iter().map(|c| c.to_header()).collect::<Vec<_>>().join(", ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub filename: String,
    pub mime_type: String,
    pub size: usize,
}

/// A fully loaded message, as shown in the reading pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub account_id: AccountId,
    pub folder_id: FolderId,
    pub uid: u32,
    /// RFC 5322 Message-ID header (without angle brackets).
    pub message_id: String,
    pub thread_key: String,
    pub in_reply_to: String,
    pub references: Vec<String>,
    pub subject: String,
    pub from: Contact,
    pub to: Vec<Contact>,
    pub cc: Vec<Contact>,
    pub date: DateTime<Utc>,
    pub snippet: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub unread: bool,
    pub starred: bool,
}

/// One row in the thread list.
#[derive(Debug, Clone)]
pub struct ThreadSummary {
    pub account_id: AccountId,
    pub thread_key: String,
    pub subject: String,
    pub participants: Vec<Contact>,
    pub snippet: String,
    pub last_date: DateTime<Utc>,
    pub count: i64,
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
    /// Folder of the most recent message in the selected view.
    pub folder_id: FolderId,
    /// Pinned conversations are listed above everything else.
    pub pinned: bool,
}

/// What the thread list is currently showing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MailboxView {
    /// The same role across every account ("Unified Inbox").
    Unified(FolderRole),
    /// One concrete folder.
    Folder(FolderId),
    /// Every folder of every account (search results).
    Everything,
}

/// Strip `Re:`/`Fwd:` style prefixes so replies group with their originals.
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let stripped = ["re:", "fw:", "fwd:", "aw:", "sv:", "antw:"]
            .iter()
            .find(|p| lower.starts_with(**p))
            .map(|p| s[p.len()..].trim_start());
        match stripped {
            Some(rest) => s = rest,
            None => break,
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_address_lists() {
        let l = Contact::parse_list("\"Doe, Jane\" <jane@x.com>, bob@y.com; Al <al@z.org>");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].name, "Doe, Jane");
        assert_eq!(l[0].email, "jane@x.com");
        assert_eq!(l[1].email, "bob@y.com");
        assert_eq!(l[2].name, "Al");
    }

    #[test]
    fn normalizes_subjects() {
        assert_eq!(normalize_subject("Re: Fwd: RE: Hello"), "Hello");
        assert_eq!(normalize_subject("Hello"), "Hello");
    }

    #[test]
    fn detects_roles() {
        assert_eq!(FolderRole::detect("[Gmail]/Sent Mail", &[]), FolderRole::Sent);
        assert_eq!(FolderRole::detect("INBOX", &[]), FolderRole::Inbox);
        assert_eq!(FolderRole::detect("Stuff", &["\\Trash".into()]), FolderRole::Trash);
        assert_eq!(FolderRole::detect("Projects", &[]), FolderRole::Other);
    }
}
