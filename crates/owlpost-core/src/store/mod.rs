//! Local SQLite cache of accounts, folders, messages, snoozes, pins and
//! tab rules. Inbox tabs live in `categories.rs` and pins in `pins.rs`.
//!
//! The database runs in WAL mode so the desktop app, the background sync
//! worker and the MCP server process can all use it at the same time.

use crate::models::*;
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

mod categories;
mod pins;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS accounts (
    id            INTEGER PRIMARY KEY,
    display_name  TEXT NOT NULL,
    email         TEXT NOT NULL UNIQUE,
    username      TEXT NOT NULL,
    imap_host     TEXT NOT NULL,
    imap_port     INTEGER NOT NULL,
    imap_security TEXT NOT NULL,
    smtp_host     TEXT NOT NULL,
    smtp_port     INTEGER NOT NULL,
    smtp_security TEXT NOT NULL,
    color         TEXT NOT NULL,
    signature     TEXT NOT NULL DEFAULT '',
    demo          INTEGER NOT NULL DEFAULT 0,
    auth          TEXT NOT NULL DEFAULT 'password'
);
CREATE TABLE IF NOT EXISTS folders (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    path         TEXT NOT NULL,
    name         TEXT NOT NULL,
    role         TEXT NOT NULL,
    uid_validity INTEGER NOT NULL DEFAULT 0,
    last_uid     INTEGER NOT NULL DEFAULT 0,
    UNIQUE(account_id, path)
);
CREATE TABLE IF NOT EXISTS messages (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    folder_id    INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    uid          INTEGER NOT NULL,
    message_id   TEXT NOT NULL,
    thread_key   TEXT NOT NULL,
    in_reply_to  TEXT NOT NULL DEFAULT '',
    refs         TEXT NOT NULL DEFAULT '[]',
    subject      TEXT NOT NULL,
    from_name    TEXT NOT NULL,
    from_email   TEXT NOT NULL,
    to_json      TEXT NOT NULL,
    cc_json      TEXT NOT NULL,
    date         INTEGER NOT NULL,
    snippet      TEXT NOT NULL,
    body_text    TEXT NOT NULL,
    body_html    TEXT,
    attachments  TEXT NOT NULL DEFAULT '[]',
    unread       INTEGER NOT NULL,
    starred      INTEGER NOT NULL,
    category     INTEGER,
    UNIQUE(folder_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(account_id, thread_key);
CREATE INDEX IF NOT EXISTS idx_messages_folder_date ON messages(folder_id, date DESC);
CREATE INDEX IF NOT EXISTS idx_messages_msgid ON messages(account_id, message_id);
CREATE TABLE IF NOT EXISTS snoozes (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    thread_key  TEXT NOT NULL,
    until       INTEGER NOT NULL,
    PRIMARY KEY(account_id, thread_key)
);
-- When a snoozed thread last woke: it sorts as if it arrived then.
CREATE TABLE IF NOT EXISTS woken (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    thread_key  TEXT NOT NULL,
    at          INTEGER NOT NULL,
    PRIMARY KEY(account_id, thread_key)
);
-- "Always put mail from this address in this inbox category"; synced
-- through the snooze state (removed = a tombstone).
CREATE TABLE IF NOT EXISTS sender_rules (
    email     TEXT PRIMARY KEY,
    category  INTEGER NOT NULL,
    updated   INTEGER NOT NULL,
    removed   INTEGER NOT NULL DEFAULT 0,
    tab       TEXT
);
-- Your own inbox tabs (Invoices…), synced like the rules. `num` is this
-- machine's category number for the tab; `key` is the same everywhere.
CREATE TABLE IF NOT EXISTS tabs (
    key      TEXT PRIMARY KEY,
    num      INTEGER NOT NULL UNIQUE,
    name     TEXT NOT NULL,
    created  INTEGER NOT NULL,
    updated  INTEGER NOT NULL,
    removed  INTEGER NOT NULL DEFAULT 0
);
-- "Put this conversation in this tab", one row per Message-ID in it, so
-- replies that arrive later stay with it.
CREATE TABLE IF NOT EXISTS thread_rules (
    message_id  TEXT PRIMARY KEY,
    category    INTEGER NOT NULL,
    tab         TEXT,
    updated     INTEGER NOT NULL,
    removed     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS pins (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    thread_key  TEXT NOT NULL,
    pinned_at   INTEGER NOT NULL,
    PRIMARY KEY(account_id, thread_key)
);
-- Each pin and unpin by Message-ID, synced through the server's state
-- (thread keys differ between machines). The newer change wins.
CREATE TABLE IF NOT EXISTS pin_marks (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    message_id  TEXT NOT NULL,
    updated     INTEGER NOT NULL,
    removed     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(account_id, message_id)
);
CREATE TABLE IF NOT EXISTS signatures (
    id      INTEGER PRIMARY KEY,
    title   TEXT NOT NULL,
    body    TEXT NOT NULL,
    html    INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS account_signatures (
    account_id    INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    signature_id  INTEGER NOT NULL REFERENCES signatures(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS outbox (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    payload     TEXT NOT NULL,
    created     INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_status (
    account_id   INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    last_ok      INTEGER,
    last_try     INTEGER,
    failures     INTEGER NOT NULL DEFAULT 0,
    error        TEXT,
    push         TEXT,
    push_detail  TEXT
);
CREATE TABLE IF NOT EXISTS activity (
    id          INTEGER PRIMARY KEY,
    at          INTEGER NOT NULL,
    account_id  INTEGER,
    level       TEXT NOT NULL,
    text        TEXT NOT NULL
);
"#;

/// One connection to OwlPost's database. Each thread and process opens its
/// own; WAL mode lets them read and write side by side.
pub struct Store {
    pub(crate) conn: Connection,
    path: PathBuf,
}

/// Default database location, e.g. `~/Library/Application Support/co.timson.OwlPost/owlpost.db`.
pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("OWLPOST_DB") {
        return PathBuf::from(p);
    }
    crate::data_dir().join("owlpost.db")
}

fn ts(dt: &DateTime<Utc>) -> i64 {
    dt.timestamp()
}
fn from_ts(t: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(t, 0).single().unwrap_or_else(Utc::now)
}

impl Store {
    /// Open (or create) the database at `path`, creating missing tables and
    /// running the column migrations below.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        let store = Self { conn, path: path.to_path_buf() };
        // Databases from before HTML signatures lack the column.
        let has_html: bool = store
            .conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('signatures') WHERE name='html'", [], |r| r.get::<_, i64>(0))?
            > 0;
        if !has_html {
            store.conn.execute_batch("ALTER TABLE signatures ADD COLUMN html INTEGER NOT NULL DEFAULT 0")?;
        }
        // …and before Google sign-in, the auth column.
        let has_auth: bool = store
            .conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('accounts') WHERE name='auth'", [], |r| r.get::<_, i64>(0))?
            > 0;
        if !has_auth {
            store.conn.execute_batch("ALTER TABLE accounts ADD COLUMN auth TEXT NOT NULL DEFAULT 'password'")?;
        }
        // …and before inbox categories (NULL: not sorted yet; sync fills it in).
        let has_category: bool = store
            .conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='category'", [], |r| r.get::<_, i64>(0))?
            > 0;
        if !has_category {
            store.conn.execute_batch("ALTER TABLE messages ADD COLUMN category INTEGER")?;
        }
        // …and before your own tabs.
        let has_tab: bool = store
            .conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('sender_rules') WHERE name='tab'", [], |r| r.get::<_, i64>(0))?
            > 0;
        if !has_tab {
            store.conn.execute_batch("ALTER TABLE sender_rules ADD COLUMN tab TEXT")?;
        }
        store.migrate_signatures()?;
        store.rebuild_snippets()?;
        Ok(store)
    }

    /// Rebuild every stored preview once after [`crate::parse::make_snippet`]
    /// improves (tracked in SQLite's `user_version`).
    fn rebuild_snippets(&self) -> Result<()> {
        let version: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version >= crate::parse::SNIPPET_VERSION {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut read = tx.prepare("SELECT id, body_text, snippet, body_html FROM messages")?;
            let mut write = tx.prepare("UPDATE messages SET snippet=?2 WHERE id=?1")?;
            let mut write_body = tx.prepare("UPDATE messages SET body_text=?2, snippet=?3 WHERE id=?1")?;
            let rows = read.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
            })?;
            for row in rows {
                let (id, body, old, html) = row?;
                // HTML-only mail whose text was drawn with table borders by
                // an older version: read it again.
                if let Some(html) = html.filter(|_| crate::parse::has_table_borders(&body)) {
                    let text = crate::parse::html_to_text(&html);
                    let snippet = crate::parse::make_snippet(&text);
                    write_body.execute(params![id, text, snippet])?;
                    continue;
                }
                let new = crate::parse::make_snippet(&body);
                if new != old {
                    write.execute(params![id, new])?;
                }
            }
        }
        tx.execute_batch(&format!("PRAGMA user_version = {}", crate::parse::SNIPPET_VERSION))?;
        tx.commit()?;
        Ok(())
    }

    /// Older databases kept one signature string per account; turn those
    /// into shared, named signatures (once — the old column is then cleared).
    fn migrate_signatures(&self) -> Result<()> {
        let legacy: Vec<(AccountId, String)> = {
            let mut st = self.conn.prepare("SELECT id, signature FROM accounts WHERE signature <> ''")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            v
        };
        for (id, body) in legacy {
            let sig = self.signature_for_text(&body)?;
            self.set_default_signature(id, Some(sig))?;
            self.conn.execute("UPDATE accounts SET signature='' WHERE id=?1", [id])?;
        }
        Ok(())
    }

    /// An existing signature with this exact text, or a new one.
    fn signature_for_text(&self, body: &str) -> Result<i64> {
        if let Some(id) = self.conn.query_row("SELECT id FROM signatures WHERE body=?1", [body], |r| r.get(0)).optional()? {
            return Ok(id);
        }
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM signatures", [], |r| r.get(0))?;
        let title = if n == 0 { "Signature".to_string() } else { format!("Signature {}", n + 1) };
        self.save_signature(&Signature { id: 0, title, body: body.to_string(), html: false })
    }

    // ----- signatures -------------------------------------------------------

    /// Every saved signature, oldest first.
    pub fn signatures(&self) -> Result<Vec<Signature>> {
        let mut st = self.conn.prepare("SELECT id, title, body, html FROM signatures ORDER BY id")?;
        let v = st
            .query_map([], |r| Ok(Signature { id: r.get(0)?, title: r.get(1)?, body: r.get(2)?, html: r.get(3)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// The signature an account uses by default.
    pub fn default_signature(&self, account_id: AccountId) -> Result<Option<Signature>> {
        let id: Option<i64> = self
            .conn
            .query_row("SELECT signature_id FROM account_signatures WHERE account_id=?1", [account_id], |r| r.get(0))
            .optional()?;
        Ok(id.and_then(|id| self.signatures().ok()?.into_iter().find(|s| s.id == id)))
    }

    /// Insert (id 0) or update a signature; returns its id.
    pub fn save_signature(&self, s: &Signature) -> Result<i64> {
        if s.id == 0 {
            self.conn.execute("INSERT INTO signatures (title, body, html) VALUES (?1, ?2, ?3)", params![s.title, s.body, s.html])?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn.execute("UPDATE signatures SET title=?2, body=?3, html=?4 WHERE id=?1", params![s.id, s.title, s.body, s.html])?;
            Ok(s.id)
        }
    }

    /// Delete a signature; accounts using it as their default lose it.
    pub fn delete_signature(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM signatures WHERE id=?1", [id])?;
        Ok(())
    }

    /// Account → default signature id.
    pub fn signature_defaults(&self) -> Result<HashMap<AccountId, i64>> {
        let mut st = self.conn.prepare("SELECT account_id, signature_id FROM account_signatures")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Set (or, with `None`, clear) an account's default signature.
    pub fn set_default_signature(&self, account_id: AccountId, signature: Option<i64>) -> Result<()> {
        match signature {
            Some(sig) => self.conn.execute(
                "INSERT INTO account_signatures (account_id, signature_id) VALUES (?1, ?2)
                 ON CONFLICT(account_id) DO UPDATE SET signature_id=excluded.signature_id",
                params![account_id, sig],
            )?,
            None => self.conn.execute("DELETE FROM account_signatures WHERE account_id=?1", [account_id])?,
        };
        Ok(())
    }

    /// Open the database at [`default_db_path`].
    pub fn open_default() -> Result<Self> {
        Self::open(&default_db_path())
    }

    /// Where this database file is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Open another connection to the same database (for worker threads).
    pub fn reopen(&self) -> Result<Self> {
        Self::open(&self.path)
    }

    // ----- accounts ---------------------------------------------------------

    /// Save a new account (its `id` is ignored) and return its id. A
    /// signature on it becomes a named signature, set as its default.
    pub fn add_account(&self, a: &Account) -> Result<AccountId> {
        self.conn.execute(
            "INSERT INTO accounts (display_name,email,username,imap_host,imap_port,imap_security,
                smtp_host,smtp_port,smtp_security,color,signature,demo,auth)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                a.display_name, a.email, a.username,
                a.imap.host, a.imap.port, a.imap.security.as_str(),
                a.smtp.host, a.smtp.port, a.smtp.security.as_str(),
                a.color, "", a.demo, a.auth.as_str()
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        // A signature on a new account becomes (or reuses) a named signature.
        if !a.signature.trim().is_empty() {
            let sig = self.signature_for_text(&a.signature)?;
            self.set_default_signature(id, Some(sig))?;
        }
        Ok(id)
    }

    /// Save changes to an account's name, servers, colour and sign-in.
    /// The email address and signature are not changed here.
    pub fn update_account(&self, a: &Account) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET display_name=?2, username=?3, imap_host=?4, imap_port=?5,
                imap_security=?6, smtp_host=?7, smtp_port=?8, smtp_security=?9, color=?10, auth=?11
             WHERE id=?1",
            params![
                a.id, a.display_name, a.username,
                a.imap.host, a.imap.port, a.imap.security.as_str(),
                a.smtp.host, a.smtp.port, a.smtp.security.as_str(),
                a.color, a.auth.as_str()
            ],
        )?;
        Ok(())
    }

    /// Delete an account and, through the foreign keys, its folders, mail,
    /// snoozes and pins. Its keychain entry is the caller's to remove.
    pub fn remove_account(&self, id: AccountId) -> Result<()> {
        self.conn.execute("DELETE FROM accounts WHERE id=?1", [id])?;
        Ok(())
    }

    /// Every account, in the order they were added.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut st = self.conn.prepare(
            "SELECT id,display_name,email,username,imap_host,imap_port,imap_security,
                    smtp_host,smtp_port,smtp_security,color,
                    '', demo, auth
             FROM accounts ORDER BY id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                display_name: r.get(1)?,
                email: r.get(2)?,
                username: r.get(3)?,
                imap: ServerConfig {
                    host: r.get(4)?,
                    port: r.get(5)?,
                    security: Security::parse(&r.get::<_, String>(6)?),
                },
                smtp: ServerConfig {
                    host: r.get(7)?,
                    port: r.get(8)?,
                    security: Security::parse(&r.get::<_, String>(9)?),
                },
                color: r.get(10)?,
                signature: r.get(11)?,
                demo: r.get(12)?,
                auth: Auth::parse(&r.get::<_, String>(13)?),
            })
        })?;
        let mut accounts: Vec<Account> = rows.collect::<rusqlite::Result<_>>()?;
        // `signature` is the default signature as text (HTML ones rendered).
        let sigs = self.signatures()?;
        for (acc, sig) in self.signature_defaults()? {
            if let (Some(a), Some(s)) = (accounts.iter_mut().find(|a| a.id == acc), sigs.iter().find(|s| s.id == sig)) {
                a.signature = s.text();
            }
        }
        Ok(accounts)
    }

    /// One account by id.
    pub fn account(&self, id: AccountId) -> Result<Option<Account>> {
        Ok(self.accounts()?.into_iter().find(|a| a.id == id))
    }

    /// One account by address (ignoring case).
    pub fn account_by_email(&self, email: &str) -> Result<Option<Account>> {
        Ok(self
            .accounts()?
            .into_iter()
            .find(|a| a.email.eq_ignore_ascii_case(email)))
    }

    // ----- folders ----------------------------------------------------------

    /// Add a folder from the server's list, or refresh its role and name;
    /// returns its id either way.
    pub fn upsert_folder(&self, account_id: AccountId, path: &str, role: FolderRole) -> Result<FolderId> {
        let name = crate::models::folder_leaf(path).to_string();
        let name = if path.eq_ignore_ascii_case("inbox") { "Inbox".to_string() } else { name };
        // The name is refreshed too: older versions split `timson.co` at the dot.
        self.conn.execute(
            "INSERT INTO folders (account_id,path,name,role) VALUES (?1,?2,?3,?4)
             ON CONFLICT(account_id,path) DO UPDATE SET role=excluded.role, name=excluded.name",
            params![account_id, path, name, role.as_str()],
        )?;
        let id = self.conn.query_row(
            "SELECT id FROM folders WHERE account_id=?1 AND path=?2",
            params![account_id, path],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// Forget an account's folders that the server no longer lists (with
    /// their cached mail). `keep` holds the paths the server sent.
    pub fn delete_folders_not_in(&self, account_id: AccountId, keep: &[String]) -> Result<()> {
        let existing = self.folders(account_id)?;
        for f in existing {
            if !keep.contains(&f.path) {
                self.conn.execute("DELETE FROM folders WHERE id=?1", [f.id])?;
            }
        }
        Ok(())
    }

    /// Forget a folder and the copies of mail stored in it.
    pub fn delete_folder(&self, id: FolderId) -> Result<()> {
        self.conn.execute("DELETE FROM messages WHERE folder_id=?1", [id])?;
        self.conn.execute("DELETE FROM folders WHERE id=?1", [id])?;
        Ok(())
    }

    /// Record how far a folder has been synced (see [`Folder::last_uid`]).
    pub fn set_folder_sync_state(&self, id: FolderId, uid_validity: u32, last_uid: u32) -> Result<()> {
        self.conn.execute(
            "UPDATE folders SET uid_validity=?2, last_uid=?3 WHERE id=?1",
            params![id, uid_validity, last_uid],
        )?;
        Ok(())
    }

    /// Wipe a folder's cached messages (used when UIDVALIDITY changes).
    pub fn clear_folder(&self, id: FolderId) -> Result<()> {
        self.conn.execute("DELETE FROM messages WHERE folder_id=?1", [id])?;
        self.conn.execute("UPDATE folders SET last_uid=0 WHERE id=?1", [id])?;
        Ok(())
    }

    fn folder_from_row(r: &Row) -> rusqlite::Result<Folder> {
        Ok(Folder {
            id: r.get(0)?,
            account_id: r.get(1)?,
            path: r.get(2)?,
            name: r.get(3)?,
            role: FolderRole::parse(&r.get::<_, String>(4)?),
            uid_validity: r.get(5)?,
            last_uid: r.get(6)?,
            unread: r.get(7)?,
        })
    }

    /// An account's folders with their unread counts: standard roles first
    /// (Inbox, Starred, Drafts…), then the rest by path.
    pub fn folders(&self, account_id: AccountId) -> Result<Vec<Folder>> {
        let mut st = self.conn.prepare(
            "SELECT f.id,f.account_id,f.path,f.name,f.role,f.uid_validity,f.last_uid,
                    (SELECT COUNT(*) FROM messages m WHERE m.folder_id=f.id AND m.unread=1)
             FROM folders f WHERE f.account_id=?1",
        )?;
        let mut v: Vec<Folder> = st
            .query_map([account_id], Self::folder_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        v.sort_by(|a, b| a.role.cmp(&b.role).then(a.path.to_lowercase().cmp(&b.path.to_lowercase())));
        Ok(v)
    }

    /// One folder by id, with its unread count.
    pub fn folder(&self, id: FolderId) -> Result<Option<Folder>> {
        Ok(self
            .conn
            .query_row(
                "SELECT f.id,f.account_id,f.path,f.name,f.role,f.uid_validity,f.last_uid,
                        (SELECT COUNT(*) FROM messages m WHERE m.folder_id=f.id AND m.unread=1)
                 FROM folders f WHERE f.id=?1",
                [id],
                Self::folder_from_row,
            )
            .optional()?)
    }

    /// An account's folder with this role (its Inbox, Trash…), if it has one.
    pub fn folder_by_role(&self, account_id: AccountId, role: FolderRole) -> Result<Option<Folder>> {
        Ok(self.folders(account_id)?.into_iter().find(|f| f.role == role))
    }

    /// Total unread messages across all accounts in folders with `role`,
    /// excluding snoozed threads.
    pub fn unified_unread(&self, role: FolderRole) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages m JOIN folders f ON f.id=m.folder_id
             WHERE f.role=?1 AND m.unread=1
               AND NOT EXISTS (SELECT 1 FROM snoozes s WHERE s.account_id=m.account_id
                               AND s.thread_key=m.thread_key AND s.until > ?2)",
            params![role.as_str(), Utc::now().timestamp()],
            |r| r.get(0),
        )?)
    }

    /// How many messages sit in folders with `role` across all accounts
    /// (the sidebar's Drafts count).
    pub fn unified_total(&self, role: FolderRole) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages m JOIN folders f ON f.id=m.folder_id WHERE f.role=?1",
            params![role.as_str()],
            |r| r.get(0),
        )?)
    }

    // ----- messages ---------------------------------------------------------

    /// Work out which thread a new message belongs to.
    ///
    /// Uses the References / In-Reply-To chain first (like a JWZ-lite), then
    /// falls back to the message's own Message-ID.
    pub fn resolve_thread_key(&self, account_id: AccountId, message_id: &str, in_reply_to: &str, references: &[String]) -> Result<String> {
        let mut candidates: Vec<&str> = references.iter().map(String::as_str).collect();
        if !in_reply_to.is_empty() {
            candidates.push(in_reply_to);
        }
        for c in &candidates {
            let found: Option<String> = self
                .conn
                .query_row(
                    "SELECT thread_key FROM messages WHERE account_id=?1 AND message_id=?2 LIMIT 1",
                    params![account_id, c],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(k) = found {
                return Ok(k);
            }
        }
        // Someone may already reference *us* (we arrived after a reply).
        let found: Option<String> = self
            .conn
            .query_row(
                "SELECT thread_key FROM messages WHERE account_id=?1 AND (in_reply_to=?2 OR thread_key=?2) LIMIT 1",
                params![account_id, message_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(k) = found {
            return Ok(k);
        }
        Ok(references
            .first()
            .cloned()
            .or_else(|| (!in_reply_to.is_empty()).then(|| in_reply_to.to_string()))
            .unwrap_or_else(|| message_id.to_string()))
    }

    /// Save a downloaded message. If the folder already has that UID, only
    /// its read and starred flags are updated.
    pub fn insert_message(&self, m: &Message) -> Result<MessageId> {
        self.conn.execute(
            "INSERT INTO messages (account_id,folder_id,uid,message_id,thread_key,in_reply_to,refs,subject,
                from_name,from_email,to_json,cc_json,date,snippet,body_text,body_html,attachments,unread,starred,category)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
             ON CONFLICT(folder_id,uid) DO UPDATE SET unread=excluded.unread, starred=excluded.starred",
            params![
                m.account_id, m.folder_id, m.uid, m.message_id, m.thread_key, m.in_reply_to,
                serde_json::to_string(&m.references)?, m.subject, m.from.name, m.from.email,
                serde_json::to_string(&m.to)?, serde_json::to_string(&m.cc)?, ts(&m.date),
                m.snippet, m.body_text, m.body_html, serde_json::to_string(&m.attachments)?,
                m.unread, m.starred, m.category
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The UIDs cached for a folder (to compare with the server's).
    pub fn known_uids(&self, folder_id: FolderId) -> Result<Vec<u32>> {
        let mut st = self.conn.prepare("SELECT uid FROM messages WHERE folder_id=?1")?;
        let v = st.query_map([folder_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Forget messages the server no longer has in this folder.
    pub fn delete_uids(&self, folder_id: FolderId, uids: &[u32]) -> Result<()> {
        for u in uids {
            self.conn.execute("DELETE FROM messages WHERE folder_id=?1 AND uid=?2", params![folder_id, u])?;
        }
        Ok(())
    }

    /// Copy one message's read and starred flags from the server.
    pub fn set_flags_by_uid(&self, folder_id: FolderId, uid: u32, unread: bool, starred: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET unread=?3, starred=?4 WHERE folder_id=?1 AND uid=?2",
            params![folder_id, uid, unread, starred],
        )?;
        Ok(())
    }

    fn message_from_row(r: &Row) -> rusqlite::Result<Message> {
        let j = |i: usize| -> rusqlite::Result<String> { r.get(i) };
        Ok(Message {
            id: r.get(0)?,
            account_id: r.get(1)?,
            folder_id: r.get(2)?,
            uid: r.get(3)?,
            message_id: r.get(4)?,
            thread_key: r.get(5)?,
            in_reply_to: r.get(6)?,
            references: serde_json::from_str(&j(7)?).unwrap_or_default(),
            subject: r.get(8)?,
            from: Contact { name: r.get(9)?, email: r.get(10)? },
            to: serde_json::from_str(&j(11)?).unwrap_or_default(),
            cc: serde_json::from_str(&j(12)?).unwrap_or_default(),
            date: from_ts(r.get(13)?),
            snippet: r.get(14)?,
            body_text: r.get(15)?,
            body_html: r.get(16)?,
            attachments: serde_json::from_str(&j(17)?).unwrap_or_default(),
            unread: r.get(18)?,
            starred: r.get(19)?,
            category: r.get::<_, Option<u8>>(20)?.unwrap_or(0),
        })
    }

    const MSG_COLS: &'static str = "id,account_id,folder_id,uid,message_id,thread_key,in_reply_to,refs,subject,
        from_name,from_email,to_json,cc_json,date,snippet,body_text,body_html,attachments,unread,starred,category";

    /// One message by id, with its body.
    pub fn message(&self, id: MessageId) -> Result<Option<Message>> {
        let sql = format!("SELECT {} FROM messages WHERE id=?1", Self::MSG_COLS);
        Ok(self.conn.query_row(&sql, [id], Self::message_from_row).optional()?)
    }

    /// All messages of a thread, oldest first, de-duplicated across folders
    /// (Gmail exposes the same message in INBOX and All Mail).
    pub fn thread_messages(&self, account_id: AccountId, thread_key: &str) -> Result<Vec<Message>> {
        let sql = format!(
            "SELECT {} FROM messages WHERE account_id=?1 AND thread_key=?2 ORDER BY date ASC, id ASC",
            Self::MSG_COLS
        );
        let mut st = self.conn.prepare(&sql)?;
        let all: Vec<Message> = st
            .query_map(params![account_id, thread_key], Self::message_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        let mut seen = HashMap::new();
        let mut out: Vec<Message> = Vec::new();
        for m in all {
            match seen.get(&m.message_id) {
                Some(&i) => {
                    let existing: &mut Message = &mut out[i];
                    existing.unread |= m.unread;
                    existing.starred |= m.starred;
                }
                None => {
                    seen.insert(m.message_id.clone(), out.len());
                    out.push(m);
                }
            }
        }
        Ok(out)
    }

    /// The copy of a message kept in a folder with `role` (e.g. the Drafts
    /// copy of a draft), if there is one.
    pub fn copy_in_role(&self, account_id: AccountId, message_id: &str, role: FolderRole) -> Result<Option<Message>> {
        let Some(folder) = self.folder_by_role(account_id, role)? else { return Ok(None) };
        let sql = format!("SELECT {} FROM messages WHERE account_id=?1 AND message_id=?2 AND folder_id=?3", Self::MSG_COLS);
        Ok(self.conn.query_row(&sql, params![account_id, message_id, folder.id], Self::message_from_row).optional()?)
    }

    /// Forget every copy of a message (a deleted draft, say).
    pub fn delete_message_everywhere(&self, account_id: AccountId, message_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM messages WHERE account_id=?1 AND message_id=?2", params![account_id, message_id])?;
        Ok(())
    }

    /// Every stored copy of every message in a thread (all folders).
    pub fn thread_copies(&self, account_id: AccountId, thread_key: &str) -> Result<Vec<Message>> {
        let sql = format!("SELECT {} FROM messages WHERE account_id=?1 AND thread_key=?2", Self::MSG_COLS);
        let mut st = self.conn.prepare(&sql)?;
        let v = st
            .query_map(params![account_id, thread_key], Self::message_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Mark every message in a conversation read or unread, here only (the
    /// worker tells the server).
    pub fn set_thread_unread(&self, account_id: AccountId, thread_key: &str, unread: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET unread=?3 WHERE account_id=?1 AND thread_key=?2",
            params![account_id, thread_key, unread],
        )?;
        Ok(())
    }

    /// Star or unstar every message in a conversation, here only.
    pub fn set_thread_starred(&self, account_id: AccountId, thread_key: &str, starred: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET starred=?3 WHERE account_id=?1 AND thread_key=?2",
            params![account_id, thread_key, starred],
        )?;
        Ok(())
    }

    /// Folders holding at least one copy of a thread's messages.
    pub fn thread_folder_ids(&self, account_id: AccountId, thread_key: &str) -> Result<Vec<FolderId>> {
        let mut st = self.conn.prepare("SELECT DISTINCT folder_id FROM messages WHERE account_id=?1 AND thread_key=?2")?;
        let v = st.query_map(params![account_id, thread_key], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Forget a thread's copies in one folder (label removed / moved out).
    pub fn delete_thread_in_folder(&self, account_id: AccountId, thread_key: &str, folder_id: FolderId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM messages WHERE account_id=?1 AND thread_key=?2 AND folder_id=?3",
            params![account_id, thread_key, folder_id],
        )?;
        Ok(())
    }

    /// Put a copy of each of a thread's messages into `to` (demo labels).
    pub fn copy_thread_local(&self, account_id: AccountId, thread_key: &str, to: FolderId) -> Result<()> {
        let have: HashSet<String> = self
            .thread_copies(account_id, thread_key)?
            .into_iter()
            .filter(|m| m.folder_id == to)
            .map(|m| m.message_id)
            .collect();
        let mut next: u32 = self.conn.query_row("SELECT COALESCE(MAX(uid),0) FROM messages WHERE folder_id=?1", [to], |r| r.get(0))?;
        for mut m in self.thread_messages(account_id, thread_key)? {
            if have.contains(&m.message_id) {
                continue;
            }
            next += 1;
            m.folder_id = to;
            m.uid = next;
            self.insert_message(&m)?;
        }
        Ok(())
    }

    /// Locally move a thread's copies in `sources` into `to`.
    pub fn move_thread_local_from(&self, account_id: AccountId, thread_key: &str, sources: &[FolderId], to: FolderId) -> Result<()> {
        for f in sources.iter().filter(|f| **f != to) {
            self.conn.execute(
                "UPDATE OR IGNORE messages SET folder_id=?4 WHERE account_id=?1 AND thread_key=?2 AND folder_id=?3",
                params![account_id, thread_key, f, to],
            )?;
            // Anything left was already in `to`.
            self.delete_thread_in_folder(account_id, thread_key, *f)?;
        }
        Ok(())
    }

    /// Locally move every message of a thread that lives in `from_role`
    /// folders into `to` (the worker mirrors this on the server).
    pub fn move_thread_local(&self, account_id: AccountId, thread_key: &str, to: FolderId) -> Result<()> {
        self.conn.execute(
            "UPDATE OR IGNORE messages SET folder_id=?3
             WHERE account_id=?1 AND thread_key=?2 AND folder_id<>?3
               AND folder_id NOT IN (SELECT id FROM folders WHERE role IN ('all','sent','drafts'))",
            params![account_id, thread_key, to],
        )?;
        Ok(())
    }

    /// The `WHERE` words that pick one mailbox's messages for [`Store::threads`].
    fn view_filter(view: &MailboxView, sql: &mut String, args: &mut Vec<Box<dyn rusqlite::ToSql>>) {
        match view {
            MailboxView::Folder(id) => {
                sql.push_str(" AND m.folder_id = ? ");
                args.push(Box::new(*id));
            }
            MailboxView::Everything => {}
            MailboxView::Unified(FolderRole::Starred) => {
                sql.push_str(" AND m.starred = 1 ");
            }
            // Gmail has no Archive folder: archived mail is what's in All
            // Mail but not the Inbox. Accounts with a real Archive (iCloud)
            // use that alone, so their All Mail (if any) isn't doubled up.
            MailboxView::Unified(FolderRole::Archive) => {
                sql.push_str(
                    " AND (f.role = 'archive'
                       OR (f.role = 'all'
                           AND NOT EXISTS (SELECT 1 FROM folders fa WHERE fa.account_id = m.account_id AND fa.role = 'archive')
                           AND NOT EXISTS (SELECT 1 FROM messages mi JOIN folders fi ON fi.id = mi.folder_id
                                           WHERE mi.account_id = m.account_id AND mi.thread_key = m.thread_key AND fi.role = 'inbox'))) ",
                );
            }
            MailboxView::Unified(role) => {
                sql.push_str(" AND f.role = ? ");
                args.push(Box::new(role.as_str().to_string()));
            }
        }
    }

    /// Thread rows for the list view.
    ///
    /// * `query` – free text, matched against subject, sender and body.
    /// * Snoozed threads are hidden everywhere except [`Store::snoozed_threads`].
    pub fn threads(&self, view: &MailboxView, query: &str, limit: usize) -> Result<Vec<ThreadSummary>> {
        let now = Utc::now().timestamp();
        let mut sql = String::from(
            "SELECT m.account_id, m.thread_key, m.subject, m.from_name, m.from_email, m.snippet,
                    m.date, m.unread, m.starred, m.attachments, m.folder_id, m.message_id, w.at
             FROM messages m JOIN folders f ON f.id = m.folder_id
             LEFT JOIN woken w ON w.account_id = m.account_id AND w.thread_key = m.thread_key
             WHERE 1=1 ",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        Self::view_filter(view, &mut sql, &mut args);
        if !query.trim().is_empty() {
            for word in query.split_whitespace() {
                let (field, term) = match word.split_once(':') {
                    Some(("from", t)) => ("(m.from_name || ' ' || m.from_email)", t),
                    Some(("subject", t)) => ("m.subject", t),
                    Some(("to", t)) => ("m.to_json", t),
                    _ => ("(m.subject || ' ' || m.from_name || ' ' || m.from_email || ' ' || m.body_text)", word),
                };
                if word == "is:unread" {
                    sql.push_str(" AND m.unread = 1 ");
                    continue;
                }
                if word == "is:starred" {
                    sql.push_str(" AND m.starred = 1 ");
                    continue;
                }
                if word == "is:pinned" {
                    sql.push_str(" AND EXISTS (SELECT 1 FROM pins p WHERE p.account_id=m.account_id AND p.thread_key=m.thread_key) ");
                    continue;
                }
                // category:promotions (the inbox tabs).
                if let Some(c) = word.strip_prefix("category:").map(|n| self.category_by_name(n)) {
                    // An unknown tab matches nothing.
                    let Some(c) = c else {
                        sql.push_str(" AND 0 ");
                        continue;
                    };
                    sql.push_str(&format!(" AND {} = ? ", Self::THREAD_CATEGORY));
                    args.push(Box::new(c));
                    continue;
                }
                if word == "has:attachment" {
                    sql.push_str(" AND m.attachments <> '[]' ");
                    continue;
                }
                sql.push_str(&format!(" AND {field} LIKE ? "));
                args.push(Box::new(format!("%{term}%")));
            }
        }
        sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM snoozes s WHERE s.account_id=m.account_id
                              AND s.thread_key=m.thread_key AND s.until > ?) ",
        );
        args.push(Box::new(now));
        // A thread back from snooze sorts by when it woke, as if it had
        // just arrived; its own messages stay newest-first.
        sql.push_str(" ORDER BY MAX(m.date, COALESCE(w.at, 0)) DESC, m.date DESC LIMIT 5000");

        let mut st = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let mut rows = st.query(params.as_slice())?;

        let pins = self.pin_set()?;
        let mut order: Vec<(AccountId, String)> = Vec::new();
        let mut map: HashMap<(AccountId, String), (ThreadSummary, Vec<String>)> = HashMap::new();
        while let Some(r) = rows.next()? {
            let account_id: AccountId = r.get(0)?;
            let key: String = r.get(1)?;
            let from = Contact { name: r.get(3)?, email: r.get(4)? };
            let unread: bool = r.get(7)?;
            let starred: bool = r.get(8)?;
            let atts: String = r.get(9)?;
            let msgid: String = r.get(11)?;
            let k = (account_id, key.clone());
            match map.get_mut(&k) {
                Some((t, ids)) => {
                    if !ids.contains(&msgid) {
                        ids.push(msgid);
                        t.count += 1;
                    }
                    t.unread |= unread;
                    t.starred |= starred;
                    t.has_attachments |= atts != "[]";
                    if !t.participants.iter().any(|p| p.email == from.email) {
                        t.participants.push(from);
                    }
                }
                None => {
                    // Old pinned threads still make the cut.
                    if order.len() >= limit && !pins.contains(&k) {
                        continue;
                    }
                    order.push(k.clone());
                    map.insert(
                        k,
                        (
                            ThreadSummary {
                                account_id,
                                thread_key: key,
                                subject: r.get(2)?,
                                participants: vec![from],
                                snippet: r.get(5)?,
                                last_date: from_ts(r.get(6)?),
                                count: 1,
                                unread,
                                starred,
                                has_attachments: atts != "[]",
                                folder_id: r.get(10)?,
                                pinned: false,
                                woken: r.get::<_, Option<i64>>(12)?.map(from_ts),
                                category: 0,
                            },
                            vec![msgid],
                        ),
                    );
                }
            }
        }
        let mut out: Vec<ThreadSummary> = order.into_iter().filter_map(|k| map.remove(&k).map(|(t, _)| t)).collect();
        // Participants were collected newest-first; show them oldest-first.
        // Who's in the conversation and how long it is come from the whole
        // thread, not just this folder (your replies live in Sent).
        let mut st = self.conn.prepare_cached(
            "SELECT from_name, from_email, MAX(date) d, COUNT(DISTINCT message_id) FROM messages
             WHERE account_id=?1 AND thread_key=?2 GROUP BY lower(from_email) ORDER BY d ASC",
        )?;
        for t in &mut out {
            t.category = self.thread_category(t.account_id, &t.thread_key)?;
            t.participants.reverse();
            t.pinned = pins.contains(&(t.account_id, t.thread_key.clone()));
            let senders: Vec<(Contact, i64)> = st
                .query_map(params![t.account_id, t.thread_key], |r| Ok((Contact { name: r.get(0)?, email: r.get(1)? }, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?;
            if !senders.is_empty() {
                t.count = t.count.max(senders.iter().map(|(_, n)| n).sum());
                t.participants = senders.into_iter().map(|(c, _)| c).collect();
            }
        }
        // Pinned conversations float to the top; the sort is stable so both
        // groups stay newest-first.
        out.sort_by_key(|t| !t.pinned);
        Ok(out)
    }

    // ----- snooze -----------------------------------------------------------

    /// Snooze a conversation here until `until` (or move its wake time).
    /// [`crate::snooze_sync::snooze_thread`] also records it on the server.
    pub fn snooze(&self, account_id: AccountId, thread_key: &str, until: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO snoozes (account_id,thread_key,until) VALUES (?1,?2,?3)
             ON CONFLICT(account_id,thread_key) DO UPDATE SET until=excluded.until",
            params![account_id, thread_key, until.timestamp()],
        )?;
        Ok(())
    }

    /// Cancel a conversation's snooze here.
    pub fn unsnooze(&self, account_id: AccountId, thread_key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM snoozes WHERE account_id=?1 AND thread_key=?2",
            params![account_id, thread_key],
        )?;
        Ok(())
    }

    /// When a conversation wakes, if it's snoozed and not yet due.
    pub fn snoozed_until(&self, account_id: AccountId, thread_key: &str) -> Result<Option<DateTime<Utc>>> {
        let t: Option<i64> = self
            .conn
            .query_row(
                "SELECT until FROM snoozes WHERE account_id=?1 AND thread_key=?2 AND until > ?3",
                params![account_id, thread_key, Utc::now().timestamp()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(t.map(from_ts))
    }

    /// Snoozed threads that are still asleep, soonest wake-up first.
    pub fn snoozed_threads(&self) -> Result<Vec<(ThreadSummary, DateTime<Utc>)>> {
        let mut st = self.conn.prepare(
            "SELECT account_id, thread_key, until FROM snoozes ORDER BY until ASC",
        )?;
        let rows: Vec<(AccountId, String, i64)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::new();
        for (acc, key, until) in rows {
            let msgs = self.thread_messages(acc, &key)?;
            let Some(last) = msgs.last() else { continue };
            let pinned = self.is_pinned(acc, &key)?;
            let category = self.thread_category(acc, &key)?;
            let mut participants: Vec<Contact> = Vec::new();
            for m in &msgs {
                if !participants.iter().any(|p| p.email == m.from.email) {
                    participants.push(m.from.clone());
                }
            }
            out.push((
                ThreadSummary {
                    account_id: acc,
                    thread_key: key,
                    subject: msgs[0].subject.clone(),
                    participants,
                    snippet: last.snippet.clone(),
                    last_date: last.date,
                    count: msgs.len() as i64,
                    unread: msgs.iter().any(|m| m.unread),
                    starred: msgs.iter().any(|m| m.starred),
                    has_attachments: msgs.iter().any(|m| !m.attachments.is_empty()),
                    folder_id: last.folder_id,
                    pinned,
                    woken: None,
                    category,
                },
                from_ts(until),
            ));
        }
        Ok(out)
    }

    /// Remove expired snoozes, marking their threads unread so they pop back
    /// to the top of the inbox. Returns the woken threads.
    /// Wake the demo accounts' due snoozes (real accounts wake through
    /// `snooze_sync`, which also moves the mail on the server).
    pub fn wake_due_snoozes(&self) -> Result<Vec<(AccountId, String)>> {
        let now = Utc::now().timestamp();
        let mut st = self.conn.prepare(
            "SELECT s.account_id, s.thread_key FROM snoozes s JOIN accounts a ON a.id=s.account_id WHERE s.until <= ?1 AND a.demo=1",
        )?;
        let due: Vec<(AccountId, String)> = st
            .query_map([now], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (acc, key) in &due {
            self.set_thread_unread(*acc, key, true)?;
            self.unsnooze(*acc, key)?;
            self.mark_woken(*acc, key)?;
        }
        Ok(due)
    }

    /// Note that a thread just woke from snooze, so it lists at the top.
    pub fn mark_woken(&self, account_id: AccountId, thread_key: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO woken (account_id,thread_key,at) VALUES (?1,?2,?3)
             ON CONFLICT(account_id,thread_key) DO UPDATE SET at=excluded.at",
            params![account_id, thread_key, Utc::now().timestamp()],
        )?;
        Ok(())
    }

    /// Snoozes in the Snoozed view, including any that are due but not yet
    /// back in the Inbox (waiting for the server).
    pub fn snoozed_count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM snoozes", [], |r| r.get(0))?)
    }

    /// Snoozes that are due, on any account.
    pub fn due_snoozes(&self) -> Result<Vec<(AccountId, String)>> {
        let mut st = self.conn.prepare("SELECT account_id, thread_key FROM snoozes WHERE until <= ?1")?;
        let v = st.query_map([Utc::now().timestamp()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// One account's snoozes: thread key and wake time.
    pub fn account_snoozes(&self, account_id: AccountId) -> Result<Vec<(String, DateTime<Utc>)>> {
        let mut st = self.conn.prepare("SELECT thread_key, until FROM snoozes WHERE account_id=?1")?;
        let v = st
            .query_map([account_id], |r| Ok((r.get::<_, String>(0)?, from_ts(r.get(1)?))))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// The threads with a copy in a folder.
    pub fn thread_keys_in_folder(&self, folder_id: FolderId) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT DISTINCT thread_key FROM messages WHERE folder_id=?1")?;
        let v = st.query_map([folder_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// The thread holding any of these Message-IDs.
    pub fn thread_key_for_ids(&self, account_id: AccountId, ids: &[String]) -> Result<Option<String>> {
        for id in ids {
            let k: Option<String> = self
                .conn
                .query_row("SELECT thread_key FROM messages WHERE account_id=?1 AND message_id=?2 LIMIT 1", params![account_id, id], |r| r.get(0))
                .optional()?;
            if k.is_some() {
                return Ok(k);
            }
        }
        Ok(None)
    }

    // ----- contacts ---------------------------------------------------------

    /// Address-book style autocomplete built from everyone you've exchanged mail with.
    pub fn suggest_contacts(&self, prefix: &str, limit: usize) -> Result<Vec<Contact>> {
        let like = format!("%{}%", prefix.trim());
        // People who wrote to you, plus people you wrote to (weighted higher).
        let mut st = self.conn.prepare(
            "WITH mine AS (SELECT lower(email) e FROM accounts),
             people(name, email, w) AS (
                SELECT from_name, from_email, 1 FROM messages WHERE lower(from_email) NOT IN (SELECT e FROM mine)
                UNION ALL
                SELECT json_extract(j.value, '$.name'), json_extract(j.value, '$.email'), 3
                FROM messages m, json_each(m.to_json || '') j WHERE lower(m.from_email) IN (SELECT e FROM mine)
                UNION ALL
                SELECT json_extract(j.value, '$.name'), json_extract(j.value, '$.email'), 3
                FROM messages m, json_each(m.cc_json || '') j WHERE lower(m.from_email) IN (SELECT e FROM mine)
             )
             SELECT COALESCE(MAX(name), ''), email, SUM(w) c FROM people
             WHERE email LIKE '%@%' AND (email LIKE ?1 OR name LIKE ?1) AND lower(email) NOT IN (SELECT e FROM mine)
             GROUP BY lower(email) ORDER BY c DESC LIMIT ?2",
        )?;
        let v = st
            .query_map(params![like, limit as i64], |r| Ok(Contact { name: r.get(0)?, email: r.get(1)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    fn tmp() -> Store {
        let p = crate::test_db_path("sp-test");
        let _ = std::fs::remove_file(&p);
        Store::open(&p).unwrap()
    }

    #[test]
    fn suggests_people_you_mail_but_not_yourself() {
        let s = tmp();
        demo::seed(&s).unwrap();
        let maya = s.suggest_contacts("may", 5).unwrap();
        assert_eq!(maya.first().map(|c| (c.name.as_str(), c.email.as_str())), Some(("Maya Chen", "maya@chen.family")));
        let me = s.accounts().unwrap()[0].email.clone();
        let mine = s.suggest_contacts(&me[..4], 20).unwrap();
        assert!(mine.iter().all(|c| !c.email.eq_ignore_ascii_case(&me)));
        // Sam is someone Alex wrote to, so sorts well even by first name.
        assert!(s.suggest_contacts("sam", 5).unwrap().iter().any(|c| c.email == "sam@northwind.demo"));
    }

    /// Real Gmail flags All Mail `\All` and has no Archive folder (unlike
    /// the demo, which calls All Mail its Archive): Archive must still show
    /// what's been archived, and not what's in the Inbox.
    #[test]
    fn gmail_archive_is_all_mail_minus_the_inbox() {
        let s = tmp();
        demo::seed(&s).unwrap();
        let mut gmail = s.accounts().unwrap()[1].clone();
        gmail.email = "real-gmail@example.com".into();
        gmail.id = s.add_account(&gmail).unwrap();
        let inbox = s.upsert_folder(gmail.id, "INBOX", FolderRole::Inbox).unwrap();
        let all = s.upsert_folder(gmail.id, "[Gmail]/All Mail", FolderRole::All).unwrap();
        let template = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 1).unwrap()[0].clone();
        let template = s.thread_messages(template.account_id, &template.thread_key).unwrap()[0].clone();
        let add = |id: &str, subject: &str, folder: FolderId, uid: u32| {
            let mut m = template.clone();
            m.account_id = gmail.id;
            m.folder_id = folder;
            m.uid = uid;
            m.message_id = id.into();
            m.subject = subject.into();
            m.in_reply_to = String::new();
            m.references = vec![];
            m.thread_key = s.resolve_thread_key(gmail.id, id, "", &[]).unwrap();
            s.insert_message(&m).unwrap();
        };
        // Still in the Inbox: a copy in both.
        add("waiting@x", "Still in the inbox", inbox, 1);
        add("waiting@x", "Still in the inbox", all, 1);
        // Archived: only in All Mail.
        add("done@x", "Archived already", all, 2);

        let archive = s.threads(&MailboxView::Unified(FolderRole::Archive), "", 100).unwrap();
        let mine: Vec<&str> = archive.iter().filter(|t| t.account_id == gmail.id).map(|t| t.subject.as_str()).collect();
        assert_eq!(mine, ["Archived already"]);
        // The iCloud-style account's real Archive still shows.
        let icloud = s.accounts().unwrap()[0].id;
        assert!(archive.iter().any(|t| t.account_id == icloud));
    }

    #[test]
    fn demo_threads_and_snooze() {
        let s = tmp();
        demo::seed(&s).unwrap();
        let inbox = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
        assert!(!inbox.is_empty());
        // Replies group into their original thread.
        assert!(inbox.iter().any(|t| t.count > 1));
        // Participants include your own replies from Sent, newest last.
        let dinner = inbox.iter().find(|t| t.subject.contains("Dinner")).unwrap();
        assert!(dinner.participants.len() >= 2, "{:?}", dinner.participants);

        let t = &inbox[0];
        s.snooze(t.account_id, &t.thread_key, Utc::now() + chrono::Duration::hours(3)).unwrap();
        let after = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
        assert_eq!(after.len(), inbox.len() - 1);
        assert_eq!(s.snoozed_threads().unwrap().len(), 1);

        // Expire it and wake.
        s.snooze(t.account_id, &t.thread_key, Utc::now() - chrono::Duration::seconds(1)).unwrap();
        let woke = s.wake_due_snoozes().unwrap();
        assert_eq!(woke.len(), 1);
        let back = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
        assert_eq!(back.len(), inbox.len());
        assert!(back.iter().find(|x| x.thread_key == t.thread_key).unwrap().unread);
        // It comes back at the top, as if it had just arrived.
        let woken = back.iter().find(|x| !x.pinned).unwrap();
        assert_eq!(woken.thread_key, t.thread_key);
        assert!(woken.woken.is_some() && woken.list_date() > woken.last_date - chrono::Duration::seconds(1));
    }

    #[test]
    fn signatures_and_defaults() {
        let s = tmp();
        demo::seed(&s).unwrap();
        // The two demo accounts share one "— Alex" signature.
        let sigs = s.signatures().unwrap();
        assert_eq!(sigs.len(), 1);
        assert!(s.accounts().unwrap().iter().all(|a| a.signature == "— Alex"));
        let a = s.accounts().unwrap()[0].clone();
        let work = s.save_signature(&Signature { id: 0, title: "Work".into(), body: "Alex Rivera\nNorthwind".into(), html: false }).unwrap();
        s.set_default_signature(a.id, Some(work)).unwrap();
        assert_eq!(s.account(a.id).unwrap().unwrap().signature, "Alex Rivera\nNorthwind");
        s.set_default_signature(a.id, None).unwrap();
        assert_eq!(s.account(a.id).unwrap().unwrap().signature, "");
        // HTML signatures read as text on the account.
        let html = s.save_signature(&Signature { id: 0, title: "HTML".into(), body: "<p><b>Alex</b> · <a href=\"https://x.co\">x.co</a></p>".into(), html: true }).unwrap();
        s.set_default_signature(a.id, Some(html)).unwrap();
        let acc = s.account(a.id).unwrap().unwrap();
        assert!(acc.signature.contains("Alex") && acc.signature.contains("x.co") && !acc.signature.contains('<'), "{}", acc.signature);
        assert!(s.default_signature(a.id).unwrap().unwrap().html);
        s.set_default_signature(a.id, None).unwrap();
        // Deleting a signature clears it as a default.
        let other = s.accounts().unwrap()[1].id;
        s.delete_signature(sigs[0].id).unwrap();
        assert_eq!(s.account(other).unwrap().unwrap().signature, "");
        // Old per-account signatures are migrated on open.
        s.conn.execute("UPDATE accounts SET signature='Cheers, A' WHERE id=?1", [other]).unwrap();
        let s2 = Store::open(s.path()).unwrap();
        assert_eq!(s2.account(other).unwrap().unwrap().signature, "Cheers, A");
    }

    #[test]
    fn pinned_threads_float_to_top() {
        let s = tmp();
        demo::seed(&s).unwrap();
        let inbox = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
        let last = inbox.last().unwrap();
        s.pin(last.account_id, &last.thread_key).unwrap();
        let after = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
        assert_eq!(after[0].thread_key, last.thread_key);
        assert!(after[0].pinned && !after[1].pinned);
        assert_eq!(after.len(), inbox.len());
        // Still pinned when the list is cut short.
        assert!(s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 1).unwrap().iter().any(|t| t.pinned));
        assert_eq!(s.threads(&MailboxView::Everything, "is:pinned", 100).unwrap().len(), 1);
        s.unpin(last.account_id, &last.thread_key).unwrap();
        assert!(!s.is_pinned(last.account_id, &last.thread_key).unwrap());
    }

    #[test]
    fn search_filters() {
        let s = tmp();
        demo::seed(&s).unwrap();
        let all = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
        let unread = s.threads(&MailboxView::Unified(FolderRole::Inbox), "is:unread", 100).unwrap();
        assert!(unread.len() < all.len());
        let r = s.threads(&MailboxView::Unified(FolderRole::Inbox), "from:nonexistent-zzz", 100).unwrap();
        assert!(r.is_empty());
    }
}
