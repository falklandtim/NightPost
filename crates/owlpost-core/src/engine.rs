//! The IMAP sync engine and server-side mail actions.
//!
//! Every function here is blocking; callers run them on a worker thread.

use crate::credentials;
use crate::models::*;
use crate::parse;
use crate::smtp::{self, OutgoingMessage};
use crate::store::Store;
use anyhow::{anyhow, Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::net::TcpStream;

type Session = imap::Session<native_tls::TlsStream<TcpStream>>;

/// How many of the newest messages to download per folder on first sync.
pub const INITIAL_WINDOW: usize = 300;
const FETCH_CHUNK: usize = 25;

pub fn connect(account: &Account) -> Result<Session> {
    let password = credentials::get_password(&account.email)?;
    connect_with(account, &password)
}

pub fn connect_with(account: &Account, password: &str) -> Result<Session> {
    let tls = native_tls::TlsConnector::builder().build()?;
    let s = &account.imap;
    let client = match s.security {
        Security::Tls => imap::connect((s.host.as_str(), s.port), &s.host, &tls),
        Security::StartTls => imap::connect_starttls((s.host.as_str(), s.port), &s.host, &tls),
        Security::None => return Err(anyhow!("unencrypted IMAP is not supported")),
    }
    .with_context(|| format!("connecting to {}:{}", s.host, s.port))?;
    let session = client
        .login(&account.username, password)
        .map_err(|(e, _)| anyhow!("IMAP login failed for {}: {e}", account.username))?;
    Ok(session)
}

/// Check credentials for the account setup screen.
pub fn test_login(account: &Account, password: &str) -> Result<()> {
    let mut s = connect_with(account, password)?;
    let _ = s.logout();
    Ok(())
}

#[derive(Debug, Clone)]
pub enum SyncProgress {
    Folder { account_id: AccountId, name: String, done: usize, total: usize },
    NewMessages { account_id: AccountId, count: usize },
}

/// Full sync of one account: folder list, new messages, flag changes, deletions.
pub fn sync_account(store: &Store, account: &Account, mut progress: impl FnMut(SyncProgress)) -> Result<usize> {
    if account.demo {
        return Ok(0);
    }
    let mut session = connect(account)?;
    let names = session.list(Some(""), Some("*"))?;
    let mut paths = Vec::new();
    for n in names.iter() {
        let attrs: Vec<String> = n
            .attributes()
            .iter()
            .map(|a| match a {
                imap::types::NameAttribute::NoSelect => "\\Noselect".to_string(),
                imap::types::NameAttribute::Custom(c) => c.to_string(),
                other => format!("{other:?}"),
            })
            .collect();
        if attrs.iter().any(|a| a.eq_ignore_ascii_case("\\Noselect") || a.eq_ignore_ascii_case("\\NonExistent")) {
            continue;
        }
        let role = FolderRole::detect(n.name(), &attrs);
        // Gmail's [Gmail]/Important duplicates INBOX; skip it.
        if n.name().eq_ignore_ascii_case("[Gmail]/Important") {
            continue;
        }
        store.upsert_folder(account.id, n.name(), role)?;
        paths.push(n.name().to_string());
    }
    drop(names);
    store.delete_folders_not_in(account.id, &paths)?;

    let mut folders = store.folders(account.id)?;
    // Inbox first so the most important mail lands quickly.
    folders.sort_by_key(|f| if f.role == FolderRole::Inbox { 0 } else { 1 });
    let total = folders.len();
    let mut new_total = 0;
    for (i, f) in folders.iter().enumerate() {
        progress(SyncProgress::Folder { account_id: account.id, name: f.name.clone(), done: i, total });
        match sync_folder(store, &mut session, account, f) {
            Ok(n) => {
                new_total += n;
                if n > 0 {
                    progress(SyncProgress::NewMessages { account_id: account.id, count: n });
                }
            }
            Err(e) => log::warn!("sync of {} failed: {e:#}", f.path),
        }
    }
    let _ = session.logout();
    Ok(new_total)
}

fn sync_folder(store: &Store, session: &mut Session, account: &Account, folder: &Folder) -> Result<usize> {
    let mbox = session.select(&folder.path)?;
    let validity = mbox.uid_validity.unwrap_or(0);
    if folder.uid_validity != 0 && folder.uid_validity != validity {
        log::info!("UIDVALIDITY changed for {}, resyncing", folder.path);
        store.clear_folder(folder.id)?;
    }
    let server: HashSet<u32> = if mbox.exists == 0 { HashSet::new() } else { session.uid_search("ALL")? };
    let known: HashSet<u32> = store.known_uids(folder.id)?.into_iter().collect();

    // Deleted on the server.
    let gone: Vec<u32> = known.difference(&server).copied().collect();
    store.delete_uids(folder.id, &gone)?;

    // Flag refresh for what we already have.
    let still: Vec<u32> = known.intersection(&server).copied().collect();
    for chunk in still.chunks(500) {
        let set = uid_set(chunk);
        for fetch in session.uid_fetch(&set, "(UID FLAGS)")?.iter() {
            if let Some(uid) = fetch.uid {
                let (unread, starred) = flags(fetch.flags());
                store.set_flags_by_uid(folder.id, uid, unread, starred)?;
            }
        }
    }

    // New mail, newest first, capped at the window.
    let mut new: Vec<u32> = server.difference(&known).copied().collect();
    new.sort_unstable_by(|a, b| b.cmp(a));
    let floor = if folder.last_uid == 0 { 0 } else { folder.last_uid };
    let budget = if folder.last_uid == 0 { INITIAL_WINDOW } else { INITIAL_WINDOW.max(new.iter().filter(|u| **u > floor).count()) };
    new.truncate(budget);
    let mut inserted = 0;
    for chunk in new.chunks(FETCH_CHUNK) {
        let set = uid_set(chunk);
        let fetches = session.uid_fetch(&set, "(UID FLAGS BODY.PEEK[])")?;
        for fetch in fetches.iter() {
            let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) else { continue };
            let Some(mut m) = parse::parse_message(body, account.id, folder.id, uid) else { continue };
            let (unread, starred) = flags(fetch.flags());
            m.unread = unread && !matches!(folder.role, FolderRole::Sent | FolderRole::Drafts);
            m.starred = starred;
            m.thread_key = store.resolve_thread_key(account.id, &m.message_id, &m.in_reply_to, &m.references)?;
            store.insert_message(&m)?;
            inserted += 1;
        }
    }
    let max_uid = server.iter().copied().max().unwrap_or(folder.last_uid);
    store.set_folder_sync_state(folder.id, validity, max_uid.max(folder.last_uid))?;
    Ok(inserted)
}

