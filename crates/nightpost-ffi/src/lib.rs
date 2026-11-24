//! The NightPost core for the iPhone app.
//!
//! UniFFI turns this file into Swift: [`Mail`] becomes a Swift class, the
//! records become structs, and [`MailError`] becomes a thrown error. Only
//! plain values cross: account and thread ids, strings, and dates as Unix
//! seconds.
//!
//! Design:
//! - The core does the work, exactly as on the desktop: the same store, the
//!   same engine functions, the same "NightPost Sync" state, so the phone and
//!   the computers agree on snoozes and pins.
//! - Everything here blocks: reads on the database, actions and sync on the
//!   network. Swift calls them from a background task, never the main
//!   thread.
//! - Actions change the local store first and then the server, as the
//!   desktop's do, so a list reloaded straight after shows the change.
//! - Network work opens its own database connection, so a long sync never
//!   holds up the lists.

use nightpost_core::presets::Provider;
use nightpost_core::{credentials, demo, engine, snooze_sync, Account, Auth, FolderRole, MailboxView, Store, ThreadSummary};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

uniffi::setup_scaffolding!();

mod compose;
mod folders;
mod google;
mod organise;
mod reader;
pub use compose::{ComposeKind, ContactSuggestion, Draft};
pub use folders::{FolderInfo, FolderKind};
pub use google::GoogleSignIn;
pub use organise::{Filing, InboxTab};
pub use reader::{InlineImage, MessagePage};

/// What went wrong, in words to show the user.
#[derive(Debug, uniffi::Error)]
pub enum MailError {
    Failed { message: String },
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailError::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for MailError {}

impl From<anyhow::Error> for MailError {
    fn from(e: anyhow::Error) -> Self {
        MailError::Failed { message: format!("{e:#}") }
    }
}

type Result<T> = std::result::Result<T, MailError>;

fn fail(message: &str) -> MailError {
    MailError::Failed { message: message.to_string() }
}

/// An account, for the account list and the colour tag on each row.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AccountInfo {
    pub id: i64,
    pub name: String,
    pub email: String,
    /// Hex colour, e.g. `#4a90d9`.
    pub color: String,
    /// Demo accounts never touch the network.
    pub demo: bool,
}

/// Whose mail servers an account uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MailProvider {
    /// Guess from the address (gmail.com, icloud.com …). Your own domain
    /// can't be guessed: pick its provider.
    Detect,
    /// Gmail, and Google Workspace on your own domain.
    Google,
    ICloud,
    Outlook,
    Yahoo,
    Fastmail,
}

impl MailProvider {
    fn resolve(self, email: &str) -> Provider {
        match self {
            MailProvider::Detect => Provider::detect(email),
            MailProvider::Google => Provider::Gmail,
            MailProvider::ICloud => Provider::ICloud,
            MailProvider::Outlook => Provider::Outlook,
            MailProvider::Yahoo => Provider::Yahoo,
            MailProvider::Fastmail => Provider::Fastmail,
        }
    }
}

/// Which list to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Mailbox {
    Inbox,
    Starred,
    Drafts,
    Sent,
    Archive,
    Trash,
    /// Snoozed conversations, soonest to wake first.
    Snoozed,
}

/// One row in a list.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ThreadRow {
    pub account_id: i64,
    /// With `account_id`, names the conversation in every other call.
    pub thread_key: String,
    pub subject: String,
    /// Who's in the conversation, as the list shows them ("Ann, Bob").
    pub people: String,
    /// The latest sender's address, for their picture.
    pub latest_email: String,
    pub snippet: String,
    /// When it sorts: the latest message, or when it came back from snooze.
    pub date: i64,
    pub count: i64,
    pub unread: bool,
    pub starred: bool,
    pub pinned: bool,
    pub has_attachments: bool,
    /// When a snoozed conversation comes back.
    pub snoozed_until: Option<i64>,
}

/// An attachment's name and size; the file itself is fetched when opened.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AttachmentInfo {
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

/// One email in a conversation.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MessageInfo {
    pub id: i64,
    pub from_name: String,
    pub from_email: String,
    /// Everyone it went to, To then Cc.
    pub recipients: Vec<String>,
    pub date: i64,
    pub subject: String,
    /// The plain-text body. HTML email comes later, through a sandboxed
    /// WKWebView, as on the Mac.
    pub body_text: String,
    pub has_html: bool,
    pub attachments: Vec<AttachmentInfo>,
    pub unread: bool,
    pub starred: bool,
}

