//! Snoozes kept on the mail server, so every OwlPost agrees (and the phone's
//! mail app stops showing snoozed mail in the inbox).
//!
//! Two folders per account (labels on Gmail):
//!
//! * [`SNOOZED_FOLDER`] holds the snoozed conversations themselves.
//! * [`STATE_FOLDER`] holds one small message, [`State`], listing each snooze:
//!   the conversation's Message-IDs (local thread keys differ between
//!   machines), when it wakes, and when that was last changed.
//!
//! Every change reads the state, merges it (the newer change to a
//! conversation wins), appends the new state and deletes the old copies.
//! Two machines writing at once just leave two copies, merged next time.
//! Cancelled snoozes stay as tombstones for [`TOMBSTONE_DAYS`] so a machine
//! that was offline doesn't bring them back.
//!
//! It also carries pins ([`Pin`]), by Message-ID, the newer pin or unpin
//! of a conversation winning.
//!
//! The same state carries the inbox tabs: your own tabs ([`Tab`]), sender
//! rules ([`SenderRule`]) and conversation rules ([`ThreadRule`]), merged
//! the same way: the newer change to a tab, address or conversation wins.
//!
//! Whichever OwlPost is running when a snooze is due moves the conversation
//! back to the Inbox and marks it unread. If another got there first, the
//! conversation is no longer in the Snoozed folder and nothing happens twice.

use crate::engine;
use crate::models::*;
use crate::store::Store;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Where snoozed conversations wait. Not plain "Snoozed": Mailspring used
/// that name, and its leftovers mustn't be mistaken for ours.
pub const SNOOZED_FOLDER: &str = "OwlPost Snoozed";
/// Where the snooze list lives.
pub const STATE_FOLDER: &str = "OwlPost Sync";
/// The state message's sender; sync skips these wherever Gmail shows them.
pub const STATE_FROM: &str = "snoozes@owlpost.invalid";
const STATE_HEADER: &str = "X-OwlPost-State";
const TOMBSTONE_DAYS: i64 = 30;
/// A snooze whose conversation isn't in the Snoozed folder this long after
/// it was set was taken out elsewhere (or its move failed): drop it.
const GRACE_SECS: i64 = 10 * 60;

/// Folders OwlPost keeps for itself, hidden from the sidebar and pickers.
pub fn is_internal_folder(path: &str) -> bool {
    path == SNOOZED_FOLDER || path == STATE_FOLDER
}

/// One snoozed (or un-snoozed) conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Message-IDs in the conversation; any one identifies it.
    pub ids: Vec<String>,
    /// When it wakes (Unix seconds).
    pub until: i64,
    /// When this entry last changed (Unix seconds); the newer change wins.
    pub updated: i64,
    /// Cancelled or woken: a tombstone.
    #[serde(default)]
    pub removed: bool,
}

impl Entry {
    fn same_conversation(&self, ids: &[String]) -> bool {
        self.ids.iter().any(|i| ids.contains(i))
    }
}

/// "Always put mail from this address in this category" (a tombstone when
/// `removed`: back to OwlPost's own guess).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenderRule {
    /// Lower-case email address.
    pub email: String,
    /// A built-in category (when `tab` is None).
    pub category: u8,
    /// One of your own tabs, by [`Tab::id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// When it last changed (Unix seconds); the newer change wins.
    pub updated: i64,
    #[serde(default)]
    pub removed: bool,
}

/// Merge sender rules: per address the newest change wins; old tombstones go.
pub fn merge_rules(docs: impl IntoIterator<Item = SenderRule>, now: i64) -> Vec<SenderRule> {
    let mut out: Vec<SenderRule> = Vec::new();
    for r in docs {
        match out.iter_mut().find(|o| o.email == r.email) {
            Some(o) if r.updated > o.updated => *o = r,
            Some(_) => {}
            None => out.push(r),
        }
    }
    out.retain(|r| !(r.removed && r.updated < now - TOMBSTONE_DAYS * 86_400));
    out.sort_by(|a, b| a.email.cmp(&b.email));
    out
}

/// "Put this conversation in this category" (a tombstone when `removed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRule {
    /// Message-IDs in the conversation; any one identifies it.
    pub ids: Vec<String>,
    pub category: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    pub updated: i64,
    #[serde(default)]
    pub removed: bool,
}

/// One of your own inbox tabs (Invoices).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    /// The same on every machine (each numbers its tabs its own way).
    pub id: String,
    pub name: String,
    /// When it was made; tabs show in this order.
    pub created: i64,
    pub updated: i64,
    #[serde(default)]
    pub removed: bool,
}

/// Merge conversation rules: per conversation the newest change wins (and
/// the Message-IDs are pooled); old tombstones go.
pub fn merge_thread_rules(docs: impl IntoIterator<Item = ThreadRule>, now: i64) -> Vec<ThreadRule> {
    let mut docs: Vec<ThreadRule> = docs.into_iter().collect();
    docs.sort_by_key(|r| std::cmp::Reverse(r.updated));
    let mut out: Vec<ThreadRule> = Vec::new();
    for r in docs {
        match out.iter_mut().find(|o| o.ids.iter().any(|i| r.ids.contains(i))) {
            // Newest first, so the one already here wins; pool the IDs.
            Some(o) => {
                for id in r.ids {
                    if !o.ids.contains(&id) {
                        o.ids.push(id);
                    }
                }
            }
            None => out.push(r),
        }
    }
    out.retain(|r| !(r.removed && r.updated < now - TOMBSTONE_DAYS * 86_400));
    for r in &mut out {
        r.ids.sort();
    }
    out.sort_by(|a, b| (a.updated, &a.ids).cmp(&(b.updated, &b.ids)));
    out
}

/// A pinned conversation (a tombstone when `removed`: unpinned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// Message-IDs in the conversation; any one identifies it.
    pub ids: Vec<String>,
    /// When it was pinned or unpinned (Unix seconds); the newer change wins.
    pub updated: i64,
    #[serde(default)]
    pub removed: bool,
}

