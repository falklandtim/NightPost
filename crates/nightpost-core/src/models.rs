//! Plain data types shared between the sync engine, the store and the UI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An account's row id in the local database (not shared between machines).
pub type AccountId = i64;
/// A folder's row id in the local database.
pub type FolderId = i64;
/// A message's row id in the local database. One email in two Gmail
/// labels is two messages here, one per folder.
pub type MessageId = i64;

/// How a connection to a mail server is encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Security {
    /// Implicit TLS (IMAPS 993 / SMTPS 465).
    Tls,
    /// Plain connection upgraded with STARTTLS (IMAP 143 / SMTP 587).
    StartTls,
    /// No encryption. Only for local test servers.
    None,
}

/// How an account signs in to its IMAP and SMTP servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Auth {
    /// A password (for iCloud and Gmail, an app-specific password), kept in the keychain.
    #[default]
    Password,
    /// "Sign in with Google" (OAuth 2.0): a refresh token in the keychain,
    /// exchanged for short-lived access tokens (XOAUTH2).
    Google,
}

impl Auth {
    /// How it's stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Auth::Password => "password",
            Auth::Google => "google",
        }
    }
    /// The inverse of [`Auth::as_str`]; anything unknown is a password.
    pub fn parse(s: &str) -> Self {
        match s {
            "google" => Auth::Google,
            _ => Auth::Password,
        }
    }
}

impl Security {
    /// How it's stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Security::Tls => "tls",
            Security::StartTls => "starttls",
            Security::None => "none",
        }
    }
    /// The inverse of [`Security::as_str`]; anything unknown is TLS, the safe choice.
    pub fn parse(s: &str) -> Self {
        match s {
            "starttls" => Security::StartTls,
            "none" => Security::None,
            _ => Security::Tls,
        }
    }
    /// The name shown in account settings.
    pub fn label(self) -> &'static str {
        match self {
            Security::Tls => "SSL/TLS",
            Security::StartTls => "STARTTLS",
            Security::None => "None",
        }
    }
}

/// Where an IMAP or SMTP server is and how to reach it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

/// A mail account. Its password or Google sign-in is in the system
/// keychain ([`crate::credentials`]), never here or in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    /// The name on outgoing mail ("Tim Morrison").
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
    /// How it signs in. Missing in older saved data, so it defaults to a password.
    #[serde(default)]
    pub auth: Auth,
}

impl Account {
    /// Gmail organises mail with labels (a message can be in several
    /// "folders" at once); other servers use plain folders.
    pub fn is_gmail(&self) -> bool {
        let h = self.imap.host.to_ascii_lowercase();
        h.ends_with("gmail.com") || h.ends_with("googlemail.com")
    }
}

