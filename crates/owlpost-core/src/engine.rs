//! The IMAP sync engine and server-side mail actions.
//!
//! Every function here is blocking; callers run them on a worker thread.

use crate::activity::Level;
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

/// Connect and sign in, by password or Google sign-in as the account uses.
pub fn connect(account: &Account) -> Result<Session> {
    match account.auth {
        Auth::Password => connect_with(account, &credentials::get_password(&account.email)?),
        Auth::Google => crate::oauth::imap_login(open_client(account)?, account).with_context(|| format!("signing in {}", account.email)),
    }
}

fn open_client(account: &Account) -> Result<imap::Client<native_tls::TlsStream<TcpStream>>> {
    let tls = native_tls::TlsConnector::builder().build()?;
    let s = &account.imap;
    match s.security {
        Security::Tls => imap::connect((s.host.as_str(), s.port), &s.host, &tls),
        Security::StartTls => imap::connect_starttls((s.host.as_str(), s.port), &s.host, &tls),
        Security::None => return Err(anyhow!("unencrypted IMAP is not supported")),
    }
    .with_context(|| format!("connecting to {}:{}", s.host, s.port))
}

/// Connect with a password (account setup checks one before saving it).
pub fn connect_with(account: &Account, password: &str) -> Result<Session> {
    let client = open_client(account)?;
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

/// Sync just the Inbox, after a push from the server. Falls back to a full
/// sync if the folders haven't been discovered yet.
pub fn sync_inbox(store: &Store, account: &Account) -> Result<usize> {
    if account.demo {
        return Ok(0);
    }
    let Some(inbox) = store.folder_by_role(account.id, FolderRole::Inbox)? else {
        return sync_account(store, account, |_| {});
    };
    let r = (|| {
        let mut session = connect(account)?;
        let n = sync_folder(store, &mut session, account, &inbox)?;
        let _ = session.logout();
        Ok(n)
    })();
    match &r {
        Ok(n) => {
            store.sync_ok(account.id);
            if *n > 0 {
                store.note(Some(account.id), Level::Info, format!("Inbox: {n} new"));
            }
        }
        Err(e) => store.sync_failed(account.id, &format!("{e:#}")),
    }
    r
}

/// Full sync of one account: folder list, new messages, flag changes, deletions.
pub fn sync_account(store: &Store, account: &Account, progress: impl FnMut(SyncProgress)) -> Result<usize> {
    if account.demo {
        return Ok(0);
    }
    let r = sync_all_folders(store, account, progress);
    match &r {
        Ok((n, folders, failed)) => {
            store.sync_ok(account.id);
            let ok = if *failed > 0 { format!(" ({failed} couldn't be synced)") } else { String::new() };
            store.note(Some(account.id), Level::Info, format!("Checked {folders} folders{ok} · {n} new"));
        }
        Err(e) => store.sync_failed(account.id, &format!("{e:#}")),
    }
    r.map(|(n, _, _)| n)
}

/// The work of `sync_account`: (new messages, folders checked, folders that failed).
fn sync_all_folders(store: &Store, account: &Account, mut progress: impl FnMut(SyncProgress)) -> Result<(usize, usize, usize)> {
    let mut session = connect(account)?;
    let names = session.list(Some(""), Some("*"))?;
    let mut found = Vec::new();
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
        // Gmail's [Gmail]/Important duplicates INBOX; skip it. The snooze
        // state folder isn't mail.
        if n.name().eq_ignore_ascii_case("[Gmail]/Important") || n.name() == crate::snooze_sync::STATE_FOLDER {
            continue;
        }
        found.push((n.name().to_string(), attrs));
    }
    drop(names);
    let mut paths = Vec::new();
    for (path, role) in folder_roles(&found) {
        store.upsert_folder(account.id, &path, role)?;
        paths.push(path);
    }
    store.delete_folders_not_in(account.id, &paths)?;

    let mut folders = store.folders(account.id)?;
    // Inbox first so the most important mail lands quickly.
    folders.sort_by_key(|f| if f.role == FolderRole::Inbox { 0 } else { 1 });
    let total = folders.len();
    let mut new_total = 0;
    let mut failed = 0;
    for (i, f) in folders.iter().enumerate() {
        progress(SyncProgress::Folder { account_id: account.id, name: f.name.clone(), done: i, total });
        match sync_folder(store, &mut session, account, f) {
            Ok(n) => {
                new_total += n;
                if n > 0 {
                    progress(SyncProgress::NewMessages { account_id: account.id, count: n });
                }
            }
            Err(e) => {
                failed += 1;
                store.note(Some(account.id), Level::Warn, format!("Couldn't sync {}: {e:#}", f.path));
            }
        }
    }
    let _ = session.logout();
    // Snoozes made, cancelled or due on other machines.
    if let Err(e) = crate::snooze_sync::sync_snoozes(store, account) {
        store.note(Some(account.id), Level::Warn, format!("Couldn't sync snoozes and tabs: {e:#}"));
    }
    Ok((new_total, total, failed))
}