/// Merge pins: per conversation the newest change wins (and the
/// Message-IDs are pooled); old tombstones go.
pub fn merge_pins(docs: impl IntoIterator<Item = Pin>, now: i64) -> Vec<Pin> {
    let mut docs: Vec<Pin> = docs.into_iter().collect();
    docs.sort_by_key(|p| std::cmp::Reverse(p.updated));
    let mut out: Vec<Pin> = Vec::new();
    for p in docs {
        match out.iter_mut().find(|o| o.ids.iter().any(|i| p.ids.contains(i))) {
            Some(o) => {
                for id in p.ids {
                    if !o.ids.contains(&id) {
                        o.ids.push(id);
                    }
                }
            }
            None => out.push(p),
        }
    }
    out.retain(|p| !(p.removed && p.updated < now - TOMBSTONE_DAYS * 86_400));
    for p in &mut out {
        p.ids.sort();
    }
    out.sort_by(|a, b| (a.updated, &a.ids).cmp(&(b.updated, &b.ids)));
    out
}

/// Merge tabs: per tab the newest change wins; old tombstones go.
pub fn merge_tabs(docs: impl IntoIterator<Item = Tab>, now: i64) -> Vec<Tab> {
    let mut out: Vec<Tab> = Vec::new();
    for t in docs {
        match out.iter_mut().find(|o| o.id == t.id) {
            Some(o) if t.updated > o.updated => *o = t,
            Some(_) => {}
            None => out.push(t),
        }
    }
    out.retain(|t| !(t.removed && t.updated < now - TOMBSTONE_DAYS * 86_400));
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// Inbox category rules by sender.
    #[serde(default)]
    pub senders: Vec<SenderRule>,
    /// Your own tabs.
    #[serde(default)]
    pub tabs: Vec<Tab>,
    /// Conversations put in a tab by hand.
    #[serde(default)]
    pub threads: Vec<ThreadRule>,
    /// Pinned conversations.
    #[serde(default)]
    pub pins: Vec<Pin>,
}

impl State {
    /// Merge copies of the state: per conversation the newest change wins
    /// (and their Message-IDs are pooled). Old tombstones are dropped.
    pub fn merge(docs: impl IntoIterator<Item = State>, now: i64) -> State {
        let mut out: Vec<Entry> = Vec::new();
        let (mut senders, mut tabs, mut threads, mut pins) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let docs: Vec<State> = docs.into_iter().collect();
        for d in &docs {
            senders.extend(d.senders.iter().cloned());
            tabs.extend(d.tabs.iter().cloned());
            threads.extend(d.threads.iter().cloned());
            pins.extend(d.pins.iter().cloned());
        }
        for e in docs.into_iter().flat_map(|d| d.entries) {
            match out.iter_mut().find(|o| o.same_conversation(&e.ids)) {
                Some(o) => {
                    for id in &e.ids {
                        if !o.ids.contains(id) {
                            o.ids.push(id.clone());
                        }
                    }
                    if e.updated > o.updated {
                        (o.until, o.updated, o.removed) = (e.until, e.updated, e.removed);
                    }
                }
                None => out.push(e),
            }
        }
        out.retain(|e| !(e.removed && e.updated < now - TOMBSTONE_DAYS * 86_400));
        out.sort_by_key(|e| (e.removed, e.until));
        State {
            version: 1,
            entries: out,
            senders: merge_rules(senders, now),
            tabs: merge_tabs(tabs, now),
            threads: merge_thread_rules(threads, now),
            pins: merge_pins(pins, now),
        }
    }

    /// Snooze (`Some(until)`) or cancel (`None`) a conversation.
    pub fn set(&mut self, ids: &[String], until: Option<i64>, now: i64) {
        let entry = Entry { ids: ids.to_vec(), until: until.unwrap_or(0), updated: now, removed: until.is_none() };
        match self.entries.iter_mut().find(|e| e.same_conversation(ids)) {
            Some(e) => {
                for id in ids {
                    if !e.ids.contains(id) {
                        e.ids.push(id.clone());
                    }
                }
                e.until = if until.is_some() { entry.until } else { e.until };
                (e.updated, e.removed) = (now, entry.removed);
            }
            None => self.entries.push(entry),
        }
    }

    /// The state as an email to append to [`STATE_FOLDER`].
    pub fn to_message(&self, now: DateTime<Utc>) -> Vec<u8> {
        use base64::Engine;
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        let b64 = base64::engine::general_purpose::STANDARD.encode(json);
        let body: Vec<&str> = b64.as_bytes().chunks(76).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
        format!(
            "From: OwlPost <{STATE_FROM}>\r\nTo: OwlPost <{STATE_FROM}>\r\n\
             Subject: OwlPost snoozes (used by OwlPost to sync snoozes; please leave it here)\r\n\
             Date: {}\r\nMessage-ID: <owlpost-state-{}-{:08x}@owlpost.invalid>\r\n\
             MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\
             {STATE_HEADER}: 1\r\n\r\n{}\r\n",
            now.to_rfc2822(),
            now.timestamp(),
            rand_u32(),
            body.join("\r\n")
        )
        .into_bytes()
    }

    /// Read a state message (None if it isn't one).
    pub fn from_message(raw: &[u8]) -> Option<State> {
        let msg = mail_parser::MessageParser::default().parse(raw)?;
        msg.header(STATE_HEADER)?;
        serde_json::from_str(msg.body_text(0)?.trim()).ok()
    }
}

fn rand_u32() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    u32::from_le_bytes(b)
}

/// What one account looks like locally, for [`plan`].
#[derive(Debug, Default)]
pub struct Local {
    /// Snoozes in the local table: thread key → wake time.
    pub snoozes: HashMap<String, i64>,
    /// Message-IDs of each thread mentioned (snoozed or in the folder).
    pub ids: HashMap<String, Vec<String>>,
    /// Threads with a copy in the Snoozed folder.
    pub parked: HashSet<String>,
}

