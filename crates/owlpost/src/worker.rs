//! Background threads: periodic IMAP sync, snooze wake-ups and server actions,
//! so the UI thread never blocks on the network.

use crossbeam_channel::{Receiver, Sender};
use owlpost_core::engine::{self, SyncProgress};
use owlpost_core::smtp::OutgoingMessage;
use owlpost_core::{credentials, Account, AccountId, Auth, FolderId, FolderRole, Message, MessageId, Store};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Cmd {
    SyncAll,
    SyncAccount(AccountId),
    SetUnread(AccountId, String, bool),
    SetStarred(AccountId, String, bool),
    /// Save a change to the inbox tabs to every server; with a conversation
    /// that moved (and the tab it left), fix its Gmail labels.
    TabChange(owlpost_core::snooze_sync::TabChange, Option<(AccountId, String, u8)>),
    Move(AccountId, String, FolderRole),
    /// Move a thread into a folder, optionally only out of the one being viewed.
    MoveTo(AccountId, String, FolderId, Option<FolderId>),
    /// Add (true) or remove a label.
    Label(AccountId, String, FolderId, bool),
    /// Create a folder/label, then optionally move or label a thread into it.
    CreateFolder(AccountId, String, Option<(Vec<String>, FolderUse)>),
    /// Send; the composer's saved draft (if any) is then deleted.
    Send(AccountId, OutgoingMessage, DraftRef),
    /// Save to the Drafts folder, replacing the composer's previous draft.
    /// `true`: the window is closing (say so when it's saved).
    SaveDraft(AccountId, OutgoingMessage, DraftRef, bool),
    /// Fetch a draft's Bcc and attachments so it can be edited.
    OpenDraft(Box<Message>),
    /// Delete a composer's saved draft (Discard).
    DeleteDraft(DraftRef),
    /// Fetch a message's raw source for "Show original".
    ShowOriginal(Box<Message>),
    /// Verify credentials, save the password to the keychain, then add.
    AddAccount(Box<Account>, String),
    /// Download attachment `usize` of a message (or use the cached copy),
    /// then open or save it.
    Attachment(Box<Message>, usize, AttachmentDo),
    /// "Sign in with Google": opens the browser, waits for it, then adds the
    /// account or switches an existing one to Google sign-in. Setting the
    /// flag cancels.
    GoogleSignIn(Option<AccountId>, Arc<AtomicBool>),
    /// Undo a move: bring a thread back from the folder it was moved to
    /// into the folders it came from.
    Restore(AccountId, String, FolderId, Vec<FolderId>),
    /// Snooze a conversation on the server too (moves it into the Snoozed
    /// folder and records the wake time for other machines).
    Snooze(AccountId, String, chrono::DateTime<chrono::Utc>),
    /// Cancel a snooze everywhere and return the conversation to the Inbox.
    Unsnooze(AccountId, String),
    /// Is a Google OAuth client saved? (Read off the UI thread: the keychain
    /// may wait on a permission prompt.)
    CheckGoogle,
}

/// Which draft a composer's commands act on. The worker remembers each
/// composer's latest saved draft, so saves that finish late are still
/// replaced or deleted; `initial` is the draft the window was opened from.
#[derive(Clone, Debug)]
pub struct DraftRef {
    pub composer: u64,
    pub initial: Option<Box<Message>>,
}

/// Each composer's newest saved draft (by composer id).
static DRAFTS: std::sync::Mutex<Option<HashMap<u64, Message>>> = std::sync::Mutex::new(None);

/// The draft a composer last saved, or the one it was opened from.
pub fn latest_draft(r: &DraftRef) -> Option<Message> {
    DRAFTS.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&r.composer).cloned())).or_else(|| r.initial.as_deref().cloned())
}

fn remember_draft(composer: u64, m: Option<Message>) {
    if let Ok(mut g) = DRAFTS.lock() {
        let map = g.get_or_insert_with(HashMap::new);
        match m {
            Some(m) => {
                map.insert(composer, m);
            }
            None => {
                map.remove(&composer);
            }
        }
    }
}

impl Cmd {
    /// The conversation a command changes: commands on the same one run in
    /// the order they were sent (so an undo never overtakes its action).
    fn thread(&self) -> Option<(AccountId, String)> {
        match self {
            Cmd::SetUnread(a, k, _) | Cmd::SetStarred(a, k, _) | Cmd::Move(a, k, _) | Cmd::MoveTo(a, k, _, _) | Cmd::Label(a, k, _, _) | Cmd::Restore(a, k, _, _) | Cmd::Snooze(a, k, _) | Cmd::Unsnooze(a, k) => Some((*a, k.clone())),
            Cmd::CreateFolder(a, _, Some((ks, _))) => ks.first().map(|k| (*a, k.clone())),
            // A composer's saves, send and discard run one after another.
            Cmd::Send(_, _, r) | Cmd::SaveDraft(_, _, r, _) | Cmd::DeleteDraft(r) => Some((0, format!("composer:{}", r.composer))),
            _ => None,
        }
    }
}