/// How a sync went for one account.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SyncResult {
    pub email: String,
    pub new_messages: u32,
    /// Why it failed, or `None` when it worked.
    pub error: Option<String>,
}

/// How far a running sync has got: which account, and how many of its
/// folders are done, for the caption by the spinner.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SyncStatus {
    /// The account being synced.
    pub email: String,
    /// The folder being synced now.
    pub folder: String,
    /// Folders finished so far in this account.
    pub done: u32,
    /// Folders in this account.
    pub total: u32,
}

/// The mail on this phone: one database, every account in it.
#[derive(uniffi::Object)]
pub struct Mail {
    /// rusqlite connections can't be shared between threads, so calls take
    /// turns on this one; network work opens its own (see [`Mail::worker`]).
    store: Mutex<Store>,
    /// Where the running sync is; `None` between syncs. Its own lock, so
    /// the app can ask while the sync holds everything else.
    progress: Mutex<Option<SyncStatus>>,
}

impl Mail {
    fn set_progress(&self, p: Option<SyncStatus>) {
        // A poisoned lock only means a panic elsewhere; the value is fine.
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = p;
    }

    fn store(&self) -> Result<std::sync::MutexGuard<'_, Store>> {
        // A panic mid-call poisons the lock; the database itself is still
        // consistent (SQLite transactions), so carry on with it.
        Ok(self.store.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// A separate connection for work that waits on the network.
    fn worker(&self) -> Result<Store> {
        Ok(self.store()?.reopen()?)
    }

    fn account(&self, store: &Store, id: i64) -> Result<Account> {
        store.account(id)?.ok_or_else(|| fail("that account no longer exists"))
    }

    fn thread_exists(&self, store: &Store, account_id: i64, thread_key: &str) -> Result<()> {
        if store.thread_messages(account_id, thread_key)?.is_empty() {
            return Err(fail("that conversation no longer exists"));
        }
        Ok(())
    }
}

#[uniffi::export]
impl Mail {
    /// Open (or create) the database in `data_dir`, keeping downloads in
    /// `cache_dir`: the app's Application Support and Caches folders.
    #[uniffi::constructor]
    pub fn open(data_dir: String, cache_dir: String) -> Result<Arc<Self>> {
        let data = PathBuf::from(data_dir);
        // Opening twice in one run (e.g. after a sign-out) is fine as long
        // as it's the same folder.
        if nightpost_core::use_folders(data.clone(), PathBuf::from(cache_dir)).is_err() && nightpost_core::data_dir() != data {
            return Err(fail("NightPost's folders can't change while it's running"));
        }
        let store = Store::open_default()?;
        Ok(Arc::new(Self { store: Mutex::new(store), progress: Mutex::new(None) }))
    }

    /// A database at `path` holding only the demo accounts: for SwiftUI
    /// previews and tests. Never touches the network.
    #[uniffi::constructor]
    pub fn open_demo(path: String) -> Result<Arc<Self>> {
        let store = Store::open(std::path::Path::new(&path))?;
        demo::seed(&store)?;
        Ok(Arc::new(Self { store: Mutex::new(store), progress: Mutex::new(None) }))
    }

    /// Every account, in the order they were added.
    pub fn accounts(&self) -> Result<Vec<AccountInfo>> {
        Ok(self.store()?.accounts()?.into_iter().map(account_info).collect())
    }

    /// Add an account that signs in with a password: for iCloud, Gmail and
    /// Google Workspace that's an app-specific password. `provider` says
    /// whose servers to use; `Detect` works for the big providers' own
    /// domains, not your own (pick `Google` for Workspace). Checks the
    /// password with the server first, keeps it in the keychain, and returns
    /// the new account; call [`Mail::sync`] next. Blocks on the network.
    pub fn add_password_account(&self, name: String, email: String, password: String, provider: MailProvider) -> Result<AccountInfo> {
        let email = email.trim().to_string();
        if !email.contains('@') {
            return Err(fail("that isn't an email address"));
        }
        let provider = provider.resolve(&email);
        let (Some(imap), Some(smtp)) = (provider.imap(), provider.smtp()) else {
            return Err(fail("NightPost can't tell who runs this address's mail: pick the provider (Google for Google Workspace)"));
        };
        let worker = self.worker()?;
        if worker.account_by_email(&email)?.is_some() {
            return Err(fail("that account is already here"));
        }
        let name = if name.trim().is_empty() { email.split('@').next().unwrap_or_default().to_string() } else { name.trim().to_string() };
        let mut acc = Account {
            id: 0,
            display_name: name,
            email: email.clone(),
            username: email.clone(),
            imap,
            smtp,
            color: provider.color().into(),
            signature: String::new(),
            demo: false,
            auth: Auth::Password,
        };
        engine::test_login(&acc, &password)?;
        credentials::set_password(&email, &password)?;
        acc.id = worker.add_account(&acc)?;
        Ok(account_info(acc))
    }

