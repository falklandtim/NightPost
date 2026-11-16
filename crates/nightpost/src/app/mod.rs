//! The app: its state, start-up, and the three-pane window
//! (sidebar · message list · reading pane) drawn each frame.
//!
//! This file holds [`NightPostApp`]'s state and the types it uses; each part
//! of the window lives in its own module, as more `impl NightPostApp` blocks:
//!
//! - `sidebar`: accounts and folders
//! - `list`: the message list, its rows, swipes and menus
//! - `reader`: the open conversation
//! - `toolbar`: the toolbar and its pop-ups
//! - `sheets`: dialogs, sheets and the other windows
//! - `actions`: what each command does (archive, move, pin, undo…)
//! - `input`: keyboard shortcuts and events from the worker threads
//!
//! The UI thread never waits on the network: actions change the local
//! store at once and send a [`Cmd`] to the worker (see `docs/architecture.md`).

use crate::composer::{Composer, ComposerAction, SignatureBook};
use crate::icons::{self, Icon, Item};
use crate::onboarding::{AccountSetup, SetupAction};
use crate::prefs::{ListSettings, Prefs, PrefsAction, SwipeAction, Tab, QUICK_ACTIONS_MIN_WIDTH};
use crate::style::{self, Colors};
use crate::worker::{AttachmentDo, Cmd, Event, FolderUse, PushState, Worker};
use chrono::{DateTime, Datelike, Utc};
use eframe::egui::{self, Key, RichText, Sense};
use nightpost_core::snooze::{self, SnoozePreset};
use nightpost_core::snooze_sync;
use nightpost_core::theme::{self, Theme};
use nightpost_core::*;
use nightpost_core::smtp::OutgoingMessage;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

mod actions;
mod input;
mod list;
mod reader;
mod sheets;
mod sidebar;
mod toolbar;
use input::*;
use reader::*;
use sheets::*;
use sidebar::*;

/// The Sync activity shortcut, as menus show it.
const ACTIVITY_KEY: &str = if cfg!(target_os = "macos") { "⌘⇧L" } else { "Ctrl+Shift+L" };

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
    ("v", "Move to folder… (type, ↑ ↓, Return)"),
    ("l", "Label… (Gmail; type, ↑ ↓, Return)"),
    ("j  /  ↓", "Next conversation"),
    ("k  /  ↑", "Previous conversation"),
    ("⌘ click", "Add to / remove from the selection"),
    ("Shift click", "Select a range"),
    ("Shift ↓  /  Shift ↑", "Extend the selection"),
    ("⌘ A", "Select every conversation in the list"),
    ("⌘ F  /  /", "Search"),
    ("g i  ·  g s  ·  g t  ·  g d  ·  g z", "Go to Inbox, Starred, Sent, Drafts, Snoozed"),
    ("?", "Show the keyboard shortcuts"),
    ("⌘ Shift L", "Sync activity: is each account connected, and what happened"),
    ("↑ ↓  ·  Return  ·  Esc", "In a menu: choose, pick (or open a submenu), close"),
    ("⌘ Z", "Undo (send, move, archive, trash, label, snooze, pin…)"),
    ("⌘ S", "Save draft (in composer)"),
    ("⌘ ,", "Preferences"),
    ("⌘ W", "Close composer / Preferences"),
    ("⌘ ↩", "Send (in composer)"),
    ("Esc", "Close / clear search / clear the selection"),
];

/// The Sync activity window: which account it shows (None: all), whether
/// only problems, and the entries last read.
struct ActivityView {
    account: Option<AccountId>,
    problems: bool,
    entries: Vec<activity::Entry>,
    read: Option<Instant>,
}

impl ActivityView {
    fn new(account: Option<AccountId>) -> Self {
        Self { account, problems: false, entries: vec![], read: None }
    }
}

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

struct OpenThread {
    account_id: AccountId,
    thread_key: String,
    messages: Vec<Message>,
    /// Which messages are expanded (older ones collapse, like Mailspring).
    expanded: Vec<bool>,
    snoozed_until: Option<DateTime<Utc>>,
}

/// A window showing a message's raw source ("Show original").
struct Original {
    id: u64,
    subject: String,
    text: String,
}