/// Each folder's role. The server's SPECIAL-USE flags win: once it marks
/// `[Gmail]/Trash` as `\\Trash`, a plain label that happens to be called
/// "Deleted Messages" or "Drafts" (old clients made these) is just a folder.
fn folder_roles(found: &[(String, Vec<String>)]) -> Vec<(String, FolderRole)> {
    let flagged: Vec<FolderRole> = found.iter().filter_map(|(_, a)| FolderRole::from_attributes(a)).collect();
    found
        .iter()
        .map(|(path, attrs)| {
            let role = FolderRole::from_attributes(attrs).unwrap_or_else(|| match FolderRole::from_name(path) {
                r if r != FolderRole::Inbox && flagged.contains(&r) => FolderRole::Other,
                r => r,
            });
            (path.clone(), role)
        })
        .collect()
}

/// Which messages to download, newest first. The first sync takes the
/// newest [`INITIAL_WINDOW`]; after that only mail that arrived since
/// (UIDs above the last one seen). Older mail outside the window stays on
/// the server: without the `last_uid` floor, every sync of a big folder
/// like All Mail re-downloaded hundreds of old messages.
fn uids_to_fetch(server: &HashSet<u32>, known: &HashSet<u32>, last_uid: u32) -> Vec<u32> {
    let mut new: Vec<u32> = server.difference(known).copied().filter(|u| last_uid == 0 || *u > last_uid).collect();
    new.sort_unstable_by(|a, b| b.cmp(a));
    if last_uid == 0 {
        new.truncate(INITIAL_WINDOW);
    }
    new
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

    let new = uids_to_fetch(&server, &known, folder.last_uid);
    let mut inserted = 0;
    for chunk in new.chunks(FETCH_CHUNK) {
        let set = uid_set(chunk);
        let fetches = session.uid_fetch(&set, "(UID FLAGS BODY.PEEK[])")?;
        for fetch in fetches.iter() {
            let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) else { continue };
            let Some(mut m) = parse::parse_message(body, account.id, folder.id, uid) else { continue };
            // OwlPost's snooze list (Gmail also shows it in All Mail and Trash).
            if m.from.email == crate::snooze_sync::STATE_FROM {
                continue;
            }
            let (unread, starred) = flags(fetch.flags());
            m.unread = unread && !matches!(folder.role, FolderRole::Sent | FolderRole::Drafts);
            m.starred = starred;
            m.thread_key = store.resolve_thread_key(account.id, &m.message_id, &m.in_reply_to, &m.references)?;
            store.insert_message(&m)?;
            inserted += 1;
        }
    }
    if folder.role == FolderRole::Inbox {
        if let Err(e) = sort_inbox(store, session, account, folder) {
            store.note(Some(account.id), Level::Warn, format!("Couldn't sort {} into tabs: {e:#}", folder.path));
        }
        if let Err(e) = crate::snooze_sync::label_tabs(store, account) {
            store.note(Some(account.id), Level::Warn, format!("Couldn't label your tabs on Gmail: {e:#}"));
        }
    }
    let max_uid = server.iter().copied().max().unwrap_or(folder.last_uid);
    store.set_folder_sync_state(folder.id, validity, max_uid.max(folder.last_uid))?;
    Ok(inserted)
}

/// Sort the Inbox into categories. Gmail's own (its Promotions and Social
/// tabs) win on Gmail; elsewhere mail downloaded before categories existed
/// is sorted from its headers, once.
fn sort_inbox(store: &Store, session: &mut Session, account: &Account, folder: &Folder) -> Result<()> {
    use crate::category::{PRIMARY, PROMOTIONS, SOCIAL};
    if account.is_gmail() {
        let promos: HashSet<u32> = session.uid_search("X-GM-RAW \"category:promotions\"")?;
        let social: HashSet<u32> = session.uid_search("X-GM-RAW \"category:social\"")?;
        for (uid, now) in store.folder_categories(folder.id)? {
            let want = if social.contains(&uid) { SOCIAL } else if promos.contains(&uid) { PROMOTIONS } else { PRIMARY };
            if now != Some(want) {
                store.set_category_by_uid(folder.id, uid, want)?;
            }
        }
        return Ok(());
    }
    let unsorted = store.unsorted_uids(folder.id)?;
    for chunk in unsorted.chunks(200) {
        for f in session.uid_fetch(uid_set(chunk), "(UID BODY.PEEK[HEADER])")?.iter() {
            if let (Some(uid), Some(h)) = (f.uid, f.header()) {
                store.set_category_by_uid(folder.id, uid, crate::category::classify_raw(h))?;
            }
        }
    }
    Ok(())
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

/// The folders a move to `target` would take the thread out of, so it can
/// be undone (see [`restore_thread`]).
pub fn move_plan(store: &Store, account: &Account, thread_key: &str, target: FolderId, from: Option<FolderId>) -> Result<Vec<FolderId>> {
    Ok(move_sources(account, copies_by_folder(store, account, thread_key)?, from, target).into_iter().map(|((id, _, _), _)| id).collect())
}

/// Undo a move: bring the thread from `from` (where it was moved to) back
/// into `to` (where it came from).
///
/// Gmail's All Mail is special: archiving there only removed the Inbox
/// label, so it's restored by copying (adding the label back). Moving out
/// of All Mail would delete the messages.
pub fn restore_thread(store: &Store, account: &Account, thread_key: &str, to: &Folder, from: &Folder) -> Result<()> {
    let copy_only = from.role == FolderRole::All;
    if account.demo {
        if copy_only {
            store.copy_thread_local(account.id, thread_key, to.id)?;
        } else {
            store.move_thread_local_from(account.id, thread_key, &[from.id], to.id)?;
        }
        return Ok(());
    }
    let copies = copies_by_folder(store, account, thread_key)?;
    let uids: Vec<u32> = copies.iter().filter(|((id, _, _), _)| *id == from.id).flat_map(|(_, u)| u.clone()).collect();
    if uids.is_empty() {
        return Err(anyhow!("the conversation is no longer in {}", from.name));
    }
    let mut s = connect(account)?;
    s.select(&from.path)?;
    let set = uid_set(&uids);
    if copy_only {
        s.uid_copy(&set, &to.path)?;
    } else {
        let has_move = s.capabilities()?.has_str("MOVE");
        if has_move {
            s.uid_mv(&set, &to.path)?;
        } else {
            s.uid_copy(&set, &to.path)?;
            s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)")?;
            s.expunge()?;
        }
        store.delete_uids(from.id, &uids)?;
    }
    let _ = sync_folder(store, &mut s, account, to);
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
    create_folder_in(store, account, None, name)
}