/// A named signature, in plain text or HTML. Each account can pick one as its default.
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
    /// How it's stored in the database.
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
    /// The inverse of [`FolderRole::as_str`]; anything unknown is `Other`.
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
    /// The name shown in the sidebar and menus.
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
    /// An emoji for the role, for text-only places such as the Move picker.
    /// The sidebar draws its own line icons.
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
    pub const UNIFIED: [FolderRole; 7] = [
        FolderRole::Inbox,
        FolderRole::Starred,
        FolderRole::Drafts,
        FolderRole::Sent,
        FolderRole::Archive,
        FolderRole::Spam,
        FolderRole::Trash,
    ];

    /// Guess a role from IMAP SPECIAL-USE attributes and/or the folder name.
    pub fn detect(name: &str, attributes: &[String]) -> Self {
        Self::from_attributes(attributes).unwrap_or_else(|| Self::from_name(name))
    }

    /// The role a SPECIAL-USE attribute (`\Trash`, `\Sent`, …) gives.
    pub fn from_attributes(attributes: &[String]) -> Option<Self> {
        attributes.iter().find_map(|a| match a.to_ascii_lowercase().as_str() {
            "\\sent" => Some(FolderRole::Sent),
            "\\drafts" => Some(FolderRole::Drafts),
            "\\trash" => Some(FolderRole::Trash),
            "\\junk" => Some(FolderRole::Spam),
            "\\archive" => Some(FolderRole::Archive),
            "\\all" => Some(FolderRole::All),
            "\\flagged" => Some(FolderRole::Starred),
            _ => None,
        })
    }

    /// A role guessed from the folder name alone.
    pub fn from_name(name: &str) -> Self {
        let leaf = folder_leaf(name).to_ascii_lowercase();
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

/// The levels of a folder path. Most servers (Gmail, iCloud, Fastmail)
/// separate levels with `/`; servers that keep everything under `INBOX.`
/// use dots. Elsewhere a dot is part of the name (`timson.co`, `Unroll.me`).
pub fn folder_segments(path: &str) -> Vec<&str> {
    if path.contains('/') {
        path.split('/').collect()
    } else if path.len() > 6 && path[..6].eq_ignore_ascii_case("inbox.") {
        path.split('.').collect()
    } else {
        vec![path]
    }
}

/// The last level of a folder path: what the sidebar shows.
pub fn folder_leaf(path: &str) -> &str {
    folder_segments(path).last().copied().unwrap_or(path)
}

/// The path of a folder's parent, if it's nested.
pub fn folder_parent(path: &str) -> Option<String> {
    let segs = folder_segments(path);
    (segs.len() > 1).then(|| segs[..segs.len() - 1].join(if path.contains('/') { "/" } else { "." }))
}

/// A folder (or Gmail label) on an account's server, as last synced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub account_id: AccountId,
    /// Full IMAP path, e.g. `[Gmail]/Sent Mail`.
    pub path: String,
    /// Human friendly name (last path segment).
    pub name: String,
    pub role: FolderRole,
    /// The server's UIDVALIDITY: when it changes, every UID we hold for the
    /// folder is stale and the folder is downloaded again.
    pub uid_validity: u32,
    /// The highest UID downloaded so far; the next sync fetches above it.
    pub last_uid: u32,
    /// Unread messages, counted locally.
    pub unread: i64,
}

/// A person in an address field: a name (may be empty) and an address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Contact {
    pub name: String,
    pub email: String,
}

impl Contact {
    /// A contact from a name (empty if unknown) and an address.
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
    /// The name if known, otherwise the whole address.
    pub fn display(&self) -> String {
        if self.name.trim().is_empty() {
            self.email.clone()
        } else {
            self.name.trim().to_string()
        }
    }
    /// As written in a To/Cc header: `"Name" <address>`, or the bare address.
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
    /// A list as one header value, comma separated.
    pub fn join(list: &[Contact]) -> String {
        list.iter().map(|c| c.to_header()).collect::<Vec<_>>().join(", ")
    }
}

/// A received attachment's name, type and size. Sync keeps only these; the
/// file is fetched from the server when first opened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub filename: String,
    pub mime_type: String,
    /// In bytes.
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
    /// Inbox category guessed from the headers ([`crate::category`]).
    pub category: u8,
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
    /// When it last came back from snooze, if it has.
    pub woken: Option<DateTime<Utc>>,
    /// Inbox category of the conversation ([`crate::category`]).
    pub category: u8,
}

impl ThreadSummary {
    /// The date the list sorts and shows it by: a thread back from snooze
    /// counts as arriving when it woke.
    pub fn list_date(&self) -> DateTime<Utc> {
        self.woken.map_or(self.last_date, |w| w.max(self.last_date))
    }
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
    fn folder_paths() {
        assert_eq!(folder_leaf("timson.co"), "timson.co");
        assert_eq!(folder_leaf("Unroll.me/Unsubscribed"), "Unsubscribed");
        assert_eq!(folder_parent("Home/6 Ferry Street/The Wall"), Some("Home/6 Ferry Street".to_string()));
        assert_eq!(folder_parent("timson.co"), None);
        assert_eq!(folder_parent("INBOX.Receipts.2026"), Some("INBOX.Receipts".to_string()));
        assert_eq!(folder_leaf("INBOX.Receipts"), "Receipts");
        assert_eq!(FolderRole::from_name("INBOX.Sent"), FolderRole::Sent);
        assert_eq!(FolderRole::from_name("[Gmail]/Sent Mail"), FolderRole::Sent);
    }

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
        assert_eq!(FolderRole::from_attributes(&["\\HasNoChildren".into()]), None);
    }
}