/// Runs actions on the same conversation one after another, in order.
#[derive(Default)]
struct Turns {
    /// Per conversation: (next turn to run, next ticket to hand out).
    state: std::sync::Mutex<HashMap<(AccountId, String), (u64, u64)>>,
    cv: std::sync::Condvar,
}

impl Turns {
    fn ticket(&self, key: &(AccountId, String)) -> u64 {
        let mut st = self.state.lock().unwrap();
        let e = st.entry(key.clone()).or_insert((0, 0));
        e.1 += 1;
        e.1 - 1
    }

    fn wait(&self, key: &(AccountId, String), ticket: u64) {
        let mut st = self.state.lock().unwrap();
        while st.get(key).map(|e| e.0) != Some(ticket) {
            st = self.cv.wait(st).unwrap();
        }
    }

    fn done(&self, key: &(AccountId, String)) {
        let mut st = self.state.lock().unwrap();
        if let Some(e) = st.get_mut(key) {
            e.0 += 1;
            if e.0 == e.1 {
                st.remove(key);
            }
        }
        self.cv.notify_all();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentDo {
    /// Open in the default app (Preview, Pages…).
    Open,
    /// Copy into Downloads (without overwriting) and show it in Finder.
    SaveToDownloads,
    /// Save to a path chosen in a Save dialog.
    SaveTo(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderUse {
    Move,
    Label,
}

pub enum Event {
    Status(String),
    SyncFinished { new: usize, errors: Vec<String> },
    /// Local data changed; reload lists.
    Changed,
    Error(String),
    Sent(String),
    AccountAdded(AccountId),
    AccountFailed(String),
    SnoozesWoke(usize),
    /// Something finished that's worth a (non-error) toast.
    Notice(String),
    /// An attachment download finished (successfully or not).
    AttachmentDone(MessageId, usize),
    Push(AccountId, PushState),
    /// Google's sign-in page is open in the browser (the URL, to copy).
    GoogleWaiting(String),
    /// Google sign-in finished: Ok(message) or Err(reason).
    GoogleDone(Result<String, String>),
    GoogleReady(bool),
    /// A composer's draft was saved (its new copy), or saving failed.
    DraftSaved(u64, Result<Box<Message>, String>),
    /// A message's raw source: (subject, text).
    Original(String, Result<String, String>),
    /// A draft is ready to edit (or couldn't be opened).
    DraftReady(Box<Message>, Result<owlpost_core::engine::DraftParts, String>),
    /// A sender's picture (by lowercase address), or `None`: use the letter.
    Avatar(String, Option<crate::avatars::Picture>),
}

/// A request for the sync thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncReq {
    /// Every account, every folder (the timer, Refresh, waking from sleep).
    All,
    /// One account, every folder.
    Account(AccountId),
    /// One account's Inbox: the server pushed a change.
    Inbox(AccountId),
}

/// Requests merged into one round of syncing.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    pub all: bool,
    pub accounts: std::collections::BTreeSet<AccountId>,
    pub inboxes: std::collections::BTreeSet<AccountId>,
}

impl From<SyncReq> for SyncPlan {
    fn from(r: SyncReq) -> Self {
        let mut p = SyncPlan::default();
        p.add(r);
        p
    }
}

impl SyncPlan {
    pub fn add(&mut self, r: SyncReq) {
        match r {
            SyncReq::All => self.all = true,
            SyncReq::Account(a) => {
                self.accounts.insert(a);
            }
            SyncReq::Inbox(a) => {
                self.inboxes.insert(a);
            }
        }
    }

    /// This account gets a full sync (which includes its Inbox).
    pub fn covers_fully(&self, a: AccountId) -> bool {
        self.all || self.accounts.contains(&a)
    }
}

/// Push connection state for one account, shown in Preferences > Accounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushState {
    Connecting,
    /// Connected and waiting: new mail arrives within seconds.
    Live,
    /// Lost the connection; trying again (checking on the timer meanwhile).
    Retrying(String),
    /// This server or account can't push; checking every 5 minutes.
    Unavailable(String),
}

const SYNC_EVERY: Duration = Duration::from_secs(5 * 60);
const TICK: Duration = Duration::from_secs(20);

pub struct Worker {
    pub tx: Sender<Cmd>,
    pub rx: Receiver<Event>,
    /// Addresses to find a picture for (Contacts, BIMI, site icon).
    pub avatars: Sender<String>,
}

impl Worker {
    pub fn spawn(db: PathBuf, ctx: eframe::egui::Context) -> Self {
        let (tx, cmd_rx) = crossbeam_channel::unbounded::<Cmd>();
        let (ev_tx, rx) = crossbeam_channel::unbounded::<Event>();
        let (sync_tx, sync_rx) = crossbeam_channel::unbounded::<SyncReq>();

        // Sender pictures: a few lookups at a time, so a screenful of new
        // senders fills in quickly without flooding the network.
        let (avatars, avatar_rx) = crossbeam_channel::unbounded::<String>();
        let cache = crate::avatars::cache_dir();
        for i in 0..4 {
            let (rx, ev, ctx, cache) = (avatar_rx.clone(), ev_tx.clone(), ctx.clone(), cache.clone());
            std::thread::Builder::new()
                .name(format!("avatars-{i}"))
                .spawn(move || {
                    for email in rx {
                        let pic = crate::avatars::find(&email, &cache);
                        let _ = ev.send(Event::Avatar(email, pic));
                        ctx.request_repaint();
                    }
                })
                .expect("spawn avatar thread");
        }

        // Sync thread: one sync at a time, coalescing requests.
        {
            let db = db.clone();
            let ev = ev_tx.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name("sync".into())
                .spawn(move || {
                    let store = match Store::open(&db) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = ev.send(Event::Error(format!("{e:#}")));
                            return;
                        }
                    };
                    let mut last = Instant::now() - SYNC_EVERY;
                    loop {
                        let req = match sync_rx.recv_timeout(TICK) {
                            Ok(r) => Some(r),
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                            Err(_) => return,
                        };
                        if store.due_snoozes().is_ok_and(|d| !d.is_empty()) {
                            if let Ok(woke) = owlpost_core::snooze_sync::wake_due(&store) {
                                if woke > 0 {
                                    let _ = ev.send(Event::SnoozesWoke(woke));
                                    ctx.request_repaint();
                                }
                            }
                        }
                        let mut plan = match req {
                            Some(r) => SyncPlan::from(r),
                            None if last.elapsed() >= SYNC_EVERY => SyncPlan::from(SyncReq::All),
                            None => continue,
                        };
                        // Merge whatever else queued up meanwhile.
                        while let Ok(r) = sync_rx.try_recv() {
                            plan.add(r);
                        }
                        let all_accounts: Vec<Account> = store.accounts().unwrap_or_default().into_iter().filter(|a| !a.demo).collect();
                        // Pushed changes: just the Inbox, quietly.
                        for a in all_accounts.iter().filter(|a| plan.inboxes.contains(&a.id) && !plan.covers_fully(a.id)) {
                            match engine::sync_inbox(&store, a) {
                                Ok(_) => {
                                    let _ = ev.send(Event::Changed);
                                    ctx.request_repaint();
                                }
                                Err(e) => log::warn!("push sync of {} failed: {e:#}", a.email),
                            }
                        }
                        if !plan.all && plan.accounts.is_empty() {
                            continue;
                        }
                        if plan.all {
                            last = Instant::now();
                        }
                        let accounts: Vec<Account> = all_accounts.into_iter().filter(|a| plan.covers_fully(a.id)).collect();
                        let mut new = 0;
                        let mut errors = vec![];
                        for a in accounts {
                            let ev2 = ev.clone();
                            let ctx2 = ctx.clone();
                            let r = engine::sync_account(&store, &a, |p| {
                                let msg = match p {
                                    SyncProgress::Folder { name, done, total, .. } => {
                                        format!("Syncing {} — {name} ({}/{total})", a.email, done + 1)
                                    }
                                    SyncProgress::NewMessages { .. } => {
                                        let _ = ev2.send(Event::Changed);
                                        return;
                                    }
                                };
                                let _ = ev2.send(Event::Status(msg));
                                ctx2.request_repaint();
                            });
                            match r {
                                Ok(n) => new += n,
                                Err(e) => errors.push(format!("{}: {e:#}", a.email)),
                            }
                        }
                        let _ = ev.send(Event::SyncFinished { new, errors });
                        ctx.request_repaint();
                    }
                })
                .expect("spawn sync thread");
        }

