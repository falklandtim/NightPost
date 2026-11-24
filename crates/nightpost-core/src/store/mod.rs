//! Local SQLite cache of accounts, folders, messages, snoozes, pins and
//! tab rules. This file holds the schema, opening the database and its
//! migrations. Each subject has its own file, all adding to [`Store`]:
//! `accounts.rs`, `folders.rs`, `messages.rs`, `threads.rs` (the
//! conversation list), `snoozes.rs`, `pins.rs`, `categories.rs` (inbox
//! tabs), `signatures.rs` and `contacts.rs`. The tests are in `tests.rs`.
//!
//! The database runs in WAL mode so the desktop app, the background sync
//! worker and the MCP server process can all use it at the same time.

use crate::models::*;
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

mod accounts;
mod categories;
mod contacts;
mod folders;
mod messages;
mod pins;
mod signatures;
mod snoozes;
mod threads;

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
-- Each folder's STATUS after its last full check, so the next sync can
-- leave it alone if nothing changed (sync_plan.rs).
CREATE TABLE IF NOT EXISTS folder_status (
    folder_id    INTEGER PRIMARY KEY REFERENCES folders(id) ON DELETE CASCADE,
    status       TEXT NOT NULL,
    checked_at   INTEGER NOT NULL
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

/// One connection to NightPost's database. Each thread and process opens its
/// own; WAL mode lets them read and write side by side.
pub struct Store {
    pub(crate) conn: Connection,
    path: PathBuf,
}

/// Default database location, e.g. `~/Library/Application Support/co.timson.NightPost/nightpost.db`.
pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("NIGHTPOST_DB") {
        return PathBuf::from(p);
    }
    crate::data_dir().join("nightpost.db")
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
}

#[cfg(test)]
mod tests;
