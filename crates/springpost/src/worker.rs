//! Background threads: periodic IMAP sync, snooze wake-ups and server actions,
//! so the UI thread never blocks on the network.

use crossbeam_channel::{Receiver, Sender};
use springpost_core::engine::{self, SyncProgress};
use springpost_core::smtp::OutgoingMessage;
use springpost_core::{credentials, Account, AccountId, FolderId, FolderRole, Store};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub enum Cmd {
    SyncAll,
    SyncAccount(AccountId),
    SetUnread(AccountId, String, bool),
    SetStarred(AccountId, String, bool),
    Move(AccountId, String, FolderRole),
    /// Move a thread into a folder, optionally only out of the one being viewed.
    MoveTo(AccountId, String, FolderId, Option<FolderId>),
    /// Add (true) or remove a label.
    Label(AccountId, String, FolderId, bool),
    /// Create a folder/label, then optionally move or label a thread into it.
    CreateFolder(AccountId, String, Option<(String, FolderUse)>),
    Send(AccountId, OutgoingMessage),
    /// Verify credentials, save the password to the keychain, then add.
    AddAccount(Box<Account>, String),
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
}

const SYNC_EVERY: Duration = Duration::from_secs(5 * 60);
const TICK: Duration = Duration::from_secs(20);

pub struct Worker {
    pub tx: Sender<Cmd>,
    pub rx: Receiver<Event>,
}

impl Worker {
    pub fn spawn(db: PathBuf, ctx: eframe::egui::Context) -> Self {
        let (tx, cmd_rx) = crossbeam_channel::unbounded::<Cmd>();
        let (ev_tx, rx) = crossbeam_channel::unbounded::<Event>();
        let (sync_tx, sync_rx) = crossbeam_channel::unbounded::<Option<AccountId>>();

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
                        if let Ok(woke) = store.wake_due_snoozes() {
                            if !woke.is_empty() {
                                let _ = ev.send(Event::SnoozesWoke(woke.len()));
                                ctx.request_repaint();
                            }
                        }
                        let target = match req {
                            Some(t) => t,
                            None if last.elapsed() >= SYNC_EVERY => None,
                            None => continue,
                        };
                        // Drain duplicates.
                        while sync_rx.try_recv().is_ok() {}
                        last = Instant::now();
                        let accounts: Vec<Account> = store
                            .accounts()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|a| !a.demo && target.map(|t| t == a.id).unwrap_or(true))
                            .collect();
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

        // Command dispatcher: each action runs on its own short-lived thread.
        std::thread::Builder::new()
            .name("actions".into())
            .spawn(move || {
                for cmd in cmd_rx {
                    match cmd {
                        Cmd::SyncAll => {
                            let _ = sync_tx.send(None);
                        }
                        Cmd::SyncAccount(id) => {
                            let _ = sync_tx.send(Some(id));
                        }
                        other => {
                            let db = db.clone();
                            let ev = ev_tx.clone();
                            let ctx = ctx.clone();
                            let sync_tx = sync_tx.clone();
                            std::thread::spawn(move || {
                                run_action(&db, other, &ev, &sync_tx);
                                ctx.request_repaint();
                            });
                        }
                    }
                }
            })
            .expect("spawn action thread");

        Self { tx, rx }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

fn run_action(db: &std::path::Path, cmd: Cmd, ev: &Sender<Event>, sync_tx: &Sender<Option<AccountId>>) {
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
            Cmd::SetUnread(a, key, unread) => engine::set_thread_unread(&store, &account(a)?, &key, unread)?,
            Cmd::SetStarred(a, key, starred) => engine::set_thread_starred(&store, &account(a)?, &key, starred)?,
            Cmd::Move(a, key, role) => {
                engine::move_thread(&store, &account(a)?, &key, role)?;
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
                match then {
                    Some((key, FolderUse::Move)) => engine::move_thread_to(&store, &acc, &key, &f, None)?,
                    Some((key, FolderUse::Label)) => engine::set_label(&store, &acc, &key, &f, true)?,
                    None => {}
                }
                let _ = ev.send(Event::Changed);
            }
            Cmd::Send(a, out) => {
                let _ = ev.send(Event::Status("Sending…".into()));
                let acc = account(a)?;
                match engine::send(&store, &acc, &out) {
                    Ok(_) => {
                        let _ = ev.send(Event::Sent(format!("Message sent from {}", acc.email)));
                        let _ = ev.send(Event::Changed);
                    }
                    Err(e) => {
                        let _ = ev.send(Event::Error(format!("Sending failed: {e:#}")));
                    }
                }
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
                        let _ = sync_tx.send(Some(id));
                    }
                    Err(e) => {
                        let _ = ev.send(Event::AccountFailed(format!("{e:#}")));
                    }
                }
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