        // Push: an IMAP IDLE connection per account, kept alive by a supervisor.
        push::spawn(db.clone(), sync_tx.clone(), ev_tx.clone(), ctx.clone());

        // Command dispatcher: each action runs on its own short-lived thread.
        std::thread::Builder::new()
            .name("actions".into())
            .spawn(move || {
                let turns = Arc::new(Turns::default());
                for cmd in cmd_rx {
                    match cmd {
                        Cmd::SyncAll => {
                            let _ = sync_tx.send(SyncReq::All);
                        }
                        Cmd::SyncAccount(id) => {
                            let _ = sync_tx.send(SyncReq::Account(id));
                        }
                        other => {
                            let db = db.clone();
                            let ev = ev_tx.clone();
                            let ctx = ctx.clone();
                            let sync_tx = sync_tx.clone();
                            // Take a turn now, in send order.
                            let turn = other.thread().map(|k| {
                                let t = turns.ticket(&k);
                                (k, t)
                            });
                            let turns = turns.clone();
                            std::thread::spawn(move || {
                                if let Some((k, t)) = &turn {
                                    turns.wait(k, *t);
                                }
                                run_action(&db, other, &ev, &sync_tx);
                                if let Some((k, _)) = &turn {
                                    turns.done(k);
                                }
                                ctx.request_repaint();
                            });
                        }
                    }
                }
            })
            .expect("spawn action thread");

