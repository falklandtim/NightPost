//! Application state and the three-pane layout.

use crate::composer::{Composer, ComposerAction, SignatureBook};
use crate::icons::{self, Icon, Item};
use crate::onboarding::{AccountSetup, SetupAction};
use crate::prefs::{ListSettings, Prefs, PrefsAction, SwipeAction, Tab, QUICK_ACTIONS_MIN_WIDTH};
use crate::style::{self, Colors};
use crate::worker::{AttachmentDo, Cmd, Event, FolderUse, PushState, Worker};
use chrono::{DateTime, Utc};
use eframe::egui::{self, Key, RichText, Sense};
use owlpost_core::snooze::{self, SnoozePreset};
use owlpost_core::theme::{self, Theme};
use owlpost_core::*;
use std::collections::{HashMap, HashSet};
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
    ("⌘ Z", "Undo (move, archive, trash, label, snooze, pin…)"),
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

/// Quick filter from the chips above the list.
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
            ListFilter::All => "All",
            ListFilter::Unread => "Unread",
            ListFilter::Starred => "Starred",
            ListFilter::Pinned => "Pinned",
            ListFilter::Attachments => "Attachments",
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

/// One entry in the laid-out message list.
#[derive(Clone, Debug, PartialEq)]
enum ListItem {
    /// A section heading: "Pinned", "Today", "October"…
    Header(String),
    /// `rows[i]`.
    Row(usize),
}

/// Height of a section heading in the list.
const HEADER_H: f32 = 30.0;
/// Space above the first item.
const LIST_TOP: f32 = 4.0;

/// The date group a conversation falls in, Canary-style.
fn date_group(when: DateTime<Utc>, now: DateTime<Utc>) -> String {
    use chrono::Datelike;
    let (d, today) = (when.with_timezone(&chrono::Local).date_naive(), now.with_timezone(&chrono::Local).date_naive());
    let days = (today - d).num_days();
    let week_start = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    if days <= 0 {
        "Today".into()
    } else if days == 1 {
        "Yesterday".into()
    } else if d >= week_start {
        "Earlier this week".into()
    } else if d >= week_start - chrono::Duration::days(7) {
        "Last week".into()
    } else if d.year() == today.year() && d.month() == today.month() {
        "Earlier this month".into()
    } else if d.year() == today.year() {
        d.format("%B").to_string()
    } else {
        d.format("%B %Y").to_string()
    }
}

/// Lay out the list: a Pinned section, then conversations grouped by day.
/// Each entry comes with its top y (from the top of the list) and height.
fn list_layout(rows: &[Row], grouped: bool, hidden_pinned: usize, now: DateTime<Utc>) -> Vec<(f32, f32, ListItem)> {
    let mut out = Vec::with_capacity(rows.len() + 8);
    let mut y = LIST_TOP;
    let mut push = |out: &mut Vec<(f32, f32, ListItem)>, item: ListItem| {
        let h = if matches!(item, ListItem::Header(_)) { HEADER_H } else { ROW_H };
        out.push((y, h, item));
        y += h;
    };
    if !grouped {
        for i in 0..rows.len() {
            push(&mut out, ListItem::Row(i));
        }
        return out;
    }
    if hidden_pinned > 0 || rows.iter().any(|r| r.t.pinned) {
        push(&mut out, ListItem::Header("Pinned".into()));
    }
    for (i, _) in rows.iter().enumerate().filter(|(_, r)| r.t.pinned) {
        push(&mut out, ListItem::Row(i));
    }
    let mut last: Option<String> = None;
    for (i, r) in rows.iter().enumerate().filter(|(_, r)| !r.t.pinned) {
        let g = date_group(r.t.last_date, now);
        if last.as_ref() != Some(&g) {
            push(&mut out, ListItem::Header(g.clone()));
            last = Some(g);
        }
        push(&mut out, ListItem::Row(i));
    }
    out
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
    /// Shows an Undo button (for the latest undoable action).
    undo: bool,
}

/// Something ⌘Z can take back.
#[derive(Clone, Debug)]
enum Undo {
    /// A thread moved into `dest` out of `sources` (archive, trash, move…).
    Moved { a: AccountId, k: String, dest: FolderId, sources: Vec<FolderId>, what: String },
    /// A label added (or removed).
    Label { a: AccountId, k: String, folder: FolderId, added: bool, name: String },
    Snoozed { a: AccountId, k: String },
    /// Pin toggled; undoing toggles it back.
    Pin { a: AccountId, k: String },
    /// Read state changed: each thread with what it was before.
    Unread { items: Vec<(AccountId, String, bool)> },
    Star { a: AccountId, k: String, was: bool },
}

impl Undo {
    fn describe(&self) -> String {
        match self {
            Undo::Moved { what, .. } => what.clone(),
            Undo::Label { added, name, .. } => format!("{} “{name}”", if *added { "label" } else { "label removal" }),
            Undo::Snoozed { .. } => "snooze".into(),
            Undo::Pin { .. } => "pin".into(),
            Undo::Unread { items } if items.len() > 1 => "mark as read".into(),
            Undo::Unread { items } => if items.first().is_some_and(|i| i.2) { "mark as read".into() } else { "mark as unread".into() },
            Undo::Star { .. } => "star".into(),
        }
    }
}

/// How many actions ⌘Z can go back.
const UNDO_DEPTH: usize = 30;

pub struct OwlPostApp {
    store: Store,
    worker: Worker,
    /// Sender photos and logos.
    faces: crate::avatars::Faces,
    themes: Vec<Theme>,
    theme_id: String,
    colors: Colors,
    accounts: Vec<Account>,
    folders: HashMap<AccountId, Vec<Folder>>,
    unified_unread: HashMap<FolderRole, i64>,
    unified_drafts: i64,
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
    /// Most recent last.
    undo: Vec<Undo>,
    snooze_custom: String,
    pending_g: Option<Instant>,
    last_reload: Instant,
    filter: ListFilter,
    show_sidebar: bool,
    /// Right edges of the sidebar and thread list last frame, so the toolbar
    /// can line its sections up with the columns below.
    sidebar_right: f32,
    list_right: f32,
    /// Where the list's rows start on screen (below its header).
    list_top: f32,
    /// Room left for the macOS traffic lights when the toolbar is drawn in
    /// the title bar (0 when the system title bar is shown).
    pub titlebar_inset: f32,
    list_settings: ListSettings,
    /// Pinned conversations left out of the list while Pinned is hidden.
    hidden_pinned: usize,
    /// The web view for HTML email (created on the first frame).
    mail_view: Option<crate::mail_view::MailView>,
    mail_view_tried: bool,
    /// Show emails as designed (HTML) rather than as text.
    html_email: bool,
    /// Load remote images straight away (trackers are stripped regardless).
    load_images: bool,
    /// Where the web view goes this frame (the reading pane below the subject).
    web_rect: Option<egui::Rect>,
    /// Conversations whose remote images the user chose to load.
    remote_images: HashSet<(AccountId, String)>,
    /// Keys pressed in the web view, fed to egui next frame.
    pending_input: Vec<egui::Event>,
    swipe: Option<Swipe>,
    last_wheel_x: Option<Instant>,
    /// Initial width of the message list (it's resizable afterwards).
    list_default_width: f32,
    /// Text typed into the Move / Label picker.
    picker_query: String,
    sigs: SignatureBook,
    /// Attachments being downloaded: (message, attachment index).
    downloading: HashSet<(MessageId, usize)>,
    /// Push (IMAP IDLE) status per account.
    push: HashMap<AccountId, PushState>,
    /// A Google OAuth client is saved in the keychain.
    google_ready: bool,
    /// A "Sign in with Google" under way: its cancel flag and, once the
    /// browser is open, the sign-in URL.
    google: Option<(std::sync::Arc<std::sync::atomic::AtomicBool>, Option<String>)>,
}

const THEME_KEY: &str = "owlpost.theme";
/// Settings keys from before the rename (SpringPost), read as a fallback.
const LEGACY_THEME_KEY: &str = "springpost.theme";
const LEGACY_LIST_KEY: &str = "springpost.list";
/// Height of a conversation in the list: names, subject and two preview lines.
const ROW_H: f32 = 94.0;
const LIST_KEY: &str = "owlpost.list";
const HTML_KEY: &str = "owlpost.html";
const IMAGES_KEY: &str = "owlpost.images";
const FACES_KEY: &str = "owlpost.faces";

