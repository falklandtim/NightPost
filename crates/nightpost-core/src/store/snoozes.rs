//! The store's snoozes: snoozing and waking conversations on this machine.
//! [`crate::snooze_sync`] keeps them in step with the other machines.

use super::*;

impl Store {
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
}