fn flags(f: &[imap::types::Flag]) -> (bool, bool) {
    let seen = f.iter().any(|x| matches!(x, imap::types::Flag::Seen));
    let flagged = f.iter().any(|x| matches!(x, imap::types::Flag::Flagged));
    (!seen, flagged)
}

/// Compact `1,2,3,7` → `1:3,7`.
pub fn uid_set(uids: &[u32]) -> String {
    let mut v = uids.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut out = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        out.push(if start == end { start.to_string() } else { format!("{start}:{end}") });
        i += 1;
    }
    out.join(",")
}

/// Group a thread's stored copies by folder path → UIDs.
fn copies_by_folder(store: &Store, account: &Account, thread_key: &str) -> Result<BTreeMap<(FolderId, String, FolderRole), Vec<u32>>> {
    let mut map: BTreeMap<(FolderId, String, FolderRole), Vec<u32>> = BTreeMap::new();
    for m in store.thread_copies(account.id, thread_key)? {
        if let Some(f) = store.folder(m.folder_id)? {
            map.entry((f.id, f.path, f.role)).or_default().push(m.uid);
        }
    }
    Ok(map)
}

fn store_flag(store: &Store, account: &Account, thread_key: &str, flag: &str, add: bool) -> Result<()> {
    let mut s = connect(account)?;
    for ((_, path, _), uids) in copies_by_folder(store, account, thread_key)? {
        s.select(&path)?;
        let op = if add { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };
        s.uid_store(uid_set(&uids), format!("{op} ({flag})"))?;
    }
    let _ = s.logout();
    Ok(())
}

pub fn set_thread_unread(store: &Store, account: &Account, thread_key: &str, unread: bool) -> Result<()> {
    store.set_thread_unread(account.id, thread_key, unread)?;
    if account.demo {
        return Ok(());
    }
    store_flag(store, account, thread_key, "\\Seen", !unread)
}

pub fn set_thread_starred(store: &Store, account: &Account, thread_key: &str, starred: bool) -> Result<()> {
    store.set_thread_starred(account.id, thread_key, starred)?;
    if account.demo {
        return Ok(());
    }
    store_flag(store, account, thread_key, "\\Flagged", starred)
}