impl Local {
    fn find(&self, ids: &[String]) -> Option<&String> {
        let mut keys: Vec<&String> = self.ids.iter().filter(|(_, v)| v.iter().any(|i| ids.contains(i))).map(|(k, _)| k).collect();
        keys.sort();
        keys.into_iter().next()
    }
}

/// What to do to bring the server, this machine and the clock into line.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Snooze locally until…
    pub snooze: Vec<(String, i64)>,
    /// Forget the local snooze.
    pub unsnooze: Vec<String>,
    /// Due: back to the Inbox, unread.
    pub wake: Vec<String>,
    /// Move into the Snoozed folder.
    pub park: Vec<String>,
    /// In the Snoozed folder but not snoozed: back to the Inbox.
    pub release: Vec<String>,
    /// The state to write back, if it changed.
    pub write: Option<State>,
}

/// Work out the changes, given the merged server state and what's local.
pub fn plan(mut state: State, local: &Local, now: i64) -> Plan {
    let before = state.clone();
    let mut p = Plan::default();
    let mut matched: HashSet<String> = HashSet::new();
    let mut live: HashSet<String> = HashSet::new();
    for e in state.entries.iter_mut() {
        let key = local.find(&e.ids).cloned();
        if let Some(k) = &key {
            matched.insert(k.clone());
        }
        if e.removed {
            if let Some(k) = key {
                if local.snoozes.contains_key(&k) {
                    p.unsnooze.push(k.clone());
                }
                if local.parked.contains(&k) {
                    p.release.push(k);
                }
            }
            continue;
        }
        match key {
            Some(k) if e.until <= now => {
                p.wake.push(k);
                (e.removed, e.updated) = (true, now);
            }
            // Taken out of the Snoozed folder elsewhere (e.g. on the phone).
            Some(k) if !local.parked.contains(&k) && e.updated < now - GRACE_SECS => {
                if local.snoozes.contains_key(&k) {
                    p.unsnooze.push(k);
                }
                (e.removed, e.updated) = (true, now);
            }
            Some(k) => {
                if local.snoozes.get(&k) != Some(&e.until) {
                    p.snooze.push((k.clone(), e.until));
                }
                live.insert(k);
            }
            // Mail this machine hasn't got; long overdue means nobody has.
            None if e.until < now - 86_400 => (e.removed, e.updated) = (true, now),
            None => {}
        }
    }
    // Snoozed here but unknown to the server: snoozes from before syncing,
    // or made while offline.
    let mut local_only: Vec<(&String, &i64)> = local.snoozes.iter().filter(|(k, _)| !matched.contains(*k)).collect();
    local_only.sort();
    for (k, until) in local_only {
        let ids = local.ids.get(k).cloned().unwrap_or_default();
        // Its messages are gone (deleted elsewhere): nothing left to snooze.
        if ids.is_empty() {
            p.unsnooze.push(k.clone());
            continue;
        }
        if *until <= now {
            p.wake.push(k.clone());
            continue;
        }
        state.set(&ids, Some(*until), now);
        live.insert(k.clone());
        if !local.parked.contains(k) {
            p.park.push(k.clone());
        }
    }
    let mut stray: Vec<&String> = local.parked.iter().filter(|k| !live.contains(*k) && !p.wake.contains(k) && !p.release.contains(k)).collect();
    stray.sort();
    p.release.extend(stray.into_iter().cloned());
    if state != before {
        p.write = Some(State::merge([state], now));
    }
    p
}

// ---------------------------------------------------------------------------
// On the server
// ---------------------------------------------------------------------------

/// The Message-IDs of a thread.
fn thread_ids(store: &Store, account: &Account, key: &str) -> Result<Vec<String>> {
    Ok(store.thread_messages(account.id, key)?.into_iter().map(|m| m.message_id).filter(|i| !i.is_empty()).collect())
}

/// Every copy of the state on the server: (UIDs, merged state). An account
/// that has never snoozed has no state folder: empty.
fn read_state(account: &Account) -> Result<(Vec<u32>, State)> {
    let mut s = engine::connect(account)?;
    if s.select(STATE_FOLDER).is_err() {
        let _ = s.logout();
        return Ok((vec![], State::default()));
    }
    let uids: Vec<u32> = s.uid_search("ALL")?.into_iter().collect();
    let mut docs = Vec::new();
    if !uids.is_empty() {
        for f in s.uid_fetch(engine::uid_set(&uids), "(UID BODY.PEEK[])")?.iter() {
            if let Some(st) = f.body().and_then(State::from_message) {
                docs.push(st);
            }
        }
    }
    let _ = s.logout();
    Ok((uids, State::merge(docs, Utc::now().timestamp())))
}

/// Replace the copies `old` with `state`. Appends first, so a failure part
/// way leaves an extra copy (merged later), never none.
fn write_state(store: &Store, account: &Account, old: &[u32], state: &State) -> Result<()> {
    let mut s = engine::connect(account)?;
    ensure_folders(store, account, &mut s)?;
    s.append_with_flags(STATE_FOLDER, state.to_message(Utc::now()), &[imap::types::Flag::Seen])?;
    if !old.is_empty() {
        s.select(STATE_FOLDER)?;
        let set = engine::uid_set(old);
        if account.is_gmail() {
            // Deleting from a label only removes the label; the copy would
            // stay in All Mail. Move it to Trash and delete it there.
            if let Ok(trash) = engine::folder_for_role(store, account, FolderRole::Trash) {
                s.uid_mv(&set, &trash.path)?;
                s.select(&trash.path)?;
                let gone: Vec<u32> = s.uid_search(format!("FROM \"{STATE_FROM}\""))?.into_iter().collect();
                if !gone.is_empty() {
                    s.uid_store(engine::uid_set(&gone), "+FLAGS.SILENT (\\Deleted)")?;
                    s.expunge()?;
                }
            }
        } else {
            s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)")?;
            s.expunge()?;
        }
    }
    let _ = s.logout();
    Ok(())
}

