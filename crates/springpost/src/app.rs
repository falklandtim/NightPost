//! Application state and the three-pane layout.

use crate::composer::{Composer, ComposerAction, SignatureBook};
use crate::icons::{self, Icon, Item};
use crate::onboarding::{AccountSetup, SetupAction};
use crate::prefs::{ListSettings, Prefs, PrefsAction, SwipeAction, Tab, QUICK_ACTIONS_MIN_WIDTH};
use crate::style::{self, Colors};
use crate::worker::{Cmd, Event, FolderUse, Worker};
use chrono::{DateTime, Utc};
use eframe::egui::{self, Key, RichText, Sense};
use springpost_core::snooze::{self, SnoozePreset};
use springpost_core::theme::{self, Theme};
use springpost_core::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const SHORTCUTS: &[(&str, &str)] = &[
    ("c", "Compose new message"),
    ("r", "Reply"),
    ("a", "Reply all"),
    ("f", "Forward"),
    ("e", "Archive"),
    ("#  /  Delete", "Move to trash"),
    ("!", "Report spam"),
    ("b  /  h", "Snooze"),
    ("s", "Star / unstar"),
    ("u", "Mark unread"),
    ("p", "Pin to top / unpin"),
    ("v", "Move to folder…"),
    ("l", "Label… (Gmail)"),
    ("j  /  ↓", "Next conversation"),
    ("k  /  ↑", "Previous conversation"),
    ("/", "Search"),
    ("g i", "Go to Inbox"),
    ("⌘ ,", "Preferences"),
    ("⌘ W", "Close composer / Preferences"),
    ("⌘ ↩", "Send (in composer)"),
    ("Esc", "Close / clear search"),
];

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Selection {
    Unified(FolderRole),
    Folder(FolderId),
    Snoozed,
}

/// Quick filter from the funnel button above the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ListFilter {
    All,
    Unread,
    Starred,
    Pinned,
    Attachments,
}