impl OwlPostApp {
    pub fn new(cc: &eframe::CreationContext<'_>, store: Store, demo: bool) -> Self {
        let get = |key: &str, legacy: &str| cc.storage.and_then(|s| s.get_string(key).or_else(|| s.get_string(legacy)));
        // Built-in theme ids were renamed along with the app.
        let saved = get(THEME_KEY, LEGACY_THEME_KEY).map(|id| id.replace("springpost-", "owlpost-"));
        let mut app = Self::with_context(&cc.egui_ctx, saved, store, demo);
        if let Some(h) = cc.storage.and_then(|s| s.get_string(HTML_KEY)) {
            app.html_email = h != "0";
        }
        if let Some(i) = cc.storage.and_then(|s| s.get_string(IMAGES_KEY)) {
            app.load_images = i != "ask";
        }
        if let Some(f) = cc.storage.and_then(|s| s.get_string(FACES_KEY)) {
            app.faces.enabled.set(f != "0");
        }
        if let Some(l) = get(LIST_KEY, LEGACY_LIST_KEY) {
            app.list_settings = ListSettings::decode(&l);
        }
        if cfg!(target_os = "macos") {
            app.titlebar_inset = 76.0;
        }
        // Debug builds: OWLPOST_OPEN=<words> opens a matching conversation at
        // launch (for checking the message view by hand).
        #[cfg(debug_assertions)]
        if let Ok(q) = std::env::var("OWLPOST_OPEN") {
            if let Some(i) = app.rows.iter().position(|r| r.t.subject.contains(q.as_str())) {
                app.select(i);
            }
        }
        app
    }

    pub fn with_context(ctx: &egui::Context, saved_theme: Option<String>, store: Store, demo: bool) -> Self {
        if demo {
            let _ = demo::seed(&store);
        }
        install_image_loaders(ctx);
        let themes = theme::discover();
        let theme_id = saved_theme.filter(|id| themes.iter().any(|t| &t.id == id)).unwrap_or_else(|| theme::MOONLIT.into());
        let colors = Colors::new(theme_palette(&themes, &theme_id, system_dark(ctx)));
        colors.apply(ctx);
        let worker = Worker::spawn(store.path().to_path_buf(), ctx.clone());
        // Tests never touch the real keychain.
        if !cfg!(test) {
            worker.send(Cmd::CheckGoogle);
        }
        let faces = crate::avatars::Faces::new(ctx, worker.avatars.clone());
        let mut app = Self {
            store,
            worker,
            faces,
            themes,
            theme_id,
            colors,
            accounts: vec![],
            folders: HashMap::new(),
            unified_unread: HashMap::new(),
            unified_drafts: 0,
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
            undo: vec![],
            snooze_custom: String::new(),
            pending_g: None,
            last_reload: Instant::now(),
            filter: ListFilter::All,
            show_sidebar: true,
            sidebar_right: 220.0,
            list_right: 620.0,
            list_top: 0.0,
            titlebar_inset: 0.0,
            list_settings: ListSettings::default(),
            hidden_pinned: 0,
            mail_view: None,
            mail_view_tried: false,
            html_email: true,
            load_images: true,
            web_rect: None,
            remote_images: HashSet::new(),
            pending_input: vec![],
            swipe: None,
            last_wheel_x: None,
            list_default_width: 400.0,
            picker_query: String::new(),
            sigs: SignatureBook::default(),
            downloading: HashSet::new(),
            push: HashMap::new(),
            // Checked in the background (a keychain read can wait on a
            // macOS permission prompt); see Event::GoogleReady.
            google_ready: false,
            google: None,
        };
        app.reload();
        if app.accounts.iter().any(|a| !a.demo) {
            app.worker.send(Cmd::SyncAll);
        }
        app
    }