/// The server's folder separator: `/` on Gmail and iCloud, `.` on some others.
fn hierarchy_delimiter(s: &mut Session) -> String {
    s.list(Some(""), Some(""))
        .ok()
        .and_then(|names| names.iter().next().and_then(|n| n.delimiter().map(str::to_string)))
        .unwrap_or_else(|| "/".to_string())
}

/// Create a folder, inside `parent` when given, and return its id. An
/// existing folder of that name is returned as it is.
pub fn create_folder_in(store: &Store, account: &Account, parent: Option<&Folder>, name: &str) -> Result<FolderId> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("folder name is empty"));
    }
    let mut session = if account.demo { None } else { Some(connect(account)?) };
    let path = match parent {
        Some(p) => {
            let delim = session.as_mut().map(hierarchy_delimiter).unwrap_or_else(|| "/".into());
            if name.contains(delim.as_str()) {
                return Err(anyhow!("a folder name can't contain “{delim}”"));
            }
            format!("{}{delim}{name}", p.path)
        }
        None => name.to_string(),
    };
    if let Some(f) = store.folders(account.id)?.into_iter().find(|f| f.path.eq_ignore_ascii_case(&path)) {
        return Ok(f.id);
    }
    if let Some(mut s) = session {
        s.create(&path).with_context(|| format!("the server wouldn't create “{path}”"))?;
        let _ = s.subscribe(&path);
        let _ = s.logout();
    }
    store.upsert_folder(account.id, &path, FolderRole::Other)
}

/// Why a folder can't be deleted, if it can't: only your own folders can
/// go, and not while they have folders inside them.
pub fn cant_delete_folder(store: &Store, folder: &Folder) -> Result<Option<String>> {
    if folder.role != FolderRole::Other || crate::snooze_sync::is_internal_folder(&folder.path) {
        return Ok(Some(format!("{} is one of the account's own folders", folder.name)));
    }
    let inside = store.folders(folder.account_id)?.into_iter().any(|f| crate::models::folder_parent(&f.path).as_deref() == Some(folder.path.as_str()));
    Ok(inside.then(|| format!("“{}” has folders inside it. Delete those first.", folder.name)))
}

/// Delete a folder (a label on Gmail). Its mail isn't lost: Gmail keeps it
/// in All Mail, and on other servers it's moved to Trash first (deleting an
/// IMAP folder deletes the mail in it).
pub fn delete_folder(store: &Store, account: &Account, folder: &Folder) -> Result<()> {
    if let Some(why) = cant_delete_folder(store, folder)? {
        return Err(anyhow!(why));
    }
    if !account.demo {
        let mut s = connect(account)?;
        let mut trash = None;
        if !account.is_gmail() {
            let mbox = s.select(&folder.path)?;
            if mbox.exists > 0 {
                let t = folder_for_role(store, account, FolderRole::Trash).context("there's nowhere to put its mail")?;
                let uids: Vec<u32> = s.uid_search("ALL")?.into_iter().collect();
                if !uids.is_empty() {
                    let set = uid_set(&uids);
                    if s.capabilities()?.has_str("MOVE") {
                        s.uid_mv(&set, &t.path)?;
                    } else {
                        s.uid_copy(&set, &t.path)?;
                        s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)")?;
                        s.expunge()?;
                    }
                }
                trash = Some(t);
            }
            // Some servers won't delete the folder that's open.
            s.close()?;
        }
        s.delete(&folder.path).with_context(|| format!("the server wouldn't delete “{}”", folder.path))?;
        let _ = s.unsubscribe(&folder.path);
        if let Some(t) = trash {
            let _ = sync_folder(store, &mut s, account, &t);
        }
        let _ = s.logout();
    } else if let Some(t) = store.folder_by_role(account.id, FolderRole::Trash)? {
        for k in store.thread_keys_in_folder(folder.id)? {
            store.move_thread_local_from(account.id, &k, &[folder.id], t.id)?;
        }
    }
    store.delete_folder(folder.id)
}