        Self { tx, rx, avatars }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

/// The whole "Sign in with Google" round trip. Returns what to tell the user.
fn google_sign_in(store: &Store, target: Option<AccountId>, cancel: &AtomicBool, ev: &Sender<Event>, sync_tx: &Sender<SyncReq>) -> anyhow::Result<String> {
    use owlpost_core::oauth;
    use owlpost_core::presets::Provider;
    let client = oauth::GoogleClient::load().ok_or_else(|| anyhow::anyhow!("add your Google client ID in Preferences > Accounts first"))?;
    let existing = match target {
        Some(id) => Some(store.account(id)?.ok_or_else(|| anyhow::anyhow!("that account no longer exists"))?),
        None => None,
    };
    let pending = oauth::begin(&client, existing.as_ref().map(|a| a.email.as_str()))?;
    let _ = ev.send(Event::GoogleWaiting(pending.url.clone()));
    open_url(&pending.url)?;
    let signed = pending.finish(&client, cancel, oauth::SIGN_IN_TIMEOUT)?;
    // Re-signing in an account must be the same Google account.
    if let Some(a) = &existing {
        if !a.email.eq_ignore_ascii_case(&signed.email) {
            anyhow::bail!("you signed in as {}, but this account is {} — try again and pick {}", signed.email, a.email, a.email);
        }
    }
    oauth::store_sign_in(&signed)?;
    let account = existing.or(store.account_by_email(&signed.email)?);
    match account {
        Some(mut a) => {
            let was = a.auth;
            a.auth = Auth::Google;
            store.update_account(&a)?;
            if was == Auth::Password {
                // The app password isn't needed any more.
                credentials::forget_password(&a.email);
            }
            let _ = sync_tx.send(SyncReq::Account(a.id));
            Ok(format!("{} now signs in with Google", a.email))
        }
        None => {
            let p = Provider::Gmail;
            let acc = Account {
                id: 0,
                display_name: if signed.name.is_empty() { signed.email.split('@').next().unwrap_or("").to_string() } else { signed.name.clone() },
                email: signed.email.clone(),
                username: signed.email.clone(),
                imap: p.imap().expect("gmail preset"),
                smtp: p.smtp().expect("gmail preset"),
                color: p.color().into(),
                signature: String::new(),
                demo: false,
                auth: Auth::Google,
            };
            let mut session = engine::connect(&acc).map_err(|e| anyhow::anyhow!("signed in, but Gmail refused the connection — is IMAP turned on for this account? ({e:#})"))?;
            let _ = session.logout();
            let id = store.add_account(&acc)?;
            let _ = ev.send(Event::AccountAdded(id));
            let _ = sync_tx.send(SyncReq::Account(id));
            // AccountAdded already announces it.
            Ok(String::new())
        }
    }
}

fn open_url(url: &str) -> anyhow::Result<()> {
    use std::process::Command;
    if cfg!(test) {
        return Ok(());
    }
    let status = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).status()
    } else if cfg!(target_os = "windows") {
        Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).status()
    } else {
        Command::new("xdg-open").arg(url).status()
    }?;
    if !status.success() {
        anyhow::bail!("couldn't open the browser");
    }
    Ok(())
}

