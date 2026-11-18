//! The store's messages: filing each one in its conversation, the copies a
//! message has in different folders (on Gmail, one per label), flags, and
//! the local half of moves, which the UI makes before the server does.

use super::*;

impl Store {
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

    /// File a copy of a message this account already has (found by its
    /// Message-ID, in any folder) under `uid` in `folder_id`, with the
    /// server's flags, instead of downloading it again. Gmail shows one
    /// email in every label it has, so most of a label's first sync is mail
    /// that's already here. Returns false if there's no such message.
    pub fn copy_known_message(&self, account_id: AccountId, message_id: &str, folder_id: FolderId, uid: u32, unread: bool, starred: bool) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT INTO messages (account_id,folder_id,uid,message_id,thread_key,in_reply_to,refs,subject,
                from_name,from_email,to_json,cc_json,date,snippet,body_text,body_html,attachments,unread,starred,category)
             SELECT account_id,?3,?4,message_id,thread_key,in_reply_to,refs,subject,
                from_name,from_email,to_json,cc_json,date,snippet,body_text,body_html,attachments,?5,?6,category
             FROM messages WHERE account_id=?1 AND message_id=?2 LIMIT 1
             ON CONFLICT(folder_id,uid) DO UPDATE SET unread=excluded.unread, starred=excluded.starred",
            params![account_id, message_id, folder_id, uid, unread, starred],
        )?;
        Ok(n > 0)
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

    pub(super) fn message_from_row(r: &Row) -> rusqlite::Result<Message> {
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

    pub(super) const MSG_COLS: &'static str = "id,account_id,folder_id,uid,message_id,thread_key,in_reply_to,refs,subject,
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
}