/// The folder copies a "move" takes out of the mailbox.
///
/// * With `from`, just that folder (the one being looked at).
/// * Otherwise Inbox / Spam / Trash on Gmail, so other labels survive, and
///   every real mailbox (not All Mail, Sent or Drafts) elsewhere.
fn move_sources(account: &Account, copies: BTreeMap<(FolderId, String, FolderRole), Vec<u32>>, from: Option<FolderId>, target: FolderId) -> Vec<((FolderId, String, FolderRole), Vec<u32>)> {
    let pinned = from.filter(|f| copies.keys().any(|(id, _, role)| id == f && !matches!(role, FolderRole::All | FolderRole::Sent | FolderRole::Drafts)));
    copies
        .into_iter()
        .filter(|((fid, _, role), _)| {
            *fid != target
                && match pinned {
                    Some(f) => *fid == f,
                    None if account.is_gmail() => matches!(role, FolderRole::Inbox | FolderRole::Spam | FolderRole::Trash),
                    None => !matches!(role, FolderRole::All | FolderRole::Sent | FolderRole::Drafts),
                }
        })
        .collect()
}

/// The folder a role maps to (Archive falls back to Gmail's All Mail).
pub fn folder_for_role(store: &Store, account: &Account, role: FolderRole) -> Result<Folder> {
    store
        .folder_by_role(account.id, role)?
        .or_else(|| if role == FolderRole::Archive { store.folder_by_role(account.id, FolderRole::All).ok().flatten() } else { None })
        .ok_or_else(|| anyhow!("{} has no {} folder", account.email, role.label()))
}

/// Archive (or trash, spam, …) a thread.
pub fn move_thread(store: &Store, account: &Account, thread_key: &str, role: FolderRole) -> Result<()> {
    let target = folder_for_role(store, account, role)?;
    move_thread_to(store, account, thread_key, &target, None)
}

/// Move a thread into any folder (on Gmail: into a label, out of the Inbox).
pub fn move_thread_to(store: &Store, account: &Account, thread_key: &str, target: &Folder, from: Option<FolderId>) -> Result<()> {
    let sources = move_sources(account, copies_by_folder(store, account, thread_key)?, from, target.id);
    if account.demo {
        let ids: Vec<FolderId> = sources.iter().map(|((id, _, _), _)| *id).collect();
        if ids.is_empty() {
            store.copy_thread_local(account.id, thread_key, target.id)?;
        }
        store.move_thread_local_from(account.id, thread_key, &ids, target.id)?;
        return Ok(());
    }
    let mut s = connect(account)?;
    let caps = s.capabilities()?;
    let has_move = caps.has_str("MOVE");
    drop(caps);
    let mut moved_from = Vec::new();
    for ((fid, path, _), uids) in sources {
        s.select(&path)?;
        let set = uid_set(&uids);
        if has_move {
            s.uid_mv(&set, &target.path)?;
        } else {
            s.uid_copy(&set, &target.path)?;
            s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)")?;
            s.expunge()?;
        }
        moved_from.push((fid, uids));
    }
    for (fid, uids) in moved_from {
        store.delete_uids(fid, &uids)?;
    }
    if let Some(f) = store.folder(target.id)? {
        let _ = sync_folder(store, &mut s, account, &f);
    }
    let _ = s.logout();
    Ok(())
}

/// Quote a mailbox / label name for an IMAP command.
fn imap_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Add or remove a label. Gmail uses real labels (`X-GM-LABELS`); on other
/// servers a label is a copy of the conversation in that folder.
pub fn set_label(store: &Store, account: &Account, thread_key: &str, label: &Folder, add: bool) -> Result<()> {
    if account.demo {
        if add {
            store.copy_thread_local(account.id, thread_key, label.id)?;
        } else {
            store.delete_thread_in_folder(account.id, thread_key, label.id)?;
        }
        return Ok(());
    }
    let copies = copies_by_folder(store, account, thread_key)?;
    let mut s = connect(account)?;
    if account.is_gmail() {
        // Any folder holding the messages will do; All Mail holds them all.
        let ((_, path, _), uids) = copies
            .iter()
            .find(|((_, _, role), _)| *role == FolderRole::All)
            .or_else(|| copies.iter().find(|((id, _, _), _)| *id != label.id))
            .or_else(|| copies.iter().next())
            .ok_or_else(|| anyhow!("conversation not found"))?;
        s.select(path)?;
        let op = if add { "+X-GM-LABELS" } else { "-X-GM-LABELS" };
        s.uid_store(uid_set(uids), format!("{op} ({})", imap_quote(&label.path)))?;
    } else if add {
        let ((_, path, _), uids) = copies
            .iter()
            .find(|((id, _, _), _)| *id != label.id)
            .ok_or_else(|| anyhow!("conversation not found"))?;
        s.select(path)?;
        s.uid_copy(uid_set(uids), &label.path)?;
    } else if let Some(((_, path, _), uids)) = copies.iter().find(|((id, _, _), _)| *id == label.id) {
        s.select(path)?;
        let set = uid_set(uids);
        s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)")?;
        s.expunge()?;
    }
    if add {
        let _ = sync_folder(store, &mut s, account, label);
    } else {
        store.delete_thread_in_folder(account.id, thread_key, label.id)?;
    }
    let _ = s.logout();
    Ok(())
}