impl ListFilter {
    const ALL: [ListFilter; 5] = [ListFilter::All, ListFilter::Unread, ListFilter::Starred, ListFilter::Pinned, ListFilter::Attachments];
    fn label(self) -> &'static str {
        match self {
            ListFilter::All => "All conversations",
            ListFilter::Unread => "Unread",
            ListFilter::Starred => "Starred",
            ListFilter::Pinned => "Pinned",
            ListFilter::Attachments => "With attachments",
        }
    }
    fn token(self) -> &'static str {
        match self {
            ListFilter::All => "",
            ListFilter::Unread => "is:unread",
            ListFilter::Starred => "is:starred",
            ListFilter::Pinned => "is:pinned",
            ListFilter::Attachments => "has:attachment",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SwipePhase {
    /// Following two-finger trackpad scrolling.
    Trackpad,
    /// Following a mouse drag.
    Drag,
    /// Fingers lifted / mouse released: decide whether to act.
    Released,
    /// Sliding back into place.
    Settling,
}

/// A conversation being swiped sideways in the list.
struct Swipe {
    account_id: AccountId,
    thread_key: String,
    offset: f32,
    phase: SwipePhase,
    last: Instant,
}

impl Swipe {
    fn new(t: &ThreadSummary, phase: SwipePhase, offset: f32) -> Self {
        Self { account_id: t.account_id, thread_key: t.thread_key.clone(), offset, phase, last: Instant::now() }
    }
}

struct Row {
    t: ThreadSummary,
    snoozed_until: Option<DateTime<Utc>>,
}

struct OpenThread {
    account_id: AccountId,
    thread_key: String,
    messages: Vec<Message>,
    /// Which messages are expanded (older ones collapse, like Mailspring).
    expanded: Vec<bool>,
    snoozed_until: Option<DateTime<Utc>>,
}

struct Toast {
    text: String,
    error: bool,
    at: Instant,
}

pub struct SpringPostApp {
    store: Store,
    worker: Worker,
    themes: Vec<Theme>,
    theme_id: String,
    colors: Colors,
    accounts: Vec<Account>,
    folders: HashMap<AccountId, Vec<Folder>>,
    unified_unread: HashMap<FolderRole, i64>,
    snoozed_count: i64,
    collapsed_accounts: Vec<AccountId>,
    selection: Selection,
    search: String,
    focus_search: bool,
    rows: Vec<Row>,
    selected: Option<usize>,
    open: Option<OpenThread>,
    composers: Vec<Composer>,
    next_composer_id: u64,
    setup: Option<AccountSetup>,
    welcome: AccountSetup,
    prefs: Option<Prefs>,
    status: String,
    syncing: bool,
    toasts: Vec<Toast>,
    snooze_custom: String,
    pending_g: Option<Instant>,
    last_reload: Instant,
    filter: ListFilter,
    show_sidebar: bool,
    /// Right edges of the sidebar and thread list last frame, so the toolbar
    /// can line its sections up with the columns below.
    sidebar_right: f32,
    list_right: f32,
    /// Room left for the macOS traffic lights when the toolbar is drawn in
    /// the title bar (0 when the system title bar is shown).
    pub titlebar_inset: f32,
    list_settings: ListSettings,
    swipe: Option<Swipe>,
    last_wheel_x: Option<Instant>,
    /// Initial width of the message list (it's resizable afterwards).
    list_default_width: f32,
    /// Text typed into the Move / Label picker.
    picker_query: String,
    sigs: SignatureBook,
}

const THEME_KEY: &str = "springpost.theme";
/// Height of a conversation in the list: names, subject and two preview lines.
const ROW_H: f32 = 94.0;
const LIST_KEY: &str = "springpost.list";

impl SpringPostApp {
    pub fn new(cc: &eframe::CreationContext<'_>, store: Store, demo: bool) -> Self {
        let saved = cc.storage.and_then(|s| s.get_string(THEME_KEY));
        let mut app = Self::with_context(&cc.egui_ctx, saved, store, demo);
        if let Some(l) = cc.storage.and_then(|s| s.get_string(LIST_KEY)) {
            app.list_settings = ListSettings::decode(&l);
        }
        if cfg!(target_os = "macos") {
            app.titlebar_inset = 76.0;
        }
        app
    }

    pub fn with_context(ctx: &egui::Context, saved_theme: Option<String>, store: Store, demo: bool) -> Self {
        if demo {
            let _ = demo::seed(&store);
        }
        install_image_loaders(ctx);
        let themes = theme::discover();
        let theme_id = saved_theme
            .filter(|id| themes.iter().any(|t| &t.id == id))
            .unwrap_or_else(|| {
                if ctx.style().visuals.dark_mode { "springpost-dark".into() } else { "springpost-light".into() }
            });
        let colors = Colors::new(themes.iter().find(|t| t.id == theme_id).map(Theme::palette).unwrap_or_else(|| themes[0].palette()));
        colors.apply(ctx);
        let worker = Worker::spawn(store.path().to_path_buf(), ctx.clone());
        let mut app = Self {
            store,
            worker,
            themes,
            theme_id,
            colors,
            accounts: vec![],
            folders: HashMap::new(),
            unified_unread: HashMap::new(),
            snoozed_count: 0,
            collapsed_accounts: vec![],
            selection: Selection::Unified(FolderRole::Inbox),
            search: String::new(),
            focus_search: false,
            rows: vec![],
            selected: None,
            open: None,
            composers: vec![],
            next_composer_id: 1,
            setup: None,
            welcome: AccountSetup::default(),
            prefs: None,
            status: String::new(),
            syncing: false,
            toasts: vec![],
            snooze_custom: String::new(),
            pending_g: None,
            last_reload: Instant::now(),
            filter: ListFilter::All,
            show_sidebar: true,
            sidebar_right: 220.0,
            list_right: 620.0,
            titlebar_inset: 0.0,
            list_settings: ListSettings::default(),
            swipe: None,
            last_wheel_x: None,
            list_default_width: 400.0,
            picker_query: String::new(),
            sigs: SignatureBook::default(),
        };
        app.reload();
        if app.accounts.iter().any(|a| !a.demo) {
            app.syncing = true;
            app.worker.send(Cmd::SyncAll);
        }
        app
    }

    // ----- data -------------------------------------------------------------

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, at: Instant::now() });
    }

    fn view(&self) -> Option<MailboxView> {
        if !self.search.trim().is_empty() {
            return Some(MailboxView::Everything);
        }
        match &self.selection {
            Selection::Unified(r) => Some(MailboxView::Unified(*r)),
            Selection::Folder(id) => Some(MailboxView::Folder(*id)),
            Selection::Snoozed => None,
        }
    }

    /// Reload sidebar counts and the thread list, preserving the selection.
    fn reload(&mut self) {
        self.last_reload = Instant::now();
        self.accounts = self.store.accounts().unwrap_or_default();
        self.folders = self
            .accounts
            .iter()
            .map(|a| (a.id, self.store.folders(a.id).unwrap_or_default()))
            .collect();
        self.unified_unread = FolderRole::UNIFIED
            .iter()
            .map(|r| (*r, self.store.unified_unread(*r).unwrap_or(0)))
            .collect();
        self.snoozed_count = self.store.snoozed_count().unwrap_or(0);
        self.sigs = SignatureBook::load(&self.store);

        let keep = self.selected.and_then(|i| self.rows.get(i)).map(|r| (r.t.account_id, r.t.thread_key.clone()));
        let query = format!("{} {}", self.search, self.filter.token());
        self.rows = match self.view() {
            Some(v) => self
                .store
                .threads(&v, query.trim(), 500)
                .unwrap_or_default()
                .into_iter()
                .map(|t| {
                    let snoozed_until = None;
                    Row { t, snoozed_until }
                })
                .collect(),
            None => self
                .store
                .snoozed_threads()
                .unwrap_or_default()
                .into_iter()
                .map(|(t, until)| Row { t, snoozed_until: Some(until) })
                .filter(|r| match self.filter {
                    ListFilter::All => true,
                    ListFilter::Unread => r.t.unread,
                    ListFilter::Starred => r.t.starred,
                    ListFilter::Pinned => r.t.pinned,
                    ListFilter::Attachments => r.t.has_attachments,
                })
                .collect(),
        };
        self.selected = keep.and_then(|(a, k)| self.rows.iter().position(|r| r.t.account_id == a && r.t.thread_key == k));
        if let Some(o) = &self.open {
            let (a, k) = (o.account_id, o.thread_key.clone());
            let msgs = self.store.thread_messages(a, &k).unwrap_or_default();
            if msgs.is_empty() {
                self.open = None;
            } else if let Some(o) = self.open.as_mut() {
                if msgs.len() != o.messages.len() {
                    o.expanded = expanded_default(&msgs);
                }
                o.messages = msgs;
                o.snoozed_until = self.store.snoozed_until(a, &k).ok().flatten();
            }
        }
    }

    fn select(&mut self, idx: usize) {
        let Some(row) = self.rows.get(idx) else { return };
        self.selected = Some(idx);
        let (a, k) = (row.t.account_id, row.t.thread_key.clone());
        let msgs = self.store.thread_messages(a, &k).unwrap_or_default();
        if row.t.unread {
            let _ = self.store.set_thread_unread(a, &k, false);
            self.worker.send(Cmd::SetUnread(a, k.clone(), false));
            if let Some(r) = self.rows.get_mut(idx) {
                r.t.unread = false;
            }
            self.unified_unread = FolderRole::UNIFIED
                .iter()
                .map(|r| (*r, self.store.unified_unread(*r).unwrap_or(0)))
                .collect();
            self.folders = self.accounts.iter().map(|a| (a.id, self.store.folders(a.id).unwrap_or_default())).collect();
        }
        self.open = Some(OpenThread {
            account_id: a,
            snoozed_until: self.store.snoozed_until(a, &k).ok().flatten(),
            thread_key: k,
            expanded: expanded_default(&msgs),
            messages: msgs,
        });
    }

    fn set_selection(&mut self, s: Selection) {
        self.selection = s;
        self.search.clear();
        self.selected = None;
        self.open = None;
        self.reload();
    }

    fn current(&self) -> Option<(AccountId, String)> {
        self.open.as_ref().map(|o| (o.account_id, o.thread_key.clone()))
    }

    /// Remove the open thread from the list (archive/trash/snooze) and move on.
    fn remove_current_and_advance(&mut self) {
        let Some((a, k)) = self.current() else { return };
        let idx = self.rows.iter().position(|r| r.t.account_id == a && r.t.thread_key == k);
        if let Some(i) = idx {
            self.rows.remove(i);
            self.open = None;
            self.selected = None;
            if !self.rows.is_empty() {
                self.select(i.min(self.rows.len() - 1));
            }
        } else {
            self.open = None;
        }
    }

    fn act_move(&mut self, role: FolderRole) {
        if let Some((a, k)) = self.current() {
            self.move_thread(a, &k, role);
        }
    }

    fn move_thread(&mut self, a: AccountId, k: &str, role: FolderRole) {
        let has_target = self.store.folder_by_role(a, role).ok().flatten().is_some()
            || (role == FolderRole::Archive && self.store.folder_by_role(a, FolderRole::All).ok().flatten().is_some());
        if !has_target {
            self.toast(format!("This account has no {} folder", role.label()), true);
            return;
        }
        self.worker.send(Cmd::Move(a, k.to_string(), role));
        self.remove_row(a, k);
        self.toast(
            match role {
                FolderRole::Archive => "Archived",
                FolderRole::Trash => "Moved to Trash",
                FolderRole::Spam => "Reported as spam",
                FolderRole::Inbox => "Moved to Inbox",
                _ => "Moved",
            },
            false,
        );
    }

    /// Drop a conversation from the list; if it was open, open the next one.
    fn remove_row(&mut self, a: AccountId, k: &str) {
        if self.current().is_some_and(|(oa, ok)| oa == a && ok == k) {
            self.remove_current_and_advance();
            return;
        }
        if let Some(i) = self.rows.iter().position(|r| r.t.account_id == a && r.t.thread_key == k) {
            self.rows.remove(i);
            match self.selected {
                Some(s) if s > i => self.selected = Some(s - 1),
                Some(s) if s == i => self.selected = None,
                _ => {}
            }
        }
    }

    fn act_star(&mut self) {
        let Some((a, k)) = self.current() else { return };
        let starred = !self.open.as_ref().map(|o| o.messages.iter().any(|m| m.starred)).unwrap_or(false);
        let _ = self.store.set_thread_starred(a, &k, starred);
        self.worker.send(Cmd::SetStarred(a, k, starred));
        self.reload();
    }

    fn act_unread(&mut self) {
        let Some((a, k)) = self.current() else { return };
        let _ = self.store.set_thread_unread(a, &k, true);
        self.worker.send(Cmd::SetUnread(a, k, true));
        self.open = None;
        self.selected = None;
        self.reload();
    }

    fn act_snooze(&mut self, until: DateTime<Utc>) {
        let Some((a, k)) = self.current() else { return };
        if let Err(e) = self.store.snooze(a, &k, until) {
            self.toast(format!("Couldn't snooze: {e:#}"), true);
            return;
        }
        if self.selection == Selection::Snoozed {
            self.reload();
        } else {
            self.remove_current_and_advance();
            self.snoozed_count = self.store.snoozed_count().unwrap_or(0);
        }
        self.toast(format!("Snoozed until {}", snooze::describe(until)), false);
    }

    fn act_unsnooze(&mut self) {
        let Some((a, k)) = self.current() else { return };
        let _ = self.store.unsnooze(a, &k);
        self.toast("Returned to Inbox", false);
        self.reload();
    }

    fn toggle_pin(&mut self, a: AccountId, k: &str) {
        let pinned = self.store.is_pinned(a, k).unwrap_or(false);
        let r = if pinned { self.store.unpin(a, k) } else { self.store.pin(a, k) };
        if let Err(e) = r {
            self.toast(format!("Couldn't pin: {e:#}"), true);
            return;
        }
        self.toast(if pinned { "Unpinned" } else { "Pinned to top" }, false);
        self.reload();
    }

    fn act_pin(&mut self) {
        if let Some((a, k)) = self.current() {
            self.toggle_pin(a, &k);
        }
    }

    fn set_row_unread(&mut self, a: AccountId, k: &str, unread: bool) {
        let _ = self.store.set_thread_unread(a, k, unread);
        self.worker.send(Cmd::SetUnread(a, k.to_string(), unread));
    }

    fn mark_all_read(&mut self) {
        let unread: Vec<(AccountId, String)> = self.rows.iter().filter(|r| r.t.unread).map(|r| (r.t.account_id, r.t.thread_key.clone())).collect();
        if unread.is_empty() {
            self.toast("Everything here is already read", false);
            return;
        }
        for (a, k) in &unread {
            self.set_row_unread(*a, k, false);
        }
        self.toast(format!("Marked {} conversation{} as read", unread.len(), if unread.len() == 1 { "" } else { "s" }), false);
        self.reload();
    }

    fn sync_now(&mut self) {
        self.syncing = true;
        self.status = "Checking for mail…".into();
        self.worker.send(Cmd::SyncAll);
    }

    fn compose_new(&mut self) {
        let acc = self
            .open
            .as_ref()
            .and_then(|o| self.accounts.iter().find(|a| a.id == o.account_id))
            .or_else(|| match self.selection {
                Selection::Folder(f) => self
                    .folders
                    .iter()
                    .find(|(_, fs)| fs.iter().any(|x| x.id == f))
                    .and_then(|(a, _)| self.accounts.iter().find(|x| x.id == *a)),
                _ => None,
            })
            .or_else(|| self.accounts.first())
            .cloned();
        if let Some(a) = acc {
            self.composers.push(Composer::new(self.next_composer_id, &a));
            self.next_composer_id += 1;
        }
    }

    fn compose_reply(&mut self, mode: u8) {
        let Some(o) = &self.open else { return };
        let Some(acc) = self.accounts.iter().find(|a| a.id == o.account_id).cloned() else { return };
        // Reply to the latest message not sent by us, falling back to the last.
        let m = o
            .messages
            .iter()
            .rev()
            .find(|m| !m.from.email.eq_ignore_ascii_case(&acc.email))
            .or(o.messages.last())
            .cloned();
        let Some(m) = m else { return };
        let id = self.next_composer_id;
        self.next_composer_id += 1;
        self.composers.push(match mode {
            0 => Composer::reply(id, &acc, &m, false),
            1 => Composer::reply(id, &acc, &m, true),
            _ => Composer::forward(id, &acc, &m),
        });
    }

    fn set_theme(&mut self, ctx: &egui::Context, id: &str) {
        if let Some(t) = self.themes.iter().find(|t| t.id == id) {
            self.theme_id = t.id.clone();
            self.colors = Colors::new(t.palette());
            self.colors.apply(ctx);
        }
    }

    // ----- events -----------------------------------------------------------

    fn handle_events(&mut self) {
        let mut changed = false;
        while let Ok(ev) = self.worker.rx.try_recv() {
            match ev {
                Event::Status(s) => {
                    self.status = s;
                    self.syncing = true;
                }
                Event::SyncFinished { new, errors } => {
                    self.syncing = false;
                    self.status = if new > 0 { format!("{new} new message{}", if new == 1 { "" } else { "s" }) } else { String::new() };
                    for e in errors {
                        self.toast(e, true);
                    }
                    changed = true;
                }
                Event::Changed => changed = true,
                Event::Error(e) => self.toast(e, true),
                Event::Sent(s) => {
                    self.status.clear();
                    self.toast(s, false);
                }
                Event::AccountAdded(id) => {
                    self.setup = None;
                    self.welcome = AccountSetup::default();
                    self.reload();
                    if let Some(a) = self.accounts.iter().find(|a| a.id == id) {
                        let msg = format!("Added {} — syncing…", a.email);
                        self.toast(msg, false);
                    }
                    self.syncing = true;
                    self.set_selection(Selection::Unified(FolderRole::Inbox));
                }
                Event::AccountFailed(e) => {
                    self.status.clear();
                    for s in [&mut self.welcome].into_iter().chain(self.setup.as_mut()) {
                        s.busy = false;
                        s.error = Some(e.clone());
                    }
                }
                Event::SnoozesWoke(n) => {
                    self.toast(format!("{n} snoozed conversation{} returned to your inbox", if n == 1 { "" } else { "s" }), false);
                    changed = true;
                }
            }
        }
        if changed || self.last_reload.elapsed() > Duration::from_secs(60) {
            self.reload();
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::Comma)) {
            self.prefs = Some(Prefs::new(Tab::Appearance));
        }
        // ⌘W closes the frontmost panel window: Preferences, then Add Account.
        // (Composers are their own windows and handle ⌘W themselves.)
        if (self.prefs.is_some() || self.setup.is_some()) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::W)) {
            if self.prefs.is_some() {
                self.prefs = None;
            } else {
                self.setup = None;
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if !self.search.is_empty() {
                self.search.clear();
                self.reload();
            }
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id)
                }
            });
        }
        // Single-key shortcuts only when nothing is being typed into.
        if ctx.wants_keyboard_input() || self.setup.is_some() {
            return;
        }
        let text: String = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        });
        let key = |k: Key| ctx.input(|i| i.key_pressed(k) && !i.modifiers.command);
        if let Some(t) = self.pending_g.take() {
            if t.elapsed() < Duration::from_millis(1200) {
                match text.as_str() {
                    "i" => self.set_selection(Selection::Unified(FolderRole::Inbox)),
                    "s" => self.set_selection(Selection::Unified(FolderRole::Starred)),
                    "t" => self.set_selection(Selection::Unified(FolderRole::Sent)),
                    "d" => self.set_selection(Selection::Unified(FolderRole::Drafts)),
                    "z" => self.set_selection(Selection::Snoozed),
                    _ => {}
                }
                return;
            }
        }
        match text.as_str() {
            "c" => self.compose_new(),
            "r" => self.compose_reply(0),
            "a" => self.compose_reply(1),
            "f" => self.compose_reply(2),
            "e" | "y" => self.act_move(FolderRole::Archive),
            "#" => self.act_move(FolderRole::Trash),
            "!" => self.act_move(FolderRole::Spam),
            "s" => self.act_star(),
            "u" => self.act_unread(),
            "p" => self.act_pin(),
            "v" | "l" => {
                let gmail = self.current().and_then(|(a, _)| self.accounts.iter().find(|x| x.id == a)).is_some_and(|a| a.is_gmail());
                if self.open.is_some() && (text == "v" || gmail) {
                    let id = if text == "v" { "picker_open_move" } else { "picker_open_label" };
                    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new(id), true));
                }
            }
            "b" | "h" => {
                if self.open.is_some() {
                    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("snooze_open"), true));
                }
            }
            "j" => self.move_selection(1),
            "k" => self.move_selection(-1),
            "/" => self.focus_search = true,
            "g" => self.pending_g = Some(Instant::now()),
            _ => {}
        }
        if key(Key::ArrowDown) {
            self.move_selection(1);
        }
        if key(Key::ArrowUp) {
            self.move_selection(-1);
        }
        if key(Key::Delete) || key(Key::Backspace) {
            self.act_move(FolderRole::Trash);
        }
    }

    fn move_selection(&mut self, d: i64) {
        if self.rows.is_empty() {
            return;
        }
        let next = match self.selected {
            Some(i) => (i as i64 + d).clamp(0, self.rows.len() as i64 - 1) as usize,
            None => 0,
        };
        self.select(next);
    }

    // ----- views ------------------------------------------------------------

    /// One bar across the top, split to line up with the columns below:
    /// sidebar toggle | list title + list actions | message actions + search.
    fn toolbar(&mut self, ctx: &egui::Context) {
        let col = self.colors.clone();
        let current = self.current();
        let open = current.is_some();
        let starred = self.open.as_ref().map(|o| o.messages.iter().any(|m| m.starred)).unwrap_or(false);
        let snoozed = self.open.as_ref().and_then(|o| o.snoozed_until);
        let pinned = current.as_ref().map(|(a, k)| self.store.is_pinned(*a, k).unwrap_or(false)).unwrap_or(false);
        let gmail = current.as_ref().and_then(|(a, _)| self.accounts.iter().find(|x| x.id == *a)).is_some_and(|a| a.is_gmail());
        egui::TopBottomPanel::top("toolbar")
            .exact_height(52.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(col.toolbar))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                // With the system title bar hidden, the toolbar is how you move the window.
                if self.titlebar_inset > 0.0 {
                    let bg = ui.interact(full, ui.id().with("drag"), Sense::click_and_drag());
                    if bg.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    if bg.double_clicked() {
                        let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                    }
                }
                let leading = self.titlebar_inset + 40.0;
                let a_right = if self.show_sidebar { self.sidebar_right.max(leading) } else { leading };
                let b_right = self.list_right.max(a_right + 240.0);
                let (top, bottom) = (full.top(), full.bottom());
                let p = ui.painter().clone();
                if self.show_sidebar {
                    p.rect_filled(egui::Rect::from_min_max(full.min, egui::pos2(a_right, bottom)), 0.0, col.sidebar);
                    p.vline(a_right - 0.5, top..=bottom, egui::Stroke::new(1.0_f32, col.border));
                }
                p.vline(b_right - 0.5, top..=bottom, egui::Stroke::new(1.0_f32, col.border));
                p.hline(full.x_range(), bottom - 0.5, egui::Stroke::new(1.0_f32, col.border));
                let section = |l: f32, r: f32| egui::Rect::from_min_max(egui::pos2(l, top), egui::pos2(r.max(l), bottom));
                let centered = |dir: egui::Layout| dir.with_cross_align(egui::Align::Center);

                // Sidebar toggle, just right of the traffic lights.
                let toggle_x = if self.show_sidebar { a_right - 40.0 } else { self.titlebar_inset + 6.0 };
                ui.scope_builder(egui::UiBuilder::new().max_rect(section(toggle_x, toggle_x + 32.0)).layout(centered(egui::Layout::left_to_right(egui::Align::Center))), |ui| {
                    let (fg, bg) = if self.show_sidebar { (col.sidebar_text, col.sidebar) } else { (col.toolbar_text, col.toolbar) };
                    if icons::bare(ui, Icon::Sidebar, if self.show_sidebar { "Hide sidebar" } else { "Show sidebar" }, fg, bg).clicked() {
                        self.show_sidebar = !self.show_sidebar;
                    }
                });

                // List section: title on the left, list actions on the right.
                ui.scope_builder(egui::UiBuilder::new().max_rect(section(a_right + 14.0, b_right - 10.0)).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
                    let filtered = self.filter != ListFilter::All;
                    let r = icons::group(
                        ui,
                        &col,
                        &[
                            Item::new(Icon::Check, "Mark all as read").enabled(!self.rows.is_empty()),
                            Item::new(Icon::Refresh, "Check for new mail").enabled(!self.syncing),
                            Item::new(Icon::Filter, "Filter").active(filtered),
                        ],
                    );
                    if r[0].clicked() {
                        self.mark_all_read();
                    }
                    if r[1].clicked() {
                        self.sync_now();
                    }
                    self.filter_popup(&r[2], &col);
                    if self.syncing {
                        ui.add_space(4.0);
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let mut title = self.list_title();
                        if filtered {
                            title = format!("{title} · {}", self.filter.label());
                        }
                        ui.add(egui::Label::new(RichText::new(title).size(15.0).strong().color(col.toolbar_text)).truncate());
                    });
                });

                // Message section.
                ui.scope_builder(egui::UiBuilder::new().max_rect(section(b_right + 12.0, full.right() - 12.0)).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if icons::round(ui, &col, Item::new(Icon::Compose, "New message (c)")).clicked() {
                        self.compose_new();
                    }
                    let r = icons::group(
                        ui,
                        &col,
                        &[
                            Item::new(Icon::Reply, "Reply (r)").enabled(open),
                            Item::new(Icon::ReplyAll, "Reply all (a)").enabled(open),
                            Item::new(Icon::Forward, "Forward (f)").enabled(open),
                        ],
                    );
                    for (i, resp) in r.iter().enumerate() {
                        if resp.clicked() {
                            self.compose_reply(i as u8);
                        }
                    }
                    let r = icons::group(
                        ui,
                        &col,
                        &[
                            Item::new(Icon::Done, "Done — archive (e)").enabled(open),
                            Item::new(Icon::Snooze, "Snooze (b)").enabled(open).active(snoozed.is_some()),
                            Item::new(if pinned { Icon::PinFilled } else { Icon::Pin }, if pinned { "Unpin (p)" } else { "Pin to top (p)" })
                                .enabled(open)
                                .active(pinned),
                            Item::new(Icon::Folder, "Move to folder (v)").enabled(open),
                            Item::new(Icon::Tag, if gmail || !open { "Label (l)" } else { "Labels are a Gmail feature — use Move on this account" }).enabled(open && gmail),
                            Item::new(Icon::More, "More").enabled(open),
                        ],
                    );
                    if r[0].clicked() {
                        self.act_move(FolderRole::Archive);
                    }
                    self.snooze_popup(ui, &r[1], &col, snoozed);
                    if r[2].clicked() {
                        self.act_pin();
                    }
                    if let Some(target) = current.clone() {
                        self.folder_popup(&r[3], FolderUse::Move, &target);
                        self.folder_popup(&r[4], FolderUse::Label, &target);
                    }
                    self.more_popup(&r[5], starred);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let w = ui.available_width().clamp(120.0, 380.0);
                        let r = icons::search_field(ui, &col, &mut self.search, w, "Search");
                        if self.focus_search {
                            r.request_focus();
                            self.focus_search = false;
                        }
                        if r.changed() {
                            self.selected = None;
                            self.open = None;
                            self.reload();
                        }
                    });
                });
            });
    }

    fn filter_popup(&mut self, btn: &egui::Response, col: &Colors) {
        let mut chosen = None;
        egui::Popup::menu(btn).show(|ui| {
            ui.set_min_width(190.0);
            for f in ListFilter::ALL {
                let mark = if f == self.filter { "✔" } else { " " };
                if ui.add(egui::Button::new(format!("{mark}  {}", f.label())).frame(false)).clicked() {
                    chosen = Some(f);
                }
            }
            let _ = col;
        });
        if let Some(f) = chosen {
            self.filter = f;
            self.selected = None;
            self.open = None;
            self.reload();
        }
    }

    /// The folder being viewed, if it belongs to `account` (so a move only
    /// takes the conversation out of that folder).
    fn viewed_folder(&self, account: AccountId) -> Option<FolderId> {
        match self.selection {
            Selection::Folder(f) if self.search.is_empty() => self.folders.get(&account)?.iter().find(|x| x.id == f).map(|x| x.id),
            _ => None,
        }
    }

    /// Folders a conversation can be moved to / labelled with.
    fn folder_choices(&self, account: AccountId, kind: FolderUse) -> Vec<Folder> {
        let all = self.folders.get(&account).cloned().unwrap_or_default();
        let has_archive = all.iter().any(|f| f.role == FolderRole::Archive);
        let from = self.viewed_folder(account);
        all.into_iter()
            .filter(|f| match kind {
                FolderUse::Label => f.role == FolderRole::Other,
                FolderUse::Move => {
                    Some(f.id) != from
                        && match f.role {
                            FolderRole::Inbox | FolderRole::Archive | FolderRole::Spam | FolderRole::Trash | FolderRole::Other => true,
                            FolderRole::All => !has_archive,
                            _ => false,
                        }
                }
            })
            .collect()
    }

    fn move_to(&mut self, a: AccountId, k: &str, folder: &Folder) {
        let from = self.viewed_folder(a);
        self.worker.send(Cmd::MoveTo(a, k.to_string(), folder.id, from));
        self.remove_row(a, k);
        self.toast(format!("Moved to {}", folder_label(folder)), false);
    }

    fn set_label(&mut self, a: AccountId, k: &str, folder: &Folder, add: bool) {
        self.worker.send(Cmd::Label(a, k.to_string(), folder.id, add));
        if !add && self.selection == Selection::Folder(folder.id) {
            self.remove_row(a, k);
        }
        self.toast(format!("{} “{}”", if add { "Labelled" } else { "Removed label" }, folder_label(folder)), false);
    }

    /// Searchable Move / Label picker under a toolbar button (`v` / `l`).
    fn folder_popup(&mut self, btn: &egui::Response, kind: FolderUse, target: &(AccountId, String)) {
        let (a, k) = (target.0, target.1.clone());
        let (open_key, popup_id) = match kind {
            FolderUse::Move => ("picker_open_move", "picker_move"),
            FolderUse::Label => ("picker_open_label", "picker_label"),
        };
        let forced = btn.ctx.memory_mut(|m| m.data.remove_temp::<bool>(egui::Id::new(open_key))).unwrap_or(false);
        let opening = btn.clicked() || forced;
        if opening {
            self.picker_query.clear();
        }
        let choices = self.folder_choices(a, kind);
        let applied: Vec<FolderId> = if kind == FolderUse::Label { self.store.thread_folder_ids(a, &k).unwrap_or_default() } else { vec![] };
        let mut pick: Option<(Folder, bool)> = None;
        let mut create: Option<String> = None;
        let col = self.colors.clone();
        let popup_id = egui::Id::new(popup_id);
        egui::Popup::from_response(btn)
            .id(popup_id)
            .open_memory(opening.then_some(egui::SetOpenCommand::Toggle))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(260.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.picker_query)
                        .hint_text(if kind == FolderUse::Move { "Move to…" } else { "Label as…" })
                        .desired_width(250.0),
                );
                if opening {
                    r.request_focus();
                }
                let q = self.picker_query.trim().to_lowercase();
                let matches: Vec<&Folder> = choices.iter().filter(|f| q.is_empty() || folder_label(f).to_lowercase().contains(&q)).collect();
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for f in &matches {
                        let on = applied.contains(&f.id);
                        let icon = match kind {
                            FolderUse::Label => if on { "☑" } else { "☐" },
                            FolderUse::Move => f.role.icon(),
                        };
                        if ui.add(egui::Button::new(format!("{icon}  {}", folder_label(f))).frame(false).min_size(egui::vec2(250.0, 24.0))).clicked() {
                            pick = Some(((*f).clone(), !on));
                        }
                    }
                    if matches.is_empty() && q.is_empty() {
                        ui.label(RichText::new(if kind == FolderUse::Label { "No labels yet — type a name to create one." } else { "No other folders." }).small().color(col.subtle));
                    }
                });
                let exact = choices.iter().any(|f| folder_label(f).to_lowercase() == q || f.path.to_lowercase() == q);
                if !q.is_empty() && !exact {
                    ui.separator();
                    let what = if kind == FolderUse::Move { "folder" } else { "label" };
                    if ui.add(egui::Button::new(format!("+  Create {what} “{}”", self.picker_query.trim())).frame(false)).clicked() {
                        create = Some(self.picker_query.trim().to_string());
                    }
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    if let Some(f) = matches.first() {
                        pick = Some(((*f).clone(), !applied.contains(&f.id)));
                    } else if !q.is_empty() {
                        create = Some(self.picker_query.trim().to_string());
                    }
                }
            });
        if pick.is_some() || create.is_some() {
            egui::Popup::close_id(&btn.ctx, popup_id);
            self.picker_query.clear();
        }
        match (pick, create) {
            (Some((f, add)), _) => match kind {
                FolderUse::Move => self.move_to(a, &k, &f),
                FolderUse::Label => self.set_label(a, &k, &f, add),
            },
            (None, Some(name)) => {
                self.worker.send(Cmd::CreateFolder(a, name.clone(), Some((k.clone(), kind))));
                if kind == FolderUse::Move {
                    self.remove_row(a, &k);
                }
                self.toast(format!("Created “{name}”"), false);
            }
            _ => {}
        }
    }

    fn more_popup(&mut self, btn: &egui::Response, starred: bool) {
        let mut action = None;
        egui::Popup::menu(btn).show(|ui| {
            ui.set_min_width(200.0);
            let items: [(&str, &str, u8); 4] = [
                (if starred { "Unstar" } else { "Star" }, "s", 0),
                ("Mark as unread", "u", 1),
                ("Move to Trash", "#", 2),
                ("Report spam", "!", 3),
            ];
            for (label, key, id) in items {
                if ui.add(egui::Button::new(label).shortcut_text(key).frame(false).min_size(egui::vec2(190.0, 24.0))).clicked() {
                    action = Some(id);
                }
            }
        });
        match action {
            Some(0) => self.act_star(),
            Some(1) => self.act_unread(),
            Some(2) => self.act_move(FolderRole::Trash),
            Some(3) => self.act_move(FolderRole::Spam),
            _ => {}
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        if !self.show_sidebar {
            return;
        }
        let col = self.colors.clone();
        let panel = egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(220.0)
            .width_range(170.0..=320.0)
            .frame(egui::Frame::new().fill(col.sidebar).inner_margin(egui::Margin::symmetric(8, 10)))
            .show(ctx, |ui| {
                // Footer: preferences and sync status.
                egui::TopBottomPanel::bottom("sidebar_footer")
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(col.sidebar))
                    .show_inside(ui, |ui| {
                        ui.horizontal(|ui| {
                            if icons::bare(ui, Icon::Gear, "Preferences (⌘,)", col.sidebar_text, col.sidebar).clicked() {
                                self.prefs = Some(Prefs::new(Tab::Appearance));
                            }
                            if !self.status.is_empty() {
                                ui.add(egui::Label::new(RichText::new(&self.status).small().color(col.sidebar_text.gamma_multiply(0.8))).truncate());
                            }
                        });
                    });
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let multi = self.accounts.len() > 1;
                    if multi {
                        section(ui, &col, "ALL ACCOUNTS");
                    }
                    for role in FolderRole::UNIFIED {
                        let label = if role == FolderRole::Inbox && multi { "Unified Inbox" } else { role.label() };
                        let count = if matches!(role, FolderRole::Inbox | FolderRole::Spam) { self.unified_unread.get(&role).copied().unwrap_or(0) } else { 0 };
                        let sel = self.selection == Selection::Unified(role) && self.search.is_empty();
                        if nav_item(ui, &col, role.icon(), label, count, sel, None) {
                            self.set_selection(Selection::Unified(role));
                        }
                        if role == FolderRole::Starred {
                            let sel = self.selection == Selection::Snoozed && self.search.is_empty();
                            if nav_item(ui, &col, "⏰", "Snoozed", self.snoozed_count, sel, Some(col.snooze)) {
                                self.set_selection(Selection::Snoozed);
                            }
                        }
                    }
                    ui.add_space(10.0);
                    let accounts = self.accounts.clone();
                    for a in &accounts {
                        let collapsed = self.collapsed_accounts.contains(&a.id);
                        let header = ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.0, style::hex(&a.color));
                            let label = RichText::new(format!("{} {}", if collapsed { "⏵" } else { "⏷" }, a.email.to_uppercase()))
                                .small()
                                .strong()
                                .color(col.sidebar_text);
                            ui.add(egui::Label::new(label).truncate().sense(Sense::click()))
                        });
                        if !a.demo {
                            header.inner.context_menu(|ui| {
                                if ui.button("⟳ Sync now").clicked() {
                                    self.syncing = true;
                                    self.worker.send(Cmd::SyncAccount(a.id));
                                    ui.close();
                                }
                            });
                        }
                        if header.inner.clicked() {
                            if collapsed {
                                self.collapsed_accounts.retain(|x| *x != a.id);
                            } else {
                                self.collapsed_accounts.push(a.id);
                            }
                        }
                        if collapsed {
                            ui.add_space(4.0);
                            continue;
                        }
                        let folders = self.folders.get(&a.id).cloned().unwrap_or_default();
                        for f in folders.iter().filter(|f| f.role != FolderRole::Other) {
                            let count = if matches!(f.role, FolderRole::Inbox | FolderRole::Spam | FolderRole::Other) { f.unread } else { 0 };
                            let sel = self.selection == Selection::Folder(f.id) && self.search.is_empty();
                            let label = if f.role == FolderRole::All { "All Mail".to_string() } else { f.role.label().to_string() };
                            if nav_item(ui, &col, f.role.icon(), &label, count, sel, None) {
                                self.set_selection(Selection::Folder(f.id));
                            }
                        }
                        for f in folders.iter().filter(|f| f.role == FolderRole::Other) {
                            let depth = f.path.matches(['/', '.']).count();
                            let sel = self.selection == Selection::Folder(f.id) && self.search.is_empty();
                            ui.horizontal(|ui| {
                                ui.add_space(depth as f32 * 10.0);
                                if nav_item(ui, &col, "📁", &f.name, f.unread, sel, None) {
                                    self.set_selection(Selection::Folder(f.id));
                                }
                            });
                        }
                        ui.add_space(8.0);
                    }
                    ui.add_space(6.0);
                    if ui.add(egui::Button::new(RichText::new("+ Add account").color(col.sidebar_text)).frame(false)).clicked() {
                        self.setup = Some(AccountSetup::default());
                    }
                });
            });
        self.sidebar_right = panel.response.rect.right();
    }

    fn list_title(&self) -> String {
        if !self.search.trim().is_empty() {
            return format!("Search: {}", self.search.trim());
        }
        match &self.selection {
            Selection::Unified(FolderRole::Inbox) if self.accounts.len() > 1 => "Unified Inbox".into(),
            Selection::Unified(r) => r.label().into(),
            Selection::Snoozed => "Snoozed".into(),
            Selection::Folder(id) => self
                .folders
                .values()
                .flatten()
                .find(|f| f.id == *id)
                .map(|f| {
                    let acc = self.accounts.iter().find(|a| a.id == f.account_id).map(|a| a.email.as_str()).unwrap_or("");
                    let name = if f.role == FolderRole::Other { f.name.clone() } else { f.role.label().to_string() };
                    if self.accounts.len() > 1 { format!("{name} — {acc}") } else { name }
                })
                .unwrap_or_default(),
        }
    }

    fn thread_list(&mut self, ctx: &egui::Context) {
        let col = self.colors.clone();
        // Two-finger trackpad swipes arrive as horizontal wheel deltas.
        let (wheel_x, wheel_y) = ctx.input(|i| {
            i.events.iter().fold((0.0_f32, 0.0_f32), |acc, e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => {
                    let k = match unit {
                        egui::MouseWheelUnit::Point => 1.0,
                        egui::MouseWheelUnit::Line => 24.0,
                        egui::MouseWheelUnit::Page => 400.0,
                    };
                    (acc.0 + delta.x * k, acc.1 + delta.y * k)
                }
                _ => acc,
            })
        });
        let now = Instant::now();
        let fresh_gesture = self.last_wheel_x.is_none_or(|t| now.duration_since(t) > Duration::from_millis(150));
        if wheel_x != 0.0 {
            self.last_wheel_x = Some(now);
        }
        let mut commit: Option<(AccountId, String, SwipeAction)> = None;
        let mut quick: Option<(AccountId, String, SwipeAction)> = None;

        let panel = egui::SidePanel::left("threads")
            .resizable(true)
            .default_width(self.list_default_width)
            .width_range(300.0..=640.0)
            .frame(egui::Frame::new().fill(col.list).stroke(egui::Stroke::new(1.0_f32, col.border)))
            .show(ctx, |ui| {
                if self.rows.is_empty() {
                    self.swipe = None;
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| {
                        let (icon, text) = match (&self.selection, self.search.is_empty()) {
                            (_, false) => ("🔍", "No conversations match your search."),
                            _ if self.filter == ListFilter::Pinned => ("📌", "Nothing pinned. Press p on a conversation to keep it at the top."),
                            _ if self.filter != ListFilter::All => ("🔍", "No conversations match this filter."),
                            (Selection::Snoozed, _) => ("⏰", "Nothing snoozed. Press b on a conversation to snooze it."),
                            (Selection::Unified(FolderRole::Inbox), _) => ("🌱", "Inbox zero. Nice work!"),
                            _ => ("📭", "No conversations here."),
                        };
                        ui.label(RichText::new(icon).size(36.0));
                        ui.label(RichText::new(text).color(col.subtle));
                    });
                    return;
                }
                let width = ui.available_width();
                let threshold = (width * 0.3).clamp(80.0, 140.0);
                let settings = self.list_settings;
                let show_quick = settings.quick_actions && width >= QUICK_ACTIONS_MIN_WIDTH;

                // Advance the current swipe: follow the trackpad, release after
                // a short pause (that's when the fingers, and momentum, stop).
                if let Some(s) = &mut self.swipe {
                    match s.phase {
                        SwipePhase::Trackpad => {
                            if wheel_x != 0.0 {
                                s.offset += wheel_x;
                                s.last = now;
                            } else if now.duration_since(s.last) > Duration::from_millis(140) {
                                s.phase = SwipePhase::Released;
                            }
                        }
                        SwipePhase::Settling => {
                            s.offset *= 0.55;
                        }
                        SwipePhase::Drag | SwipePhase::Released => {}
                    }
                }
                if let Some(s) = &mut self.swipe {
                    if s.phase == SwipePhase::Released {
                        let action = if s.offset > 0.0 { settings.swipe_right } else { settings.swipe_left };
                        if s.offset.abs() >= threshold && action != SwipeAction::Nothing {
                            commit = Some((s.account_id, s.thread_key.clone(), action));
                            if matches!(action, SwipeAction::Archive | SwipeAction::Trash) {
                                // The row is going away; don't animate it back.
                                s.offset = 0.0;
                            }
                        }
                        s.phase = SwipePhase::Settling;
                    }
                }
                if self.swipe.as_ref().is_some_and(|s| s.phase == SwipePhase::Settling && s.offset.abs() < 1.0) {
                    self.swipe = None;
                }
                if self.swipe.is_some() {
                    ctx.request_repaint();
                }
                let swiping = self.swipe.is_some();

                let row_h = ROW_H;
                let n = self.rows.len();
                let mut clicked = None;
                let mut menu: Option<(AccountId, String, u8)> = None;
                let mut folder_menu: Option<(AccountId, String, Folder, FolderUse, bool)> = None;
                let multi = self.accounts.len() > 1;
                egui::ScrollArea::vertical().auto_shrink([false, false]).scroll_source(egui::scroll_area::ScrollSource { mouse_wheel: !swiping, ..egui::scroll_area::ScrollSource::ALL }).show_rows(ui, row_h, n, |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for i in range {
                        let t = self.rows[i].t.clone();
                        let snoozed_until = self.rows[i].snoozed_until;
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click_and_drag());
                        if self.selected == Some(i) && ui.ctx().input(|inp| inp.key_pressed(Key::ArrowDown) || inp.key_pressed(Key::ArrowUp) || inp.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "j" || t == "k"))) {
                            ui.scroll_to_rect(rect, None);
                        }
                        let is_this = |s: &Swipe| s.account_id == t.account_id && s.thread_key == t.thread_key;

                        // Start a swipe: a fresh, mostly-horizontal trackpad gesture over this row…
                        if self.swipe.is_none() && resp.contains_pointer() && fresh_gesture && wheel_x.abs() > wheel_y.abs() && wheel_x.abs() > 0.5 {
                            self.swipe = Some(Swipe::new(&t, SwipePhase::Trackpad, wheel_x));
                        }
                        // …or a mouse drag.
                        if resp.dragged() {
                            let dx = resp.drag_delta().x;
                            match &mut self.swipe {
                                Some(s) if is_this(s) => {
                                    s.phase = SwipePhase::Drag;
                                    s.offset += dx;
                                }
                                None if dx != 0.0 => self.swipe = Some(Swipe::new(&t, SwipePhase::Drag, dx)),
                                _ => {}
                            }
                        }
                        if resp.drag_stopped() {
                            if let Some(s) = &mut self.swipe {
                                if is_this(s) {
                                    s.phase = SwipePhase::Released;
                                }
                            }
                        }
                        let mut offset = self.swipe.as_ref().filter(|s| is_this(s)).map(|s| s.offset).unwrap_or(0.0);
                        // A side with no action doesn't move.
                        if (offset > 0.0 && settings.swipe_right == SwipeAction::Nothing) || (offset < 0.0 && settings.swipe_left == SwipeAction::Nothing) {
                            offset = 0.0;
                        }
                        offset = offset.clamp(-rect.width() * 0.85, rect.width() * 0.85);

                        let p = ui.painter_at(rect);
                        if offset != 0.0 {
                            let action = if offset > 0.0 { settings.swipe_right } else { settings.swipe_left };
                            let armed = offset.abs() >= threshold;
                            let base = swipe_color(&col, action);
                            p.rect_filled(rect, 0.0, if armed { base } else { style::blend(base, col.list, 0.55) });
                            let label = swipe_label(action, t.unread, t.pinned);
                            let fg = egui::Color32::WHITE;
                            let galley = p.layout_no_wrap(label.to_string(), egui::FontId::proportional(13.0), fg);
                            let cy = rect.center().y;
                            let (icon_x, text_x) = if offset > 0.0 {
                                (rect.left() + 26.0, rect.left() + 44.0)
                            } else {
                                (rect.right() - 26.0, rect.right() - 44.0 - galley.size().x)
                            };
                            icons::paint(&p, egui::pos2(icon_x, cy), 18.0, swipe_icon(action), fg);
                            p.galley(egui::pos2(text_x, cy - galley.size().y / 2.0), galley, fg);
                        }
                        let rect_c = rect.translate(egui::vec2(offset, 0.0));

                        let selected = self.selected == Some(i);
                        let hovered = resp.contains_pointer() && !swiping;
                        let (bg, fg, sub) = if selected {
                            (col.list_focused, col.list_focused_text, col.list_focused_text.gamma_multiply(0.8))
                        } else if hovered {
                            (col.list_hover, col.text, col.subtle)
                        } else {
                            (col.list, col.text, col.subtle)
                        };
                        // Canary-style: no dividers, a rounded highlight inset from the edges.
                        p.rect_filled(rect_c, 0.0, col.list);
                        if bg != col.list {
                            p.rect_filled(rect_c.shrink2(egui::vec2(6.0, 2.0)), 8.0, bg);
                        }
                        let rect = rect_c;
                        let left = rect.left() + 16.0;
                        let top = rect.top();
                        if t.unread {
                            p.circle_filled(egui::pos2(rect.left() + 9.0, top + 30.0), 3.5, if selected { fg } else { col.unread });
                        }
                        let latest = t.participants.last().cloned().unwrap_or_default();
                        style::paint_avatar(&p, egui::pos2(left + 18.0, top + 30.0), &latest.name, &latest.email, 36.0);
                        let x = left + 48.0;
                        let mut right = rect.right() - 16.0;

                        // Line 1: people … date ▌(account colour)
                        if multi {
                            if let Some(a) = self.accounts.iter().find(|a| a.id == t.account_id) {
                                let bar = egui::Rect::from_min_size(egui::pos2(right - 3.0, top + 13.0), egui::vec2(3.0, 13.0));
                                p.rect_filled(bar, 1.5, style::hex(&a.color));
                                right -= 9.0;
                            }
                        }
                        let date = match snoozed_until {
                            Some(u) => format!("⏰ {}", snooze::describe(u)),
                            None => style::short_date(t.last_date),
                        };
                        let date_g = p.layout_no_wrap(date, egui::FontId::proportional(12.0), if snoozed_until.is_some() && !selected { col.snooze } else { sub });
                        let date_w = date_g.size().x;
                        p.galley(egui::pos2(right - date_w, top + 12.0), date_g, sub);
                        let me = self.accounts.iter().find(|a| a.id == t.account_id);
                        let (first, rest, from_me) = participants_parts(&t.participants, me);
                        let mut nx = x;
                        if from_me {
                            // You sent the latest message.
                            icons::paint(&p, egui::pos2(nx + 6.0, top + 19.0), 12.0, Icon::Reply, sub);
                            nx += 17.0;
                        }
                        let names_max = right - date_w - 10.0;
                        let name_font = egui::FontId::proportional(14.5);
                        let w = style::bold_line(&p, egui::pos2(nx, top + 10.0), &first, name_font.clone(), fg, names_max - nx);
                        if !rest.is_empty() && nx + w < names_max {
                            style::line(&p, egui::pos2(nx + w, top + 10.0), &rest, name_font, sub, names_max - nx - w);
                        }

                        // Line 2: subject … pin, attachment, star, count
                        let mut ix = rect.right() - 16.0;
                        let mut badge = |p: &egui::Painter, w: f32, draw: &dyn Fn(&egui::Painter, egui::Pos2)| {
                            draw(p, egui::pos2(ix - w / 2.0, top + 39.0));
                            ix -= w + 4.0;
                        };
                        if t.count > 1 {
                            let g = p.layout_no_wrap(t.count.to_string(), egui::FontId::proportional(12.5), fg);
                            let w = g.size().x + 1.0;
                            badge(&p, w, &|p, c| {
                                let pos = egui::pos2(c.x - w / 2.0, c.y - g.size().y / 2.0);
                                p.galley(pos, g.clone(), fg);
                                p.galley(pos + egui::vec2(0.6, 0.0), g.clone(), fg);
                            });
                        }
                        if t.has_attachments {
                            badge(&p, 12.0, &|p, c| {
                                p.text(c, egui::Align2::CENTER_CENTER, "📎", egui::FontId::proportional(12.0), sub);
                            });
                        }
                        if t.starred {
                            badge(&p, 12.0, &|p, c| {
                                p.text(c, egui::Align2::CENTER_CENTER, "★", egui::FontId::proportional(13.0), col.star);
                            });
                        }
                        if t.pinned {
                            let pin = if selected { fg } else { col.warning };
                            badge(&p, 12.0, &|p, c| icons::paint(p, c, 12.0, Icon::PinFilled, pin));
                        }
                        let subject_font = egui::FontId::proportional(13.5);
                        if t.unread {
                            style::bold_line(&p, egui::pos2(x, top + 31.0), &t.subject, subject_font, fg, ix - x);
                        } else {
                            style::line(&p, egui::pos2(x, top + 31.0), &t.subject, subject_font, fg, ix - x);
                        }

                        // Lines 3–4: a two-line preview.
                        style::paragraph(&p, egui::pos2(x, top + 51.0), &t.snippet, egui::FontId::proportional(12.5), sub, rect.right() - 16.0 - x, 2);

                        // Quick actions on hover, in a wide enough list.
                        if show_quick && hovered {
                            let items = [
                                (Item::new(Icon::Envelope, if t.unread { "Mark as read" } else { "Mark as unread" }), SwipeAction::ToggleRead),
                                (Item::new(Icon::Done, "Archive"), SwipeAction::Archive),
                                (Item::new(Icon::Trash, "Move to Trash"), SwipeAction::Trash),
                                (Item::new(Icon::Snooze, "Snooze"), SwipeAction::Snooze),
                                (Item::new(if t.pinned { Icon::PinFilled } else { Icon::Pin }, if t.pinned { "Unpin" } else { "Pin to top" }).active(t.pinned), SwipeAction::Pin),
                            ];
                            let (seg, h) = (28.0, 30.0);
                            let w = seg * items.len() as f32 + 6.0;
                            let qr = egui::Rect::from_min_size(egui::pos2(rect.right() - 16.0 - w, rect.center().y - h / 2.0), egui::vec2(w, h));
                            // Cover the right-hand end of the row, fading in from the left.
                            // Stay inside the row's rounded highlight.
                            let hl = rect.shrink2(egui::vec2(6.0, 2.0));
                            let cover = egui::Rect::from_min_max(egui::pos2(qr.left() - 8.0, hl.top()), hl.right_bottom());
                            p.rect_filled(cover, egui::CornerRadius { nw: 0, sw: 0, ne: 8, se: 8 }, bg);
                            let fade = egui::Rect::from_min_max(egui::pos2(cover.left() - 28.0, cover.top()), egui::pos2(cover.left(), cover.bottom()));
                            let mut mesh = egui::Mesh::default();
                            let clear = egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 0);
                            mesh.colored_vertex(fade.left_top(), clear);
                            mesh.colored_vertex(fade.right_top(), bg);
                            mesh.colored_vertex(fade.right_bottom(), bg);
                            mesh.colored_vertex(fade.left_bottom(), clear);
                            mesh.add_triangle(0, 1, 2);
                            mesh.add_triangle(0, 2, 3);
                            p.add(mesh);
                            let tone = icons::Tone { fg, bg, accent: if selected { fg } else { col.accent } };
                            let (list, acts): (Vec<Item>, Vec<SwipeAction>) = items.into_iter().unzip();
                            // A child Ui so the buttons overlay the row instead of taking layout space.
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(qr));
                            let r = icons::group_with(&mut child, tone, &list, seg, h, ("quick", i));
                            for (resp, act) in r.iter().zip(acts) {
                                if resp.clicked() {
                                    quick = Some((t.account_id, t.thread_key.clone(), act));
                                }
                            }
                        }

                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button(if t.pinned { "Unpin" } else { "Pin to top" }).clicked() {
                                menu = Some((t.account_id, t.thread_key.clone(), 0));
                            }
                            if ui.button(if t.unread { "Mark as read" } else { "Mark as unread" }).clicked() {
                                menu = Some((t.account_id, t.thread_key.clone(), if t.unread { 1 } else { 2 }));
                            }
                            ui.separator();
                            ui.menu_button("Move to", |ui| {
                                for f in self.folder_choices(t.account_id, FolderUse::Move) {
                                    if ui.button(format!("{}  {}", f.role.icon(), folder_label(&f))).clicked() {
                                        folder_menu = Some((t.account_id, t.thread_key.clone(), f, FolderUse::Move, true));
                                    }
                                }
                            });
                            if self.accounts.iter().any(|a| a.id == t.account_id && a.is_gmail()) {
                                ui.menu_button("Label", |ui| {
                                    let on = self.store.thread_folder_ids(t.account_id, &t.thread_key).unwrap_or_default();
                                    let labels = self.folder_choices(t.account_id, FolderUse::Label);
                                    if labels.is_empty() {
                                        ui.label("No labels yet");
                                    }
                                    for f in labels {
                                        let has = on.contains(&f.id);
                                        if ui.button(format!("{}  {}", if has { "☑" } else { "☐" }, folder_label(&f))).clicked() {
                                            folder_menu = Some((t.account_id, t.thread_key.clone(), f, FolderUse::Label, !has));
                                        }
                                    }
                                });
                            }
                        });
                    }
                });
                if let Some(i) = clicked {
                    if quick.is_none() {
                        self.select(i);
                    }
                }
                match folder_menu {
                    Some((a, k, f, FolderUse::Move, _)) => self.move_to(a, &k, &f),
                    Some((a, k, f, FolderUse::Label, add)) => self.set_label(a, &k, &f, add),
                    None => {}
                }
                match menu {
                    Some((a, k, 0)) => self.toggle_pin(a, &k),
                    Some((a, k, n)) => {
                        self.set_row_unread(a, &k, n == 2);
                        self.reload();
                    }
                    None => {}
                }
            });
        self.list_right = panel.response.rect.right();
        if let Some((a, k, act)) = commit.or(quick) {
            self.thread_action(ctx, a, &k, act);
        }
    }

    /// Run a swipe / quick action on any conversation, not just the open one.
    fn thread_action(&mut self, ctx: &egui::Context, a: AccountId, k: &str, act: SwipeAction) {
        match act {
            SwipeAction::Nothing => {}
            SwipeAction::Archive => self.move_thread(a, k, FolderRole::Archive),
            SwipeAction::Trash => self.move_thread(a, k, FolderRole::Trash),
            SwipeAction::Pin => self.toggle_pin(a, k),
            SwipeAction::ToggleRead => {
                let unread = self.rows.iter().find(|r| r.t.account_id == a && r.t.thread_key == k).map(|r| r.t.unread).unwrap_or(false);
                self.set_row_unread(a, k, !unread);
                self.reload();
            }
            SwipeAction::Snooze => {
                // Open the conversation and pop the snooze menu in the toolbar.
                if let Some(i) = self.rows.iter().position(|r| r.t.account_id == a && r.t.thread_key == k) {
                    self.select(i);
                    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("snooze_open"), true));
                }
            }
        }
    }

    fn reader(&mut self, ctx: &egui::Context) {
        let col = self.colors.clone();
        // Mailspring paints the message area with @background-secondary and
        // the messages as cards on top of it.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(col.bg_secondary))
            .show(ctx, |ui| {
                let Some(open) = &self.open else {
                    ui.centered_and_justified(|ui| {
                        let unread = self.unified_unread.get(&FolderRole::Inbox).copied().unwrap_or(0);
                        ui.label(
                            RichText::new(if unread > 0 { format!("{unread} unread in your inbox") } else { "No conversation selected".into() })
                                .color(col.very_subtle)
                                .size(15.0),
                        );
                    });
                    return;
                };
                let subject = open.messages.first().map(|m| m.subject.clone()).unwrap_or_default();
                let snoozed = open.snoozed_until;
                let pinned = self.store.is_pinned(open.account_id, &open.thread_key).unwrap_or(false);
                let in_folders = self.store.thread_folder_ids(open.account_id, &open.thread_key).unwrap_or_default();
                let labels: Vec<Folder> = self
                    .folders
                    .get(&open.account_id)
                    .map(|fs| fs.iter().filter(|f| f.role == FolderRole::Other && in_folders.contains(&f.id)).cloned().collect())
                    .unwrap_or_default();
                let thread_ref = (open.account_id, open.thread_key.clone());
                let mut remove_label: Option<Folder> = None;
                let account = self.accounts.iter().find(|a| a.id == open.account_id).cloned();

                let Some(open) = &mut self.open else { return };
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    egui::Frame::new().inner_margin(egui::Margin { left: 24, right: 24, top: 18, bottom: 24 }).show(ui, |ui| {
                        let max_w = ui.available_width().min(860.0);
                        ui.set_max_width(max_w);
                        ui.label(RichText::new(&subject).size(21.0).strong().color(col.heading));
                        ui.horizontal(|ui| {
                            if let Some(a) = &account {
                                chip(ui, &a.email, style::hex(&a.color));
                            }
                            if pinned {
                                chip(ui, "📌 Pinned", col.accent);
                            }
                            for f in &labels {
                                // Theme-aware: the label's hue, pulled towards the text colour.
                                let c = style::blend(style::avatar_color(&f.path), col.text, 0.55);
                                if label_chip(ui, &folder_label(f), c).clicked() {
                                    remove_label = Some(f.clone());
                                }
                            }
                            if let Some(u) = snoozed {
                                chip(ui, &format!("⏰ Snoozed until {}", snooze::describe(u)), col.snooze);
                            }
                        });
                        ui.add_space(10.0);
                        let n = open.messages.len();
                        for (i, m) in open.messages.iter().enumerate() {
                            let expanded = open.expanded.get(i).copied().unwrap_or(true);
                            let r = message_card(ui, &col, m, expanded, i + 1 == n);
                            if r && i + 1 != n {
                                if let Some(e) = open.expanded.get_mut(i) {
                                    *e = !*e;
                                }
                            }
                            ui.add_space(10.0);
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            for (i, (icon, label)) in [(Icon::Reply, "Reply"), (Icon::ReplyAll, "Reply all"), (Icon::Forward, "Forward")].into_iter().enumerate() {
                                if icons::labeled(ui, icon, label, col.text, col.bg_secondary).clicked() {
                                    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("reply_click"), i as u8));
                                }
                            }
                        });
                    });
                });
                if let Some(f) = remove_label {
                    self.set_label(thread_ref.0, &thread_ref.1, &f, false);
                }
                if let Some(mode) = ctx.memory_mut(|m| m.data.remove_temp::<u8>(egui::Id::new("reply_click"))) {
                    self.compose_reply(mode);
                }
            });
    }

    fn snooze_popup(&mut self, ui: &mut egui::Ui, btn: &egui::Response, col: &Colors, snoozed: Option<DateTime<Utc>>) {
        let open_id = egui::Id::new("snooze_open");
        let force_open = ui.ctx().memory_mut(|m| m.data.remove_temp::<bool>(open_id)).unwrap_or(false);
        let popup_id = egui::Id::new("snooze_popup");
        let toggle = (btn.clicked() || force_open).then_some(egui::SetOpenCommand::Toggle);
        let mut chosen: Option<DateTime<Utc>> = None;
        let mut unsnooze = false;
        egui::Popup::from_response(btn)
            .id(popup_id)
            .open_memory(toggle)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(250.0);
                ui.label(RichText::new("Snooze until…").strong());
                ui.add_space(4.0);
                let now = chrono::Local::now();
                for p in SnoozePreset::ALL {
                    let when = p.resolve(now);
                    let r = ui.add(
                        egui::Button::new(format!("{}  {}", p.icon(), p.label()))
                            .right_text(RichText::new(snooze::describe(when)).small().color(col.subtle))
                            .frame(false)
                            .min_size(egui::vec2(240.0, 26.0)),
                    );
                    if r.clicked() {
                        chosen = Some(when);
                    }
                }
                ui.separator();
                ui.label(RichText::new("Or type a time").small().color(col.subtle));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.snooze_custom)
                        .hint_text("3h, friday, 2026-12-01 09:00")
                        .desired_width(240.0),
                );
                let parsed = snooze::parse_when(&self.snooze_custom, now);
                if !self.snooze_custom.trim().is_empty() {
                    match &parsed {
                        Ok(t) => {
                            ui.label(RichText::new(format!("→ {}", snooze::describe(*t))).small().color(col.success));
                        }
                        Err(_) => {
                            ui.label(RichText::new("Not sure when that is").small().color(col.error));
                        }
                    }
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    if let Ok(t) = parsed {
                        chosen = Some(t);
                    }
                }
                if snoozed.is_some() {
                    ui.separator();
                    if ui.button("Unsnooze — move to Inbox now").clicked() {
                        unsnooze = true;
                    }
                }
            });
        if let Some(t) = chosen {
            egui::Popup::close_id(ui.ctx(), popup_id);
            self.snooze_custom.clear();
            if t > Utc::now() {
                self.act_snooze(t);
            }
        }
        if unsnooze {
            egui::Popup::close_id(ui.ctx(), popup_id);
            self.act_unsnooze();
        }
    }

    fn windows(&mut self, ctx: &egui::Context) {
        let col = self.colors.clone();
        // Composers.
        let mut i = 0;
        while i < self.composers.len() {
            match self.composers[i].ui(ctx, &self.accounts, &self.store, &col, &self.sigs) {
                ComposerAction::None => i += 1,
                ComposerAction::Discard => {
                    self.composers.remove(i);
                }
                ComposerAction::Send(acc, out) => {
                    if out.to.is_empty() && out.cc.is_empty() && out.bcc.is_empty() {
                        self.toast("Add at least one recipient", true);
                        i += 1;
                        continue;
                    }
                    if let Some(bad) = out.to.iter().chain(&out.cc).chain(&out.bcc).find(|c| !c.email.contains('@')) {
                        self.toast(format!("“{}” isn't a valid email address", bad.email), true);
                        i += 1;
                        continue;
                    }
                    self.worker.send(Cmd::Send(acc, *out));
                    self.composers.remove(i);
                }
            }
        }
        // Add-account window.
        if let Some(setup) = &mut self.setup {
            match setup.window(ctx, &col) {
                SetupAction::Submit(a, pw) => self.worker.send(Cmd::AddAccount(a, pw)),
                SetupAction::Cancel => self.setup = None,
                SetupAction::Demo => {
                    let _ = demo::seed(&self.store);
                    self.setup = None;
                    self.reload();
                }
                SetupAction::None => {}
            }
        }
        // Preferences.
        if let Some(prefs) = &mut self.prefs {
            let act = prefs.ui(ctx, &col, &self.themes, &self.theme_id, &self.accounts, &mut self.list_settings, &self.sigs);
            match act {
                PrefsAction::None => {}
                PrefsAction::Close => self.prefs = None,
                PrefsAction::SetTheme(id) => self.set_theme(ctx, &id),
                PrefsAction::ReloadThemes => {
                    self.themes = theme::discover();
                    let msg = format!("Found {} themes", self.themes.len());
                    if let Some(p) = &mut self.prefs {
                        p.message = Some((true, msg));
                    }
                }
                PrefsAction::InstallTheme(path) => {
                    let result = theme::install(std::path::Path::new(&path));
                    self.themes = theme::discover();
                    match result {
                        Ok(t) => {
                            let id = t.id.clone();
                            self.set_theme(ctx, &id);
                            if let Some(p) = &mut self.prefs {
                                p.message = Some((true, format!("Installed “{}”", t.display_name)));
                                p.install_path.clear();
                            }
                        }
                        Err(e) => {
                            if let Some(p) = &mut self.prefs {
                                p.message = Some((false, format!("{e:#}")));
                            }
                        }
                    }
                }
                PrefsAction::AddAccount => self.setup = Some(AccountSetup::default()),
                PrefsAction::RemoveAccount(id) => {
                    if let Some(a) = self.accounts.iter().find(|a| a.id == id) {
                        if !a.demo {
                            let _ = credentials::delete_password(&a.email);
                        }
                    }
                    let _ = self.store.remove_account(id);
                    self.set_selection(Selection::Unified(FolderRole::Inbox));
                }
                PrefsAction::SaveAccount(a) => {
                    let _ = self.store.update_account(&a);
                    if let Some(p) = &mut self.prefs {
                        p.editing = None;
                        p.message = Some((true, "Saved".into()));
                    }
                    self.reload();
                }
                PrefsAction::SaveSignature(mut sig) => {
                    // Pasted pages (e.g. from Canary) carry styles that would restyle the whole email.
                    let cleaned = sig.html && {
                        let c = springpost_core::parse::clean_signature_html(&sig.body);
                        let changed = c != sig.body.trim();
                        sig.body = c;
                        changed
                    };
                    match self.store.save_signature(&sig) {
                    Ok(id) => {
                        self.reload();
                        if let Some(p) = &mut self.prefs {
                            p.sig_edit = Some(Signature { id, ..sig });
                            p.message = Some((true, if cleaned { "Signature saved — removed the copied page styles".into() } else { "Signature saved".into() }));
                        }
                    }
                    Err(e) => self.toast(format!("Couldn't save signature: {e:#}"), true),
                    }
                }
                PrefsAction::DeleteSignature(id) => {
                    let _ = self.store.delete_signature(id);
                    if let Some(p) = &mut self.prefs {
                        p.sig_edit = None;
                        p.message = Some((true, "Signature deleted".into()));
                    }
                    self.reload();
                }
                PrefsAction::SetDefaultSignature(account, sig) => {
                    let _ = self.store.set_default_signature(account, sig);
                    self.reload();
                }
                PrefsAction::AddDemo => {
                    let _ = demo::seed(&self.store);
                    self.reload();
                }
                PrefsAction::RemoveDemo => {
                    let _ = demo::remove(&self.store);
                    self.set_selection(Selection::Unified(FolderRole::Inbox));
                }
            }
        }
        // Toasts.
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(if t.error { 8 } else { 4 }));
        if !self.toasts.is_empty() {
            egui::Area::new(egui::Id::new("toasts"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -20.0])
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    for t in &self.toasts {
                        egui::Frame::new()
                            .fill(if t.error { col.error } else { egui::Color32::from_rgb(40, 44, 52) })
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(16, 10))
                            .show(ui, |ui| {
                                ui.label(RichText::new(&t.text).color(egui::Color32::WHITE));
                            });
                        ui.add_space(6.0);
                    }
                });
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