/// Where dragged conversations can be dropped in the sidebar.
#[derive(Clone, Debug)]
enum DropTarget {
    /// All Accounts → Inbox, Archive, Spam or Trash: works for every account.
    Role(FolderRole),
    /// One account's folder.
    Folder(Folder),
}

/// What a composer's draft commands refer to.
fn draft_ref(c: &Composer) -> crate::worker::DraftRef {
    crate::worker::DraftRef { composer: c.id, initial: c.draft.clone() }
}

/// A message on its way: sent when `due` passes, unless Undo brings its
/// composer back first.
struct PendingSend {
    id: u64,
    account: AccountId,
    out: OutgoingMessage,
    composer: Composer,
    due: Instant,
}

/// The calendar in the Snooze menu.
#[derive(Clone, Debug)]
struct SnoozePick {
    open: bool,
    /// First day of the month shown.
    month: chrono::NaiveDate,
    day: Option<chrono::NaiveDate>,
    hour: u32,
    minute: u32,
}

impl Default for SnoozePick {
    fn default() -> Self {
        let today = chrono::Local::now().date_naive();
        // 9:00, like the presets.
        Self { open: false, month: today.with_day(1).unwrap_or(today), day: None, hour: 9, minute: 0 }
    }
}

/// The All tab: the whole Inbox, whatever its category.
const ALL_TAB: u8 = u8::MAX;

/// The sidebar's folder dialogs.
enum FolderDialog {
    /// New folder in an account, inside `parent` when given.
    New { account: AccountId, parent: Option<Folder>, name: String, error: Option<String> },
    /// Are you sure? `gmail`: its mail stays in All Mail rather than going to Trash.
    Delete { folder: Folder, gmail: bool },
}

/// The "New tab" dialog: the name so far, and what goes in the tab once made.
struct NewTab {
    name: String,
    then: NewTabFor,
    error: Option<String>,
}

#[derive(Clone)]
enum NewTabFor {
    Sender(String),
    Thread(AccountId, String),
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
    /// A sender rule changed: what it was before (None: automatic).
    SenderRule { email: String, was: Option<u8> },
    /// A conversation put in a tab by hand: where it was put before (None: no rule).
    ThreadRule { a: AccountId, k: String, was: Option<u8> },
    /// A tab removed (its tombstone); undoing brings it back.
    TabRemoved { tab: nightpost_core::snooze_sync::Tab },
    /// One action on several selected conversations, undone together.
    Batch { items: Vec<Undo>, what: String },
    /// A message waiting to send (Undo Send).
    Send { id: u64 },
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
            Undo::SenderRule { .. } | Undo::ThreadRule { .. } => "sorting".into(),
            Undo::TabRemoved { tab } => format!("removing the “{}” tab", tab.name),
            Undo::Batch { what, .. } => what.clone(),
            Undo::Send { .. } => "send".into(),
        }
    }
}

/// How many actions ⌘Z can go back.
const UNDO_DEPTH: usize = 30;