    // ----- data -------------------------------------------------------------

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, at: Instant::now(), undo: false });
    }

    /// Remember an action for ⌘Z and say what happened, with an Undo button.
    fn did(&mut self, text: impl Into<String>, undo: Undo) {
        self.undo.push(undo);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        for t in &mut self.toasts {
            t.undo = false;
        }
        self.toasts.push(Toast { text: text.into(), error: false, at: Instant::now(), undo: true });
    }

    /// ⌘Z: take back the most recent action.
    fn undo_last(&mut self) {
        let Some(u) = self.undo.pop() else {
            self.toast("Nothing to undo", false);
            return;
        };
        for t in &mut self.toasts {
            t.undo = false;
        }
        match &u {
            Undo::Moved { a, k, dest, sources, .. } => {
                self.worker.send(Cmd::Restore(*a, k.clone(), *dest, sources.clone()));
            }
            Undo::Label { a, k, folder, added, .. } => {
                self.worker.send(Cmd::Label(*a, k.clone(), *folder, !added));
            }
            Undo::Snoozed { a, k } => {
                let _ = self.store.unsnooze(*a, k);
                self.snoozed_count = self.store.snoozed_count().unwrap_or(0);
            }
            Undo::Pin { a, k } => {
                let _ = if self.store.is_pinned(*a, k).unwrap_or(false) { self.store.unpin(*a, k) } else { self.store.pin(*a, k) };
            }
            Undo::Unread { items } => {
                for (a, k, was) in items {
                    let _ = self.store.set_thread_unread(*a, k, *was);
                    self.worker.send(Cmd::SetUnread(*a, k.clone(), *was));
                }
            }
            Undo::Star { a, k, was } => {
                let _ = self.store.set_thread_starred(*a, k, *was);
                self.worker.send(Cmd::SetStarred(*a, k.clone(), *was));
            }
        }
        self.toast(format!("Undid {}", u.describe()), false);
        self.reload();
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
        self.unified_drafts = self.store.unified_total(FolderRole::Drafts).unwrap_or(0);
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
        // A hidden Pinned section keeps its conversations out of the list
        // (and out of j/k), until "Show".
        self.hidden_pinned = 0;
        if self.list_settings.hide_pinned && self.filter != ListFilter::Pinned && self.selection != Selection::Snoozed {
            self.hidden_pinned = self.rows.iter().filter(|r| r.t.pinned).count();
            self.rows.retain(|r| !r.t.pinned);
        }
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
        // Where it's going and what it leaves, so ⌘Z can bring it back.
        let plan = self.accounts.iter().find(|x| x.id == a).and_then(|acc| {
            let target = engine::folder_for_role(&self.store, acc, role).ok()?;
            Some((target.id, engine::move_plan(&self.store, acc, k, target.id, None).ok()?))
        });
        self.worker.send(Cmd::Move(a, k.to_string(), role));
        self.remove_row(a, k);
        let (text, what) = match role {
            FolderRole::Archive => ("Archived", "archive"),
            FolderRole::Trash => ("Moved to Trash", "move to Trash"),
            FolderRole::Spam => ("Reported as spam", "spam report"),
            FolderRole::Inbox => ("Moved to Inbox", "move to Inbox"),
            _ => ("Moved", "move"),
        };
        match plan {
            Some((dest, sources)) if !sources.is_empty() => {
                self.did(text, Undo::Moved { a, k: k.to_string(), dest, sources, what: what.into() });
            }
            _ => self.toast(text, false),
        }
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
        self.worker.send(Cmd::SetStarred(a, k.clone(), starred));
        self.undo.push(Undo::Star { a, k, was: !starred });
        self.reload();
    }

    fn act_unread(&mut self) {
        let Some((a, k)) = self.current() else { return };
        let _ = self.store.set_thread_unread(a, &k, true);
        self.worker.send(Cmd::SetUnread(a, k.clone(), true));
        self.did("Marked as unread", Undo::Unread { items: vec![(a, k, false)] });
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
        self.did(format!("Snoozed until {}", snooze::describe(until)), Undo::Snoozed { a, k });
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
        self.did(if pinned { "Unpinned" } else { "Pinned to top" }, Undo::Pin { a, k: k.to_string() });
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
        let n = unread.len();
        self.did(
            format!("Marked {n} conversation{} as read", if n == 1 { "" } else { "s" }),
            Undo::Unread { items: unread.into_iter().map(|(a, k)| (a, k, true)).collect() },
        );
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
            self.colors = Colors::new(theme_palette(&self.themes, id, system_dark(ctx)));
            self.colors.apply(ctx);
        }
    }

    /// "Moonlit" follows the system: switch when the Mac's appearance does.
    fn follow_system_theme(&mut self, ctx: &egui::Context) {
        if self.theme_id == theme::MOONLIT && system_dark(ctx) != self.colors.p.dark {
            let id = self.theme_id.clone();
            self.set_theme(ctx, &id);
        }
    }

    // ----- events -----------------------------------------------------------

    fn handle_events(&mut self) {
        let mut changed = false;
        while let Ok(ev) = self.worker.rx.try_recv() {
            match ev {
                // Progress text only: the toolbar spinner is for syncs you
                // asked for, not the quiet background ones.
                Event::Status(s) => self.status = s,
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
                Event::Notice(s) => self.toast(s, false),
                Event::GoogleReady(ready) => self.google_ready = ready,
                Event::Avatar(email, pic) => self.faces.insert(email, pic),
                Event::GoogleWaiting(url) => {
                    if let Some(g) = &mut self.google {
                        g.1 = Some(url);
                    }
                }
                Event::GoogleDone(r) => {
                    self.google = None;
                    match r {
                        Ok(msg) => {
                            if !msg.is_empty() {
                                self.toast(msg, false);
                            }
                            self.reload();
                        }
                        Err(e) => {
                            if !e.contains("cancelled") {
                                self.toast(format!("Google sign-in: {e}"), true);
                            }
                            for s in [&mut self.welcome].into_iter().chain(self.setup.as_mut()) {
                                s.error = Some(e.clone());
                            }
                        }
                    }
                }
                Event::Push(id, st) => {
                    self.push.insert(id, st);
                }
                Event::AttachmentDone(id, i) => {
                    self.downloading.remove(&(id, i));
                }
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
        // ⌘Z: undo the last action (text fields keep ⌘Z for their own undo).
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Z)) {
            self.undo_last();
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
            .exact_height(60.0)
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
                let (top, bottom) = (full.top(), full.bottom());
                let p = ui.painter().clone();
                p.hline(full.x_range(), bottom - 0.5, egui::Stroke::new(1.0_f32, col.border));
                let section = |l: f32, r: f32| egui::Rect::from_min_max(egui::pos2(l, top), egui::pos2(r.max(l), bottom));

                // Brand: the owl and the name, after the traffic lights.
                let logo_x = self.titlebar_inset + if self.titlebar_inset > 0.0 { 4.0 } else { 18.0 };
                let logo = egui::Rect::from_center_size(egui::pos2(logo_x + 15.0, full.center().y), egui::vec2(30.0, 30.0));
                egui::Image::new(egui::include_image!("../assets/logo-128.png")).corner_radius(8).paint_at(ui, logo);
                if col.p.dark {
                    p.rect_stroke(logo, 8.0, egui::Stroke::new(1.0_f32, col.border), egui::StrokeKind::Inside);
                }
                let toggle_w = 32.0;
                let a_right = if self.show_sidebar { self.sidebar_right.max(logo.right() + toggle_w + 16.0) } else { logo.right() + toggle_w + 20.0 };
                let toggle_x = a_right - toggle_w - 10.0;
                let name_font = egui::FontId::proportional(19.0);
                let name_w = p.layout_no_wrap("OwlPost".into(), name_font.clone(), col.heading).size().x;
                if self.show_sidebar && logo.right() + 10.0 + name_w + 4.0 < toggle_x {
                    style::bold_line(&p, egui::pos2(logo.right() + 10.0, full.center().y - 11.0), "OwlPost", name_font, col.heading, name_w + 2.0);
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(section(toggle_x, toggle_x + toggle_w)).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                    let fg = style::blend(col.toolbar_text, col.toolbar, 0.7);
                    if icons::bare(ui, Icon::Sidebar, if self.show_sidebar { "Hide sidebar" } else { "Show sidebar" }, fg, col.toolbar).clicked() {
                        self.show_sidebar = !self.show_sidebar;
                    }
                });

                // Actions, then search on the right.
                ui.scope_builder(egui::UiBuilder::new().max_rect(section(a_right + 14.0, full.right() - 16.0)).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    if icons::filled(ui, Icon::Pencil, "New message", col.button_emphasis, col.button_emphasis_text).on_hover_text("New message (c)").clicked() {
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
                            Item::new(Icon::Trash, "Move to Trash (#)").enabled(open),
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
                    if r[5].clicked() {
                        self.act_move(FolderRole::Trash);
                    }
                    self.more_popup(&r[6], starred);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let w = ui.available_width().clamp(140.0, 400.0);
                        let r = icons::search_field(ui, &col, &mut self.search, w, "Search mail");
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
        let sources = self.accounts.iter().find(|x| x.id == a).and_then(|acc| engine::move_plan(&self.store, acc, k, folder.id, from).ok()).unwrap_or_default();
        self.worker.send(Cmd::MoveTo(a, k.to_string(), folder.id, from));
        self.remove_row(a, k);
        let text = format!("Moved to {}", folder_label(folder));
        if sources.is_empty() {
            self.toast(text, false);
        } else {
            self.did(text, Undo::Moved { a, k: k.to_string(), dest: folder.id, sources, what: format!("move to {}", folder_label(folder)) });
        }
    }

    fn set_label(&mut self, a: AccountId, k: &str, folder: &Folder, add: bool) {
        self.worker.send(Cmd::Label(a, k.to_string(), folder.id, add));
        if !add && self.selection == Selection::Folder(folder.id) {
            self.remove_row(a, k);
        }
        let name = folder_label(folder);
        self.did(
            format!("{} “{name}”", if add { "Labelled" } else { "Removed label" }),
            Undo::Label { a, k: k.to_string(), folder: folder.id, added: add, name },
        );
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
            .default_width(244.0)
            .width_range(180.0..=320.0)
            .frame(egui::Frame::new().fill(col.sidebar).inner_margin(egui::Margin { left: 12, right: 12, top: 18, bottom: 10 }))
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
                        let count = match role {
                            FolderRole::Inbox | FolderRole::Spam => self.unified_unread.get(&role).copied().unwrap_or(0),
                            // Drafts show how many there are, as Mailspring does.
                            FolderRole::Drafts => self.unified_drafts,
                            _ => 0,
                        };
                        let sel = self.selection == Selection::Unified(role) && self.search.is_empty();
                        if nav_item(ui, &col, role_icon(role), label, count, sel) {
                            self.set_selection(Selection::Unified(role));
                        }
                        if role == FolderRole::Starred {
                            let sel = self.selection == Selection::Snoozed && self.search.is_empty();
                            if nav_item(ui, &col, Icon::Snooze, "Snoozed", self.snoozed_count, sel) {
                                self.set_selection(Selection::Snoozed);
                            }
                        }
                    }
                    ui.add_space(22.0);
                    let accounts = self.accounts.clone();
                    for a in &accounts {
                        let collapsed = self.collapsed_accounts.contains(&a.id);
                        let header = ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add_space(4.0);
                            let (chev, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), Sense::hover());
                            let c = chev.center();
                            let sub = col.sidebar_text.gamma_multiply(0.75);
                            let pts = if collapsed { vec![c + egui::vec2(-1.5, -3.5), c + egui::vec2(2.0, 0.0), c + egui::vec2(-1.5, 3.5)] } else { vec![c + egui::vec2(-3.5, -1.5), c + egui::vec2(0.0, 2.0), c + egui::vec2(3.5, -1.5)] };
                            ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.4_f32, sub)));
                            if multi {
                                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 4.0, style::hex(&a.color));
                            }
                            let mut job = egui::text::LayoutJob::default();
                            job.append(&a.email.to_uppercase(), 0.0, egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: sub, extra_letter_spacing: 1.1, ..Default::default() });
                            job.wrap = egui::text::TextWrapping::truncate_at_width(ui.available_width());
                            ui.add(egui::Label::new(job).sense(Sense::click()))
                        });
                        ui.add_space(6.0);
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
                        // With one account the list above already is that
                        // account's Inbox, Sent and so on.
                        for f in folders.iter().filter(|f| f.role != FolderRole::Other && (multi || !FolderRole::UNIFIED.contains(&f.role))) {
                            let count = if matches!(f.role, FolderRole::Inbox | FolderRole::Spam | FolderRole::Other) { f.unread } else { 0 };
                            let sel = self.selection == Selection::Folder(f.id) && self.search.is_empty();
                            let label = if f.role == FolderRole::All { "All Mail".to_string() } else { f.role.label().to_string() };
                            if nav_item(ui, &col, role_icon(f.role), &label, count, sel) {
                                self.set_selection(Selection::Folder(f.id));
                            }
                        }
                        for f in folders.iter().filter(|f| f.role == FolderRole::Other) {
                            let depth = f.path.matches(['/', '.']).count();
                            let sel = self.selection == Selection::Folder(f.id) && self.search.is_empty();
                            ui.horizontal(|ui| {
                                ui.add_space(depth as f32 * 14.0);
                                if nav_item(ui, &col, Icon::Folder, &f.name, f.unread, sel) {
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

    /// Unread conversations in what the list shows, for the header.
    fn list_unread(&self) -> i64 {
        if !self.search.trim().is_empty() || self.filter != ListFilter::All {
            return self.rows.iter().filter(|r| r.t.unread).count() as i64;
        }
        match &self.selection {
            Selection::Unified(r) => self.unified_unread.get(r).copied().unwrap_or(0),
            Selection::Folder(id) => self.folders.values().flatten().find(|f| f.id == *id).map(|f| f.unread).unwrap_or(0),
            Selection::Snoozed => self.rows.iter().filter(|r| r.t.unread).count() as i64,
        }
    }

    /// Above the list: the folder's name, today's date and unread count,
    /// list actions, and the filter chips.
    fn list_header(&mut self, ui: &mut egui::Ui, col: &Colors) {
        let mut chosen = None;
        egui::Frame::new().inner_margin(egui::Margin { left: 22, right: 14, top: 20, bottom: 12 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let fg = col.subtle;
                    let refresh = ui.add_enabled_ui(!self.syncing, |ui| icons::bare(ui, Icon::Refresh, "Check for new mail", fg, col.list)).inner;
                    if refresh.clicked() {
                        self.sync_now();
                    }
                    let unread = self.rows.iter().any(|r| r.t.unread);
                    if ui.add_enabled_ui(unread, |ui| icons::bare(ui, Icon::Check, "Mark all as read", fg, col.list)).inner.clicked() {
                        self.mark_all_read();
                    }
                    if self.syncing {
                        ui.add_space(4.0);
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), Sense::hover());
                        style::bold_line(ui.painter(), r.left_top() + egui::vec2(0.0, 1.0), &self.list_title(), egui::FontId::proportional(32.0), col.heading, r.width());
                    });
                });
            });
            let mut sub = chrono::Local::now().format("%A %-d %B").to_string();
            let unread = self.list_unread();
            if unread > 0 {
                sub.push_str(&format!(" · {unread} unread"));
            }
            ui.label(RichText::new(sub).size(13.0).color(col.subtle));
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                for f in ListFilter::ALL {
                    // "With attachments" is a paperclip, so the chips fit on one line.
                    let r = if f == ListFilter::Attachments {
                        filter_chip(ui, col, "", f == self.filter).on_hover_text("With attachments")
                    } else {
                        filter_chip(ui, col, f.label(), f == self.filter)
                    };
                    if r.clicked() && f != self.filter {
                        chosen = Some(f);
                    }
                }
            });
        });
        if let Some(f) = chosen {
            self.filter = f;
            self.selected = None;
            self.open = None;
            self.reload();
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
                self.list_header(ui, &col);
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
                let mut clicked = None;
                let mut menu: Option<(AccountId, String, u8)> = None;
                let mut folder_menu: Option<(AccountId, String, Folder, FolderUse, bool)> = None;
                let mut toggle_pinned = false;
                let multi = self.accounts.len() > 1;
                let grouped = self.selection != Selection::Snoozed;
                let layout = list_layout(&self.rows, grouped, self.hidden_pinned, Utc::now());
                let total = layout.last().map(|(y, h, _)| y + h).unwrap_or(0.0) + 12.0;
                self.list_top = ui.cursor().top();
                egui::ScrollArea::vertical().auto_shrink([false, false]).scroll_source(egui::scroll_area::ScrollSource { mouse_wheel: !swiping, ..egui::scroll_area::ScrollSource::ALL }).show_viewport(ui, |ui, viewport| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let width = ui.available_width();
                    let origin = ui.max_rect().min;
                    ui.allocate_space(egui::vec2(width, total));
                    for (y, h, item) in &layout {
                        if y + h < viewport.top() || *y > viewport.bottom() {
                            continue;
                        }
                        let i = match item {
                            ListItem::Header(title) => {
                                let r = egui::Rect::from_min_size(origin + egui::vec2(0.0, *y), egui::vec2(width, *h));
                                if section_header(ui, &col, r, title, (title == "Pinned").then_some(self.hidden_pinned)) {
                                    toggle_pinned = true;
                                }
                                continue;
                            }
                            ListItem::Row(i) => *i,
                        };
                        let t = self.rows[i].t.clone();
                        let snoozed_until = self.rows[i].snoozed_until;
                        let rect = egui::Rect::from_min_size(origin + egui::vec2(0.0, *y), egui::vec2(width, row_h));
                        let resp = ui.interact(rect, ui.id().with(("row", t.account_id, &t.thread_key)), Sense::click_and_drag());
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
                        // A soft highlight (Moonlit) keeps the normal text colours;
                        // a strong one (most Mailspring themes) inverts them.
                        let soft = selected && col.p.list_focused.over(col.p.list).contrast(col.p.list) < 1.6;
                        let (bg, fg, sub) = if selected && !soft {
                            (col.list_focused, col.list_focused_text, col.list_focused_text.gamma_multiply(0.8))
                        } else if selected {
                            (col.list_focused, col.list_focused_text, col.subtle)
                        } else if hovered {
                            (col.list_hover, col.text, col.subtle)
                        } else {
                            (col.list, col.text, col.subtle)
                        };
                        let selected = selected && !soft;
                        // Full-width rows with a hairline between them.
                        p.rect_filled(rect_c, 0.0, bg);
                        p.hline(rect_c.x_range(), rect_c.bottom() - 0.5, egui::Stroke::new(1.0_f32, col.list_border));
                        let rect = rect_c;
                        let top = rect.top();
                        let latest = t.participants.last().cloned().unwrap_or_default();
                        self.faces.paint(&p, egui::pos2(rect.left() + 36.0, top + 29.0), &latest.name, &latest.email, 34.0);
                        let x = rect.left() + 62.0;
                        let mut right = rect.right() - 20.0;

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
                        // Unread: a dot before the name, as in Canary.
                        if t.unread {
                            p.circle_filled(egui::pos2(nx + 4.0, top + 19.0), 4.0, if selected { fg } else { col.unread });
                            nx += 13.0;
                        }
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
                        let mut ix = rect.right() - 20.0;
                        let mut badge = |p: &egui::Painter, w: f32, draw: &dyn Fn(&egui::Painter, egui::Pos2)| {
                            draw(p, egui::pos2(ix - w / 2.0, top + 39.0));
                            ix -= w + 4.0;
                        };
                        if t.count > 1 {
                            // The message count in a small grey box, as in Canary.
                            let g = p.layout_no_wrap(t.count.to_string(), egui::FontId::proportional(11.5), fg);
                            let w = (g.size().x + 9.0).max(17.0);
                            let boxed = if selected { fg.gamma_multiply(0.25) } else { style::blend(col.text, bg, 0.09) };
                            badge(&p, w, &|p, c| {
                                p.rect_filled(egui::Rect::from_center_size(c, egui::vec2(w, 16.0)), 3.0, boxed);
                                p.galley(egui::pos2(c.x - g.size().x / 2.0, c.y - g.size().y / 2.0), g.clone(), fg);
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
                        style::paragraph(&p, egui::pos2(x, top + 51.0), &t.snippet, egui::FontId::proportional(12.5), sub, rect.right() - 20.0 - x, 2);

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
                            let qr = egui::Rect::from_min_size(egui::pos2(rect.right() - 20.0 - w, rect.center().y - h / 2.0), egui::vec2(w, h));
                            // Cover the right-hand end of the row, fading in from the left.
                            let hl = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - 1.0));
                            let cover = egui::Rect::from_min_max(egui::pos2(qr.left() - 8.0, hl.top()), hl.right_bottom());
                            p.rect_filled(cover, 0.0, bg);
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
                if toggle_pinned {
                    self.list_settings.hide_pinned = !self.list_settings.hide_pinned;
                    self.reload();
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
                self.did(if unread { "Marked as read" } else { "Marked as unread" }, Undo::Unread { items: vec![(a, k.to_string(), unread)] });
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
        // The conversation reads like a page: messages sit straight on the
        // background, separated by rules.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(col.bg))
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
                let subject = open.messages.first().map(|m| style::drawable(&m.subject).into_owned()).unwrap_or_default();
                // "OLLAMA · SAT 7 NOV, 12:53 PM": who wrote last, and when.
                let kicker = open
                    .messages
                    .last()
                    .map(|m| {
                        let who = if m.from.name.is_empty() { m.from.email.clone() } else { m.from.name.clone() };
                        let when = m.date.with_timezone(&chrono::Local).format("%a %-d %b, %-I:%M %p");
                        format!("{} · {}", style::drawable(&who), when).to_uppercase()
                    })
                    .unwrap_or_default();
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
                let mut attachment_req: Option<(Message, usize, AttachmentClick)> = None;
                let downloading = self.downloading.clone();
                // The account only needs naming when there's more than one.
                let account = self.accounts.iter().find(|a| a.id == open.account_id).filter(|_| self.accounts.len() > 1).cloned();

                // HTML email: the subject stays here and the web view shows
                // the conversation below it.
                let web = self.html_email && self.mail_view.is_some();
                let Some(open) = &mut self.open else { return };
                let mut draw = |ui: &mut egui::Ui| {
                    // Generous space around the conversation, like Canary.
                    let margin = egui::Margin { left: 48, right: 48, top: 34, bottom: if web { 4 } else { 36 } };
                    egui::Frame::new().inner_margin(margin).show(ui, |ui| {
                        let max_w = ui.available_width().min(860.0);
                        ui.set_max_width(max_w);
                        let mut job = egui::text::LayoutJob::default();
                        job.append(&kicker, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(12.0), color: col.link, extra_letter_spacing: 1.4, ..Default::default() });
                        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
                        ui.label(job);
                        ui.add_space(6.0);
                        heavy_text(ui, &subject, 30.0, col.heading);
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
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
                        if web {
                            return;
                        }
                        ui.add_space(16.0);
                        let n = open.messages.len();
                        for (i, m) in open.messages.iter().enumerate() {
                            let expanded = open.expanded.get(i).copied().unwrap_or(true);
                            if i > 0 {
                                ui.add_space(6.0);
                            }
                            let (r, clicked) = message_card(ui, &col, m, expanded, i + 1 == n, &downloading, &self.faces);
                            if let Some((idx, what)) = clicked {
                                attachment_req = Some((m.clone(), idx, what));
                            }
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
                                if icons::labeled(ui, icon, label, col.text, col.bg).clicked() {
                                    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("reply_click"), i as u8));
                                }
                            }
                        });
                    });
                };
                let mut web_rect = None;
                if web {
                    draw(ui);
                    web_rect = Some(ui.available_rect_before_wrap());
                } else {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| draw(ui));
                }
                self.web_rect = web_rect;
                if let Some(f) = remove_label {
                    self.set_label(thread_ref.0, &thread_ref.1, &f, false);
                }
                if let Some((m, idx, what)) = attachment_req {
                    self.attachment(m, idx, what);
                }
                if let Some(mode) = ctx.memory_mut(|m| m.data.remove_temp::<u8>(egui::Id::new("reply_click"))) {
                    self.compose_reply(mode);
                }
            });
    }

    /// Act on clicks and keys from the HTML message view.
    fn mail_view_events(&mut self, ctx: &egui::Context) {
        use crate::mail_view::ViewEvent;
        let events = self.mail_view.as_ref().map(|v| v.events()).unwrap_or_default();
        for e in events {
            match e {
                ViewEvent::OpenLink(url) => {
                    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    }
                }
                ViewEvent::Attachment(id, i) => {
                    if let Some(m) = self.open.as_ref().and_then(|o| o.messages.iter().find(|m| m.id == id)).cloned() {
                        self.attachment(m, i, AttachmentClick::Open);
                    }
                }
                ViewEvent::LoadImages => {
                    if let Some(k) = self.current() {
                        self.remote_images.insert(k);
                    }
                }
                ViewEvent::Reply => self.compose_reply(0),
                ViewEvent::ReplyAll => self.compose_reply(1),
                ViewEvent::Forward => self.compose_reply(2),
                ViewEvent::Key(k, cmd) => {
                    self.pending_input.extend(crate::mail_view::key_events(&k, cmd));
                    ctx.request_repaint();
                }
            }
        }
    }

    /// Put the web view over the reading pane, or hide it while something
    /// egui draws (a menu, Preferences) would be underneath it.
    fn place_mail_view(&mut self, ctx: &egui::Context) {
        let covered = self.prefs.is_some() || self.setup.is_some() || egui::Popup::is_any_open(ctx);
        let (Some(view), Some(rect), Some(open)) = (self.mail_view.as_mut(), self.web_rect, self.open.as_ref()) else {
            if let Some(v) = self.mail_view.as_mut() {
                v.hide();
            }
            return;
        };
        if covered || !self.html_email {
            view.hide();
            return;
        }
        let col = &self.colors;
        let key = (open.account_id, open.thread_key.clone());
        let remote = self.load_images || self.remote_images.contains(&key);
        let page_key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            let faces: Vec<bool> = open.messages.iter().map(|m| self.faces.texture(&m.from.email).is_some()).collect();
            (&key, remote, &self.theme_id, col.p.dark, open.messages.iter().map(|m| (m.id, m.starred, m.attachments.len())).collect::<Vec<_>>(), faces).hash(&mut h);
            h.finish()
        };
        let faces = &self.faces;
        view.show(rect, ctx.zoom_factor(), page_key, || {
            let nonce = format!("{:x}", page_key ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7));
            crate::mail_view::document(&crate::mail_view::Page { messages: &open.messages, expanded: &open.expanded, remote_images: remote, nonce: &nonce, avatar: &|e| faces.data_uri(e) }, col)
        });
    }

    /// Start "Sign in with Google" (one at a time).
    fn google_sign_in(&mut self, target: Option<AccountId>) {
        if self.google.is_some() {
            return;
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.worker.send(Cmd::GoogleSignIn(target, cancel.clone()));
        self.google = Some((cancel, None));
    }

    fn cancel_google(&mut self) {
        if let Some((cancel, _)) = self.google.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Open or save a received attachment (downloaded on a worker thread).
    fn attachment(&mut self, m: Message, index: usize, what: AttachmentClick) {
        let Some(a) = m.attachments.get(index) else { return };
        let what = match what {
            AttachmentClick::Open => AttachmentDo::Open,
            AttachmentClick::SaveToDownloads => AttachmentDo::SaveToDownloads,
            AttachmentClick::SaveAs => {
                let mut dialog = rfd::FileDialog::new().set_title("Save Attachment").set_file_name(engine::safe_filename(&a.filename));
                if let Some(d) = directories::UserDirs::new().and_then(|u| u.download_dir().map(|d| d.to_path_buf())) {
                    dialog = dialog.set_directory(d);
                }
                let Some(path) = dialog.save_file() else { return };
                AttachmentDo::SaveTo(path)
            }
        };
        if self.downloading.insert((m.id, index)) {
            self.worker.send(Cmd::Attachment(Box::new(m), index, what));
        }
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
                            ui.label(RichText::new(format!("until {}", snooze::describe(*t))).small().color(col.success));
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
            setup.google_ready = self.google_ready;
            setup.google_waiting = self.google.is_some();
            match setup.window(ctx, &col) {
                SetupAction::Submit(a, pw) => self.worker.send(Cmd::AddAccount(a, pw)),
                SetupAction::Google => self.google_sign_in(None),
                SetupAction::CancelGoogle => self.cancel_google(),
                SetupAction::GoogleSettings => {
                    self.setup = None;
                    self.prefs = Some(Prefs::new(Tab::Accounts));
                }
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
            prefs.push = self.push.clone();
            prefs.google_ready = self.google_ready;
            prefs.html_email = self.html_email;
            prefs.load_images = self.load_images;
            prefs.faces = self.faces.enabled.get();
            prefs.google_waiting = self.google.as_ref().map(|g| g.1.clone());
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
                PrefsAction::SaveGoogleClient(id, secret) => {
                    let r = owlpost_core::oauth::GoogleClient { id, secret }.save();
                    self.google_ready = r.is_ok() || self.google_ready;
                    if let Some(p) = &mut self.prefs {
                        p.message = Some(match r {
                            Ok(()) => (true, "Google sign-in is set up — use Sign in with Google to add or switch an account".into()),
                            Err(e) => (false, format!("{e:#}")),
                        });
                    }
                }
                PrefsAction::ClearGoogleClient => {
                    owlpost_core::oauth::GoogleClient::clear();
                    self.google_ready = false;
                }
                PrefsAction::GoogleSignIn(target) => self.google_sign_in(target),
                PrefsAction::CancelGoogle => self.cancel_google(),
                PrefsAction::SetHtmlEmail(on) => self.html_email = on,
                PrefsAction::SetLoadImages(on) => self.load_images = on,
                PrefsAction::SetFaces(on) => {
                    self.faces.enabled.set(on);
                    self.faces.clear();
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
                        let c = owlpost_core::parse::clean_signature_html(&sig.body);
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
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(if t.error || t.undo { 8 } else { 4 }));
        if !self.toasts.is_empty() {
            // Over the message list: the reading pane may be a web view,
            // which would cover anything egui draws there.
            let screen = ctx.content_rect();
            let x = if self.list_right > self.sidebar_right { (self.sidebar_right + self.list_right) / 2.0 } else { screen.center().x };
            egui::Area::new(egui::Id::new("toasts"))
                .pivot(egui::Align2::CENTER_BOTTOM)
                .fixed_pos(egui::pos2(x, screen.bottom() - 20.0))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    let mut undo_clicked = false;
                    for t in &self.toasts {
                        egui::Frame::new()
                            .fill(if t.error { col.error } else { egui::Color32::from_rgb(40, 44, 52) })
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(16, 10))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&t.text).color(egui::Color32::WHITE));
                                    if t.undo {
                                        ui.add_space(10.0);
                                        let b = egui::Button::new(RichText::new("Undo").strong().color(egui::Color32::from_rgb(130, 180, 255))).frame(false);
                                        if ui.add(b).on_hover_text("⌘Z").clicked() {
                                            undo_clicked = true;
                                        }
                                    }
                                });
                            });
                        ui.add_space(6.0);
                    }
                    if undo_clicked {
                        self.undo_last();
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

/// A section heading in the message list ("Pinned", "Today"…). For Pinned,
/// `pinned` is how many are hidden, and a Hide / Show button folds the
/// section. Returns true when that button is clicked.
/// Wrapped text drawn twice, half a point apart, for a heavier weight
/// (egui's bundled font has no bold).
fn heavy_text(ui: &mut egui::Ui, text: &str, size: f32, color: egui::Color32) {
    let w = ui.available_width();
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat::simple(egui::FontId::proportional(size), color));
    job.wrap.max_width = w - 1.0;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(rect.min + egui::vec2(0.6, 0.0), galley.clone(), color);
    ui.painter().galley(rect.min, galley, color);
}