/// Let signature images load from the web.
pub fn install_image_loaders(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    // Tried first: decodes by content, so mislabelled images (Dropbox) still show.
    ctx.add_image_loader(std::sync::Arc::new(crate::html_view::SniffingImageLoader::default()));
}

fn swipe_color(col: &Colors, a: SwipeAction) -> egui::Color32 {
    match a {
        SwipeAction::Archive => col.success,
        SwipeAction::Trash => col.error,
        SwipeAction::Snooze => col.snooze,
        SwipeAction::Pin => col.accent,
        SwipeAction::ToggleRead => col.unread,
        SwipeAction::Nothing => col.list,
    }
}

fn swipe_icon(a: SwipeAction) -> Icon {
    match a {
        SwipeAction::Archive => Icon::Done,
        SwipeAction::Trash => Icon::Trash,
        SwipeAction::Snooze => Icon::Snooze,
        SwipeAction::Pin => Icon::PinFilled,
        SwipeAction::ToggleRead | SwipeAction::Nothing => Icon::Envelope,
    }
}

fn swipe_label(a: SwipeAction, unread: bool, pinned: bool) -> &'static str {
    match a {
        SwipeAction::Archive => "Archive",
        SwipeAction::Trash => "Trash",
        SwipeAction::Snooze => "Snooze",
        SwipeAction::Pin if pinned => "Unpin",
        SwipeAction::Pin => "Pin",
        SwipeAction::ToggleRead if unread => "Mark read",
        SwipeAction::ToggleRead => "Mark unread",
        SwipeAction::Nothing => "",
    }
}