    /// Remove an account and its mail from this phone (the server keeps it)
    /// and forget its password or Google sign-in.
    pub fn remove_account(&self, account_id: i64) -> Result<()> {
        let store = self.store()?;
        let acc = self.account(&store, account_id)?;
        store.remove_account(account_id)?;
        if !acc.demo {
            // Also drops a Google refresh token, so a removed account can't
            // keep reading mail.
            let _ = credentials::delete_password(&acc.email);
        }
        Ok(())
    }

    /// The conversations in a list across every account, newest first with
    /// pinned ones on top. `query` uses the desktop's search words
    /// (`from:`, `is:unread` …); empty shows everything. Snoozed
    /// conversations stay out of the Inbox until they wake.
    pub fn threads(&self, mailbox: Mailbox, query: String, limit: u32) -> Result<Vec<ThreadRow>> {
        let store = self.store()?;
        let role = match mailbox {
            Mailbox::Inbox => FolderRole::Inbox,
            Mailbox::Starred => FolderRole::Starred,
            Mailbox::Drafts => FolderRole::Drafts,
            Mailbox::Sent => FolderRole::Sent,
            Mailbox::Archive => FolderRole::Archive,
            Mailbox::Trash => FolderRole::Trash,
            Mailbox::Snoozed => {
                let snoozed = store.snoozed_threads()?;
                return Ok(snoozed.into_iter().take(limit as usize).map(|(t, until)| thread_row(t, Some(until.timestamp()))).collect());
            }
        };
        Ok(store.threads(&MailboxView::Unified(role), &query, limit as usize)?.into_iter().map(|t| thread_row(t, None)).collect())
    }

    /// The emails in a conversation, oldest first.
    pub fn messages(&self, account_id: i64, thread_key: String) -> Result<Vec<MessageInfo>> {
        let store = self.store()?;
        Ok(store.thread_messages(account_id, &thread_key)?.into_iter().map(message_info).collect())
    }

    /// Fetch new mail and bring snoozes, pins and tabs into line with the
    /// server, for one account or (with `None`) all of them. Wakes snoozes
    /// that are due. One account failing doesn't stop the others. Blocks on
    /// the network.
    pub fn sync(&self, account_id: Option<i64>) -> Result<Vec<SyncResult>> {
        let store = self.worker()?;
        let accounts: Vec<Account> = match account_id {
            Some(id) => vec![self.account(&store, id)?],
            None => store.accounts()?,
        };
        let _ = snooze_sync::wake_due(&store);
        let results = accounts
            .into_iter()
            .map(|a| {
                let report = |p: engine::SyncProgress| {
                    if let engine::SyncProgress::Folder { name, done, total, .. } = p {
                        let n = |x: usize| x.try_into().unwrap_or(u32::MAX);
                        self.set_progress(Some(SyncStatus { email: a.email.clone(), folder: name, done: n(done), total: n(total) }));
                    }
                };
                match engine::sync_account(&store, &a, report) {
                    Ok(n) => SyncResult { email: a.email, new_messages: n.try_into().unwrap_or(u32::MAX), error: None },
                    Err(e) => SyncResult { email: a.email, new_messages: 0, error: Some(format!("{e:#}")) },
                }
            })
            .collect();
        self.set_progress(None);
        Ok(results)
    }

    /// How far the running sync has got, or `None` if none is running.
    /// Doesn't wait for the sync: ask as often as you like.
    pub fn sync_progress(&self) -> Option<SyncStatus> {
        self.progress.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Mark a conversation read or unread, here and on the server.
    pub fn set_read(&self, account_id: i64, thread_key: String, read: bool) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        Ok(engine::set_thread_unread(&store, &acc, &thread_key, !read)?)
    }