/// Create the two folders if they're missing; returns the Snoozed folder.
fn ensure_folders(store: &Store, account: &Account, s: &mut imap::Session<native_tls::TlsStream<std::net::TcpStream>>) -> Result<Folder> {
    // The imap crate sends LIST's pattern as is, so quote it (the names have spaces).
    let exists = |s: &mut imap::Session<_>, name: &str| s.list(Some(""), Some(&format!("\"{name}\""))).map(|l| !l.is_empty()).unwrap_or(false);
    for name in [SNOOZED_FOLDER, STATE_FOLDER] {
        if !exists(s, name) {
            s.create(name)?;
            let _ = s.subscribe(name);
        }
    }
    snoozed_folder(store, account)
}

fn snoozed_folder(store: &Store, account: &Account) -> Result<Folder> {
    let id = store.upsert_folder(account.id, SNOOZED_FOLDER, FolderRole::Other)?;
    store.folder(id)?.ok_or_else(|| anyhow!("{SNOOZED_FOLDER} vanished"))
}

/// Read, change and write the state.
fn update_state(store: &Store, account: &Account, f: impl FnOnce(&mut State)) -> Result<()> {
    let (old, mut state) = read_state(account)?;
    f(&mut state);
    write_state(store, account, &old, &State::merge([state], Utc::now().timestamp()))
}

/// Move a thread into the Snoozed folder (out of the Inbox when it's there).
/// A thread with nothing to move (archived on Gmail) gets a copy instead,
/// so it's still in the folder and comes back to the Inbox when it wakes.
fn park(store: &Store, account: &Account, key: &str, snoozed: &Folder) -> Result<()> {
    let inbox = store.folder_by_role(account.id, FolderRole::Inbox)?.map(|f| f.id);
    if engine::move_plan(store, account, key, snoozed.id, inbox)?.is_empty() {
        return engine::set_label(store, account, key, snoozed, true);
    }
    engine::move_thread_to(store, account, key, snoozed, inbox)
}

/// Move a thread from the Snoozed folder back to the Inbox, if it's there.
fn unpark(store: &Store, account: &Account, key: &str, snoozed: &Folder) -> Result<()> {
    if !store.thread_folder_ids(account.id, key)?.contains(&snoozed.id) {
        return Ok(());
    }
    let inbox = engine::folder_for_role(store, account, FolderRole::Inbox)?;
    engine::restore_thread(store, account, key, &inbox, snoozed)
}

/// Snooze a conversation everywhere. On failure nothing stays snoozed.
pub fn snooze_thread(store: &Store, account: &Account, key: &str, until: DateTime<Utc>) -> Result<()> {
    store.snooze(account.id, key, until)?;
    if account.demo {
        return Ok(());
    }
    let ids = thread_ids(store, account, key)?;
    let now = Utc::now().timestamp();
    let r = update_state(store, account, |st| st.set(&ids, Some(until.timestamp()), now)).and_then(|_| {
        let snoozed = snoozed_folder(store, account)?;
        park(store, account, key, &snoozed)
    });
    if let Err(e) = r {
        store.unsnooze(account.id, key)?;
        let _ = update_state(store, account, |st| st.set(&ids, None, Utc::now().timestamp()));
        return Err(e);
    }
    // The move may have filed the thread under a new key.
    rekey(store, account, key, &ids, until)?;
    Ok(())
}

/// After a move, keep the local snooze on whatever key the thread has now.
fn rekey(store: &Store, account: &Account, key: &str, ids: &[String], until: DateTime<Utc>) -> Result<()> {
    if let Some(now_key) = store.thread_key_for_ids(account.id, ids)? {
        if now_key != key {
            store.unsnooze(account.id, key)?;
            store.snooze(account.id, &now_key, until)?;
        }
    }
    Ok(())
}

/// Cancel a snooze everywhere and put the conversation back in the Inbox.
pub fn unsnooze_thread(store: &Store, account: &Account, key: &str) -> Result<()> {
    store.unsnooze(account.id, key)?;
    if account.demo {
        return Ok(());
    }
    let ids = thread_ids(store, account, key)?;
    update_state(store, account, |st| st.set(&ids, None, Utc::now().timestamp()))?;
    if let Some(f) = store.folders(account.id)?.into_iter().find(|f| f.path == SNOOZED_FOLDER) {
        unpark(store, account, key, &f)?;
    }
    Ok(())
}