/// Fetch (or reuse) an attachment, then do `what` with it. Returns a
/// message to show when there's something to say.
fn attachment(store: &Store, m: &Message, index: usize, what: AttachmentDo) -> anyhow::Result<Option<String>> {
    let account = store.account(m.account_id)?.ok_or_else(|| anyhow::anyhow!("the account no longer exists"))?;
    let path = engine::attachment_file(store, &account, m, index)?;
    match what {
        AttachmentDo::Open => {
            open_file(&path, false)?;
            Ok(None)
        }
        AttachmentDo::SaveTo(dest) => {
            std::fs::copy(&path, &dest)?;
            Ok(Some(format!("Saved {}", dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())))
        }
        AttachmentDo::SaveToDownloads => {
            let dir = directories::UserDirs::new()
                .and_then(|u| u.download_dir().map(|d| d.to_path_buf()))
                .ok_or_else(|| anyhow::anyhow!("couldn't find your Downloads folder"))?;
            let dest = unused_path(&dir, &engine::safe_filename(&m.attachments[index].filename));
            std::fs::copy(&path, &dest)?;
            let _ = open_file(&dest, true);
            Ok(Some(format!("Saved {} to Downloads", dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())))
        }
    }
}

/// `dir/name`, or `dir/name (2).ext`… if that's taken: never overwrite.
pub fn unused_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).expect("some name is free")
}

/// Open a file in its default app, or (`reveal`) show it in Finder / the file manager.
fn open_file(path: &std::path::Path, reveal: bool) -> anyhow::Result<()> {
    use std::process::Command;
    if cfg!(test) {
        // Tests check the download, not that Preview pops up.
        return if path.is_file() { Ok(()) } else { Err(anyhow::anyhow!("missing {}", path.display())) };
    }
    let status = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        if reveal {
            c.arg("-R");
        }
        c.arg(path).status()
    } else if cfg!(target_os = "windows") {
        if reveal {
            Command::new("explorer").arg(format!("/select,{}", path.display())).status()
        } else {
            Command::new("cmd").args(["/C", "start", ""]).arg(path).status()
        }
    } else {
        let target = if reveal { path.parent().unwrap_or(path) } else { path };
        Command::new("xdg-open").arg(target).status()
    }?;
    if !status.success() && !cfg!(target_os = "windows") {
        anyhow::bail!("no app is set up to open this kind of file");
    }
    Ok(())
}