fn expanded_default(msgs: &[Message]) -> Vec<bool> {
    let n = msgs.len();
    msgs.iter().enumerate().map(|(i, m)| i + 1 == n || m.unread).collect()
}

/// The list's "who" line, Canary style: the latest sender first (shown in
/// bold), then everyone else, e.g. ("Tim", ", Bee & Tom", true).
/// The flag is set when you sent the latest message.
fn participants_parts(ps: &[Contact], me: Option<&Account>) -> (String, String, bool) {
    let my_email = me.map(|a| a.email.to_ascii_lowercase());
    let is_me = |c: &Contact| Some(c.email.to_ascii_lowercase()) == my_email;
    let my_name = me.map(|a| a.display_name.split_whitespace().next().unwrap_or("").to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Me".into());
    // `ps` is oldest-first; show newest-first.
    let newest: Vec<&Contact> = ps.iter().rev().collect();
    let name = |c: &Contact| if is_me(c) { my_name.clone() } else if newest.len() > 1 { c.short() } else { c.display() };
    let Some(first) = newest.first() else { return (String::new(), String::new(), false) };
    let others: Vec<String> = newest[1..].iter().take(3).map(|c| name(c)).collect();
    let mut rest = String::new();
    for (i, n) in others.iter().enumerate() {
        rest.push_str(if i + 1 == others.len() && newest.len() <= 4 { " & " } else { ", " });
        rest.push_str(n);
    }
    if newest.len() > 4 {
        rest.push_str(&format!(" & {} more", newest.len() - 4));
    }
    (name(first), rest, is_me(first))
}

fn section(ui: &mut egui::Ui, col: &Colors, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).small().strong().color(col.sidebar_text));
    ui.add_space(2.0);
}

fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color));
        });
}

/// A label chip with a × to remove it.
fn label_chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    let r = egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(10)
        .inner_margin(egui::Margin { left: 8, right: 4, top: 2, bottom: 2 })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new(text).small().color(color));
            ui.add(egui::Button::new(RichText::new("×").small().color(color)).frame(false).small())
        });
    r.inner.on_hover_text("Remove label")
}

/// How a folder is named in menus: the role for system folders, else its path.
fn folder_label(f: &Folder) -> String {
    match f.role {
        FolderRole::Other => f.path.clone(),
        FolderRole::All => "Archive (All Mail)".into(),
        r => r.label().into(),
    }
}

fn nav_item(ui: &mut egui::Ui, col: &Colors, icon: &str, label: &str, count: i64, selected: bool, accent: Option<egui::Color32>) -> bool {
    let h = 28.0;
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::click());
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 6.0, col.sidebar_active);
    } else if r.hovered() {
        p.rect_filled(rect, 6.0, col.sidebar_active.gamma_multiply(0.5));
    }
    let fg = if selected { col.sidebar_active_text } else { col.sidebar_text };
    p.text(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(14.0),
        accent.unwrap_or(fg),
    );
    let mut badge_w = 0.0;
    if count > 0 {
        let g = p.layout_no_wrap(count.to_string(), egui::FontId::proportional(11.0), col.inverse);
        badge_w = g.size().x + 12.0;
        let br = egui::Rect::from_center_size(egui::pos2(rect.right() - 6.0 - badge_w / 2.0, rect.center().y), egui::vec2(badge_w, 17.0));
        p.rect_filled(br, 8.5, accent.unwrap_or(col.accent));
        p.galley(br.center() - g.size() / 2.0, g, col.inverse);
    }
    style::line(
        &p,
        egui::pos2(rect.left() + 34.0, rect.center().y - 8.5),
        label,
        egui::FontId::proportional(13.5),
        fg,
        rect.width() - 44.0 - badge_w,
    );
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Returns true when the header was clicked (toggle expand).
fn message_card(ui: &mut egui::Ui, col: &Colors, m: &Message, expanded: bool, is_last: bool) -> bool {
    let mut toggled = false;
    egui::Frame::new()
        .fill(col.bg)
        .stroke(egui::Stroke::new(1.0_f32, col.border))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(16))
        .shadow(egui::Shadow { offset: [0, 1], blur: 4, spread: 0, color: egui::Color32::from_black_alpha(18) })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui.horizontal(|ui| {
                style::avatar(ui, &m.from.name, &m.from.email, 36.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(m.from.display()).strong().color(col.text));
                        if !m.from.name.is_empty() {
                            ui.label(RichText::new(format!("<{}>", m.from.email)).small().color(col.very_subtle));
                        }
                    });
                    if expanded {
                        let mut to = format!("to {}", m.to.iter().map(|c| c.display()).collect::<Vec<_>>().join(", "));
                        if !m.cc.is_empty() {
                            to.push_str(&format!("  ·  cc {}", m.cc.iter().map(|c| c.display()).collect::<Vec<_>>().join(", ")));
                        }
                        ui.add(egui::Label::new(RichText::new(to).small().color(col.subtle)).truncate());
                    } else {
                        ui.add(egui::Label::new(RichText::new(&m.snippet).small().color(col.subtle)).truncate());
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.label(RichText::new(style::long_date(m.date)).small().color(col.very_subtle));
                    if m.starred {
                        ui.label(RichText::new("★").color(col.star));
                    }
                });
            });
            if !is_last && ui.interact(header.response.rect, ui.id().with(("hdr", m.id)), Sense::click()).clicked() {
                toggled = true;
            }
            if expanded {
                ui.add_space(10.0);
                body(ui, col, &m.body_text);
                if !m.attachments.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        for a in &m.attachments {
                            egui::Frame::new()
                                .fill(col.bg_secondary)
                                .stroke(egui::Stroke::new(1.0_f32, col.border))
                                .corner_radius(6)
                                .inner_margin(egui::Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.label(format!("📎 {}", a.filename));
                                    ui.label(RichText::new(human_size(a.size)).small().color(col.subtle));
                                });
                        }
                    });
                }
            }
        });
    toggled
}

