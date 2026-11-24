//! The store's folders: each account's folders and their roles (Inbox,
//! Sent, Trash…), the sync position in each, and the unified counts.

use super::*;

impl Store {
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

    /// Give a folder a new path after it was renamed on the server, keeping
    /// its mail and sync position. Does nothing if `from` isn't known, or a
    /// folder called `to` already is (the next folder sync sorts that out).
    pub fn rename_folder(&self, account_id: AccountId, from: &str, to: &str) -> Result<()> {
        let name = crate::models::folder_leaf(to).to_string();
        self.conn.execute(
            "UPDATE folders SET path=?3, name=?4 WHERE account_id=?1 AND path=?2
             AND NOT EXISTS (SELECT 1 FROM folders WHERE account_id=?1 AND path=?3)",
            params![account_id, from, to, name],
        )?;
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

    /// The folder's status after its last full check (`sync_plan`), and
    /// when that was, in seconds since 1970.
    pub fn folder_status(&self, id: FolderId) -> Result<Option<(String, i64)>> {
        Ok(self
            .conn
            .query_row("SELECT status, checked_at FROM folder_status WHERE folder_id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    /// Save the folder's status after a full check.
    pub fn set_folder_status(&self, id: FolderId, status: &str, checked_at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO folder_status (folder_id, status, checked_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(folder_id) DO UPDATE SET status=excluded.status, checked_at=excluded.checked_at",
            params![id, status, checked_at],
        )?;
        Ok(())
    }

    /// Wipe a folder's cached messages (used when UIDVALIDITY changes).
    pub fn clear_folder(&self, id: FolderId) -> Result<()> {
        // Its saved status no longer describes what's here.
        self.conn.execute("DELETE FROM folder_status WHERE folder_id=?1", [id])?;
        self.conn.execute("DELETE FROM messages WHERE folder_id=?1", [id])?;
        self.conn.execute("UPDATE folders SET last_uid=0 WHERE id=?1", [id])?;
        Ok(())
    }

    pub(super) fn folder_from_row(r: &Row) -> rusqlite::Result<Folder> {
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
}
