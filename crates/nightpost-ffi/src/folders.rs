//! An account's own folders (and Gmail labels), for the phone's Folders
//! screen, and the conversations in one of them.
//!
//! The list matches the desktop sidebar: the standard folders first (Inbox,
//! Sent, Trash …), then the account's own folders as a tree, each after its
//! parent. NightPost's hidden folders ("NightPost Sync", "NightPost Snoozed") are
//! left out, as on the desktop.

use super::{thread_row, Mail, Result, ThreadRow};
use nightpost_core::{folder_leaf, folder_parent, folder_segments, snooze_sync, Folder, FolderRole, MailboxView};
use std::collections::HashMap;

/// What kind of folder it is, for its icon and name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FolderKind {
    Inbox,
    Starred,
    Drafts,
    Sent,
    Archive,
    /// Gmail's All Mail.
    AllMail,
    Spam,
    Trash,
    /// One of the account's own folders or labels.
    Other,
}

/// One folder in the Folders screen.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FolderInfo {
    pub id: i64,
    pub account_id: i64,
    /// What to show: "Sent" for a standard folder, else the last level of
    /// the path ("Receipts" for `Work/Receipts`).
    pub name: String,
    /// The full path on the server, e.g. `Work/Receipts`.
    pub path: String,
    pub kind: FolderKind,
    /// How deep in the tree: 0 at the top, 1 under a parent, and so on.
    pub depth: u32,
    /// The nearest folder above it that exists (a server may list `A/B/C`
    /// without `A/B`), so the screen can fold subfolders away.
    pub parent_id: Option<i64>,
    /// Unread messages, counted on this phone.
    pub unread: i64,
}

fn kind(role: FolderRole) -> FolderKind {
    match role {
        FolderRole::Inbox => FolderKind::Inbox,
        FolderRole::Starred => FolderKind::Starred,
        FolderRole::Drafts => FolderKind::Drafts,
        FolderRole::Sent => FolderKind::Sent,
        FolderRole::Archive => FolderKind::Archive,
        FolderRole::All => FolderKind::AllMail,
        FolderRole::Spam => FolderKind::Spam,
        FolderRole::Trash => FolderKind::Trash,
        FolderRole::Other => FolderKind::Other,
    }
}

/// Standard folders in role order, then the rest in tree order.
pub(crate) fn folder_list(folders: Vec<Folder>) -> Vec<FolderInfo> {
    let (system, mut others): (Vec<Folder>, Vec<Folder>) =
        folders.into_iter().filter(|f| !snooze_sync::is_internal_folder(&f.path)).partition(|f| f.role != FolderRole::Other);
    // By levels, not by the raw path: "A B" would otherwise sort between
    // "A" and "A/B" and split a parent from its subfolders.
    others.sort_by_cached_key(|f| folder_segments(&f.path).iter().map(|s| s.to_lowercase()).collect::<Vec<_>>());
    let ids: HashMap<String, i64> = others.iter().map(|f| (f.path.clone(), f.id)).collect();
    let mut out: Vec<FolderInfo> = system
        .into_iter()
        .map(|f| FolderInfo { id: f.id, account_id: f.account_id, name: f.role.label().to_string(), path: f.path, kind: kind(f.role), depth: 0, parent_id: None, unread: f.unread })
        .collect();
    for f in others {
        let mut ancestors = vec![];
        let mut p = folder_parent(&f.path);
        while let Some(path) = p {
            if let Some(&id) = ids.get(&path) {
                ancestors.push(id);
            }
            p = folder_parent(&path);
        }
        out.push(FolderInfo {
            id: f.id,
            account_id: f.account_id,
            name: folder_leaf(&f.path).to_string(),
            depth: ancestors.len().try_into().unwrap_or(u32::MAX),
            parent_id: ancestors.first().copied(),
            path: f.path,
            kind: FolderKind::Other,
            unread: f.unread,
        });
    }
    out
}

#[uniffi::export]
impl Mail {
    /// An account's folders, as the desktop sidebar lists them: standard
    /// ones first, then its own folders as a tree (each after its parent).
    pub fn folders(&self, account_id: i64) -> Result<Vec<FolderInfo>> {
        let store = self.store()?;
        self.account(&store, account_id)?;
        Ok(folder_list(store.folders(account_id)?))
    }

    /// Sync one folder straight away, for a folder just opened while the
    /// full sync is still working through the others. Returns how many
    /// new emails came. Blocks on the network; nothing for the demo.
    pub fn sync_folder(&self, folder_id: i64) -> Result<u32> {
        let store = self.worker()?;
        let folder = store.folder(folder_id)?.ok_or_else(|| super::fail("that folder no longer exists"))?;
        let account = self.account(&store, folder.account_id)?;
        let n = nightpost_core::engine::sync_folder_now(&store, &account, &folder)?;
        Ok(n.try_into().unwrap_or(u32::MAX))
    }

    /// The conversations in one folder, newest first with pinned ones on
    /// top. `query` uses the desktop's search words; empty shows them all.
    pub fn folder_threads(&self, folder_id: i64, query: String, limit: u32) -> Result<Vec<ThreadRow>> {
        let store = self.store()?;
        if store.folder(folder_id)?.is_none() {
            return Err(super::fail("that folder no longer exists"));
        }
        Ok(store.threads(&MailboxView::Folder(folder_id), &query, limit as usize)?.into_iter().map(|t| thread_row(t, None)).collect())
    }
}