/// Render a plain-text body: quoted lines dimmed, links clickable.
fn body(ui: &mut egui::Ui, col: &Colors, text: &str) {
    let mut quoted = String::new();
    let flush_quote = |ui: &mut egui::Ui, q: &mut String| {
        if q.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(RichText::new("••• show quoted text").small().color(col.very_subtle))
            .id_salt(ui.next_auto_id())
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new(q.trim_end()).color(col.subtle));
            });
        q.clear();
    };
    let mut para = String::new();
    let flush_para = |ui: &mut egui::Ui, p: &mut String| {
        if p.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (i, line) in p.trim_end_matches('\n').split('\n').enumerate() {
                if i > 0 {
                    ui.end_row();
                }
                for (j, word) in line.split(' ').enumerate() {
                    if j > 0 {
                        ui.label(" ");
                    }
                    let w = word.trim_end_matches(['.', ',', ')', ';']);
                    if w.starts_with("http://") || w.starts_with("https://") {
                        ui.hyperlink_to(RichText::new(w).color(col.link), w);
                        ui.label(&word[w.len()..]);
                    } else {
                        ui.label(RichText::new(word).color(col.text));
                    }
                }
            }
        });
        p.clear();
    };
    for line in text.lines() {
        let is_quote = line.trim_start().starts_with('>') || (line.starts_with("On ") && line.trim_end().ends_with("wrote:"));
        if is_quote || !quoted.is_empty() && line.trim().is_empty() {
            flush_para(ui, &mut para);
            quoted.push_str(line);
            quoted.push('\n');
        } else {
            flush_quote(ui, &mut quoted);
            para.push_str(line);
            para.push('\n');
        }
    }
    flush_para(ui, &mut para);
    flush_quote(ui, &mut quoted);
}

