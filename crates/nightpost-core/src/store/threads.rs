//! The conversation list: the rows for a mailbox view or a search. A row
//! is chosen by the folder being viewed, but who's in it, how long it is and
//! its preview come from the whole conversation.

use super::*;

impl Store {
    /// The `WHERE` words that pick one mailbox's messages for [`Store::threads`].
    pub(super) fn view_filter(view: &MailboxView, sql: &mut String, args: &mut Vec<Box<dyn rusqlite::ToSql>>) {
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

    /// Add a search's words to the list query: `from:`, `subject:`, `to:`,
    /// `is:unread`, `is:starred`, `is:pinned`, `category:` and
    /// `has:attachment`, and plain words matched against subject, sender
    /// and body.
    pub(super) fn search_filter(&self, query: &str, sql: &mut String, args: &mut Vec<Box<dyn rusqlite::ToSql>>) {
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
        self.search_filter(query, &mut sql, &mut args);
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
        for t in &mut out {
            t.pinned = pins.contains(&(t.account_id, t.thread_key.clone()));
        }
        self.fill_from_whole_thread(&mut out)?;
        // Pinned conversations float to the top; the sort is stable so both
        // groups stay newest-first.
        out.sort_by_key(|t| !t.pinned);
        Ok(out)
    }

    /// The parts of a list row that come from the whole conversation, not
    /// just the folder being viewed: who's in it, how long it is, its tab,
    /// and the preview text. Your replies live in Sent (Gmail never puts
    /// them in the Inbox), so without this an Inbox row never showed them.
    /// The row's time and place stay the viewed folder's: Tim chose that
    /// replying shouldn't move a conversation up the Inbox.
    pub(super) fn fill_from_whole_thread(&self, out: &mut [ThreadSummary]) -> Result<()> {
        let mut senders_st = self.conn.prepare_cached(
            "SELECT from_name, from_email, MAX(date) d, COUNT(DISTINCT message_id) FROM messages
             WHERE account_id=?1 AND thread_key=?2 GROUP BY lower(from_email) ORDER BY d ASC",
        )?;
        // Drafts aren't the conversation yet, and mail in Trash or Spam has
        // been thrown out of it.
        let mut newest_st = self.conn.prepare_cached(
            "SELECT m.snippet, m.date FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id=?1 AND m.thread_key=?2 AND f.role NOT IN ('drafts','trash','spam')
             ORDER BY m.date DESC LIMIT 1",
        )?;
        for t in out {
            t.category = self.thread_category(t.account_id, &t.thread_key)?;
            // Participants were collected newest-first; show them oldest-first.
            t.participants.reverse();
            let senders: Vec<(Contact, i64)> = senders_st
                .query_map(params![t.account_id, t.thread_key], |r| Ok((Contact { name: r.get(0)?, email: r.get(1)? }, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?;
            if !senders.is_empty() {
                t.count = t.count.max(senders.iter().map(|(_, n)| n).sum());
                t.participants = senders.into_iter().map(|(c, _)| c).collect();
            }
            // Only a newer message replaces the preview, so the Drafts and
            // Trash folders still show their own mail.
            let newest: Option<(String, i64)> = newest_st.query_row(params![t.account_id, t.thread_key], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            if let Some((snippet, date)) = newest {
                if date > t.last_date.timestamp() {
                    t.snippet = snippet;
                }
            }
        }
        Ok(())
    }
}
