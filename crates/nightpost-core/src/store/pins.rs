//! The store's pins: which conversations are pinned here, and each pin and
//! unpin by Message-ID, synced through the server state
//! ([`crate::snooze_sync`]).

use super::*;

impl Store {
    /// Pin a conversation to the top of every list it appears in (and
    /// record it for the other machines).
    pub fn pin(&self, account_id: AccountId, thread_key: &str) -> Result<()> {
        self.set_pinned_here(account_id, thread_key, true)?;
        self.mark_pin(account_id, thread_key, false)
    }

    /// Unpin a conversation (and record it for the other machines).
    pub fn unpin(&self, account_id: AccountId, thread_key: &str) -> Result<()> {
        self.set_pinned_here(account_id, thread_key, false)?;
        self.mark_pin(account_id, thread_key, true)
    }

    /// Pin or unpin on this machine only (applying a change made elsewhere).
    pub(crate) fn set_pinned_here(&self, account_id: AccountId, thread_key: &str, pinned: bool) -> Result<()> {
        if pinned {
            self.conn.execute(
                "INSERT OR IGNORE INTO pins (account_id,thread_key,pinned_at) VALUES (?1,?2,?3)",
                params![account_id, thread_key, Utc::now().timestamp()],
            )?;
        } else {
            self.conn.execute("DELETE FROM pins WHERE account_id=?1 AND thread_key=?2", params![account_id, thread_key])?;
        }
        Ok(())
    }

    pub(super) fn thread_message_ids(&self, account_id: AccountId, thread_key: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached("SELECT DISTINCT message_id FROM messages WHERE account_id=?1 AND thread_key=?2 AND message_id<>'' ORDER BY message_id")?;
        let v = st.query_map(params![account_id, thread_key], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Record a pin (or, `removed`, an unpin) of a conversation, timed after
    /// any earlier change to it so an undo straight after still wins.
    pub(super) fn mark_pin(&self, account_id: AccountId, thread_key: &str, removed: bool) -> Result<()> {
        let ids = self.thread_message_ids(account_id, thread_key)?;
        let mut prev: Option<i64> = None;
        for id in &ids {
            let u: Option<i64> =
                self.conn.query_row("SELECT updated FROM pin_marks WHERE account_id=?1 AND message_id=?2", params![account_id, id], |r| r.get(0)).optional()?;
            prev = prev.max(u);
        }
        let now = Utc::now().timestamp();
        let updated = prev.map_or(now, |p| now.max(p + 1));
        self.put_pin_mark(account_id, &crate::snooze_sync::Pin { ids, updated, removed })
    }

    /// Every pin and unpin recorded for an account, one per change.
    pub fn pin_marks(&self, account_id: AccountId) -> Result<Vec<crate::snooze_sync::Pin>> {
        let mut st = self.conn.prepare("SELECT message_id, updated, removed FROM pin_marks WHERE account_id=?1 ORDER BY updated, message_id")?;
        let rows: Vec<(String, i64, bool)> = st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut out: Vec<crate::snooze_sync::Pin> = Vec::new();
        for (id, updated, removed) in rows {
            match out.iter_mut().find(|o| o.updated == updated && o.removed == removed) {
                Some(o) => o.ids.push(id),
                None => out.push(crate::snooze_sync::Pin { ids: vec![id], updated, removed }),
            }
        }
        Ok(out)
    }

    /// Store a pin or unpin, for each Message-ID unless the one here is newer.
    pub fn put_pin_mark(&self, account_id: AccountId, p: &crate::snooze_sync::Pin) -> Result<()> {
        let mut st = self.conn.prepare_cached(
            "INSERT INTO pin_marks (account_id,message_id,updated,removed) VALUES (?1,?2,?3,?4)
             ON CONFLICT(account_id,message_id) DO UPDATE SET updated=excluded.updated, removed=excluded.removed
             WHERE excluded.updated >= pin_marks.updated",
        )?;
        for id in &p.ids {
            st.execute(params![account_id, id, p.updated, p.removed])?;
        }
        Ok(())
    }

    /// Pins made before pins were synced have no record: give them one,
    /// dated when they were pinned.
    pub fn backfill_pin_marks(&self, account_id: AccountId) -> Result<()> {
        let mut st = self.conn.prepare("SELECT thread_key, pinned_at FROM pins WHERE account_id=?1")?;
        let pinned: Vec<(String, i64)> = st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        for (key, at) in pinned {
            let ids = self.thread_message_ids(account_id, &key)?;
            let mut known = false;
            for id in &ids {
                known |= self.conn.query_row("SELECT 1 FROM pin_marks WHERE account_id=?1 AND message_id=?2", params![account_id, id], |_| Ok(())).optional()?.is_some();
            }
            if !known && !ids.is_empty() {
                self.put_pin_mark(account_id, &crate::snooze_sync::Pin { ids, updated: at, removed: false })?;
            }
        }
        Ok(())
    }

    /// Is this conversation pinned here?
    pub fn is_pinned(&self, account_id: AccountId, thread_key: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM pins WHERE account_id=?1 AND thread_key=?2", params![account_id, thread_key], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub(super) fn pin_set(&self) -> Result<std::collections::HashSet<(AccountId, String)>> {
        let mut st = self.conn.prepare("SELECT account_id, thread_key FROM pins")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }
}