/// A rounded filter chip; the chosen one is filled with the accent.
fn filter_chip(ui: &mut egui::Ui, col: &Colors, label: &str, on: bool) -> egui::Response {
    let font = egui::FontId::proportional(13.5);
    let fg = if on { col.button_emphasis_text } else { col.text };
    let g = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    // No label means the paperclip.
    let w = if label.is_empty() { 40.0 } else { g.size().x + 26.0 };
    let (rect, r) = ui.allocate_exact_size(egui::vec2(w, 30.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 15.0, col.button_emphasis);
        p.galley(rect.center() - g.size() / 2.0 + egui::vec2(0.5, 0.0), g.clone(), fg);
    } else {
        let fill = if r.hovered() { col.list_hover } else { style::blend(col.bg, col.list, 0.6) };
        p.rect(rect, 15.0, fill, egui::Stroke::new(1.0_f32, col.border), egui::StrokeKind::Inside);
    }
    if label.is_empty() {
        icons::paint(p, rect.center(), 15.0, Icon::Paperclip, fg);
    }
    p.galley(rect.center() - g.size() / 2.0, g, fg);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn section_header(ui: &mut egui::Ui, col: &Colors, rect: egui::Rect, title: &str, pinned: Option<usize>) -> bool {
    let p = ui.painter_at(rect);
    let y = rect.bottom() - 9.0;
    let font = egui::FontId::proportional(12.5);
    let text = match pinned {
        Some(n) if n > 0 => format!("{title} · {n} hidden"),
        _ => title.to_string(),
    };
    p.text(egui::pos2(rect.left() + 18.0, y), egui::Align2::LEFT_BOTTOM, text, font.clone(), col.subtle);
    let Some(hidden) = pinned else { return false };
    let label = if hidden > 0 { "Show" } else { "Hide" };
    let g = p.layout_no_wrap(label.into(), font, col.subtle);
    let br = egui::Rect::from_min_size(egui::pos2(rect.right() - 20.0 - g.size().x, y - g.size().y), g.size()).expand(4.0);
    let r = ui.interact(br, ui.id().with(("section", title)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    p.galley(egui::pos2(br.left() + 4.0, br.top() + 4.0), g, if r.hovered() { col.text } else { col.subtle });
    r.clicked()
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

/// Whether the system asks for dark mode (light when it doesn't say).
fn system_dark(ctx: &egui::Context) -> bool {
    ctx.system_theme() == Some(egui::Theme::Dark)
}

/// The palette for theme `id`; "Moonlit" picks its light or dark variant.
fn theme_palette(themes: &[Theme], id: &str, dark: bool) -> owlpost_core::theme::Palette {
    let id = if id == theme::MOONLIT { if dark { theme::MOONLIT_DARK } else { theme::MOONLIT_LIGHT } } else { id };
    themes.iter().find(|t| t.id == id).unwrap_or(&themes[0]).palette()
}

/// A small letter-spaced heading in the sidebar ("ALL ACCOUNTS").
fn section(ui: &mut egui::Ui, col: &Colors, text: &str) {
    let mut job = egui::text::LayoutJob::default();
    let sub = col.sidebar_text.gamma_multiply(0.75);
    job.append(text, 4.0, egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: sub, extra_letter_spacing: 1.1, ..Default::default() });
    ui.label(job);
    ui.add_space(4.0);
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

/// The sidebar's line icon for a folder role.
fn role_icon(role: FolderRole) -> Icon {
    match role {
        FolderRole::Inbox => Icon::Inbox,
        FolderRole::Starred => Icon::Star,
        FolderRole::Drafts => Icon::Pencil,
        FolderRole::Sent => Icon::Send,
        FolderRole::Archive | FolderRole::All => Icon::Archive,
        FolderRole::Spam => Icon::Warning,
        FolderRole::Trash => Icon::Trash,
        FolderRole::Other => Icon::Folder,
    }
}

/// A sidebar row: line icon, name, and a quiet count on the right.
fn nav_item(ui: &mut egui::Ui, col: &Colors, icon: Icon, label: &str, count: i64, selected: bool) -> bool {
    let h = 31.0;
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::click());
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 8.0, col.sidebar_active);
    } else if r.hovered() {
        p.rect_filled(rect, 8.0, style::blend(col.sidebar_active, col.sidebar, 0.5));
    }
    let fg = if selected { col.sidebar_active_text } else { col.sidebar_text };
    icons::paint(&p, egui::pos2(rect.left() + 19.0, rect.center().y), 14.5, icon, fg);
    let mut count_w = 0.0;
    if count > 0 {
        let sub = if selected { fg } else { style::blend(col.sidebar_text, col.sidebar, 0.7) };
        let g = p.layout_no_wrap(count.to_string(), egui::FontId::proportional(12.0), sub);
        count_w = g.size().x + 8.0;
        let pos = egui::pos2(rect.right() - 12.0 - g.size().x, rect.center().y - g.size().y / 2.0);
        if selected {
            // Drawn twice for weight, like the label.
            p.galley(pos + egui::vec2(0.5, 0.0), g.clone(), sub);
        }
        p.galley(pos, g, sub);
    }
    let pos = egui::pos2(rect.left() + 38.0, rect.center().y - 8.5);
    let font = egui::FontId::proportional(14.0);
    let max = rect.width() - 50.0 - count_w;
    if selected {
        style::bold_line(&p, pos, label, font, fg, max);
    } else {
        style::line(&p, pos, label, font, fg, max);
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Returns true when the header was clicked (toggle expand).
/// What was asked of an attachment chip in the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttachmentClick {
    Open,
    SaveToDownloads,
    SaveAs,
}

/// One message in the reader. Returns whether its header was clicked
/// (to fold it) and any attachment that was clicked.
fn message_card(
    ui: &mut egui::Ui,
    col: &Colors,
    m: &Message,
    expanded: bool,
    is_last: bool,
    downloading: &HashSet<(MessageId, usize)>,
    faces: &crate::avatars::Faces,
) -> (bool, Option<(usize, AttachmentClick)>) {
    let mut toggled = false;
    let mut clicked = None;
    // Flat, on the page: a rule above each message instead of a card.
    hairline(ui, col);
    egui::Frame::new()
        .inner_margin(egui::Margin { left: 0, right: 0, top: 18, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui.horizontal(|ui| {
                faces.show(ui, &m.from.name, &m.from.email, 36.0);
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
                // Header, a hairline, then (like Canary) attachments above the text.
                ui.add_space(12.0);
                hairline(ui, col);
                if !m.attachments.is_empty() {
                    ui.add_space(12.0);
                    ui.horizontal_wrapped(|ui| {
                        for (i, a) in m.attachments.iter().enumerate() {
                            if let Some(what) = attachment_chip(ui, col, a, downloading.contains(&(m.id, i)), (m.id, i)) {
                                clicked = Some((i, what));
                            }
                        }
                    });
                    ui.add_space(12.0);
                    hairline(ui, col);
                }
                ui.add_space(20.0);
                body(ui, col, &m.body_text);
            }
        });
    (toggled, clicked)
}

/// A thin full-width divider inside a message card.
fn hairline(ui: &mut egui::Ui, col: &Colors) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, egui::Stroke::new(1.0_f32, style::blend(col.border, col.bg, 0.7)));
}

/// A received attachment: click to open, right-click for Save options.
fn attachment_chip(ui: &mut egui::Ui, col: &Colors, a: &Attachment, busy: bool, salt: (MessageId, usize)) -> Option<AttachmentClick> {
    let id = ui.id().with(("attachment", salt));
    let hovered = ui.ctx().memory(|mem| mem.data.get_temp::<bool>(id)).unwrap_or(false);
    let fill = if hovered { style::blend(col.text, col.bg_secondary, 0.07) } else { col.bg_secondary };
    let inner = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0_f32, col.border))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if busy {
                    ui.add(egui::Spinner::new().size(14.0).color(col.subtle));
                } else {
                    ui.label(RichText::new("📎").color(col.subtle));
                }
                ui.label(RichText::new(&a.filename).color(col.text));
                ui.label(RichText::new(human_size(a.size)).small().color(col.subtle));
            });
        });
    let r = ui
        .interact(inner.response.rect, id.with("click"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if busy { "Downloading…" } else { "Click to open · right-click to save" });
    ui.ctx().memory_mut(|mem| mem.data.insert_temp(id, r.hovered()));
    let mut what = None;
    if r.clicked() && !busy {
        what = Some(AttachmentClick::Open);
    }
    r.context_menu(|ui| {
        if ui.button("Open").clicked() {
            what = Some(AttachmentClick::Open);
            ui.close();
        }
        if ui.button("Save to Downloads").clicked() {
            what = Some(AttachmentClick::SaveToDownloads);
            ui.close();
        }
        if ui.button("Save As…").clicked() {
            what = Some(AttachmentClick::SaveAs);
            ui.close();
        }
    });
    what
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

impl eframe::App for OwlPostApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // The web view hangs off the native window, so it's made here.
        if !self.mail_view_tried {
            self.mail_view_tried = true;
            self.mail_view = crate::mail_view::MailView::new(frame, self.colors.bg);
        }
        self.frame(ctx);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Shortcuts typed while a message (the web view) had focus.
        for e in self.pending_input.drain(..) {
            if let egui::Event::Key { modifiers, .. } = &e {
                raw_input.modifiers = *modifiers;
            }
            raw_input.events.push(e);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(THEME_KEY, self.theme_id.clone());
        storage.set_string(LIST_KEY, self.list_settings.encode());
        storage.set_string(HTML_KEY, if self.html_email { "1" } else { "0" }.into());
        storage.set_string(IMAGES_KEY, if self.load_images { "load" } else { "ask" }.into());
        storage.set_string(FACES_KEY, if self.faces.enabled.get() { "1" } else { "0" }.into());
    }
}

