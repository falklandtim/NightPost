//! The store's inbox tabs: each message's category, sender and conversation
//! rules, and your own tabs. The rules and tabs sync through the server
//! state ([`crate::snooze_sync`]).

use super::*;

impl Store {
    /// A conversation's category, as SQL over the row `m`. In order: a
    /// rule for the conversation itself (the newest), a sender rule (the
    /// newest message from a sender with one), and otherwise the lowest of
    /// its messages' guesses, so a reply makes it Primary. Unsorted
    /// messages don't count; none sorted is Primary. A rule for a tab that
    /// was removed counts as no rule.
    pub(super) const THREAD_CATEGORY: &'static str = "(SELECT COALESCE(
        (SELECT CASE WHEN q.tab IS NULL THEN q.category ELSE (SELECT t.num FROM tabs t WHERE t.key = q.tab AND t.removed = 0) END
          FROM messages z JOIN thread_rules q ON q.message_id = z.message_id AND q.removed = 0
          WHERE z.account_id = m.account_id AND z.thread_key = m.thread_key ORDER BY q.updated DESC LIMIT 1),
        (SELECT CASE WHEN r.tab IS NULL THEN r.category ELSE (SELECT t.num FROM tabs t WHERE t.key = r.tab AND t.removed = 0) END
          FROM messages y JOIN sender_rules r ON r.email = lower(y.from_email) AND r.removed = 0
          WHERE y.account_id = m.account_id AND y.thread_key = m.thread_key ORDER BY y.date DESC LIMIT 1),
        (SELECT MIN(x.category) FROM messages x WHERE x.account_id = m.account_id AND x.thread_key = m.thread_key),
        0))";

    /// Which inbox tab a conversation is in, after rules (see [`crate::category`]).
    pub fn thread_category(&self, account_id: AccountId, thread_key: &str) -> Result<u8> {
        let sql = format!("SELECT {} FROM (SELECT ?1 AS account_id, ?2 AS thread_key) m", Self::THREAD_CATEGORY);
        Ok(self.conn.query_row(&sql, params![account_id, thread_key], |r| r.get(0))?)
    }

    /// UIDs in a folder not sorted into a category yet.
    pub fn unsorted_uids(&self, folder_id: FolderId) -> Result<Vec<u32>> {
        let mut st = self.conn.prepare("SELECT uid FROM messages WHERE folder_id=?1 AND category IS NULL")?;
        let v = st.query_map([folder_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Each message in a folder with its category (None: unsorted).
    pub fn folder_categories(&self, folder_id: FolderId) -> Result<Vec<(u32, Option<u8>)>> {
        let mut st = self.conn.prepare("SELECT uid, category FROM messages WHERE folder_id=?1")?;
        let v = st.query_map([folder_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Sort a message (every copy of it in the account, found by the one
    /// at `folder_id`/`uid`).
    pub fn set_category_by_uid(&self, folder_id: FolderId, uid: u32, category: u8) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET category=?3 WHERE (account_id, message_id) IN
               (SELECT account_id, message_id FROM messages WHERE folder_id=?1 AND uid=?2)",
            params![folder_id, uid, category],
        )?;
        Ok(())
    }

    /// Every sender rule, tombstones included.
    pub fn sender_rules(&self) -> Result<Vec<crate::snooze_sync::SenderRule>> {
        let mut st = self.conn.prepare("SELECT email, category, tab, updated, removed FROM sender_rules ORDER BY email")?;
        let v = st
            .query_map([], |r| {
                Ok(crate::snooze_sync::SenderRule { email: r.get(0)?, category: r.get(1)?, tab: r.get(2)?, updated: r.get(3)?, removed: r.get(4)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// The category this address's mail always goes to, if there's a rule
    /// (and its tab, if it's one of yours, hasn't been removed).
    pub fn sender_rule(&self, email: &str) -> Result<Option<u8>> {
        Ok(self
            .conn
            .query_row(
                "SELECT CASE WHEN r.tab IS NULL THEN r.category ELSE (SELECT t.num FROM tabs t WHERE t.key = r.tab AND t.removed = 0) END
                 FROM sender_rules r WHERE r.email=lower(?1) AND r.removed=0",
                [email.trim()],
                |r| r.get::<_, Option<u8>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Store a rule, unless the one here is newer.
    pub fn put_sender_rule(&self, r: &crate::snooze_sync::SenderRule) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sender_rules (email,category,tab,updated,removed) VALUES (lower(?1),?2,?3,?4,?5)
             ON CONFLICT(email) DO UPDATE SET category=excluded.category, tab=excluded.tab, updated=excluded.updated, removed=excluded.removed
             WHERE excluded.updated >= sender_rules.updated",
            params![r.email.trim(), r.category, r.tab, r.updated, r.removed],
        )?;
        Ok(())
    }

    /// Every conversation rule, tombstones included, one per change (the
    /// Message-IDs set together).
    pub fn thread_rules(&self) -> Result<Vec<crate::snooze_sync::ThreadRule>> {
        let mut st = self.conn.prepare("SELECT message_id, category, tab, updated, removed FROM thread_rules ORDER BY updated, message_id")?;
        let rows: Vec<(String, u8, Option<String>, i64, bool)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut out: Vec<crate::snooze_sync::ThreadRule> = Vec::new();
        for (id, category, tab, updated, removed) in rows {
            match out.iter_mut().find(|o| o.updated == updated && o.category == category && o.tab == tab && o.removed == removed) {
                Some(o) => o.ids.push(id),
                None => out.push(crate::snooze_sync::ThreadRule { ids: vec![id], category, tab, updated, removed }),
            }
        }
        Ok(out)
    }

    /// Store a conversation rule, for each Message-ID unless the one here is newer.
    pub fn put_thread_rule(&self, r: &crate::snooze_sync::ThreadRule) -> Result<()> {
        let mut st = self.conn.prepare_cached(
            "INSERT INTO thread_rules (message_id,category,tab,updated,removed) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(message_id) DO UPDATE SET category=excluded.category, tab=excluded.tab, updated=excluded.updated, removed=excluded.removed
             WHERE excluded.updated >= thread_rules.updated",
        )?;
        for id in &r.ids {
            st.execute(params![id, r.category, r.tab, r.updated, r.removed])?;
        }
        Ok(())
    }

    /// The category a conversation was put in by hand, if it was.
    pub fn thread_rule(&self, account_id: AccountId, thread_key: &str) -> Result<Option<u8>> {
        Ok(self
            .conn
            .query_row(
                "SELECT CASE WHEN q.tab IS NULL THEN q.category ELSE (SELECT t.num FROM tabs t WHERE t.key = q.tab AND t.removed = 0) END
                 FROM messages z JOIN thread_rules q ON q.message_id = z.message_id AND q.removed = 0
                 WHERE z.account_id = ?1 AND z.thread_key = ?2 ORDER BY q.updated DESC LIMIT 1",
                params![account_id, thread_key],
                |r| r.get::<_, Option<u8>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Your own tabs, in the order they were made: (category number, name).
    pub fn tabs(&self) -> Result<Vec<(u8, String)>> {
        let mut st = self.conn.prepare("SELECT num, name FROM tabs WHERE removed=0 ORDER BY created, key")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Every tab, tombstones included.
    pub fn all_tabs(&self) -> Result<Vec<crate::snooze_sync::Tab>> {
        let mut st = self.conn.prepare("SELECT key, name, created, updated, removed FROM tabs ORDER BY key")?;
        let v = st
            .query_map([], |r| Ok(crate::snooze_sync::Tab { id: r.get(0)?, name: r.get(1)?, created: r.get(2)?, updated: r.get(3)?, removed: r.get(4)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Store a tab, unless the one here is newer. A new one gets the next
    /// category number.
    pub fn put_tab(&self, t: &crate::snooze_sync::Tab) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tabs (key,num,name,created,updated,removed)
               VALUES (?1, (SELECT MAX(COALESCE(MAX(num), 0) + 1, ?6) FROM tabs), ?2, ?3, ?4, ?5)
             ON CONFLICT(key) DO UPDATE SET name=excluded.name, updated=excluded.updated, removed=excluded.removed
             WHERE excluded.updated >= tabs.updated",
            params![t.id, t.name, t.created, t.updated, t.removed, crate::category::FIRST_TAB],
        )?;
        Ok(())
    }

    /// A tab's key (the same on every machine) from its number here.
    pub fn tab_key(&self, num: u8) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT key FROM tabs WHERE num=?1", [num], |r| r.get(0)).optional()?)
    }

    /// A category's name: "promotions", or one of your tabs ("Invoices").
    pub fn category_name(&self, c: u8) -> String {
        if !crate::category::is_tab(c) {
            return crate::category::name(c).to_string();
        }
        self.conn.query_row("SELECT name FROM tabs WHERE num=?1", [c], |r| r.get(0)).unwrap_or_else(|_| "primary".into())
    }

    /// A category by name (any case) or number; your tabs included.
    pub fn category_by_name(&self, name: &str) -> Option<u8> {
        let name = name.trim();
        if let Some(c) = crate::category::parse(name) {
            return Some(c);
        }
        if let Ok(n) = name.parse::<u8>() {
            return Some(n);
        }
        self.conn.query_row("SELECT num FROM tabs WHERE removed=0 AND lower(name)=lower(?1)", [name], |r| r.get(0)).optional().ok().flatten()
    }
}