/// Bring one account into line with the server: pick up snoozes made on
/// other machines, drop ones cancelled there, wake what's due, and upload
/// snoozes only this machine knows. Returns how many woke.
pub fn sync_snoozes(store: &Store, account: &Account) -> Result<usize> {
    if account.demo {
        return Ok(store.wake_due_snoozes()?.iter().filter(|(a, _)| *a == account.id).count());
    }
    let snoozed = store.folders(account.id)?.into_iter().find(|f| f.path == SNOOZED_FOLDER);
    let mut local = Local::default();
    for (k, until) in store.account_snoozes(account.id)? {
        local.snoozes.insert(k, until.timestamp());
    }
    if let Some(f) = &snoozed {
        local.parked = store.thread_keys_in_folder(f.id)?.into_iter().collect();
    }
    let (old, mut state) = read_state(account)?;
    // Tabs and their rules: pool this machine's with the server's, keep the
    // result here, and write it back if the server was missing anything.
    let now = Utc::now().timestamp();
    let tabs = merge_tabs(state.tabs.iter().cloned().chain(store.all_tabs()?), now);
    for t in &tabs {
        store.put_tab(t)?;
    }
    let rules = merge_rules(state.senders.iter().cloned().chain(store.sender_rules()?), now);
    for r in &rules {
        store.put_sender_rule(r)?;
    }
    let threads = merge_thread_rules(state.threads.iter().cloned().chain(store.thread_rules()?), now);
    for r in &threads {
        store.put_thread_rule(r)?;
    }
    let (pins, pins_changed) = pool_pins(store, account.id, &state.pins, now)?;
    let rules_changed = rules != state.senders || tabs != state.tabs || threads != merge_thread_rules(state.threads.clone(), now) || pins_changed;
    (state.senders, state.tabs, state.threads, state.pins) = (rules, tabs, threads, pins);
    // Never snoozed anywhere: nothing else to do.
    if local.snoozes.is_empty() && local.parked.is_empty() && state.entries.iter().all(|e| e.removed) {
        if rules_changed {
            write_state(store, account, &old, &state)?;
        }
        return Ok(0);
    }
    let unplanned = state.clone();
    let keys: Vec<String> = local.snoozes.keys().chain(local.parked.iter()).cloned().collect();
    for k in keys {
        let ids = thread_ids(store, account, &k)?;
        local.ids.insert(k, ids);
    }
    // Threads the server knows about that aren't snoozed or parked here.
    for e in state.entries.iter().filter(|e| !e.removed) {
        if local.find(&e.ids).is_none() {
            if let Some(k) = store.thread_key_for_ids(account.id, &e.ids)? {
                let ids = thread_ids(store, account, &k)?;
                local.ids.insert(k, ids);
            }
        }
    }
    let p = plan(state, &local, now);
    if let Some(st) = &p.write {
        write_state(store, account, &old, st)?;
    } else if rules_changed {
        write_state(store, account, &old, &unplanned)?;
    }
    for (k, until) in &p.snooze {
        store.snooze(account.id, k, DateTime::from_timestamp(*until, 0).unwrap_or_default())?;
    }
    for k in &p.unsnooze {
        store.unsnooze(account.id, k)?;
    }
    let snoozed = match (&snoozed, p.park.is_empty()) {
        (Some(f), _) => Some(f.clone()),
        (None, false) => Some(snoozed_folder(store, account)?),
        (None, true) => None,
    };
    if let Some(f) = &snoozed {
        for k in &p.park {
            if let Err(e) = park(store, account, k, f) {
                log::warn!("couldn't move a snoozed conversation into {SNOOZED_FOLDER}: {e:#}");
            }
        }
        for k in &p.release {
            if let Err(e) = unpark(store, account, k, f) {
                log::warn!("couldn't return a conversation from {SNOOZED_FOLDER}: {e:#}");
            }
        }
    }
    for k in &p.wake {
        store.unsnooze(account.id, k)?;
        if let Some(f) = &snoozed {
            unpark(store, account, k, f)?;
        }
        // The move may have changed its key; mark whatever it is now.
        let ids = local.ids.get(k).cloned().unwrap_or_default();
        let key = store.thread_key_for_ids(account.id, &ids)?.unwrap_or_else(|| k.clone());
        store.mark_woken(account.id, &key)?;
        engine::set_thread_unread(store, account, &key, true)?;
    }
    Ok(p.wake.len())
}

/// Pool this machine's pins with the server's, keep the result here and
/// pin or unpin to match. Returns the pooled pins and whether the server
/// is missing any of it.
fn pool_pins(store: &Store, account_id: AccountId, server: &[Pin], now: i64) -> Result<(Vec<Pin>, bool)> {
    store.backfill_pin_marks(account_id)?;
    let pins = merge_pins(server.iter().cloned().chain(store.pin_marks(account_id)?), now);
    for p in &pins {
        store.put_pin_mark(account_id, p)?;
    }
    // Oldest first, so the newest change to a conversation is the one kept.
    let mut want: HashMap<String, bool> = HashMap::new();
    for p in &pins {
        if let Some(k) = store.thread_key_for_ids(account_id, &p.ids)? {
            want.insert(k, !p.removed);
        }
    }
    for (k, pinned) in want {
        if store.is_pinned(account_id, &k)? != pinned {
            store.set_pinned_here(account_id, &k, pinned)?;
        }
    }
    let changed = pins != merge_pins(server.iter().cloned(), now);
    Ok((pins, changed))
}

/// Save this machine's pins and unpins to the server, at once rather than
/// at the next sync.
pub fn push_pins(store: &Store, account: &Account) -> Result<()> {
    if account.demo {
        return Ok(());
    }
    let (old, mut state) = read_state(account)?;
    let now = Utc::now().timestamp();
    let (pins, changed) = pool_pins(store, account.id, &state.pins, now)?;
    if changed {
        state.pins = pins;
        write_state(store, account, &old, &State::merge([state], now))?;
    }
    Ok(())
}

/// How a category is written in the state: a built-in one by number,
/// one of your tabs by its id. None (back to automatic) is a tombstone.
fn target(store: &Store, category: Option<u8>) -> Result<(u8, Option<String>, bool)> {
    match category {
        None => Ok((0, None, true)),
        Some(c) if crate::category::is_tab(c) => Ok((0, Some(store.tab_key(c)?.ok_or_else(|| anyhow!("that tab doesn't exist"))?), false)),
        Some(c) => Ok((c, None, false)),
    }
}

/// A time for a change that replaces one made at `prev`: now, but always
/// later than `prev`, so an undo straight after wins everywhere.
fn stamp(prev: Option<i64>) -> i64 {
    let now = Utc::now().timestamp();
    prev.map_or(now, |p| now.max(p + 1))
}

/// A rule putting mail from `email` in `category` (None: back to OwlPost's guess).
pub fn sender_rule(store: &Store, email: &str, category: Option<u8>) -> Result<SenderRule> {
    let (category, tab, removed) = target(store, category)?;
    let email = email.trim().to_lowercase();
    let prev = store.sender_rules()?.into_iter().find(|r| r.email == email).map(|r| r.updated);
    Ok(SenderRule { email, category, tab, updated: stamp(prev), removed })
}

/// A rule putting one conversation in `category` (None: sort it like the rest).
pub fn thread_rule(store: &Store, account_id: AccountId, key: &str, category: Option<u8>) -> Result<ThreadRule> {
    let (category, tab, removed) = target(store, category)?;
    let ids: Vec<String> = store.thread_messages(account_id, key)?.into_iter().map(|m| m.message_id).filter(|i| !i.is_empty()).collect();
    if ids.is_empty() {
        return Err(anyhow!("that conversation has no Message-IDs to remember it by"));
    }
    let prev = store.thread_rules()?.into_iter().filter(|r| r.ids.iter().any(|i| ids.contains(i))).map(|r| r.updated).max();
    Ok(ThreadRule { ids, category, tab, updated: stamp(prev), removed })
}