impl OwlPostApp {
    pub fn frame(&mut self, ctx: &egui::Context) {
        self.follow_system_theme(ctx);
        self.handle_events();
        self.web_rect = None;
        self.mail_view_events(ctx);
        if self.accounts.is_empty() {
            let col = self.colors.clone();
            egui::CentralPanel::default().frame(egui::Frame::new().fill(col.bg_secondary)).show(ctx, |ui| {
                self.welcome.google_ready = self.google_ready;
                self.welcome.google_waiting = self.google.is_some();
                egui::ScrollArea::vertical().show(ui, |ui| match self.welcome.welcome(ui, &col) {
                    SetupAction::Submit(a, pw) => self.worker.send(Cmd::AddAccount(a, pw)),
                    SetupAction::Google => self.google_sign_in(None),
                    SetupAction::CancelGoogle => self.cancel_google(),
                    SetupAction::GoogleSettings => self.prefs = Some(Prefs::new(Tab::Accounts)),
                    SetupAction::Demo => {
                        let _ = demo::seed(&self.store);
                        self.reload();
                    }
                    _ => {}
                });
            });
            self.windows(ctx);
            self.place_mail_view(ctx);
            return;
        }
        self.handle_keys(ctx);
        self.toolbar(ctx);
        self.sidebar(ctx);
        self.thread_list(ctx);
        self.reader(ctx);
        self.windows(ctx);
        self.place_mail_view(ctx);
        // Keep relative times and snooze wake-ups fresh.
        ctx.request_repaint_after(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use egui_kittest::Harness;

    fn shots_dir() -> std::path::PathBuf {
        let d = std::env::var("OWLPOST_SHOTS").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("owlpost-shots"));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn harness(theme: &str) -> Harness<'static, OwlPostApp> {
        harness_with(theme, |_| {})
    }

    fn harness_with(theme: &str, setup: impl FnOnce(&mut OwlPostApp)) -> Harness<'static, OwlPostApp> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let db = std::env::temp_dir().join(format!("sp-ui-{theme}-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let store = Store::open(&db).unwrap();
        let theme = theme.to_string();
        let h = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_state(|ctx, app: &mut OwlPostApp| app.frame(ctx), {
                let ctx = egui::Context::default();
                let mut app = OwlPostApp::with_context(&ctx, Some(theme), store, true);
                setup(&mut app);
                app
            });
        // The app was built against a scratch context; set up the harness's own.
        let c = h.state().colors.clone();
        c.apply(&h.ctx);
        install_image_loaders(&h.ctx);
        h
    }