fn human_size(b: usize) -> String {
    match b {
        0..=1023 => format!("{b} B"),
        1024..=1_048_575 => format!("{:.0} KB", b as f64 / 1024.0),
        _ => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

impl eframe::App for SpringPostApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(THEME_KEY, self.theme_id.clone());
        storage.set_string(LIST_KEY, self.list_settings.encode());
    }
}

impl SpringPostApp {
    pub fn frame(&mut self, ctx: &egui::Context) {
        self.handle_events();
        if self.accounts.is_empty() {
            let col = self.colors.clone();
            egui::CentralPanel::default().frame(egui::Frame::new().fill(col.bg_secondary)).show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match self.welcome.welcome(ui, &col) {
                    SetupAction::Submit(a, pw) => self.worker.send(Cmd::AddAccount(a, pw)),
                    SetupAction::Demo => {
                        let _ = demo::seed(&self.store);
                        self.reload();
                    }
                    _ => {}
                });
            });
            self.windows(ctx);
            return;
        }
        self.handle_keys(ctx);
        self.toolbar(ctx);
        self.sidebar(ctx);
        self.thread_list(ctx);
        self.reader(ctx);
        self.windows(ctx);
        // Keep relative times and snooze wake-ups fresh.
        ctx.request_repaint_after(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use egui_kittest::Harness;

    fn shots_dir() -> std::path::PathBuf {
        let d = std::env::var("SPRINGPOST_SHOTS").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("springpost-shots"));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn harness(theme: &str) -> Harness<'static, SpringPostApp> {
        harness_with(theme, |_| {})
    }

    fn harness_with(theme: &str, setup: impl FnOnce(&mut SpringPostApp)) -> Harness<'static, SpringPostApp> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let db = std::env::temp_dir().join(format!("sp-ui-{theme}-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let store = Store::open(&db).unwrap();
        let theme = theme.to_string();
        let h = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_state(|ctx, app: &mut SpringPostApp| app.frame(ctx), {
                let ctx = egui::Context::default();
                let mut app = SpringPostApp::with_context(&ctx, Some(theme), store, true);
                setup(&mut app);
                app
            });
        // The app was built against a scratch context; set up the harness's own.
        let c = h.state().colors.clone();
        c.apply(&h.ctx);
        install_image_loaders(&h.ctx);
        h
    }

    fn save(h: &mut Harness<'static, SpringPostApp>, name: &str) {
        match h.render() {
            Ok(img) => {
                let p = shots_dir().join(format!("{name}.png"));
                img.save(&p).unwrap();
                eprintln!("wrote {}", p.display());
            }
            Err(e) => eprintln!("render unavailable: {e}"),
        }
    }

    #[test]
    fn renders_every_builtin_theme_and_snoozes() {
        for theme in ["springpost-light", "springpost-dark", "springpost-darkside"] {
            let mut h = harness(theme);
            let c = h.state().colors.clone();
        c.apply(&h.ctx);
            h.state_mut().titlebar_inset = 76.0;
            h.run_steps(3);
            // Pin an older conversation; it should jump to the top.
            let (a, k) = { let r = &h.state().rows[3]; (r.t.account_id, r.t.thread_key.clone()) };
            h.state_mut().toggle_pin(a, &k);
            assert_eq!(h.state().rows[0].t.thread_key, k);
            assert!(h.state().rows[0].t.pinned);
            // Open the first thread, like a click would.
            h.state_mut().select(0);
            h.run_steps(3);
            save(&mut h, theme);
            if theme == "springpost-light" {
                let before = h.state().rows.len();
                h.state_mut().act_snooze(Utc::now() + chrono::Duration::hours(4));
                h.run_steps(2);
                assert_eq!(h.state().rows.len(), before - 1);
                assert_eq!(h.state().snoozed_count, 1);
                h.state_mut().set_selection(Selection::Snoozed);
                h.state_mut().select(0);
                h.run_steps(3);
                save(&mut h, "snoozed-view");
                h.state_mut().compose_reply(0);
                h.run_steps(3);
                save(&mut h, "composer");
                h.state_mut().prefs = Some(Prefs::new(Tab::Appearance));
                h.run_steps(3);
                save(&mut h, "prefs-themes");
                h.state_mut().prefs = Some(Prefs::new(Tab::Mcp));
                h.run_steps(3);
                save(&mut h, "prefs-mcp");
            }
        }
    }

    /// Render real Mailspring theme packages when pointed at a folder of them:
    /// `SPRINGPOST_TEST_THEMES=/path/to/packages cargo test -p springpost`
    #[test]
    fn renders_external_mailspring_themes() {
        let Ok(dir) = std::env::var("SPRINGPOST_TEST_THEMES") else { return };
        let mut dirs: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let Ok(t) = theme::Theme::load_package(&d, theme::ThemeSource::Mailspring) else { continue };
            let mut h = harness("springpost-light");
            h.state_mut().themes.push(t.clone());
            let ctx = h.ctx.clone();
            h.state_mut().set_theme(&ctx, &t.id);
            h.run_steps(2);
            h.state_mut().select(1);
            h.run_steps(3);
            save(&mut h, &format!("mailspring-{}", t.id));
        }
    }

    fn row_center(h: &Harness<'static, SpringPostApp>, i: usize) -> egui::Pos2 {
        let s = h.state();
        egui::pos2((s.sidebar_right + s.list_right) / 2.0, 52.0 + ROW_H / 2.0 + ROW_H * i as f32)
    }

    #[test]
    fn swipe_quick_actions_and_composer_window() {
        let mut h = harness_with("springpost-light", |a| a.list_default_width = 500.0);
        h.run_steps(4);
        let before = h.state().rows.len();
        let first = h.state().rows[0].t.thread_key.clone();

        // Mouse drag to the right past the threshold: archive (the default).
        let start = row_center(&h, 0);
        h.drag_at(start);
        h.run_steps(1);
        for dx in [40.0, 90.0, 140.0, 190.0] {
            h.hover_at(start + egui::vec2(dx, 0.0));
            h.run_steps(1);
        }
        save(&mut h, "swipe-right-archive");
        h.drop_at(start + egui::vec2(190.0, 0.0));
        h.run_steps(3);
        assert_eq!(h.state().rows.len(), before - 1, "drag-swipe should archive");
        assert!(h.state().rows.iter().all(|r| r.t.thread_key != first));
        assert!(h.state().open.is_none(), "a swipe isn't a click");
        assert!(h.state().toasts.iter().any(|t| t.text == "Archived"));

        // A short drag springs back without doing anything.
        let start = row_center(&h, 0);
        h.drag_at(start);
        h.run_steps(1);
        h.hover_at(start - egui::vec2(30.0, 0.0));
        h.run_steps(1);
        h.drop_at(start - egui::vec2(30.0, 0.0));
        h.run_steps(8);
        assert_eq!(h.state().rows.len(), before - 1);

        // Two-finger trackpad swipe to the left: trash.
        let target = h.state().rows[1].t.thread_key.clone();
        h.hover_at(row_center(&h, 1));
        h.run_steps(1);
        for _ in 0..8 {
            h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(-25.0, 0.0), modifiers: Default::default() });
            h.run_steps(1);
        }
        save(&mut h, "swipe-left-trash");
        std::thread::sleep(Duration::from_millis(200));
        h.run_steps(3);
        assert!(h.state().rows.iter().all(|r| r.t.thread_key != target), "trackpad swipe should trash");
        assert_eq!(h.state().rows.len(), before - 2);

        // Hover quick actions in the wide list: click the pin (5th button).
        let c = row_center(&h, 2);
        h.hover_at(c);
        h.run_steps(2);
        save(&mut h, "quick-actions");
        let key = h.state().rows[2].t.thread_key.clone();
        let right = h.state().list_right - 1.0 - 16.0;
        let pin_btn = egui::pos2(right - 3.0 - 14.0, c.y);
        h.hover_at(pin_btn);
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: pin_btn, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: pin_btn, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(2);
        assert_eq!(h.state().rows[0].t.thread_key, key, "quick-action pin should move it to the top");
        assert!(h.state().open.is_none(), "quick actions don't open the conversation");

        // No quick actions when the list is narrow.
        let mut n = harness("springpost-light");
        n.run_steps(3);
        n.hover_at(row_center(&n, 0));
        n.run_steps(2);
        save(&mut n, "narrow-no-quick-actions");

        // The composer is its own native window; the offscreen test renderer
        // can't open windows, so egui embeds it and we check that path draws.
        h.state_mut().compose_new();
        h.run_steps(3);
        assert_eq!(h.state().composers.len(), 1);
        save(&mut h, "composer-new");
    }

    #[test]
    fn move_and_label() {
        let mut h = harness("springpost-light");
        h.run_steps(3);
        let idx = h.state().rows.iter().position(|r| r.t.subject.contains("IMAP IDLE")).unwrap();
        h.state_mut().select(idx);
        h.run_steps(2);
        // Open the Label picker with `l`, like the keyboard shortcut.
        h.event(egui::Event::Text("l".into()));
        h.run_steps(3);
        save(&mut h, "label-picker");
        let (a, k) = h.state().current().unwrap();
        let clients = h.state().folders[&a].iter().find(|f| f.path == "Clients").cloned().unwrap();
        h.state_mut().set_label(a, &k, &clients, true);
        // The worker applies it in the background.
        for _ in 0..50 {
            h.run_steps(1);
            if h.state().store.thread_folder_ids(a, &k).unwrap().contains(&clients.id) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(h.state().store.thread_folder_ids(a, &k).unwrap().contains(&clients.id));
        h.run_steps(2);
        save(&mut h, "labels-in-reader");

        // Move to a folder: leaves the inbox.
        let before = h.state().rows.len();
        let proj = h.state().folders[&a].iter().find(|f| f.path == "Projects/SpringPost").cloned().unwrap();
        h.state_mut().move_to(a, &k, &proj);
        assert_eq!(h.state().rows.len(), before - 1);
        for _ in 0..50 {
            h.run_steps(1);
            let inbox = h.state().store.folder_by_role(a, FolderRole::Inbox).unwrap().unwrap().id;
            if !h.state().store.thread_folder_ids(a, &k).unwrap().contains(&inbox) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        h.state_mut().reload();
        assert!(h.state().rows.iter().all(|r| r.t.thread_key != k), "moved thread should stay out of the inbox");
        h.state_mut().set_selection(Selection::Folder(proj.id));
        assert!(h.state().rows.iter().any(|r| r.t.thread_key == k));
    }

    #[test]
    fn signatures_tab_and_composer_picker() {
        let mut h = harness("springpost-light");
        h.run_steps(2);
        let work = h.state().store.save_signature(&Signature { id: 0, title: "Work".into(), body: "Alex Rivera\nNorthwind · Product".into(), html: false }).unwrap();
        let gmail = h.state().accounts.iter().find(|a| a.is_gmail()).unwrap().id;
        h.state().store.set_default_signature(gmail, Some(work)).unwrap();
        h.state_mut().reload();
        h.state_mut().prefs = Some(Prefs::new(Tab::Signatures));
        h.run_steps(3);
        save(&mut h, "prefs-signatures");
        h.state_mut().prefs = None;
        // A new message from the Gmail account gets its default signature.
        let acc = h.state().accounts.iter().find(|a| a.id == gmail).cloned().unwrap();
        h.state_mut().composers.push(Composer::new(99, &acc));
        h.run_steps(3);
        assert!(h.state().composers[0].draft().body.ends_with("Alex Rivera\nNorthwind · Product"));
        save(&mut h, "composer-signature");
        h.state_mut().composers.clear();

        // An HTML signature: edited with the toggle on, shown as text while
        // composing, sent as HTML.
        let html = Signature {
            id: 0,
            title: "Canary".into(),
            body: include_str!("../../springpost-core/tests/fixtures/canary-signature.html").into(),
            html: true,
        };
        let id = h.state().store.save_signature(&html).unwrap();
        h.state().store.set_default_signature(gmail, Some(id)).unwrap();
        h.state_mut().reload();
        let mut p = Prefs::new(Tab::Signatures);
        p.sig_edit = Some(Signature { id, ..html.clone() });
        h.state_mut().prefs = Some(p);
        // Give the signature image a moment to download (it renders a spinner offline).
        for _ in 0..40 {
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(100));
        }
        let url = "https://www.dropbox.com/scl/fi/4p1cn5i52pwncw4ef7qer/Timbo-headshot-2025-Sig-250x250.png?rlkey=tf2nqzy247vscirjk8x923fet&st=56dp1mmc&dl=1";
        match h.ctx.try_load_image(url, egui::SizeHint::default()) {
            Ok(egui::load::ImagePoll::Ready { image }) => eprintln!("signature image loaded: {:?}", image.size),
            Ok(egui::load::ImagePoll::Pending { .. }) => eprintln!("signature image still loading (slow or no network)"),
            Err(e) => eprintln!("signature image failed: {e}"),
        }
        save(&mut h, "prefs-signature-html");
        h.state_mut().prefs = None;
        let acc = h.state().accounts.iter().find(|a| a.id == gmail).cloned().unwrap();
        h.state_mut().composers.push(Composer::new(100, &acc));
        h.run_steps(3);
        let out = h.state().composers[0].draft();
        assert!(out.body.contains("Tim Morrison") && !out.body.contains("<b>"), "{}", out.body);
        assert!(out.html_signature.is_some());
        h.state_mut().composers.clear();
        let mut c2 = Composer::new(101, &acc);
        c2.to = "ann@example.com".into();
        c2.body = "Hi Ann,\n\nThanks for today — photos coming tomorrow.".into();
        h.state_mut().composers.push(c2);
        for _ in 0..10 {
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(50));
        }
        save(&mut h, "composer-signature-html");
    }

    #[test]
    fn command_w_closes_windows() {
        let mut h = harness("springpost-light");
        h.run_steps(2);
        // Preferences.
        h.state_mut().prefs = Some(Prefs::new(Tab::Appearance));
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
        h.run_steps(2);
        assert!(h.state().prefs.is_none(), "⌘W closes Preferences");
        // A blank composer closes straight away.
        h.state_mut().compose_new();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
        h.run_steps(2);
        assert!(h.state().composers.is_empty(), "⌘W closes an empty composer");
        // One with text asks first, and stays open.
        h.state_mut().compose_new();
        h.state_mut().composers[0].body = "Half-written thought".into();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
        h.run_steps(2);
        assert_eq!(h.state().composers.len(), 1, "⌘W never silently drops a draft");
        save(&mut h, "composer-confirm-close");
        // With Preferences open over a composer, ⌘W closes Preferences only.
        h.state_mut().prefs = Some(Prefs::new(Tab::Appearance));
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
        h.run_steps(2);
        assert!(h.state().prefs.is_none());
        assert_eq!(h.state().composers.len(), 1);
    }

    #[test]
    fn welcome_screen() {
        let db = std::env::temp_dir().join(format!("sp-ui-welcome-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let store = Store::open(&db).unwrap();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1100.0, 720.0))
            .build_state(|ctx, app: &mut SpringPostApp| app.frame(ctx), {
                let ctx = egui::Context::default();
                SpringPostApp::with_context(&ctx, Some("springpost-light".into()), store, false)
            });
        let c = h.state().colors.clone();
        c.apply(&h.ctx);
        h.run_steps(3);
        save(&mut h, "welcome");
    }
}