/// Create a folder (a label on Gmail) and return its id.
pub fn create_folder(store: &Store, account: &Account, name: &str) -> Result<FolderId> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("folder name is empty"));
    }
    if let Some(f) = store.folders(account.id)?.into_iter().find(|f| f.path.eq_ignore_ascii_case(name)) {
        return Ok(f.id);
    }
    if !account.demo {
        let mut s = connect(account)?;
        s.create(name)?;
        let _ = s.subscribe(name);
        let _ = s.logout();
    }
    store.upsert_folder(account.id, name, FolderRole::Other)
}

/// Send a message and file a copy in Sent when the server won't.
pub fn send(store: &Store, account: &Account, out: &OutgoingMessage) -> Result<String> {
    if account.demo {
        return demo_send(store, account, out);
    }
    let password = credentials::get_password(&account.email)?;
    let (id, raw) = smtp::send(account, &password, out)?;
    if !smtp::server_saves_sent_copy(account) {
        if let Some(sent) = store.folder_by_role(account.id, FolderRole::Sent)? {
            match connect_with(account, &password) {
                Ok(mut s) => {
                    if let Err(e) = s.append_with_flags(&sent.path, &raw, &[imap::types::Flag::Seen]) {
                        log::warn!("could not save to Sent: {e}");
                    }
                    let _ = sync_folder(store, &mut s, account, &sent);
                    let _ = s.logout();
                }
                Err(e) => log::warn!("could not save to Sent: {e}"),
            }
        }
    }
    Ok(id)
}

fn demo_send(store: &Store, account: &Account, out: &OutgoingMessage) -> Result<String> {
    let (id, raw) = smtp::build(account, out)?;
    let sent = store
        .folder_by_role(account.id, FolderRole::Sent)?
        .ok_or_else(|| anyhow!("no Sent folder"))?;
    let uid = store.known_uids(sent.id)?.into_iter().max().unwrap_or(0) + 1;
    if let Some(mut m) = parse::parse_message(&raw, account.id, sent.id, uid) {
        m.unread = false;
        m.thread_key = store.resolve_thread_key(account.id, &m.message_id, &m.in_reply_to, &m.references)?;
        store.insert_message(&m)?;
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmail_archive_keeps_labels() {
        let p = crate::test_db_path("sp-eng");
        let store = Store::open(&p).unwrap();
        crate::demo::seed(&store).unwrap();
        let gmail = store.accounts().unwrap().into_iter().find(|a| a.is_gmail()).unwrap();
        let t = store.threads(&MailboxView::Folder(store.folder_by_role(gmail.id, FolderRole::Inbox).unwrap().unwrap().id), "", 10).unwrap().remove(0);
        let label = store.folders(gmail.id).unwrap().into_iter().find(|f| f.role == FolderRole::Other).unwrap();
        set_label(&store, &gmail, &t.thread_key, &label, true).unwrap();
        move_thread(&store, &gmail, &t.thread_key, FolderRole::Archive).unwrap();
        let ids = store.thread_folder_ids(gmail.id, &t.thread_key).unwrap();
        assert!(ids.contains(&label.id), "archiving on Gmail must keep other labels");
        assert!(!ids.contains(&store.folder_by_role(gmail.id, FolderRole::Inbox).unwrap().unwrap().id));
    }

    #[test]
    fn compacts_uid_sets() {
        assert_eq!(super::uid_set(&[7, 1, 2, 3, 9, 10]), "1:3,7,9:10");
        assert_eq!(super::uid_set(&[5]), "5");
    }
}