    fn save_ui(h: &mut Harness<'static>, name: &str) {
        if let Ok(img) = h.render() {
            img.save(shots_dir().join(format!("{name}.png"))).unwrap();
        }
    }

    fn save(h: &mut Harness<'static, OwlPostApp>, name: &str) {
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
        for theme in ["owlpost-light", "owlpost-dark", "owlpost-darkside", theme::MOONLIT_LIGHT, theme::MOONLIT_DARK] {
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
            if theme == "owlpost-light" {
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
    /// `OWLPOST_TEST_THEMES=/path/to/packages cargo test -p owlpost`
    #[test]
    fn renders_external_mailspring_themes() {
        let Ok(dir) = std::env::var("OWLPOST_TEST_THEMES") else { return };
        let mut dirs: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let Ok(t) = theme::Theme::load_package(&d, theme::ThemeSource::Mailspring) else { continue };
            let mut h = harness("owlpost-light");
            h.state_mut().themes.push(t.clone());
            let ctx = h.ctx.clone();
            h.state_mut().set_theme(&ctx, &t.id);
            h.run_steps(2);
            h.state_mut().select(1);
            h.run_steps(3);
            save(&mut h, &format!("mailspring-{}", t.id));
        }
    }

    fn row_center(h: &Harness<'static, OwlPostApp>, i: usize) -> egui::Pos2 {
        let s = h.state();
        let layout = list_layout(&s.rows, s.selection != Selection::Snoozed, s.hidden_pinned, Utc::now());
        let y = layout.iter().find(|(_, _, it)| *it == ListItem::Row(i)).map(|(y, _, _)| *y).unwrap();
        egui::pos2((s.sidebar_right + s.list_right) / 2.0, s.list_top + y + ROW_H / 2.0)
    }

    #[test]
    fn swipe_quick_actions_and_composer_window() {
        let mut h = harness_with("owlpost-light", |a| a.list_default_width = 500.0);
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
        let mut n = harness("owlpost-light");
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
        let mut h = harness("owlpost-light");
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
        let proj = h.state().folders[&a].iter().find(|f| f.path == "Projects/OwlPost").cloned().unwrap();
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
        let mut h = harness("owlpost-light");
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
            body: include_str!("../../owlpost-core/tests/fixtures/canary-signature.html").into(),
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
        let mut h = harness("owlpost-light");
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
    fn attachments_and_image_sizes() {
        use crate::attachments::ImageSize;
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        h.state_mut().compose_new();
        h.run_steps(2);
        // Dragging files onto the window attaches them.
        let dir = std::env::temp_dir().join(format!("sp-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("Beach day.jpg");
        std::fs::write(&photo, crate::attachments::tests::jpeg(3000, 2000)).unwrap();
        let pdf = dir.join("Quote 2026.pdf");
        std::fs::write(&pdf, b"%PDF-1.4 a quote").unwrap();
        h.input_mut().dropped_files = vec![
            egui::DroppedFile { path: Some(photo.clone()), ..Default::default() },
            egui::DroppedFile { path: Some(pdf), ..Default::default() },
        ];
        h.run_steps(2);
        let names: Vec<_> = h.state().composers[0].attachments.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, ["Beach day.jpg", "Quote 2026.pdf"]);
        assert!(!h.state().composers[0].is_blank(), "attachments count as content");

        // Pick Medium: the photo is shrunk in the background.
        {
            let c = &mut h.state_mut().composers[0];
            c.to = "Maya <maya@example.com>".into();
            c.subject = "Photos from Saturday".into();
            c.body = "Here's the one I mentioned.".into();
            c.image_size = ImageSize::Medium;
        }
        for _ in 0..200 {
            h.run_steps(1);
            if !h.state().composers[0].resizing() && !h.state().composers[0].attachments[0].pending(ImageSize::Small) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        h.run_steps(3);
        save(&mut h, "composer-attachments");
        let out = h.state().composers[0].draft();
        assert_eq!(out.attachments.len(), 2);
        let img = image::load_from_memory(&out.attachments[0].data).unwrap();
        assert_eq!((img.width(), img.height()), (1280, 853));
        assert_eq!(out.attachments[1].data, b"%PDF-1.4 a quote");
        assert_eq!(out.attachments[1].content_type, "application/pdf");
        // Removing one.
        h.state_mut().composers[0].attachments.pop();
        h.run_steps(2);
        assert_eq!(h.state().composers[0].draft().attachments.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn open_received_attachment() {
        use egui_kittest::kittest::Queryable;
        let cache = std::env::temp_dir().join(format!("sp-ui-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        std::env::set_var("OWLPOST_CACHE", &cache);
        let mut h = harness("owlpost-light");
        h.run_steps(3);
        let idx = h.state().rows.iter().position(|r| r.t.subject.contains("receipt")).unwrap();
        h.state_mut().select(idx);
        h.run_steps(3);
        let at = h.get_by_label("receipt.pdf").rect().center();
        h.hover_at(at);
        h.run_steps(2);
        save(&mut h, "attachment-hover");
        // Click: downloaded (a stand-in for demo mail), cached, then opened.
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(1);
        assert_eq!(h.state().downloading.len(), 1, "shows as downloading");
        for _ in 0..100 {
            h.run_steps(1);
            if h.state().downloading.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(h.state().downloading.is_empty());
        assert!(h.state().toasts.iter().all(|t| !t.error), "{:?}", h.state().toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
        let cached: Vec<_> = std::fs::read_dir(cache.join("attachments")).unwrap().flatten().flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten()).collect();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].file_name(), "receipt.pdf");
        assert!(std::fs::read(cached[0].path()).unwrap().starts_with(b"%PDF"));
        // Right-click offers the save options.
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
        h.run_steps(2);
        assert!(h.query_by_label("Save to Downloads").is_some() && h.query_by_label("Save As…").is_some());
        save(&mut h, "attachment-menu");
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn push_status_lines() {
        use crate::worker::PushState;
        let palette = theme::discover().into_iter().find(|t| t.id == "owlpost-light").unwrap().palette();
        let col = Colors::new(palette);
        let states = [
            Some(PushState::Live),
            Some(PushState::Connecting),
            Some(PushState::Retrying("connecting to imap.mail.me.com:993: timed out".into())),
            Some(PushState::Unavailable("the server doesn't support it".into())),
        ];
        let mut h = Harness::builder().with_size(egui::vec2(360.0, 120.0)).build_ui(move |ui| {
            col.apply(ui.ctx());
            for st in &states {
                crate::prefs::push_line(ui, &col, st.as_ref());
            }
        });
        h.run_steps(2);
        save_ui(&mut h, "push-status");
        use egui_kittest::kittest::Queryable;
        for text in ["New mail: instant (push)", "New mail: connecting push…", "New mail: reconnecting push…", "New mail: every 5 minutes"] {
            assert!(h.query_by_label(text).is_some(), "{text}");
        }
    }

    #[test]
    fn google_sign_in_screens() {
        use egui_kittest::kittest::Queryable;
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        assert!(!h.state().google_ready, "tests never read the real keychain");
        h.state_mut().prefs = Some(Prefs::new(Tab::Accounts));
        h.run_steps(3);
        assert!(h.query_by_label("Client ID").is_some());
        assert!(h.query_by_label("How to create a Google client").is_some());
        save(&mut h, "prefs-google-setup");
        // Set up: the sign-in button appears.
        h.state_mut().google_ready = true;
        h.run_steps(3);
        assert!(h.query_by_label("Sign in with Google…").is_some());
        save(&mut h, "prefs-google-ready");
        // Signing in: waiting for the browser.
        h.state_mut().google = Some((Default::default(), Some("https://accounts.google.com/o/oauth2/v2/auth?x".into())));
        h.run_steps(3);
        assert!(h.query_by_label("Finish signing in in your browser…").is_some());
        save(&mut h, "prefs-google-waiting");
        h.state_mut().cancel_google();
        h.state_mut().prefs = None;
        // The add-account window for Gmail.
        h.state_mut().setup = Some(AccountSetup { provider: Some(owlpost_core::presets::Provider::Gmail), ..Default::default() });
        h.run_steps(3);
        assert!(h.query_by_label("Sign in with Google").is_some());
        assert!(h.query_by_label("…or connect with an app password:").is_some());
        save(&mut h, "setup-gmail-google");
    }

    #[test]
    fn command_z_undoes() {
        let mut h = harness("owlpost-light");
        h.run_steps(3);
        let in_list = |h: &Harness<'static, OwlPostApp>, k: &str| h.state().rows.iter().any(|r| r.t.thread_key == k);
        let wait_for = |h: &mut Harness<'static, OwlPostApp>, k: &str, present: bool| {
            for _ in 0..100 {
                h.run_steps(1);
                h.state_mut().reload();
                if h.state().rows.iter().any(|r| r.t.thread_key == k) == present {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        // Archive with `e`, then ⌘Z.
        let idx = h.state().rows.iter().position(|r| r.t.subject.contains("Dinner")).unwrap();
        let key = h.state().rows[idx].t.thread_key.clone();
        h.state_mut().select(idx);
        h.run_steps(2);
        h.event(egui::Event::Text("e".into()));
        h.run_steps(2);
        assert!(!in_list(&h, &key), "archived");
        assert!(h.state().toasts.iter().any(|t| t.undo && t.text == "Archived"));
        save(&mut h, "undo-toast");
        assert!(wait_for(&mut h, &key, false), "the archive finishes");
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(2);
        assert!(wait_for(&mut h, &key, true), "⌘Z brings it back to the Inbox");
        assert!(h.state().toasts.iter().any(|t| t.text == "Undid archive"));

        // Trash straight after, then click Undo on the toast at once: the undo
        // waits for the move to finish rather than racing it.
        let idx = h.state().rows.iter().position(|r| r.t.thread_key == key).unwrap();
        h.state_mut().select(idx);
        h.run_steps(1);
        h.state_mut().act_move(FolderRole::Trash);
        h.state_mut().undo_last();
        assert!(wait_for(&mut h, &key, true), "trash undone");
        let acct = h.state().rows.iter().find(|r| r.t.thread_key == key).unwrap().t.account_id;
        let trash = h.state().store.folder_by_role(acct, FolderRole::Trash).unwrap().unwrap();
        for _ in 0..50 {
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!h.state().store.thread_folder_ids(acct, &key).unwrap().contains(&trash.id), "and out of the Trash");

        // Snooze, pin and read state.
        let idx = h.state().rows.iter().position(|r| r.t.thread_key == key).unwrap();
        h.state_mut().select(idx);
        h.run_steps(1);
        h.state_mut().act_snooze(Utc::now() + chrono::Duration::hours(3));
        assert!(!in_list(&h, &key));
        h.state_mut().undo_last();
        h.run_steps(1);
        assert!(in_list(&h, &key), "unsnoozed");
        h.state_mut().toggle_pin(acct, &key);
        assert!(h.state().store.is_pinned(acct, &key).unwrap());
        h.state_mut().undo_last();
        assert!(!h.state().store.is_pinned(acct, &key).unwrap(), "unpinned again");
        // Nothing left to undo.
        while !h.state().undo.is_empty() {
            h.state_mut().undo.pop();
        }
        h.state_mut().undo_last();
        assert!(h.state().toasts.iter().any(|t| t.text == "Nothing to undo"));
    }

    #[test]
    fn pinned_section_hides_and_shows() {
        let mut h = harness("owlpost-light");
        h.run_steps(3);
        let (a, k) = (h.state().rows[2].t.account_id, h.state().rows[2].t.thread_key.clone());
        h.state_mut().toggle_pin(a, &k);
        h.run_steps(3);
        let layout = list_layout(&h.state().rows, true, 0, Utc::now());
        assert_eq!(layout[0].2, ListItem::Header("Pinned".into()));
        assert_eq!(layout[1].2, ListItem::Row(0));
        assert!(h.state().rows[0].t.pinned);
        save(&mut h, "list-pinned");
        // Click "Hide" in the Pinned header.
        let s = h.state();
        let at = egui::pos2(s.list_right - 30.0, s.list_top + LIST_TOP + HEADER_H - 14.0);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(2);
        assert!(h.state().list_settings.hide_pinned);
        assert_eq!(h.state().hidden_pinned, 1);
        assert!(h.state().rows.iter().all(|r| !r.t.pinned), "pinned conversations are folded away");
        save(&mut h, "list-pinned-hidden");
        assert_eq!(ListSettings::decode(&h.state().list_settings.encode()), h.state().list_settings, "remembered");
        assert!(!ListSettings::decode("trash,archive,1").hide_pinned, "older settings still load");
    }

    #[test]
    fn date_groups() {
        use chrono::{Local, TimeZone};
        let now = Local.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap().with_timezone(&Utc); // a Saturday
        let at = |d: u32, m: u32, y: i32| Local.with_ymd_and_hms(y, m, d, 9, 0, 0).unwrap().with_timezone(&Utc);
        assert_eq!(date_group(at(7, 11, 2026), now), "Today");
        assert_eq!(date_group(at(6, 11, 2026), now), "Yesterday");
        assert_eq!(date_group(at(3, 11, 2026), now), "Earlier this week");
        assert_eq!(date_group(at(28, 10, 2026), now), "Last week");
        assert_eq!(date_group(at(2, 10, 2026), now), "October");
        assert_eq!(date_group(at(2, 10, 2025), now), "October 2025");
    }

    #[test]
    fn unused_download_names() {
        let dir = std::env::temp_dir().join(format!("sp-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(crate::worker::unused_path(&dir, "a.pdf"), dir.join("a.pdf"));
        std::fs::write(dir.join("a.pdf"), b"x").unwrap();
        assert_eq!(crate::worker::unused_path(&dir, "a.pdf"), dir.join("a (2).pdf"));
        std::fs::write(dir.join("a (2).pdf"), b"x").unwrap();
        assert_eq!(crate::worker::unused_path(&dir, "a.pdf"), dir.join("a (3).pdf"));
        std::fs::write(dir.join("README"), b"x").unwrap();
        assert_eq!(crate::worker::unused_path(&dir, "README"), dir.join("README (2)"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn welcome_screen() {
        let db = std::env::temp_dir().join(format!("sp-ui-welcome-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let store = Store::open(&db).unwrap();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1100.0, 720.0))
            .build_state(|ctx, app: &mut OwlPostApp| app.frame(ctx), {
                let ctx = egui::Context::default();
                OwlPostApp::with_context(&ctx, Some("owlpost-light".into()), store, false)
            });
        let c = h.state().colors.clone();
        c.apply(&h.ctx);
        h.run_steps(3);
        save(&mut h, "welcome");
    }
}