fn run_action(db: &std::path::Path, cmd: Cmd, ev: &Sender<Event>, sync_tx: &Sender<SyncReq>) {
    let store = match Store::open(db) {
        Ok(s) => s,
        Err(e) => {
            let _ = ev.send(Event::Error(format!("{e:#}")));
            return;
        }
    };
    let account = |id: AccountId| -> anyhow::Result<Account> {
        store.account(id)?.ok_or_else(|| anyhow::anyhow!("account {id} no longer exists"))
    };
    let result: anyhow::Result<()> = (|| {
        match cmd {
            Cmd::Attachment(m, index, what) => {
                let name = m.attachments.get(index).map(|a| a.filename.clone()).unwrap_or_default();
                let r = attachment(&store, &m, index, what);
                let _ = ev.send(Event::AttachmentDone(m.id, index));
                match r {
                    Ok(Some(notice)) => {
                        let _ = ev.send(Event::Notice(notice));
                    }
                    Ok(None) => {}
                    Err(e) => return Err(e.context(format!("Couldn't open {name}"))),
                }
            }
            Cmd::SetUnread(a, key, unread) => engine::set_thread_unread(&store, &account(a)?, &key, unread)?,
            Cmd::SetStarred(a, key, starred) => engine::set_thread_starred(&store, &account(a)?, &key, starred)?,
            Cmd::TabChange(change, moved) => {
                let r = owlpost_core::snooze_sync::save_tab_change(&store, &change);
                if let Some((a, key, was)) = moved {
                    let acc = account(a)?;
                    if !acc.demo {
                        owlpost_core::snooze_sync::unlabel_tab(&store, &acc, &key, was)?;
                        owlpost_core::snooze_sync::label_tabs(&store, &acc)?;
                    }
                }
                r?
            }
            Cmd::Snooze(a, key, until) => {
                // On failure the snooze is undone, so the conversation reappears.
                let r = owlpost_core::snooze_sync::snooze_thread(&store, &account(a)?, &key, until);
                let _ = ev.send(Event::Changed);
                r.map_err(|e| e.context("Couldn't snooze on the server"))?;
            }
            Cmd::Unsnooze(a, key) => {
                let r = owlpost_core::snooze_sync::unsnooze_thread(&store, &account(a)?, &key);
                let _ = ev.send(Event::Changed);
                r?;
            }
            Cmd::Move(a, key, role) => {
                engine::move_thread(&store, &account(a)?, &key, role)?;
                let _ = ev.send(Event::Changed);
            }
            Cmd::Restore(a, key, from, to) => {
                let acc = account(a)?;
                let from = store.folder(from)?.ok_or_else(|| anyhow::anyhow!("that folder no longer exists"))?;
                for id in to {
                    if let Some(f) = store.folder(id)? {
                        engine::restore_thread(&store, &acc, &key, &f, &from)?;
                    }
                }
                let _ = ev.send(Event::Changed);
            }
            Cmd::MoveTo(a, key, folder, from) => {
                let f = store.folder(folder)?.ok_or_else(|| anyhow::anyhow!("that folder no longer exists"))?;
                engine::move_thread_to(&store, &account(a)?, &key, &f, from)?;
                let _ = ev.send(Event::Changed);
            }
            Cmd::Label(a, key, folder, add) => {
                let f = store.folder(folder)?.ok_or_else(|| anyhow::anyhow!("that label no longer exists"))?;
                engine::set_label(&store, &account(a)?, &key, &f, add)?;
                let _ = ev.send(Event::Changed);
            }
            Cmd::CreateFolder(a, name, then) => {
                let acc = account(a)?;
                let id = engine::create_folder(&store, &acc, &name)?;
                let f = store.folder(id)?.ok_or_else(|| anyhow::anyhow!("folder vanished"))?;
                if let Some((keys, then)) = then {
                    for key in keys {
                        match then {
                            FolderUse::Move => engine::move_thread_to(&store, &acc, &key, &f, None)?,
                            FolderUse::Label => engine::set_label(&store, &acc, &key, &f, true)?,
                        }
                    }
                }
                let _ = ev.send(Event::Changed);
            }
            Cmd::Send(a, out, draft) => {
                let _ = ev.send(Event::Status("Sending…".into()));
                let acc = account(a)?;
                match engine::send(&store, &acc, &out) {
                    Ok(_) => {
                        let _ = ev.send(Event::Sent(format!("Message sent from {}", acc.email)));
                        if let Some(d) = latest_draft(&draft) {
                            if let Err(e) = account(d.account_id).and_then(|da| engine::delete_draft(&store, &da, &d)) {
                                let _ = ev.send(Event::Error(format!("Sent, but the draft couldn't be removed: {e:#}")));
                            }
                        }
                        remember_draft(draft.composer, None);
                        let _ = ev.send(Event::Changed);
                    }
                    Err(e) => {
                        let _ = ev.send(Event::Error(format!("Sending failed: {e:#}")));
                    }
                }
            }
            Cmd::SaveDraft(a, out, draft, closing) => {
                let acc = account(a)?;
                let replace = latest_draft(&draft);
                // A draft moved to another account: save it there, then
                // delete the old one from its own account.
                let same = replace.as_ref().is_some_and(|d| d.account_id == a);
                let r = engine::save_draft(&store, &acc, &out, if same { replace.as_ref() } else { None });
                if let (Ok(_), Some(d), false) = (&r, &replace, same) {
                    let _ = account(d.account_id).and_then(|da| engine::delete_draft(&store, &da, d));
                }
                let saved = r.as_ref().map_err(|e| format!("{e:#}")).and_then(|id| {
                    store.copy_in_role(a, id, FolderRole::Drafts).ok().flatten().ok_or_else(|| "the saved draft didn't sync back".to_string())
                });
                if let Ok(m) = &saved {
                    remember_draft(draft.composer, Some(m.clone()));
                }
                let _ = ev.send(Event::DraftSaved(draft.composer, saved.map(Box::new)));
                let _ = ev.send(Event::Changed);
                r.map_err(|e| e.context("Couldn't save the draft"))?;
                if closing {
                    let _ = ev.send(Event::Notice(format!("Draft saved to {}", acc.email)));
                }
            }
            Cmd::DeleteDraft(draft) => {
                let r = match latest_draft(&draft) {
                    Some(m) => account(m.account_id).and_then(|acc| engine::delete_draft(&store, &acc, &m)),
                    None => Ok(()),
                };
                remember_draft(draft.composer, None);
                let _ = ev.send(Event::Changed);
                r.map_err(|e| e.context("Couldn't delete the draft"))?;
            }
            Cmd::ShowOriginal(m) => {
                let r = account(m.account_id).and_then(|acc| {
                    if acc.demo {
                        anyhow::bail!("demo messages have no original on a server");
                    }
                    engine::fetch_raw(&store, &acc, &m)
                });
                let _ = ev.send(Event::Original(m.subject.clone(), r.map(|raw| String::from_utf8_lossy(&raw).into_owned()).map_err(|e| format!("{e:#}"))));
            }
            Cmd::OpenDraft(m) => {
                let r = account(m.account_id).and_then(|acc| engine::open_draft(&store, &acc, &m)).map_err(|e| format!("Couldn't open the draft: {e:#}"));
                let _ = ev.send(Event::DraftReady(m, r));
            }
            Cmd::AddAccount(acc, password) => {
                let _ = ev.send(Event::Status(format!("Connecting to {}…", acc.imap.host)));
                if let Err(e) = engine::test_login(&acc, &password) {
                    let _ = ev.send(Event::AccountFailed(format!("{e:#}")));
                    return Ok(());
                }
                if let Err(e) = credentials::set_password(&acc.email, &password) {
                    let _ = ev.send(Event::AccountFailed(format!("{e:#}")));
                    return Ok(());
                }
                match store.add_account(&acc) {
                    Ok(id) => {
                        let _ = ev.send(Event::AccountAdded(id));
                        let _ = sync_tx.send(SyncReq::Account(id));
                    }
                    Err(e) => {
                        let _ = ev.send(Event::AccountFailed(format!("{e:#}")));
                    }
                }
            }
            Cmd::CheckGoogle => {
                // Debug builds can skip it, so test runs never prompt for keychain access.
                let skip = cfg!(debug_assertions) && std::env::var_os("OWLPOST_NO_KEYCHAIN").is_some();
                let _ = ev.send(Event::GoogleReady(!skip && owlpost_core::oauth::GoogleClient::load().is_some()));
            }
            Cmd::GoogleSignIn(target, cancel) => {
                let r = google_sign_in(&store, target, &cancel, ev, sync_tx);
                let _ = ev.send(Event::GoogleDone(r.map_err(|e| format!("{e:#}"))));
            }
            Cmd::SyncAll | Cmd::SyncAccount(_) => unreachable!(),
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = ev.send(Event::Error(format!("{e:#}")));
        let _ = ev.send(Event::Changed);
    }
}

/// Push mail: one IDLE thread per account, watched by a supervisor that
/// starts threads for new accounts, stops them for removed ones, replaces
/// any that stop responding, and restarts everything after the Mac sleeps.
mod push {
    use super::{Event, PushState, SyncReq};
    use crossbeam_channel::Sender;
    use owlpost_core::idle::{self, IdleEvent};
    use owlpost_core::{AccountId, FolderRole, Store};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime};

    const CHECK: Duration = Duration::from_secs(30);
    /// A thread that hasn't been heard from for this long is replaced.
    const STALE: Duration = Duration::from_secs(idle::CYCLE.as_secs() + 3 * 60);

    #[derive(Clone)]
    struct Watch {
        generation: u64,
        beat: Instant,
        /// The server can't push: don't keep retrying.
        unsupported: bool,
    }

    type Watches = Arc<Mutex<HashMap<AccountId, Watch>>>;

    pub fn spawn(db: PathBuf, sync_tx: Sender<SyncReq>, ev: Sender<Event>, ctx: eframe::egui::Context) {
        let watches: Watches = Default::default();
        let notify = move |ev: &Sender<Event>, id: AccountId, st: PushState| {
            let _ = ev.send(Event::Push(id, st));
            ctx.request_repaint();
        };
        let _ = std::thread::Builder::new().name("push".into()).spawn(move || {
            let Ok(store) = Store::open(&db) else { return };
            let mut next_gen = 1u64;
            let mut told_no_push = std::collections::HashSet::new();
            let mut last_wall = SystemTime::now();
            loop {
                // A big jump in the wall clock means the Mac slept: every
                // connection is probably dead, so start over and catch up.
                let now = SystemTime::now();
                let slept = now.duration_since(last_wall).map(|d| d > CHECK * 3).unwrap_or(false);
                last_wall = now;
                if slept {
                    log::info!("woke from sleep: reconnecting push");
                    watches.lock().unwrap().clear();
                    let _ = sync_tx.send(SyncReq::All);
                }
                let accounts = store.accounts().unwrap_or_default();
                {
                    let mut w = watches.lock().unwrap();
                    // Removed accounts: forgetting them stops their threads.
                    w.retain(|id, _| accounts.iter().any(|a| a.id == *id));
                }
                for a in accounts.iter().filter(|a| !a.demo) {
                    if !idle::can_push(a) {
                        if told_no_push.insert(a.id) {
                            notify(&ev, a.id, PushState::Unavailable("this account connects with STARTTLS rather than TLS".into()));
                        }
                        continue;
                    }
                    let start = {
                        let mut w = watches.lock().unwrap();
                        match w.get(&a.id) {
                            Some(x) if x.unsupported || x.beat.elapsed() < STALE => None,
                            _ => {
                                next_gen += 1;
                                w.insert(a.id, Watch { generation: next_gen, beat: Instant::now(), unsupported: false });
                                Some(next_gen)
                            }
                        }
                    };
                    if let Some(generation) = start {
                        let (db, sync_tx, ev, watches, notify) = (db.clone(), sync_tx.clone(), ev.clone(), watches.clone(), notify.clone());
                        let id = a.id;
                        let _ = std::thread::Builder::new()
                            .name(format!("push-{id}"))
                            .spawn(move || run(&db, id, generation, &watches, &sync_tx, &|st| notify(&ev, id, st)));
                    }
                }
                std::thread::sleep(CHECK);
            }
        });
    }

    /// This thread is still the current one for the account; also records
    /// that it's alive.
    fn current(watches: &Watches, id: AccountId, generation: u64) -> bool {
        let mut w = watches.lock().unwrap();
        match w.get_mut(&id) {
            Some(x) if x.generation == generation => {
                x.beat = Instant::now();
                true
            }
            _ => false,
        }
    }

    fn run(db: &std::path::Path, id: AccountId, generation: u64, watches: &Watches, sync_tx: &Sender<SyncReq>, notify: &dyn Fn(PushState)) {
        let Ok(store) = Store::open(db) else { return };
        let mut failures = 0u32;
        while current(watches, id, generation) {
            let Ok(Some(account)) = store.account(id) else { return };
            // Folders come from the first full sync; wait for it.
            let inbox = match store.folder_by_role(id, FolderRole::Inbox) {
                Ok(Some(f)) => f.path,
                _ => {
                    std::thread::sleep(Duration::from_secs(10));
                    continue;
                }
            };
            notify(PushState::Connecting);
            let result = idle::connect(&account).and_then(|mut session| {
                if !idle::supports_idle(&mut session)? {
                    return Err(anyhow::anyhow!(Unsupported));
                }
                notify(PushState::Live);
                failures = 0;
                // Catch up on anything that arrived while disconnected.
                let _ = sync_tx.send(SyncReq::Inbox(id));
                let r = idle::watch(&mut session, &inbox, idle::CYCLE, |event| {
                    if event == IdleEvent::Changed {
                        let _ = sync_tx.send(SyncReq::Inbox(id));
                    }
                    current(watches, id, generation)
                });
                let _ = session.logout();
                r
            });
            match result {
                Ok(()) => return, // replaced or account removed
                Err(e) if e.is::<Unsupported>() => {
                    if let Some(w) = watches.lock().unwrap().get_mut(&id) {
                        w.unsupported = true;
                    }
                    notify(PushState::Unavailable("the server doesn't support it".into()));
                    return;
                }
                Err(e) => {
                    failures += 1;
                    let wait = idle::backoff(failures);
                    log::warn!("push for {} dropped: {e:#}; retrying in {}s", account.email, wait.as_secs());
                    notify(PushState::Retrying(format!("{e:#}")));
                    // Sleep in short steps so a replacement or removal ends this thread promptly.
                    let until = Instant::now() + wait;
                    while Instant::now() < until {
                        std::thread::sleep(Duration::from_secs(1));
                        if !current(watches, id, generation) {
                            return;
                        }
                    }
                }
            }
        }
    }

    #[derive(Debug)]
    struct Unsupported;
    impl std::fmt::Display for Unsupported {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the server doesn't support IDLE")
        }
    }
    impl std::error::Error for Unsupported {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_conversation_runs_in_order() {
        let turns = Arc::new(Turns::default());
        let key = (1, "t".to_string());
        let log = Arc::new(std::sync::Mutex::new(vec![]));
        // Tickets in send order; threads start in reverse order.
        let tickets: Vec<u64> = (0..5).map(|_| turns.ticket(&key)).collect();
        let handles: Vec<_> = tickets
            .into_iter()
            .rev()
            .map(|t| {
                let (turns, key, log) = (turns.clone(), key.clone(), log.clone());
                std::thread::spawn(move || {
                    turns.wait(&key, t);
                    log.lock().unwrap().push(t);
                    turns.done(&key);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*log.lock().unwrap(), [0, 1, 2, 3, 4]);
        assert!(turns.state.lock().unwrap().is_empty(), "finished conversations are forgotten");
    }

    #[test]
    fn sync_requests_merge() {
        let mut p = SyncPlan::from(SyncReq::Inbox(1));
        p.add(SyncReq::Inbox(1));
        p.add(SyncReq::Inbox(2));
        p.add(SyncReq::Account(2));
        assert!(!p.all && !p.covers_fully(1) && p.covers_fully(2));
        assert_eq!(p.inboxes.len(), 2, "duplicates collapse");
        p.add(SyncReq::All);
        assert!(p.covers_fully(1) && p.covers_fully(99));
    }
}