/// A new tab called `name` (or the one already called that).
pub fn new_tab(store: &Store, name: &str) -> Result<Tab> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("give the tab a name"));
    }
    if name.chars().count() > 30 {
        return Err(anyhow!("keep the tab name under 30 characters"));
    }
    if crate::category::parse(name).is_some() {
        return Err(anyhow!("there's already a {} tab", crate::category::label(crate::category::parse(name).unwrap_or(0))));
    }
    if let Some(t) = store.all_tabs()?.into_iter().find(|t| !t.removed && t.name.eq_ignore_ascii_case(name)) {
        return Ok(t);
    }
    let now = Utc::now().timestamp();
    Ok(Tab { id: format!("{:08x}{:08x}", rand_u32(), rand_u32()), name: name.to_string(), created: now, updated: now, removed: false })
}

/// The tombstone that removes tab number `num`.
pub fn removed_tab(store: &Store, num: u8) -> Result<Tab> {
    let key = store.tab_key(num)?.ok_or_else(|| anyhow!("that tab doesn't exist"))?;
    let mut t = store.all_tabs()?.into_iter().find(|t| t.id == key).ok_or_else(|| anyhow!("that tab doesn't exist"))?;
    (t.updated, t.removed) = (stamp(Some(t.updated)), true);
    Ok(t)
}

/// A removed tab brought back.
pub fn restored_tab(tab: &Tab) -> Tab {
    Tab { updated: stamp(Some(tab.updated)), removed: false, ..tab.clone() }
}

/// A change to the inbox tabs, to keep here and on every server.
#[derive(Debug, Clone)]
pub enum TabChange {
    Tab(Tab),
    Sender(SenderRule),
    Thread(ThreadRule),
}

/// Keep a change here (at once) and in every account's server copy of the
/// state.
pub fn save_tab_change(store: &Store, change: &TabChange) -> Result<()> {
    match change {
        TabChange::Tab(t) => store.put_tab(t)?,
        TabChange::Sender(r) => store.put_sender_rule(r)?,
        TabChange::Thread(r) => store.put_thread_rule(r)?,
    }
    let mut err = None;
    for a in store.accounts()?.into_iter().filter(|a| !a.demo) {
        // A failure is caught up by the next sync (this machine's copy is newer).
        let r = update_state(store, &a, |st| match change {
            TabChange::Tab(t) => st.tabs.push(t.clone()),
            TabChange::Sender(r) => st.senders.push(r.clone()),
            TabChange::Thread(r) => st.threads.push(r.clone()),
        });
        if let Err(e) = r {
            log::warn!("couldn't save the tab change to {}: {e:#}", a.email);
            err = Some(e);
        }
    }
    match err {
        Some(e) => Err(e.context("Saved here; the other machines will get it at the next sync")),
        None => Ok(()),
    }
}

/// Always put mail from `email` in `category` (`None`: back to OwlPost's
/// guess), here and on every account's server copy of the state.
pub fn set_sender_rule(store: &Store, email: &str, category: Option<u8>) -> Result<()> {
    save_tab_change(store, &TabChange::Sender(sender_rule(store, email, category)?))
}

/// On Gmail, give every Inbox conversation in one of your tabs a label of
/// the same name, so it's grouped on the phone too.
pub fn label_tabs(store: &Store, account: &Account) -> Result<()> {
    if !account.is_gmail() {
        return Ok(());
    }
    let Some(inbox) = store.folder_by_role(account.id, FolderRole::Inbox)? else { return Ok(()) };
    for (num, name) in store.tabs()? {
        let threads = store.threads(&MailboxView::Folder(inbox.id), &format!("category:{num}"), 5000)?;
        if threads.is_empty() {
            continue;
        }
        let id = engine::create_folder(store, account, &name)?;
        let Some(label) = store.folder(id)? else { continue };
        for t in threads {
            if !store.thread_folder_ids(account.id, &t.thread_key)?.contains(&label.id) {
                engine::set_label(store, account, &t.thread_key, &label, true)?;
            }
        }
    }
    Ok(())
}

/// On Gmail, take a conversation moved out of tab `was` out of its label.
pub fn unlabel_tab(store: &Store, account: &Account, key: &str, was: u8) -> Result<()> {
    if !account.is_gmail() || !crate::category::is_tab(was) {
        return Ok(());
    }
    let name = store.category_name(was);
    if let Some(label) = store.folders(account.id)?.into_iter().find(|f| f.path.eq_ignore_ascii_case(&name)) {
        if store.thread_folder_ids(account.id, key)?.contains(&label.id) {
            engine::set_label(store, account, key, &label, false)?;
        }
    }
    Ok(())
}