pub struct NightPostApp {
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
    /// Conversations picked with ⌘- or ⇧-click. With two or more, actions
    /// apply to all of them and the reading pane shows a summary.
    marked: Vec<(AccountId, String)>,
    /// Where a ⇧-click range starts.
    anchor: Option<usize>,
    /// Conversations just moved out of view: kept out of the list until the
    /// move lands (so a reload mid-batch doesn't flash them back).
    hiding: HashMap<(AccountId, String), Instant>,
    /// Sent messages held back for a few seconds (Undo Send).
    pending_sends: Vec<PendingSend>,
    next_send_id: u64,
    /// "Show original" windows.
    originals: Vec<Original>,
    /// Conversations being dragged towards the sidebar.
    dragging: Option<Vec<(AccountId, String)>>,
    /// The sidebar entry under the pointer while dragging (set each frame).
    drop_target: Option<DropTarget>,
    /// Parent folders whose subfolders are shown (see [`folder_key`]);
    /// the rest start folded.
    open_folders: HashSet<String>,
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
    /// The Snooze menu's date and time picker.
    snooze_pick: SnoozePick,
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
    /// The Inbox tab showing (an `nightpost_core::category` value, or [`ALL_TAB`]).
    category: u8,
    /// The Inbox tabs showing: category, name and unread conversations.
    tabs: Vec<(u8, String, i64)>,
    /// The "New tab" dialog, while it's open.
    new_tab: Option<NewTab>,
    /// The New folder / Delete folder dialog, while it's open.
    folder_dialog: Option<FolderDialog>,
    /// Recolour HTML emails to match a dark theme.
    dark_emails: bool,
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
    /// The highlighted row in the Move / Label picker (arrow keys move it).
    picker_index: usize,
    /// The keyboard shortcuts sheet (?).
    show_keys: bool,
    /// Each account's sync health (from the database, which the sync
    /// threads keep up to date) and when it was last read.
    health: HashMap<AccountId, activity::Status>,
    health_read: Option<Instant>,
    /// The Sync activity window.
    activity: Option<ActivityView>,
    sigs: SignatureBook,
    /// Attachments being downloaded: (message, attachment index).
    downloading: HashSet<(MessageId, usize)>,
    /// Drafts being fetched to edit (by Message-ID), so a double click
    /// opens one composer.
    opening_drafts: HashSet<String>,
    /// Push (IMAP IDLE) status per account.
    push: HashMap<AccountId, PushState>,
    /// A Google OAuth client is saved in the keychain.
    google_ready: bool,
    /// A "Sign in with Google" under way: its cancel flag and, once the
    /// browser is open, the sign-in URL.
    google: Option<(std::sync::Arc<std::sync::atomic::AtomicBool>, Option<String>)>,
}

const THEME_KEY: &str = "nightpost.theme";
/// Key prefixes from before the renames (OwlPost, and SpringPost before it),
/// read as a fallback so settings carry over.
const LEGACY_KEY_PREFIXES: [&str; 2] = ["owlpost.", "springpost."];
/// Height of a conversation in the list: names, subject and two preview lines.
const ROW_H: f32 = 94.0;
const LIST_KEY: &str = "nightpost.list";
const HTML_KEY: &str = "nightpost.html";
const IMAGES_KEY: &str = "nightpost.images";
const FACES_KEY: &str = "nightpost.faces";
const DARK_EMAILS_KEY: &str = "nightpost.dark_emails";
/// Parent folders opened in the sidebar (`<account id>\t<path>` per line).
const OPEN_FOLDERS_KEY: &str = "nightpost.open_folders";
const COLLAPSED_ACCOUNTS_KEY: &str = "nightpost.collapsed_accounts";

impl NightPostApp {
    pub fn new(cc: &eframe::CreationContext<'_>, store: Store, demo: bool) -> Self {
        // A setting saved under its NightPost key, or else under the key it
        // had before the app was renamed.
        let get = |key: &str| {
            let s = cc.storage?;
            let rest = key.strip_prefix("nightpost.").unwrap_or(key);
            s.get_string(key).or_else(|| LEGACY_KEY_PREFIXES.iter().find_map(|p| s.get_string(&format!("{p}{rest}"))))
        };
        // Built-in theme ids were renamed along with the app.
        let saved = get(THEME_KEY).map(|id| id.replace("springpost-", "nightpost-").replace("owlpost-", "nightpost-"));
        let mut app = Self::with_context(&cc.egui_ctx, saved, store, demo);
        // Formatted composer windows (WebKit / WebKitGTK); without them the
        // composer stays in egui.
        crate::web_host::init(&cc.egui_ctx);
        if let Some(h) = get(HTML_KEY) {
            app.html_email = h != "0";
        }
        if let Some(i) = get(IMAGES_KEY) {
            app.load_images = i != "ask";
        }
        if let Some(d) = get(DARK_EMAILS_KEY) {
            app.dark_emails = d != "0";
        }
        if let Some(f) = get(FACES_KEY) {
            app.faces.enabled.set(f != "0");
        }
        if let Some(l) = get(LIST_KEY) {
            app.list_settings = ListSettings::decode(&l);
        }
        if let Some(f) = get(OPEN_FOLDERS_KEY) {
            app.open_folders = f.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        }
        if let Some(c) = get(COLLAPSED_ACCOUNTS_KEY) {
            app.collapsed_accounts = c.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        }
        if cfg!(target_os = "macos") {
            app.titlebar_inset = 76.0;
        }
        // Debug builds: NIGHTPOST_OPEN=<words> opens a matching conversation at
        // launch (for checking the message view by hand).
        #[cfg(debug_assertions)]
        if let Ok(q) = std::env::var("NIGHTPOST_OPEN") {
            if let Some(i) = app.rows.iter().position(|r| r.t.subject.contains(q.as_str())) {
                app.select(i);
            }
        }
        // …and NIGHTPOST_COMPOSE=new|reply opens a composer.
        #[cfg(debug_assertions)]
        match std::env::var("NIGHTPOST_COMPOSE").as_deref() {
            Ok("reply") => app.compose_reply(0),
            Ok(_) => app.compose_new(),
            Err(_) => {}
        }
        app
    }

