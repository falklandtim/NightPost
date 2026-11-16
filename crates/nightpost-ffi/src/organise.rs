//! Filing a conversation from the phone: moving it to a folder, its labels,
//! and the Inbox tabs (which tab it's in, and where a sender's mail goes).
//!
//! The same core calls as the desktop and the MCP server. Moves and labels
//! change the store, then the server. Tabs and sorting rules are kept here
//! at once and then in every account's "NightPost Sync" state, so the
//! computers agree; Gmail also gets a label named after your own tab.

use super::folders::{folder_list, FolderInfo};
use super::{fail, Mail, Result};
use nightpost_core::{category, engine, snooze_sync, Folder, FolderRole, MailboxView, Store};

/// One Inbox tab, for the row of tabs above the list.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct InboxTab {
    /// What to filter on (pass it back); `None` is All, the whole Inbox.
    pub category: Option<u8>,
    pub name: String,
    /// One of your own tabs (Invoices), which can be removed; the built-in
    /// ones (Primary, Promotions, Social) can't.
    pub own: bool,
    /// Unread conversations in it, up to 500.
    pub unread: i64,
}

/// Everything the Move sheet needs about one conversation.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Filing {
    /// The account's folders, as the Folders screen lists them.
    pub folders: Vec<FolderInfo>,
    /// The folders (labels, on Gmail) the conversation is in now.
    pub in_folders: Vec<i64>,
    /// The tabs it can go in: built-in first, then your own. No All.
    pub tabs: Vec<InboxTab>,
    /// The tab it's in now.
    pub tab: u8,
    /// It was put in that tab by hand, rather than sorted automatically.
    pub by_hand: bool,
    /// The newest sender who isn't one of your accounts, lower case; `None`
    /// for a conversation only you wrote in.
    pub sender: Option<String>,
    /// Where that sender's mail always goes; `None` is sorted automatically.
    pub sender_tab: Option<u8>,
}

/// The tabs in order: Primary, Promotions, Social, then your own.
fn tab_names(store: &Store) -> Result<Vec<(u8, String)>> {
    let mut tabs: Vec<(u8, String)> = category::ALL.iter().map(|c| (*c, category::label(*c).to_string())).collect();
    tabs.extend(store.tabs()?);
    Ok(tabs)
}

#[uniffi::export]
impl Mail {
    /// The Inbox tabs with their unread counts: All first, then Primary,
    /// Promotions, Social and your own. Counts the unified Inbox, or one
    /// account's Inbox folder when `folder_id` is given. To list a tab, add
    /// `category:<number>` to the query of `threads` or `folder_threads`.
    pub fn inbox_tabs(&self, folder_id: Option<i64>) -> Result<Vec<InboxTab>> {
        let store = self.store()?;
        let view = folder_id.map_or(MailboxView::Unified(FolderRole::Inbox), MailboxView::Folder);
        let unread = |q: &str| -> Result<i64> { Ok(store.threads(&view, q, 500)?.len().try_into().unwrap_or(i64::MAX)) };
        let mut out = vec![InboxTab { category: None, name: "All".into(), own: false, unread: unread("is:unread")? }];
        for (c, name) in tab_names(&store)? {
            out.push(InboxTab { category: Some(c), name, own: category::is_tab(c), unread: unread(&format!("is:unread category:{c}"))? });
        }
        Ok(out)
    }

    /// Where a conversation is filed: its folders and labels, its tab, and
    /// its sender's tab, with the choices for each.
    pub fn filing(&self, account_id: i64, thread_key: String) -> Result<Filing> {
        let store = self.store()?;
        self.account(&store, account_id)?;
        let msgs = store.thread_messages(account_id, &thread_key)?;
        if msgs.is_empty() {
            return Err(fail("that conversation no longer exists"));
        }
        let mine: Vec<String> = store.accounts()?.into_iter().map(|a| a.email.to_lowercase()).collect();
        let sender = msgs.iter().rev().map(|m| m.from.email.trim().to_lowercase()).find(|e| e.contains('@') && !mine.contains(e));
        let sender_tab = match &sender {
            Some(e) => store.sender_rule(e)?,
            None => None,
        };
        Ok(Filing {
            folders: folder_list(store.folders(account_id)?),
            in_folders: store.thread_folder_ids(account_id, &thread_key)?,
            tabs: tab_names(&store)?.into_iter().map(|(c, name)| InboxTab { category: Some(c), name, own: category::is_tab(c), unread: 0 }).collect(),
            tab: store.thread_category(account_id, &thread_key)?,
            by_hand: store.thread_rule(account_id, &thread_key)?.is_some(),
            sender,
            sender_tab,
        })
    }