/// Wake every snooze that's due, on every account. Returns how many woke.
pub fn wake_due(store: &Store) -> Result<usize> {
    let due: HashSet<AccountId> = store.due_snoozes()?.into_iter().map(|(a, _)| a).collect();
    let mut woke = 0;
    for a in store.accounts()?.into_iter().filter(|a| due.contains(&a.id)) {
        match sync_snoozes(store, &a) {
            Ok(n) => woke += n,
            Err(e) => log::warn!("waking snoozes for {} failed: {e:#}", a.email),
        }
    }
    Ok(woke)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn entry(i: &[&str], until: i64, updated: i64, removed: bool) -> Entry {
        Entry { ids: ids(i), until, updated, removed }
    }
    const NOW: i64 = 1_800_000_000;

    #[test]
    fn newer_change_wins_and_ids_pool() {
        let a = State { version: 1, entries: vec![entry(&["a"], NOW + 100, NOW - 50, false)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let b = State { version: 1, entries: vec![entry(&["a", "b"], 0, NOW - 10, true), entry(&["c"], NOW + 5, NOW - 5, false)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let m = State::merge([a.clone(), b.clone()], NOW);
        assert_eq!(m.entries.len(), 2);
        let first = m.entries.iter().find(|e| e.ids.contains(&"a".into())).unwrap();
        assert!(first.removed, "the later unsnooze wins");
        assert_eq!(first.ids, ids(&["a", "b"]));
        // Order of copies doesn't matter.
        assert_eq!(State::merge([b, a], NOW).entries.len(), 2);
    }

    #[test]
    fn sender_rules_merge_newest_wins() {
        let r = |e: &str, c: u8, at: i64, removed: bool| SenderRule { email: e.into(), category: c, tab: None, updated: at, removed };
        let a = State { version: 1, entries: vec![], senders: vec![r("shop@x.com", 1, NOW - 50, false), r("old@x.com", 0, NOW - 40 * 86_400, true)], ..Default::default() };
        let b = State { version: 1, entries: vec![], senders: vec![r("shop@x.com", 0, NOW - 10, true), r("li@linkedin.com", 2, NOW - 5, false)], ..Default::default() };
        let m = State::merge([a, b.clone()], NOW);
        assert_eq!(m.senders, vec![r("li@linkedin.com", 2, NOW - 5, false), r("shop@x.com", 0, NOW - 10, true)]);
        // A state without rules (an older OwlPost's) still reads.
        let raw = State { version: 1, entries: vec![], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] }.to_message(DateTime::from_timestamp(NOW, 0).unwrap());
        assert_eq!(State::from_message(&raw).unwrap().senders, vec![]);
        let back = State::from_message(&b.to_message(DateTime::from_timestamp(NOW, 0).unwrap())).unwrap();
        assert_eq!(back.senders, b.senders);
    }

    #[test]
    fn tabs_and_thread_rules_merge() {
        let tab = |id: &str, name: &str, at: i64, removed: bool| Tab { id: id.into(), name: name.into(), created: NOW - 100, updated: at, removed };
        let rule = |i: &[&str], tab: &str, at: i64| ThreadRule { ids: ids(i), category: 0, tab: Some(tab.into()), updated: at, removed: false };
        let a = State { tabs: vec![tab("t1", "Invoices", NOW - 50, false)], threads: vec![rule(&["m1"], "t1", NOW - 50)], ..Default::default() };
        let b = State {
            tabs: vec![tab("t1", "Invoices", NOW - 10, true), tab("t2", "Kids", NOW - 5, false)],
            threads: vec![rule(&["m2", "m1"], "t2", NOW - 20), rule(&["m9"], "t1", NOW - 30)],
            ..Default::default()
        };
        let m = State::merge([a.clone(), b.clone()], NOW);
        assert_eq!(m.tabs, vec![tab("t1", "Invoices", NOW - 10, true), tab("t2", "Kids", NOW - 5, false)]);
        // The newer rule for the conversation wins and its IDs are pooled.
        assert_eq!(m.threads, vec![rule(&["m9"], "t1", NOW - 30), rule(&["m1", "m2"], "t2", NOW - 20)]);
        assert_eq!(State::merge([b, a], NOW), m);
        // Merging again changes nothing (so sync doesn't rewrite the state).
        assert_eq!(State::merge([m.clone()], NOW), m);
    }

    #[test]
    fn pins_merge_newest_wins() {
        let pin = |i: &[&str], at: i64, removed: bool| Pin { ids: ids(i), updated: at, removed };
        let a = State { pins: vec![pin(&["m1"], NOW - 50, false), pin(&["m5"], NOW - 40 * 86_400, true)], ..Default::default() };
        let b = State { pins: vec![pin(&["m2", "m1"], NOW - 10, true), pin(&["m9"], NOW - 30, false)], ..Default::default() };
        let m = State::merge([a.clone(), b.clone()], NOW);
        // The later unpin wins, the IDs are pooled, the old tombstone goes.
        assert_eq!(m.pins, vec![pin(&["m9"], NOW - 30, false), pin(&["m1", "m2"], NOW - 10, true)]);
        assert_eq!(State::merge([b, a], NOW), m);
        assert_eq!(State::merge([m.clone()], NOW), m);
        // Through an email and back.
        assert_eq!(State::from_message(&m.to_message(DateTime::from_timestamp(NOW, 0).unwrap())).unwrap().pins, m.pins);
    }

    #[test]
    fn pins_follow_you_between_machines() {
        let machine = |name: &str| {
            let s = Store::open(&crate::test_db_path(name)).unwrap();
            crate::demo::seed(&s).unwrap();
            s
        };
        let (mac, laptop) = (machine("pins-mac"), machine("pins-laptop"));
        let inbox = |s: &Store| s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
        let t = inbox(&mac).into_iter().last().unwrap();
        let ids = thread_ids(&mac, &mac.account(t.account_id).unwrap().unwrap(), &t.thread_key).unwrap();
        let same = |s: &Store| s.thread_key_for_ids(t.account_id, &ids).unwrap().unwrap();
        let now = Utc::now().timestamp();

        // A pin made before pins were synced is uploaded.
        mac.set_pinned_here(t.account_id, &t.thread_key, true).unwrap();
        let (server, changed) = pool_pins(&mac, t.account_id, &[], now).unwrap();
        assert!(changed && server.len() == 1 && !server[0].removed);
        // The laptop picks it up, and has nothing new to say.
        let (back, changed) = pool_pins(&laptop, t.account_id, &server, now).unwrap();
        assert!(laptop.is_pinned(t.account_id, &same(&laptop)).unwrap());
        assert!(!changed && back == server);
        assert!(inbox(&laptop)[0].pinned);

        // Unpinned on the laptop (even in the same second): the Mac follows.
        laptop.unpin(t.account_id, &same(&laptop)).unwrap();
        let (server, changed) = pool_pins(&laptop, t.account_id, &server, now).unwrap();
        assert!(changed && server[0].removed);
        pool_pins(&mac, t.account_id, &server, now).unwrap();
        assert!(!mac.is_pinned(t.account_id, &t.thread_key).unwrap());
        // And the Mac's old pin doesn't come back on its next sync.
        let (again, changed) = pool_pins(&mac, t.account_id, &server, now).unwrap();
        assert!(!changed && again == server);
    }

    #[test]
    fn old_tombstones_are_dropped() {
        let s = State { version: 1, entries: vec![entry(&["a"], 0, NOW - 31 * 86_400, true), entry(&["b"], 0, NOW - 86_400, true)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        assert_eq!(State::merge([s], NOW).entries, vec![entry(&["b"], 0, NOW - 86_400, true)]);
    }

    #[test]
    fn state_round_trips_through_an_email() {
        let s = State { version: 1, entries: vec![entry(&["abc@example.com", "dé@x"], NOW, NOW, false)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let raw = s.to_message(DateTime::from_timestamp(NOW, 0).unwrap());
        assert_eq!(State::from_message(&raw), Some(s));
        assert!(String::from_utf8(raw).unwrap().contains(STATE_FROM));
        assert_eq!(State::from_message(b"From: a@b\r\nSubject: hi\r\n\r\nhello\r\n"), None, "ordinary mail isn't state");
    }

    fn local(snoozes: &[(&str, i64)], parked: &[&str], threads: &[(&str, &[&str])]) -> Local {
        Local {
            snoozes: snoozes.iter().map(|(k, u)| (k.to_string(), *u)).collect(),
            parked: parked.iter().map(|k| k.to_string()).collect(),
            ids: threads.iter().map(|(k, i)| (k.to_string(), ids(i))).collect(),
        }
    }

    #[test]
    fn picks_up_a_snooze_from_another_machine() {
        let st = State { version: 1, entries: vec![entry(&["m1"], NOW + 3600, NOW - 60, false)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let p = plan(st, &local(&[], &["t1"], &[("t1", &["m1"])]), NOW);
        assert_eq!(p.snooze, vec![("t1".into(), NOW + 3600)]);
        assert!(p.write.is_none() && p.wake.is_empty() && p.release.is_empty());
    }

    #[test]
    fn wakes_due_snoozes_once() {
        let st = State { version: 1, entries: vec![entry(&["m1"], NOW - 1, NOW - 600, false)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let p = plan(st, &local(&[("t1", NOW - 1)], &["t1"], &[("t1", &["m1"])]), NOW);
        assert_eq!(p.wake, vec!["t1".to_string()]);
        let w = p.write.expect("tombstone written");
        assert!(w.entries[0].removed);
        // The next machine sees the tombstone and does nothing more.
        let again = plan(w, &local(&[("t1", NOW - 1)], &[], &[("t1", &["m1"])]), NOW + 5);
        assert!(again.wake.is_empty());
        assert_eq!(again.unsnooze, vec!["t1".to_string()]);
    }

    #[test]
    fn cancelled_elsewhere_is_dropped_here() {
        let st = State { version: 1, entries: vec![entry(&["m1"], NOW + 3600, NOW - 5, true)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let p = plan(st, &local(&[("t1", NOW + 3600)], &["t1"], &[("t1", &["m1"])]), NOW);
        assert_eq!(p.unsnooze, vec!["t1".to_string()]);
        assert_eq!(p.release, vec!["t1".to_string()]);
        assert!(p.write.is_none());
    }

    #[test]
    fn local_snoozes_are_uploaded_and_parked() {
        let p = plan(State::default(), &local(&[("t1", NOW + 3600)], &[], &[("t1", &["m1", "m2"])]), NOW);
        assert_eq!(p.park, vec!["t1".to_string()]);
        let w = p.write.unwrap();
        assert_eq!(w.entries, vec![entry(&["m1", "m2"], NOW + 3600, NOW, false)]);
    }

    #[test]
    fn taken_out_of_the_folder_elsewhere_means_unsnoozed() {
        // Snoozed an hour ago, but no longer in the Snoozed folder.
        let st = State { version: 1, entries: vec![entry(&["m1"], NOW + 3600, NOW - 3600, false)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let p = plan(st.clone(), &local(&[("t1", NOW + 3600)], &[], &[("t1", &["m1"])]), NOW);
        assert_eq!(p.unsnooze, vec!["t1".to_string()]);
        assert!(p.write.unwrap().entries[0].removed);
        // Just snoozed (the move may still be on its way): leave it.
        let fresh = State { version: 1, entries: vec![entry(&["m1"], NOW + 3600, NOW - 30, false)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let p = plan(fresh, &local(&[("t1", NOW + 3600)], &[], &[("t1", &["m1"])]), NOW);
        assert!(p.unsnooze.is_empty() && p.write.is_none());
    }

    #[test]
    fn snoozes_of_deleted_mail_are_forgotten() {
        let p = plan(State::default(), &local(&[("gone", NOW - 5)], &[], &[]), NOW);
        assert_eq!(p.unsnooze, vec!["gone".to_string()]);
        assert!(p.wake.is_empty() && p.write.is_none());
    }

    #[test]
    fn strays_in_the_folder_go_back_to_the_inbox() {
        let p = plan(State::default(), &local(&[], &["t9"], &[("t9", &["m9"])]), NOW);
        assert_eq!(p.release, vec!["t9".to_string()]);
    }

    #[test]
    fn unknown_mail_is_kept_until_long_overdue() {
        let st = State { version: 1, entries: vec![entry(&["x"], NOW + 60, NOW - 9000, false), entry(&["y"], NOW - 2 * 86_400, NOW - 9000, false)], senders: vec![], tabs: vec![], threads: vec![], pins: vec![] };
        let p = plan(st, &Local::default(), NOW);
        let w = p.write.unwrap();
        assert!(!w.entries.iter().find(|e| e.ids == ids(&["x"])).unwrap().removed);
        assert!(w.entries.iter().find(|e| e.ids == ids(&["y"])).unwrap().removed);
    }
}