    /// Star or unstar a conversation, here and on the server.
    pub fn set_starred(&self, account_id: i64, thread_key: String, starred: bool) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        Ok(engine::set_thread_starred(&store, &acc, &thread_key, starred)?)
    }

    /// Pin a conversation to the top of the list, or unpin it, on every
    /// device (through the "NightPost Sync" state).
    pub fn set_pinned(&self, account_id: i64, thread_key: String, pinned: bool) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        self.thread_exists(&store, account_id, &thread_key)?;
        if pinned {
            store.pin(account_id, &thread_key)?;
        } else {
            store.unpin(account_id, &thread_key)?;
        }
        if !acc.demo {
            snooze_sync::push_pins(&store, &acc)?;
        }
        Ok(())
    }

    /// Archive a conversation (on Gmail, take it out of the Inbox).
    pub fn archive(&self, account_id: i64, thread_key: String) -> Result<()> {
        self.move_to(account_id, &thread_key, FolderRole::Archive)
    }

    /// Move a conversation to the Trash. Nothing is deleted outright.
    pub fn trash(&self, account_id: i64, thread_key: String) -> Result<()> {
        self.move_to(account_id, &thread_key, FolderRole::Trash)
    }

    /// Report a conversation as spam: move it to the account's Spam (or
    /// Junk) folder, as the desktop's Spam swipe does.
    pub fn spam(&self, account_id: i64, thread_key: String) -> Result<()> {
        self.move_to(account_id, &thread_key, FolderRole::Spam)
    }

    /// Hide a conversation until `until` (Unix seconds), on every device.
    pub fn snooze(&self, account_id: i64, thread_key: String, until: i64) -> Result<()> {
        let until = chrono::DateTime::from_timestamp(until, 0).ok_or_else(|| fail("that isn't a valid time"))?;
        if until <= chrono::Utc::now() {
            return Err(fail("pick a time in the future"));
        }
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        self.thread_exists(&store, account_id, &thread_key)?;
        Ok(snooze_sync::snooze_thread(&store, &acc, &thread_key, until)?)
    }

    /// Cancel a snooze and put the conversation back in the Inbox now.
    pub fn unsnooze(&self, account_id: i64, thread_key: String) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        Ok(snooze_sync::unsnooze_thread(&store, &acc, &thread_key)?)
    }
}

impl Mail {
    fn move_to(&self, account_id: i64, thread_key: &str, role: FolderRole) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        self.thread_exists(&store, account_id, thread_key)?;
        Ok(engine::move_thread(&store, &acc, thread_key, role)?)
    }
}

fn account_info(a: Account) -> AccountInfo {
    AccountInfo { id: a.id, name: a.display_name, email: a.email, color: a.color, demo: a.demo }
}

fn thread_row(t: ThreadSummary, snoozed_until: Option<i64>) -> ThreadRow {
    let names: Vec<&str> = t.participants.iter().map(|c| if c.name.is_empty() { c.email.as_str() } else { c.name.as_str() }).collect();
    ThreadRow {
        account_id: t.account_id,
        date: t.list_date().timestamp(),
        latest_email: t.participants.last().map(|c| c.email.clone()).unwrap_or_default(),
        people: names.join(", "),
        thread_key: t.thread_key,
        subject: t.subject,
        snippet: t.snippet,
        count: t.count,
        unread: t.unread,
        starred: t.starred,
        pinned: t.pinned,
        has_attachments: t.has_attachments,
        snoozed_until,
    }
}

fn message_info(m: nightpost_core::Message) -> MessageInfo {
    MessageInfo {
        id: m.id,
        from_name: m.from.name,
        from_email: m.from.email,
        recipients: m.to.iter().chain(&m.cc).map(|c| if c.name.is_empty() { c.email.clone() } else { format!("{} <{}>", c.name, c.email) }).collect(),
        date: m.date.timestamp(),
        subject: m.subject,
        body_text: m.body_text,
        has_html: m.body_html.is_some(),
        attachments: m.attachments.into_iter().map(|a| AttachmentInfo { filename: a.filename, mime_type: a.mime_type, size: a.size as u64 }).collect(),
        unread: m.unread,
        starred: m.starred,
    }
}

#[cfg(test)]
mod tests;