/// A message copied off a server: its raw bytes, flags and arrival date.
type Downloaded = (Vec<u8>, Vec<imap::types::Flag<'static>>, Option<chrono::DateTime<chrono::FixedOffset>>);

/// Move a conversation into a folder of another account. Each message is
/// copied across (keeping its date, and whether it's read or starred), then
/// the originals go to the first account's Trash, so if anything fails part
/// way nothing is lost. Returns how many messages were copied.
pub fn move_thread_across(store: &Store, from: &Account, thread_key: &str, viewed: Option<FolderId>, to: &Account, target: &Folder) -> Result<usize> {
    if target.account_id != to.id {
        return Err(anyhow!("that folder belongs to another account"));
    }
    let trash = folder_for_role(store, from, FolderRole::Trash).with_context(|| format!("can't take the originals out of {}", from.email))?;
    let sources = move_sources(from, copies_by_folder(store, from, thread_key)?, viewed, trash.id);
    // Already in Trash: copy it out of there, and leave the originals.
    let sources = if sources.is_empty() { move_sources(from, copies_by_folder(store, from, thread_key)?, viewed, 0) } else { sources };
    if sources.is_empty() {
        return Err(anyhow!("the conversation isn't in a folder it can be moved from"));
    }
    // Each message once, though Gmail lists it under several labels.
    let mut seen = HashSet::new();
    let wanted: Vec<FolderId> = sources.iter().map(|((id, _, _), _)| *id).collect();
    let picked: Vec<Message> = store
        .thread_copies(from.id, thread_key)?
        .into_iter()
        .filter(|m| wanted.contains(&m.folder_id))
        .filter(|m| m.message_id.is_empty() || seen.insert(m.message_id.clone()))
        .collect();
    if from.demo || to.demo {
        let mut uid = store.known_uids(target.id)?.into_iter().max().unwrap_or(0);
        for m in &picked {
            uid += 1;
            let mut c = m.clone();
            c.account_id = to.id;
            c.folder_id = target.id;
            c.uid = uid;
            c.thread_key = store.resolve_thread_key(to.id, &c.message_id, &c.in_reply_to, &c.references)?;
            store.insert_message(&c)?;
        }
        if !wanted.contains(&trash.id) {
            store.move_thread_local_from(from.id, thread_key, &wanted, trash.id)?;
        }
        return Ok(picked.len());
    }
    // Download them all before writing anything.
    let mut raws: Vec<Downloaded> = Vec::new();
    let mut s = connect(from)?;
    let mut by_folder: BTreeMap<FolderId, Vec<&Message>> = BTreeMap::new();
    for m in &picked {
        by_folder.entry(m.folder_id).or_default().push(m);
    }
    for (fid, msgs) in by_folder {
        let folder = store.folder(fid)?.ok_or_else(|| anyhow!("a folder the conversation is in no longer exists"))?;
        s.select(&folder.path)?;
        for m in msgs {
            let fetches = s.uid_fetch(m.uid.to_string(), "(BODY.PEEK[] INTERNALDATE)")?;
            let Some(f) = fetches.iter().find(|f| f.body().is_some()) else {
                return Err(anyhow!("a message is no longer on the server. Sync, then try again"));
            };
            let mut flags = Vec::new();
            if !m.unread {
                flags.push(imap::types::Flag::Seen);
            }
            if m.starred {
                flags.push(imap::types::Flag::Flagged);
            }
            raws.push((f.body().unwrap_or_default().to_vec(), flags, f.internal_date()));
        }
    }
    let mut t = connect(to)?;
    for (raw, flags, date) in &raws {
        t.append_with_flags_and_date(&target.path, raw, flags, *date)
            .with_context(|| format!("couldn't copy the mail into {} on {}", target.name, to.email))?;
    }
    let _ = sync_folder(store, &mut t, to, target);
    let _ = t.logout();
    let _ = s.logout();
    if !wanted.contains(&trash.id) {
        move_thread_to(store, from, thread_key, &trash, viewed)?;
    }
    Ok(raws.len())
}

/// Send a message and file a copy in Sent when the server won't.
pub fn send(store: &Store, account: &Account, out: &OutgoingMessage) -> Result<String> {
    if account.demo {
        return demo_send(store, account, out);
    }
    // A password, or for Google sign-in a fresh access token.
    let secret = match account.auth {
        Auth::Password => credentials::get_password(&account.email)?,
        Auth::Google => crate::oauth::access_token(account)?,
    };
    let (id, raw) = smtp::send(account, &secret, out)?;
    if !smtp::server_saves_sent_copy(account) {
        if let Some(sent) = store.folder_by_role(account.id, FolderRole::Sent)? {
            match connect(account) {
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

/// Save a draft in the account's Drafts folder on the server, where other
/// mail apps (and OwlPost on other Macs) see it. `replace` is the draft this
/// one supersedes: it's deleted once the new one is safely saved. Returns
/// the new draft's Message-ID.
pub fn save_draft(store: &Store, account: &Account, out: &OutgoingMessage, replace: Option<&Message>) -> Result<String> {
    let (id, raw) = smtp::build_draft(account, out)?;
    let drafts = folder_for_role(store, account, FolderRole::Drafts).context("this account has no Drafts folder")?;
    if account.demo {
        let uid = store.known_uids(drafts.id)?.into_iter().max().unwrap_or(0) + 1;
        if let Some(mut m) = parse::parse_message(&raw, account.id, drafts.id, uid) {
            m.unread = false;
            m.thread_key = store.resolve_thread_key(account.id, &m.message_id, &m.in_reply_to, &m.references)?;
            store.insert_message(&m)?;
        }
    } else {
        let mut s = connect(account)?;
        s.append_with_flags(&drafts.path, &raw, &[imap::types::Flag::Draft, imap::types::Flag::Seen])
            .context("couldn't save the draft on the server")?;
        let _ = sync_folder(store, &mut s, account, &drafts);
        let _ = s.logout();
    }
    if let Some(old) = replace {
        if let Err(e) = delete_draft(store, account, old) {
            log::warn!("couldn't remove the old draft: {e:#}");
        }
    }
    Ok(id)
}

/// Delete a draft from the server (after it's sent or replaced).
pub fn delete_draft(store: &Store, account: &Account, draft: &Message) -> Result<()> {
    let copy = store.copy_in_role(account.id, &draft.message_id, FolderRole::Drafts)?;
    if !account.demo {
        if let Some(m) = &copy {
            let folder = store.folder(m.folder_id)?.ok_or_else(|| anyhow!("the Drafts folder no longer exists"))?;
            let mut s = connect(account)?;
            s.select(&folder.path)?;
            s.uid_store(m.uid.to_string(), "+FLAGS.SILENT (\\Deleted)")?;
            s.expunge()?;
            let _ = s.logout();
        }
    }
    // Gmail also lists drafts in All Mail: forget every copy.
    store.delete_message_everywhere(account.id, &draft.message_id)
}

/// What a draft needs, beyond what sync stores, to reopen it for editing.
#[derive(Debug, Default)]
pub struct DraftParts {
    pub bcc: Vec<Contact>,
    /// Its attachments, saved as files (inline pictures are left out).
    pub attachments: Vec<std::path::PathBuf>,
}

/// Fetch a draft's Bcc and attachments, to finish it in the composer.
pub fn open_draft(store: &Store, account: &Account, m: &Message) -> Result<DraftParts> {
    if account.demo {
        let attachments = (0..m.attachments.len()).map(|i| attachment_file(store, account, m, i)).collect::<Result<_>>()?;
        return Ok(DraftParts { bcc: vec![], attachments });
    }
    let raw = fetch_raw(store, account, m)?;
    draft_parts(&raw, &attachment_cache_dir().join("drafts").join(safe_filename(&m.message_id)))
}

/// The Bcc and attachments of a raw draft; attachments are written to `dir`.
pub fn draft_parts(raw: &[u8], dir: &std::path::Path) -> Result<DraftParts> {
    use mail_parser::MimeHeaders;
    let msg = mail_parser::MessageParser::default().parse(raw).ok_or_else(|| anyhow!("couldn't read the draft"))?;
    let mut out = DraftParts { bcc: parse::contacts(msg.bcc()), attachments: vec![] };
    for part in msg.attachments() {
        // Pictures shown inside the HTML (a signature's logo, say) aren't
        // attachments the composer could show.
        if part.content_id().is_some() && part.content_disposition().is_none_or(|d| d.is_inline()) {
            continue;
        }
        std::fs::create_dir_all(dir)?;
        let mut path = dir.join(safe_filename(part.attachment_name().unwrap_or("attachment")));
        let mut n = 1;
        while out.attachments.contains(&path) {
            n += 1;
            let name = safe_filename(part.attachment_name().unwrap_or("attachment"));
            let (stem, ext) = name.rsplit_once('.').map(|(s, e)| (s.to_string(), format!(".{e}"))).unwrap_or((name.clone(), String::new()));
            path = dir.join(format!("{stem} {n}{ext}"));
        }
        std::fs::write(&path, part.contents())?;
        out.attachments.push(path);
    }
    Ok(out)
}

/// The full raw message, fetched again from the server.
pub fn fetch_raw(store: &Store, account: &Account, m: &Message) -> Result<Vec<u8>> {
    let folder = store.folder(m.folder_id)?.ok_or_else(|| anyhow!("the message's folder no longer exists"))?;
    let mut s = connect(account)?;
    s.select(&folder.path)?;
    let fetches = s.uid_fetch(m.uid.to_string(), "BODY.PEEK[]")?;
    let raw = fetches
        .iter()
        .find_map(|f| f.body().map(<[u8]>::to_vec))
        .ok_or_else(|| anyhow!("the message is no longer on the server — try syncing"))?;
    let _ = s.logout();
    Ok(raw)
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

/// Where downloaded attachments are kept, e.g.
/// `~/Library/Caches/co.timson.OwlPost/attachments`. It's a cache: the
/// server keeps the real copy, and macOS may clear this folder.
pub fn attachment_cache_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("OWLPOST_CACHE") {
        return std::path::PathBuf::from(p).join("attachments");
    }
    directories::ProjectDirs::from("co", "timson", "OwlPost")
        .map(|d| d.cache_dir().join("attachments"))
        .unwrap_or_else(|| std::env::temp_dir().join("owlpost-attachments"))
}

/// A file name that's safe on every OS (no path separators or reserved
/// characters), keeping the extension.
pub fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() { "attachment".into() } else { cleaned }
}

/// The cached copy of attachment `index` of `m`: downloaded from the server
/// the first time, then reused.
pub fn attachment_file(store: &Store, account: &Account, m: &Message, index: usize) -> Result<std::path::PathBuf> {
    attachment_file_in(&attachment_cache_dir(), store, account, m, index)
}

pub fn attachment_file_in(cache: &std::path::Path, store: &Store, account: &Account, m: &Message, index: usize) -> Result<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = m.attachments.get(index).ok_or_else(|| anyhow!("that attachment no longer exists"))?;
    // Keyed by Message-ID, so it survives the message moving folders.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&m.message_id, m.account_id).hash(&mut h);
    let dir = cache.join(format!("{:016x}-{index}", h.finish()));
    let path = dir.join(safe_filename(&meta.filename));
    if path.is_file() {
        return Ok(path);
    }
    let data = if account.demo { demo_attachment(&meta.filename) } else { download_attachment(store, account, m, index)? };
    std::fs::create_dir_all(&dir)?;
    // Write then rename, so a half-finished download is never opened.
    let tmp = dir.join(format!(".{}.part", std::process::id()));
    std::fs::write(&tmp, &data)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Fetch the message again and pull out one attachment.
fn download_attachment(store: &Store, account: &Account, m: &Message, index: usize) -> Result<Vec<u8>> {
    let raw = fetch_raw(store, account, m)?;
    attachment_from_raw(&raw, index, &m.attachments[index].filename)
}

/// Attachment `index` of a raw message (matched by name if the order differs).
pub fn attachment_from_raw(raw: &[u8], index: usize, filename: &str) -> Result<Vec<u8>> {
    let msg = mail_parser::MessageParser::default().parse(raw).ok_or_else(|| anyhow!("couldn't read the message"))?;
    let name = |a: &mail_parser::MessagePart| mail_parser::MimeHeaders::attachment_name(a).unwrap_or("attachment").to_string();
    let parts: Vec<_> = msg.attachments().collect();
    let part = parts
        .get(index)
        .filter(|a| name(a) == filename)
        .or_else(|| parts.iter().find(|a| name(a) == filename))
        .ok_or_else(|| anyhow!("couldn't find {filename} in the message"))?;
    Ok(part.contents().to_vec())
}

/// Demo accounts have no server: make a stand-in file to open.
fn demo_attachment(filename: &str) -> Vec<u8> {
    if filename.to_ascii_lowercase().ends_with(".pdf") {
        // A minimal one-page PDF that says what it is.
        let text = "(OwlPost demo attachment) Tj";
        let content = format!("BT /F1 24 Tf 72 720 Td {text} ET");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = vec![];
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
        }
        let xref = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1));
        for off in offsets {
            pdf.push_str(&format!("{off:010} 00000 n \n"));
        }
        pdf.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1));
        return pdf.into_bytes();
    }
    format!("This is a stand-in for {filename}, attached to a demo message in OwlPost.\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_new_mail_is_fetched_after_the_first_sync() {
        let server: HashSet<u32> = (1..=1000).collect();
        // First sync: the newest window.
        let first = uids_to_fetch(&server, &HashSet::new(), 0);
        assert_eq!(first.len(), INITIAL_WINDOW);
        assert_eq!((first[0], *first.last().unwrap()), (1000, 701));
        // Next sync, nothing new: nothing to download (older mail stays put).
        let known: HashSet<u32> = (701..=1000).collect();
        assert!(uids_to_fetch(&server, &known, 1000).is_empty());
        // Three new messages arrive (one moved in from elsewhere).
        let mut server2 = server.clone();
        server2.extend([1001, 1002, 1005]);
        assert_eq!(uids_to_fetch(&server2, &known, 1000), vec![1005, 1002, 1001]);
    }

    #[test]
    fn special_use_flags_beat_folder_names() {
        let f = |p: &str, a: &[&str]| (p.to_string(), a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        // Gmail, with labels left behind by Apple Mail.
        let gmail = folder_roles(&[f("INBOX", &[]), f("Drafts", &[]), f("Deleted Messages", &[]), f("[Gmail]/Drafts", &["\\Drafts"]), f("[Gmail]/Trash", &["\\Trash"])]);
        let role = |p: &str| gmail.iter().find(|(q, _)| q == p).unwrap().1;
        assert_eq!(role("INBOX"), FolderRole::Inbox);
        assert_eq!(role("Drafts"), FolderRole::Other);
        assert_eq!(role("Deleted Messages"), FolderRole::Other);
        assert_eq!(role("[Gmail]/Drafts"), FolderRole::Drafts);
        assert_eq!(role("[Gmail]/Trash"), FolderRole::Trash);
        // A server without SPECIAL-USE still gets roles from names.
        let plain = folder_roles(&[f("INBOX", &[]), f("Trash", &[]), f("Sent", &[])]);
        assert_eq!(plain[1].1, FolderRole::Trash);
        assert_eq!(plain[2].1, FolderRole::Sent);
    }

    #[test]
    fn attachments_download_once_and_cache() {
        let p = crate::test_db_path("sp-att");
        let store = Store::open(&p).unwrap();
        crate::demo::seed(&store).unwrap();
        let account = store.accounts().unwrap().into_iter().find(|a| a.demo).unwrap();
        let inbox = store.folder_by_role(account.id, FolderRole::Inbox).unwrap().unwrap();
        let threads = store.threads(&MailboxView::Folder(inbox.id), "has:attachment", 10).unwrap();
        let m = threads
            .iter()
            .flat_map(|t| store.thread_messages(account.id, &t.thread_key).unwrap())
            .find(|m| m.attachments.iter().any(|a| a.filename.ends_with(".pdf")))
            .expect("a demo message with a PDF");
        let cache = std::env::temp_dir().join(format!("sp-att-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let path = attachment_file_in(&cache, &store, &account, &m, 0).unwrap();
        assert_eq!(path.file_name().unwrap().to_string_lossy(), safe_filename(&m.attachments[0].filename));
        assert!(std::fs::read(&path).unwrap().starts_with(b"%PDF-1.4"));
        // Second time comes from the cache.
        std::fs::write(&path, b"cached").unwrap();
        assert_eq!(std::fs::read(attachment_file_in(&cache, &store, &account, &m, 0).unwrap()).unwrap(), b"cached");
        assert!(attachment_file_in(&cache, &store, &account, &m, 5).is_err());
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn drafts_keep_bcc_and_attachments() {
        let store = Store::open(&crate::test_db_path("sp-draft")).unwrap();
        crate::demo::seed(&store).unwrap();
        let account = store.accounts().unwrap().remove(0);
        let out = OutgoingMessage {
            bcc: vec![Contact::new("Sam", "sam@example.com")],
            subject: "Plan".into(),
            body: "Draft text".into(),
            attachments: vec![smtp::OutgoingAttachment::new("notes.txt", b"hello".to_vec())],
            ..Default::default()
        };
        // No recipients at all: fine for a draft, not for sending.
        let empty = OutgoingMessage { subject: "Later".into(), ..Default::default() };
        assert!(smtp::build(&account, &empty).is_err() && smtp::build_draft(&account, &empty).is_ok());
        let (_, raw) = smtp::build_draft(&account, &out).unwrap();
        let dir = std::env::temp_dir().join(format!("sp-draft-{}", std::process::id()));
        let parts = draft_parts(&raw, &dir).unwrap();
        assert_eq!(parts.bcc[0].email, "sam@example.com");
        assert_eq!(parts.attachments.len(), 1);
        assert_eq!(std::fs::read(&parts.attachments[0]).unwrap(), b"hello");
    }

    #[test]
    fn finds_attachments_in_raw_messages() {
        use crate::smtp::{build, OutgoingAttachment, OutgoingMessage};
        let account = Account {
            id: 1,
            display_name: "A".into(),
            email: "a@example.com".into(),
            username: "a@example.com".into(),
            imap: ServerConfig { host: "imap.example.com".into(), port: 993, security: Security::Tls },
            smtp: ServerConfig { host: "smtp.example.com".into(), port: 465, security: Security::Tls },
            color: "#000".into(),
            signature: String::new(),
            demo: false,
            auth: Auth::Password,
        };
        let out = OutgoingMessage {
            to: vec![Contact::new("B", "b@example.com")],
            body: "hi".into(),
            attachments: vec![OutgoingAttachment::new("one.txt", b"first".to_vec()), OutgoingAttachment::new("two.bin", vec![0, 1, 2, 255])],
            ..Default::default()
        };
        let raw = build(&account, &out).unwrap().1;
        assert_eq!(attachment_from_raw(&raw, 1, "two.bin").unwrap(), [0, 1, 2, 255]);
        assert_eq!(attachment_from_raw(&raw, 0, "one.txt").unwrap(), b"first");
        // Order changed on the server: found by name.
        assert_eq!(attachment_from_raw(&raw, 0, "two.bin").unwrap(), [0, 1, 2, 255]);
        assert!(attachment_from_raw(&raw, 0, "missing.pdf").is_err());
        assert_eq!(safe_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_filename("Q3: plan?.pdf"), "Q3_ plan_.pdf");
        assert_eq!(safe_filename(""), "attachment");
    }

    #[test]
    fn moves_can_be_undone() {
        let p = crate::test_db_path("sp-undo");
        let store = Store::open(&p).unwrap();
        crate::demo::seed(&store).unwrap();
        for gmail in [false, true] {
            let account = store.accounts().unwrap().into_iter().find(|a| a.is_gmail() == gmail).unwrap();
            let inbox = store.folder_by_role(account.id, FolderRole::Inbox).unwrap().unwrap();
            let key = store.threads(&MailboxView::Folder(inbox.id), "", 10).unwrap().remove(0).thread_key;
            for role in [FolderRole::Archive, FolderRole::Trash] {
                let target = folder_for_role(&store, &account, role).unwrap();
                let plan = move_plan(&store, &account, &key, target.id, None).unwrap();
                assert_eq!(plan, [inbox.id], "{role:?} takes it out of the Inbox");
                move_thread(&store, &account, &key, role).unwrap();
                assert!(!store.thread_folder_ids(account.id, &key).unwrap().contains(&inbox.id));
                for f in plan {
                    restore_thread(&store, &account, &key, &store.folder(f).unwrap().unwrap(), &target).unwrap();
                }
                let now = store.thread_folder_ids(account.id, &key).unwrap();
                assert!(now.contains(&inbox.id), "undo puts it back in the Inbox ({role:?}, gmail={gmail})");
                if role == FolderRole::Trash {
                    assert!(!now.contains(&target.id), "and out of the Trash");
                }
            }
        }
    }

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
    fn folders_can_be_made_and_deleted() {
        let store = Store::open(&crate::test_db_path("sp-folders")).unwrap();
        crate::demo::seed(&store).unwrap();
        let account = store.accounts().unwrap().into_iter().find(|a| !a.is_gmail()).unwrap();
        let top = create_folder_in(&store, &account, None, "Receipts").unwrap();
        let top = store.folder(top).unwrap().unwrap();
        let sub = create_folder_in(&store, &account, Some(&top), "2026").unwrap();
        let sub = store.folder(sub).unwrap().unwrap();
        assert_eq!(sub.path, "Receipts/2026");
        assert_eq!(create_folder_in(&store, &account, Some(&top), "2026").unwrap(), sub.id, "making it twice finds the first");

        let inbox = store.folder_by_role(account.id, FolderRole::Inbox).unwrap().unwrap();
        assert!(cant_delete_folder(&store, &inbox).unwrap().is_some(), "the Inbox stays");
        assert!(cant_delete_folder(&store, &top).unwrap().is_some(), "not while it has a folder inside");
        assert!(delete_folder(&store, &account, &top).is_err());
        delete_folder(&store, &account, &sub).unwrap();
        assert!(store.folder(sub.id).unwrap().is_none());

        // Mail in a deleted folder ends up in Trash, not gone.
        let key = store.threads(&MailboxView::Folder(inbox.id), "", 10).unwrap().remove(0).thread_key;
        move_thread_to(&store, &account, &key, &top, None).unwrap();
        delete_folder(&store, &account, &top).unwrap();
        let trash = store.folder_by_role(account.id, FolderRole::Trash).unwrap().unwrap();
        assert!(store.thread_folder_ids(account.id, &key).unwrap().contains(&trash.id));
        assert!(store.folder(top.id).unwrap().is_none());
    }

    #[test]
    fn moves_between_accounts() {
        let store = Store::open(&crate::test_db_path("sp-across")).unwrap();
        crate::demo::seed(&store).unwrap();
        let accounts = store.accounts().unwrap();
        let from = accounts.iter().find(|a| !a.is_gmail()).unwrap();
        let to = accounts.iter().find(|a| a.is_gmail()).unwrap();
        let inbox = store.folder_by_role(from.id, FolderRole::Inbox).unwrap().unwrap();
        let key = store.threads(&MailboxView::Folder(inbox.id), "", 10).unwrap().remove(0).thread_key;
        let messages = store.thread_copies(from.id, &key).unwrap().iter().filter(|m| m.folder_id == inbox.id).count();
        let label = store.folders(to.id).unwrap().into_iter().find(|f| f.role == FolderRole::Other).unwrap();

        let wrong = move_thread_across(&store, from, &key, None, to, &store.folder_by_role(from.id, FolderRole::Archive).unwrap().unwrap());
        assert!(wrong.is_err(), "the folder must be in the other account");
        assert_eq!(move_thread_across(&store, from, &key, None, to, &label).unwrap(), messages);

        let copied = store.threads(&MailboxView::Folder(label.id), "", 50).unwrap();
        assert!(copied.iter().any(|t| t.account_id == to.id), "it arrives in the other account");
        let left = store.thread_folder_ids(from.id, &key).unwrap();
        assert!(!left.contains(&inbox.id), "and leaves the first account's Inbox");
        assert!(left.contains(&store.folder_by_role(from.id, FolderRole::Trash).unwrap().unwrap().id), "for its Trash");
    }

    #[test]
    fn compacts_uid_sets() {
        assert_eq!(super::uid_set(&[7, 1, 2, 3, 9, 10]), "1:3,7,9:10");
        assert_eq!(super::uid_set(&[5]), "5");
    }
}