    /// Move a conversation into one of its account's folders (on Gmail,
    /// into a label and out of the Inbox). With `from_folder_id`, only out
    /// of that folder, the one being looked at. Blocks on the network.
    pub fn move_to_folder(&self, account_id: i64, thread_key: String, folder_id: i64, from_folder_id: Option<i64>) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        self.thread_exists(&store, account_id, &thread_key)?;
        let target = self.folder_of(&store, account_id, folder_id)?;
        Ok(engine::move_thread_to(&store, &acc, &thread_key, &target, from_folder_id)?)
    }

    /// Add a label to a conversation, or take it off. Gmail uses real
    /// labels; other servers keep a copy in that folder. Blocks on the
    /// network.
    pub fn set_label(&self, account_id: i64, thread_key: String, folder_id: i64, on: bool) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        self.thread_exists(&store, account_id, &thread_key)?;
        let label = self.folder_of(&store, account_id, folder_id)?;
        Ok(engine::set_label(&store, &acc, &thread_key, &label, on)?)
    }

    /// Make a folder (a label on Gmail) at the top level, or return the one
    /// already called that. Blocks on the network.
    pub fn create_folder(&self, account_id: i64, name: String) -> Result<FolderInfo> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        let id = engine::create_folder(&store, &acc, &name)?;
        folder_list(store.folders(account_id)?).into_iter().find(|f| f.id == id).ok_or_else(|| fail("couldn't make the folder"))
    }

    /// Put a conversation in a tab (`None`: sort it like the rest again).
    /// It beats the sender's rule, and replies stay with it.
    pub fn put_in_tab(&self, account_id: i64, thread_key: String, category: Option<u8>) -> Result<()> {
        let store = self.worker()?;
        let acc = self.account(&store, account_id)?;
        self.thread_exists(&store, account_id, &thread_key)?;
        let was = store.thread_category(account_id, &thread_key)?;
        let rule = snooze_sync::thread_rule(&store, account_id, &thread_key, category)?;
        snooze_sync::save_tab_change(&store, &snooze_sync::TabChange::Thread(rule))?;
        if !acc.demo {
            // Gmail: the label named after your tab follows the conversation.
            snooze_sync::unlabel_tab(&store, &acc, &thread_key, was)?;
            snooze_sync::label_tabs(&store, &acc)?;
        }
        Ok(())
    }

    /// Always put mail from `email` in a tab (`None`: sort it
    /// automatically again), including mail already here.
    pub fn sort_sender(&self, email: String, category: Option<u8>) -> Result<()> {
        let email = email.trim().to_lowercase();
        if !email.contains('@') {
            return Err(fail("that isn't an email address"));
        }
        let store = self.worker()?;
        Ok(snooze_sync::set_sender_rule(&store, &email, category)?)
    }

    /// Make one of your own tabs and return its number, or the number of
    /// the tab already called that.
    pub fn new_tab(&self, name: String) -> Result<u8> {
        let store = self.worker()?;
        let tab = snooze_sync::new_tab(&store, &name)?;
        if !store.all_tabs()?.iter().any(|t| t.id == tab.id && !t.removed) {
            snooze_sync::save_tab_change(&store, &snooze_sync::TabChange::Tab(tab.clone()))?;
        }
        // By the list, not `category_by_name`, which reads "16" as a number.
        store.tabs()?.into_iter().find(|(_, n)| *n == tab.name).map(|(c, _)| c).ok_or_else(|| fail("couldn't make the tab"))
    }

    /// Remove one of your own tabs. Its mail goes back to being sorted
    /// automatically; its rules are kept, in case it comes back.
    pub fn remove_tab(&self, category: u8) -> Result<()> {
        if !category::is_tab(category) {
            return Err(fail("Primary, Promotions and Social can't be removed"));
        }
        let store = self.worker()?;
        let tab = snooze_sync::removed_tab(&store, category)?;
        Ok(snooze_sync::save_tab_change(&store, &snooze_sync::TabChange::Tab(tab))?)
    }
}

impl Mail {
    /// One of the account's folders, or an error if it's gone.
    fn folder_of(&self, store: &Store, account_id: i64, folder_id: i64) -> Result<Folder> {
        store.folder(folder_id)?.filter(|f| f.account_id == account_id).ok_or_else(|| fail("that folder no longer exists"))
    }
}