    pub fn with_context(ctx: &egui::Context, saved_theme: Option<String>, store: Store, demo: bool) -> Self {
        if demo {
            let _ = demo::seed(&store);
        }
        install_image_loaders(ctx);
        let themes = theme::discover();
        // Themes that were dropped: their dark ones become Moonlit Dark,
        // anything else follows the system.
        let theme_id = match saved_theme.as_deref() {
            Some(id) if themes.iter().any(|t| t.id == id) => id.to_string(),
            Some("nightpost-dark" | "nightpost-darkside") => theme::MOONLIT_DARK.into(),
            _ => theme::MOONLIT.into(),
        };
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
            open_folders: HashSet::new(),
            marked: vec![],
            anchor: None,
            hiding: HashMap::new(),
            pending_sends: vec![],
            next_send_id: 1,
            originals: vec![],
            dragging: None,
            drop_target: None,
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
            snooze_pick: SnoozePick::default(),
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
            dark_emails: true,
            category: ALL_TAB,
            tabs: Vec::new(),
            new_tab: None,
            folder_dialog: None,
            web_rect: None,
            remote_images: HashSet::new(),
            pending_input: vec![],
            swipe: None,
            last_wheel_x: None,
            list_default_width: 400.0,
            picker_query: String::new(),
            picker_index: 0,
            show_keys: false,
            health: HashMap::new(),
            health_read: None,
            activity: None,
            sigs: SignatureBook::default(),
            downloading: HashSet::new(),
            opening_drafts: HashSet::new(),
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

    // ----- Undo Send ----------------------------------------------------------

    // ----- multi-select -------------------------------------------------------

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
        let mut query = format!("{} {}", self.search, self.filter.token());
        if self.tabs_shown() {
            let mut list: Vec<(u8, String)> = vec![(ALL_TAB, "All".to_string())];
            list.extend(nightpost_core::category::ALL.iter().map(|c| (*c, nightpost_core::category::label(*c).to_string())));
            list.extend(self.store.tabs().unwrap_or_default());
            // The tab was removed (perhaps on another machine).
            if !list.iter().any(|(c, _)| *c == self.category) {
                self.category = ALL_TAB;
            }
            if self.category != ALL_TAB {
                query.push_str(&format!(" category:{}", self.category));
            }
            if let Some(v) = self.view() {
                self.tabs = list
                    .into_iter()
                    .map(|(c, name)| {
                        let q = if c == ALL_TAB { "is:unread".to_string() } else { format!("is:unread category:{c}") };
                        let unread = self.store.threads(&v, &q, 500).map(|t| t.len() as i64).unwrap_or(0);
                        (c, name, unread)
                    })
                    .collect();
            }
        }
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
        // Moved away but not yet landed: keep them hidden. Once a reload no
        // longer finds one here, its move is done.
        let now = Instant::now();
        let listed: HashSet<(AccountId, &str)> = self.rows.iter().map(|r| (r.t.account_id, r.t.thread_key.as_str())).collect();
        self.hiding.retain(|(a, k), at| now.duration_since(*at) < Duration::from_secs(60) && listed.contains(&(*a, k.as_str())));
        let hiding = &self.hiding;
        self.rows.retain(|r| !hiding.contains_key(&(r.t.account_id, r.t.thread_key.clone())));
        // A hidden Pinned section keeps its conversations out of the list
        // (and out of j/k), until "Show".
        self.hidden_pinned = 0;
        if self.list_settings.hide_pinned && self.filter != ListFilter::Pinned && self.selection != Selection::Snoozed {
            self.hidden_pinned = self.rows.iter().filter(|r| r.t.pinned).count();
            self.rows.retain(|r| !r.t.pinned);
        }
        self.selected = keep.and_then(|(a, k)| self.rows.iter().position(|r| r.t.account_id == a && r.t.thread_key == k));
        let rows = &self.rows;
        self.marked.retain(|(a, k)| rows.iter().any(|r| r.t.account_id == *a && r.t.thread_key == *k));
        if self.marked.len() < 2 {
            self.marked.clear();
        }
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
        self.marked.clear();
        self.anchor = Some(idx);
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

    /// The Inbox tabs (Primary, Promotions, Social) show over an Inbox,
    /// unified or one account's, when not searching.
    fn tabs_shown(&self) -> bool {
        self.search.trim().is_empty()
            && match &self.selection {
                Selection::Unified(r) => *r == FolderRole::Inbox,
                Selection::Folder(id) => self.folders.values().flatten().any(|f| f.id == *id && f.role == FolderRole::Inbox),
                Selection::Snoozed => false,
            }
    }

    /// A tab's name: Promotions, or one of yours (Invoices).
    fn category_label(&self, c: u8) -> String {
        if nightpost_core::category::is_tab(c) {
            self.store.category_name(c)
        } else {
            nightpost_core::category::label(c).to_string()
        }
    }

    fn set_selection(&mut self, s: Selection) {
        self.selection = s;
        self.search.clear();
        self.clear_marks();
        self.selected = None;
        self.open = None;
        self.reload();
    }

    fn current(&self) -> Option<(AccountId, String)> {
        self.open.as_ref().map(|o| (o.account_id, o.thread_key.clone()))
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

    // ----- views ------------------------------------------------------------

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

/// Whether the system asks for dark mode (light when it doesn't say).
fn system_dark(ctx: &egui::Context) -> bool {
    ctx.system_theme() == Some(egui::Theme::Dark)
}

/// The palette for theme `id`; "Moonlit" picks its light or dark variant.
fn theme_palette(themes: &[Theme], id: &str, dark: bool) -> nightpost_core::theme::Palette {
    let id = if id == theme::MOONLIT { if dark { theme::MOONLIT_DARK } else { theme::MOONLIT_LIGHT } } else { id };
    themes.iter().find(|t| t.id == id).unwrap_or(&themes[0]).palette()
}

/// Returns true when the header was clicked (toggle expand).
/// What was asked of an attachment chip in the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttachmentClick {
    Open,
    SaveToDownloads,
    SaveAs,
}

impl eframe::App for NightPostApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.send_pending_now();
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // The web view hangs off the native window, so it's made here.
        if !self.mail_view_tried {
            self.mail_view_tried = true;
            self.mail_view = crate::mail_view::MailView::new(frame, self.colors.bg);
            crate::web_host::set_main_window(frame);
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
        storage.set_string(DARK_EMAILS_KEY, if self.dark_emails { "1" } else { "0" }.into());
        storage.set_string(FACES_KEY, if self.faces.enabled.get() { "1" } else { "0" }.into());
        let mut open: Vec<&String> = self.open_folders.iter().collect();
        open.sort();
        storage.set_string(OPEN_FOLDERS_KEY, open.into_iter().cloned().collect::<Vec<_>>().join("\n"));
        storage.set_string(COLLAPSED_ACCOUNTS_KEY, self.collapsed_accounts.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(","));
    }
}

impl NightPostApp {
    pub fn frame(&mut self, ctx: &egui::Context) {
        self.follow_system_theme(ctx);
        self.handle_events();
        if !self.pending_sends.is_empty() {
            self.flush_sends(false);
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        // Set again by the sidebar this frame if the pointer is over a target.
        self.drop_target = None;
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
        self.refresh_health();
        menu_keys(ctx);
        self.handle_keys(ctx);
        self.toolbar(ctx);
        self.sidebar(ctx);
        self.thread_list(ctx);
        self.reader(ctx);
        self.windows(ctx);
        self.new_tab_dialog(ctx);
        self.folder_dialog_ui(ctx);
        self.keys_sheet(ctx);
        self.activity_sheet(ctx);
        self.drag_overlay(ctx);
        self.place_mail_view(ctx);
        // Keep relative times and snooze wake-ups fresh.
        ctx.request_repaint_after(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod ui_tests;
