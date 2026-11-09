//! Application state and the three-pane layout.

use crate::composer::{Composer, ComposerAction, SignatureBook};
use crate::icons::{self, Icon, Item};
use crate::onboarding::{AccountSetup, SetupAction};
use crate::prefs::{ListSettings, Prefs, PrefsAction, SwipeAction, Tab, QUICK_ACTIONS_MIN_WIDTH};
use crate::style::{self, Colors};
use crate::worker::{AttachmentDo, Cmd, Event, FolderUse, PushState, Worker};
use chrono::{DateTime, Datelike, Utc};
use eframe::egui::{self, Key, RichText, Sense};
use owlpost_core::snooze::{self, SnoozePreset};
use owlpost_core::snooze_sync;
use owlpost_core::theme::{self, Theme};
use owlpost_core::*;
use owlpost_core::smtp::OutgoingMessage;
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
    ("⌘ click", "Add to / remove from the selection"),
    ("Shift click", "Select a range"),
    ("Shift ↓  /  Shift ↑", "Extend the selection"),
    ("⌘ A", "Select every conversation in the list"),
    ("/", "Search"),
    ("g i", "Go to Inbox"),
    ("⌘ Z", "Undo (send, move, archive, trash, label, snooze, pin…)"),
    ("⌘ S", "Save draft (in composer)"),
    ("⌘ ,", "Preferences"),
    ("⌘ W", "Close composer / Preferences"),
    ("⌘ ↩", "Send (in composer)"),
    ("Esc", "Close / clear search / clear the selection"),
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
        let g = date_group(r.t.list_date(), now);
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
    TabRemoved { tab: owlpost_core::snooze_sync::Tab },
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
    /// The Inbox tab showing (an `owlpost_core::category` value).
    category: u8,
    /// The Inbox tabs showing: category, name and unread conversations.
    tabs: Vec<(u8, String, i64)>,
    /// The "New tab" dialog, while it's open.
    new_tab: Option<NewTab>,
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
const DARK_EMAILS_KEY: &str = "owlpost.dark_emails";
/// Parent folders opened in the sidebar ("<account id>\t<path>" per line).
const OPEN_FOLDERS_KEY: &str = "owlpost.open_folders";
const COLLAPSED_ACCOUNTS_KEY: &str = "owlpost.collapsed_accounts";

impl OwlPostApp {
    pub fn new(cc: &eframe::CreationContext<'_>, store: Store, demo: bool) -> Self {
        let get = |key: &str, legacy: &str| cc.storage.and_then(|s| s.get_string(key).or_else(|| s.get_string(legacy)));
        // Built-in theme ids were renamed along with the app.
        let saved = get(THEME_KEY, LEGACY_THEME_KEY).map(|id| id.replace("springpost-", "owlpost-"));
        let mut app = Self::with_context(&cc.egui_ctx, saved, store, demo);
        // Formatted composer windows (WebKit / WebKitGTK); without them the
        // composer stays in egui.
        crate::web_host::init(&cc.egui_ctx);
        if let Some(h) = cc.storage.and_then(|s| s.get_string(HTML_KEY)) {
            app.html_email = h != "0";
        }
        if let Some(i) = cc.storage.and_then(|s| s.get_string(IMAGES_KEY)) {
            app.load_images = i != "ask";
        }
        if let Some(d) = cc.storage.and_then(|s| s.get_string(DARK_EMAILS_KEY)) {
            app.dark_emails = d != "0";
        }
        if let Some(f) = cc.storage.and_then(|s| s.get_string(FACES_KEY)) {
            app.faces.enabled.set(f != "0");
        }
        if let Some(l) = get(LIST_KEY, LEGACY_LIST_KEY) {
            app.list_settings = ListSettings::decode(&l);
        }
        if let Some(f) = cc.storage.and_then(|s| s.get_string(OPEN_FOLDERS_KEY)) {
            app.open_folders = f.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        }
        if let Some(c) = cc.storage.and_then(|s| s.get_string(COLLAPSED_ACCOUNTS_KEY)) {
            app.collapsed_accounts = c.split(',').filter_map(|x| x.trim().parse().ok()).collect();
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
        // …and OWLPOST_COMPOSE=new|reply opens a composer.
        #[cfg(debug_assertions)]
        match std::env::var("OWLPOST_COMPOSE").as_deref() {
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
            category: owlpost_core::category::PRIMARY,
            tabs: Vec::new(),
            new_tab: None,
            web_rect: None,
            remote_images: HashSet::new(),
            pending_input: vec![],
            swipe: None,
            last_wheel_x: None,
            list_default_width: 400.0,
            picker_query: String::new(),
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
        if let Undo::Send { id } = u {
            self.cancel_send(id);
            return;
        }
        self.undo_apply(&u);
        self.toast(format!("Undid {}", u.describe()), false);
        self.reload();
    }

    fn undo_apply(&mut self, u: &Undo) {
        match u {
            Undo::Batch { items, .. } => {
                for i in items.iter().rev() {
                    self.undo_apply(i);
                }
            }
            Undo::Send { id } => self.cancel_send(*id),
            Undo::Moved { a, k, dest, sources, .. } => {
                self.hiding.remove(&(*a, k.clone()));
                self.worker.send(Cmd::Restore(*a, k.clone(), *dest, sources.clone()));
            }
            Undo::Label { a, k, folder, added, .. } => {
                self.hiding.remove(&(*a, k.clone()));
                self.worker.send(Cmd::Label(*a, k.clone(), *folder, !added));
            }
            Undo::Snoozed { a, k } => {
                let _ = self.store.unsnooze(*a, k);
                self.worker.send(Cmd::Unsnooze(*a, k.clone()));
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
            Undo::SenderRule { email, was } => self.put_sender_rule(email, *was),
            Undo::ThreadRule { a, k, was } => self.put_thread_rule(*a, k, *was),
            Undo::TabRemoved { tab } => self.save_tab_change(owlpost_core::snooze_sync::TabChange::Tab(owlpost_core::snooze_sync::restored_tab(tab)), None),
        }
    }

    // ----- Undo Send ----------------------------------------------------------

    /// Send now, or after the Undo Send delay (the composer is kept until then).
    fn queue_send(&mut self, account: AccountId, out: OutgoingMessage, composer: Composer) {
        let secs = self.list_settings.undo_send_secs;
        if secs == 0 {
            self.worker.send(Cmd::Send(account, out, draft_ref(&composer)));
            return;
        }
        let id = self.next_send_id;
        self.next_send_id += 1;
        composer.hide_window();
        self.pending_sends.push(PendingSend { id, account, out, composer, due: Instant::now() + Duration::from_secs(secs as u64) });
        self.undo.push(Undo::Send { id });
    }

    /// Hand messages whose delay has passed to the worker (all of them when `all`).
    fn flush_sends(&mut self, all: bool) {
        let now = Instant::now();
        let (due, wait): (Vec<PendingSend>, Vec<PendingSend>) = std::mem::take(&mut self.pending_sends).into_iter().partition(|p| all || p.due <= now);
        self.pending_sends = wait;
        for p in due {
            self.undo.retain(|u| !matches!(u, Undo::Send { id } if *id == p.id));
            let r = draft_ref(&p.composer);
            self.worker.send(Cmd::Send(p.account, p.out, r));
        }
    }

    /// Undo Send: bring the message back into its window.
    fn cancel_send(&mut self, id: u64) {
        self.undo.retain(|u| !matches!(u, Undo::Send { id: x } if *x == id));
        match self.pending_sends.iter().position(|p| p.id == id) {
            Some(i) => {
                let p = self.pending_sends.remove(i);
                p.composer.show_window();
                self.composers.push(p.composer);
                self.toast("Sending cancelled", false);
            }
            None => self.toast("Too late — that message has already gone", true),
        }
    }

    /// Quitting with messages still waiting: send them before the app goes.
    fn send_pending_now(&mut self) {
        for p in std::mem::take(&mut self.pending_sends) {
            let Some(acc) = self.accounts.iter().find(|a| a.id == p.account).cloned() else { continue };
            let saved = crate::worker::latest_draft(&draft_ref(&p.composer));
            match engine::send(&self.store, &acc, &p.out) {
                Ok(_) => {
                    if let Some(d) = saved {
                        if let Some(da) = self.accounts.iter().find(|a| a.id == d.account_id) {
                            let _ = engine::delete_draft(&self.store, da, &d);
                        }
                    }
                }
                // Keep it rather than lose it: save it as a draft.
                Err(e) => {
                    log::warn!("sending on quit failed: {e:#}");
                    let _ = engine::save_draft(&self.store, &acc, &p.out, saved.as_ref());
                }
            }
        }
    }

    /// Send a held message straight away.
    fn send_now(&mut self, id: u64) {
        if let Some(p) = self.pending_sends.iter_mut().find(|p| p.id == id) {
            p.due = Instant::now();
        }
        self.flush_sends(false);
    }

    // ----- multi-select -------------------------------------------------------

    fn multi(&self) -> bool {
        self.marked.len() >= 2
    }

    fn row_key(&self, i: usize) -> Option<(AccountId, String)> {
        self.rows.get(i).map(|r| (r.t.account_id, r.t.thread_key.clone()))
    }

    fn is_marked(&self, a: AccountId, k: &str) -> bool {
        self.multi() && self.marked.iter().any(|(ma, mk)| *ma == a && mk == k)
    }

    /// ⌘-click: add a conversation to the selection, or take it out.
    fn toggle_mark(&mut self, i: usize) {
        let Some(key) = self.row_key(i) else { return };
        if self.marked.is_empty() {
            if let Some(cur) = self.current() {
                self.marked.push(cur);
            }
        }
        match self.marked.iter().position(|x| *x == key) {
            Some(p) => {
                self.marked.remove(p);
            }
            None => self.marked.push(key),
        }
        self.anchor = Some(i);
        self.marks_changed();
    }

    /// ⇧-click: everything from the anchor to here.
    fn mark_range(&mut self, i: usize) {
        let from = self.anchor.or(self.selected).unwrap_or(i);
        let (lo, hi) = (from.min(i), from.max(i));
        self.marked = (lo..=hi).filter_map(|j| self.row_key(j)).collect();
        self.anchor = Some(from);
        self.marks_changed();
    }

    /// ⇧↑ / ⇧↓: grow or shrink the selection from the anchor.
    fn extend_marks(&mut self, d: i64) {
        if self.rows.is_empty() {
            return;
        }
        let from = self.anchor.or(self.selected).unwrap_or(0);
        // The moving end: the marked row furthest from the anchor.
        let end = self
            .marked
            .iter()
            .filter_map(|(a, k)| self.rows.iter().position(|r| r.t.account_id == *a && r.t.thread_key == *k))
            .max_by_key(|&j| (j as i64 - from as i64).abs())
            .unwrap_or(from);
        let to = (end as i64 + d).clamp(0, self.rows.len() as i64 - 1) as usize;
        let (lo, hi) = (from.min(to), from.max(to));
        self.marked = (lo..=hi).filter_map(|j| self.row_key(j)).collect();
        self.anchor = Some(from);
        self.marks_changed();
    }

    fn select_all(&mut self) {
        self.marked = (0..self.rows.len()).filter_map(|j| self.row_key(j)).collect();
        self.anchor = Some(0);
        self.marks_changed();
    }

    fn clear_marks(&mut self) {
        self.marked.clear();
    }

    /// With several marked the reading pane shows a summary; back to one,
    /// that conversation opens.
    fn marks_changed(&mut self) {
        match self.marked.len() {
            0 => {}
            1 => {
                let (a, k) = self.marked.remove(0);
                if let Some(i) = self.rows.iter().position(|r| r.t.account_id == a && r.t.thread_key == k) {
                    let anchor = self.anchor;
                    self.select(i);
                    self.anchor = anchor;
                }
            }
            _ => {
                self.open = None;
                self.selected = None;
            }
        }
    }

    /// The conversations an action applies to: the selection, or the open one.
    fn targets(&self) -> Vec<(AccountId, String)> {
        if self.multi() {
            self.marked.clone()
        } else {
            self.current().into_iter().collect()
        }
    }

    /// Run `f` on the open conversation, or on every selected one. With
    /// several, their undos become one ⌘Z and one toast sums them up.
    fn for_each_target(&mut self, text: impl FnOnce(usize) -> String, mut f: impl FnMut(&mut Self, AccountId, &str)) {
        if !self.multi() {
            if let Some((a, k)) = self.current() {
                f(self, a, &k);
            }
            return;
        }
        let targets = std::mem::take(&mut self.marked);
        self.apply_to(targets, text, f);
    }

    /// Run `f` on these conversations; with several, one ⌘Z and one toast.
    fn apply_to(&mut self, targets: Vec<(AccountId, String)>, text: impl FnOnce(usize) -> String, mut f: impl FnMut(&mut Self, AccountId, &str)) {
        if let [(a, k)] = targets.as_slice() {
            f(self, *a, k);
            return;
        }
        self.marked.clear();
        self.open = None;
        self.selected = None;
        let (u0, t0) = (self.undo.len(), self.toasts.len());
        for (a, k) in &targets {
            f(self, *a, k);
        }
        let mut errors: Vec<Toast> = self.toasts.drain(t0..).filter(|t| t.error).collect();
        errors.dedup_by(|x, y| x.text == y.text);
        let items: Vec<Undo> = self.undo.drain(u0..).collect();
        let n = targets.len();
        let text = text(n);
        if items.is_empty() {
            self.toast(text, false);
        } else {
            let what = format!("{} on {n} conversations", items[0].describe());
            self.did(text, Undo::Batch { items, what });
        }
        self.toasts.extend(errors);
        // No reload here: moves finish in the background, and reloading now
        // would show the conversations again until they do.
    }

    /// The rows behind the selection (for "are they all starred?" and such).
    fn target_rows(&self) -> Vec<&ThreadSummary> {
        let t = self.targets();
        self.rows.iter().map(|r| &r.t).filter(|r| t.iter().any(|(a, k)| *a == r.account_id && *k == r.thread_key)).collect()
    }

    /// The account every selected conversation belongs to, if they share one
    /// (folders and labels are per account).
    fn targets_account(&self) -> Option<AccountId> {
        let t = self.targets();
        let a = t.first()?.0;
        t.iter().all(|(x, _)| *x == a).then_some(a)
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
        let mut query = format!("{} {}", self.search, self.filter.token());
        if self.tabs_shown() {
            let mut list: Vec<(u8, String)> = owlpost_core::category::ALL.iter().map(|c| (*c, owlpost_core::category::label(*c).to_string())).collect();
            list.extend(self.store.tabs().unwrap_or_default());
            // The tab was removed (perhaps on another machine).
            if !list.iter().any(|(c, _)| *c == self.category) {
                self.category = owlpost_core::category::PRIMARY;
            }
            query.push_str(&format!(" category:{}", self.category));
            if let Some(v) = self.view() {
                self.tabs = list
                    .into_iter()
                    .map(|(c, name)| {
                        let unread = self.store.threads(&v, &format!("is:unread category:{c}"), 500).map(|t| t.len() as i64).unwrap_or(0);
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
        if owlpost_core::category::is_tab(c) {
            self.store.category_name(c)
        } else {
            owlpost_core::category::label(c).to_string()
        }
    }

    /// Keep a tab change here at once and on the servers in the background
    /// (`label`: a conversation that moved, and the tab it left, for Gmail's labels).
    fn save_tab_change(&mut self, change: owlpost_core::snooze_sync::TabChange, label: Option<(AccountId, String, u8)>) {
        use owlpost_core::snooze_sync::TabChange;
        let _ = match &change {
            TabChange::Tab(t) => self.store.put_tab(t),
            TabChange::Sender(r) => self.store.put_sender_rule(r),
            TabChange::Thread(r) => self.store.put_thread_rule(r),
        };
        self.worker.send(Cmd::TabChange(change, label));
        self.reload();
    }

    /// Always sort this sender's mail into `category` (None: automatic),
    /// here at once and on the server in the background.
    fn put_sender_rule(&mut self, email: &str, category: Option<u8>) {
        match owlpost_core::snooze_sync::sender_rule(&self.store, email, category) {
            Ok(r) => self.save_tab_change(owlpost_core::snooze_sync::TabChange::Sender(r), None),
            Err(e) => self.toast(format!("{e:#}"), true),
        }
    }

    /// Sort a sender (undoable), from the list's context menu.
    fn sort_sender(&mut self, email: &str, category: Option<u8>) {
        let was = self.store.sender_rule(email).ok().flatten();
        if was == category {
            return;
        }
        self.put_sender_rule(email, category);
        let text = match category {
            Some(c) => format!("Mail from {email} goes to {}", self.category_label(c)),
            None => format!("Mail from {email} is sorted automatically"),
        };
        self.did(text, Undo::SenderRule { email: email.to_string(), was });
    }

    /// Put one conversation in `category` (None: sort it like the rest).
    fn put_thread_rule(&mut self, a: AccountId, k: &str, category: Option<u8>) {
        let before = self.store.thread_category(a, k).unwrap_or(0);
        match owlpost_core::snooze_sync::thread_rule(&self.store, a, k, category) {
            Ok(r) => self.save_tab_change(owlpost_core::snooze_sync::TabChange::Thread(r), Some((a, k.to_string(), before))),
            Err(e) => self.toast(format!("{e:#}"), true),
        }
    }

    /// Put a conversation in a tab (undoable), from the list's context menu.
    fn sort_thread(&mut self, a: AccountId, k: &str, category: Option<u8>) {
        let was = self.store.thread_rule(a, k).ok().flatten();
        if was == category {
            return;
        }
        self.put_thread_rule(a, k, category);
        let text = match category {
            Some(c) => format!("Moved to {}", self.category_label(c)),
            None => format!("Sorted like the rest ({})", self.category_label(self.store.thread_category(a, k).unwrap_or(0))),
        };
        self.did(text, Undo::ThreadRule { a, k: k.to_string(), was });
    }

    /// Make a tab and put a sender's mail or a conversation in it.
    fn create_tab(&mut self, name: &str, then: &NewTabFor) -> Result<(), String> {
        let tab = owlpost_core::snooze_sync::new_tab(&self.store, name).map_err(|e| format!("{e:#}"))?;
        let existed = self.store.all_tabs().unwrap_or_default().iter().any(|t| t.id == tab.id);
        if !existed {
            self.save_tab_change(owlpost_core::snooze_sync::TabChange::Tab(tab.clone()), None);
        }
        let Some(c) = self.store.category_by_name(&tab.name) else { return Err("Couldn't make the tab".into()) };
        match then {
            NewTabFor::Sender(email) => self.sort_sender(email, Some(c)),
            NewTabFor::Thread(a, k) => self.sort_thread(*a, k, Some(c)),
        }
        Ok(())
    }

    /// Remove one of your tabs (undoable). Its mail goes back to being
    /// sorted automatically; its rules stay, in case it comes back.
    fn remove_tab(&mut self, c: u8) {
        let Ok(tab) = owlpost_core::snooze_sync::removed_tab(&self.store, c) else { return };
        let name = tab.name.clone();
        self.save_tab_change(owlpost_core::snooze_sync::TabChange::Tab(tab.clone()), None);
        self.did(format!("Removed the “{name}” tab"), Undo::TabRemoved { tab });
    }

    /// The "New tab" dialog.
    fn new_tab_dialog(&mut self, ctx: &egui::Context) {
        let Some(nt) = self.new_tab.as_mut() else { return };
        let col = self.colors.clone();
        let mut done: Option<bool> = None;
        egui::Modal::new(egui::Id::new("new-tab")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(RichText::new("New tab").size(17.0).strong().color(col.heading));
            ui.add_space(4.0);
            let what = match &nt.then {
                NewTabFor::Sender(email) => format!("Mail from {email} will go in it."),
                NewTabFor::Thread(..) => "This conversation will go in it.".to_string(),
            };
            ui.label(RichText::new(what).color(col.subtle));
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::singleline(&mut nt.name).hint_text("Invoices").desired_width(f32::INFINITY));
            if !r.has_focus() && nt.error.is_none() && nt.name.is_empty() {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = Some(true);
            }
            if let Some(e) = &nt.error {
                ui.label(RichText::new(e).color(col.error));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(!nt.name.trim().is_empty(), egui::Button::new("Make tab")).clicked() {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
        match done {
            Some(true) => {
                let (name, then) = (nt.name.clone(), nt.then.clone());
                match self.create_tab(&name, &then) {
                    Ok(()) => self.new_tab = None,
                    Err(e) => {
                        if let Some(nt) = self.new_tab.as_mut() {
                            nt.error = Some(e);
                        }
                    }
                }
            }
            Some(false) => self.new_tab = None,
            None => {}
        }
    }

    /// List everything from one address (a search, so Esc or clearing it goes back).
    fn show_sender(&mut self, email: &str) {
        self.search = format!("from:{email}");
        self.clear_marks();
        self.selected = None;
        self.open = None;
        self.reload();
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
        let done = match role {
            FolderRole::Archive => "Archived",
            FolderRole::Trash => "Moved to Trash",
            FolderRole::Spam => "Reported as spam",
            FolderRole::Inbox => "Moved to Inbox",
            _ => "Moved",
        };
        self.for_each_target(|n| format!("{done} {n} conversations"), |s, a, k| s.move_thread(a, k, role));
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
        self.hiding.insert((a, k.to_string()), Instant::now());
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
        if self.multi() {
            // Star them all, unless they all are already.
            let star = !self.target_rows().iter().all(|t| t.starred);
            self.for_each_target(
                |n| format!("{} {n} conversations", if star { "Starred" } else { "Unstarred" }),
                |s, a, k| {
                    let was = s.rows.iter().find(|r| r.t.account_id == a && r.t.thread_key == k).is_some_and(|r| r.t.starred);
                    if was != star {
                        let _ = s.store.set_thread_starred(a, k, star);
                        s.worker.send(Cmd::SetStarred(a, k.to_string(), star));
                        s.undo.push(Undo::Star { a, k: k.to_string(), was });
                    }
                },
            );
            self.reload();
            return;
        }
        let Some((a, k)) = self.current() else { return };
        let starred = !self.open.as_ref().map(|o| o.messages.iter().any(|m| m.starred)).unwrap_or(false);
        let _ = self.store.set_thread_starred(a, &k, starred);
        self.worker.send(Cmd::SetStarred(a, k.clone(), starred));
        self.undo.push(Undo::Star { a, k, was: !starred });
        self.reload();
    }

    fn act_unread(&mut self) {
        if self.multi() {
            // Any unread among them: mark them all read; otherwise all unread.
            let unread = !self.target_rows().iter().any(|t| t.unread);
            let items: Vec<(AccountId, String, bool)> =
                self.target_rows().iter().filter(|t| t.unread != unread).map(|t| (t.account_id, t.thread_key.clone(), t.unread)).collect();
            for (a, k, _) in &items {
                self.set_row_unread(*a, k, unread);
            }
            let n = self.marked.len();
            self.clear_marks();
            self.did(format!("Marked {n} conversations as {}", if unread { "unread" } else { "read" }), Undo::Unread { items });
            self.reload();
            return;
        }
        let Some((a, k)) = self.current() else { return };
        let _ = self.store.set_thread_unread(a, &k, true);
        self.worker.send(Cmd::SetUnread(a, k.clone(), true));
        self.did("Marked as unread", Undo::Unread { items: vec![(a, k, false)] });
        self.open = None;
        self.selected = None;
        self.reload();
    }

    fn act_snooze(&mut self, until: DateTime<Utc>) {
        if self.multi() {
            self.for_each_target(
                |n| format!("Snoozed {n} conversations until {}", snooze::describe(until)),
                |s, a, k| match s.store.snooze(a, k, until) {
                    Ok(()) => {
                        s.worker.send(Cmd::Snooze(a, k.to_string(), until));
                        s.undo.push(Undo::Snoozed { a, k: k.to_string() });
                    }
                    Err(e) => s.toast(format!("Couldn't snooze: {e:#}"), true),
                },
            );
            self.snoozed_count = self.store.snoozed_count().unwrap_or(0);
            self.reload();
            return;
        }
        let Some((a, k)) = self.current() else { return };
        // Hidden here at once; the worker files it on the server.
        if let Err(e) = self.store.snooze(a, &k, until) {
            self.toast(format!("Couldn't snooze: {e:#}"), true);
            return;
        }
        self.worker.send(Cmd::Snooze(a, k.clone(), until));
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
        self.worker.send(Cmd::Unsnooze(a, k.clone()));
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
        if !self.multi() {
            if let Some((a, k)) = self.current() {
                self.toggle_pin(a, &k);
            }
            return;
        }
        // Pin them all, unless they all are already.
        let pin = !self.target_rows().iter().all(|t| t.pinned);
        self.for_each_target(
            |n| format!("{} {n} conversations", if pin { "Pinned" } else { "Unpinned" }),
            |s, a, k| {
                if s.store.is_pinned(a, k).unwrap_or(false) != pin {
                    s.toggle_pin(a, k);
                }
            },
        );
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

    /// Dragged conversations released over a sidebar entry.
    fn drop_on(&mut self, keys: Vec<(AccountId, String)>, target: DropTarget) {
        match target {
            DropTarget::Role(role) => {
                let done = match role {
                    FolderRole::Archive => "Archived",
                    FolderRole::Trash => "Moved to Trash",
                    FolderRole::Spam => "Reported as spam",
                    _ => "Moved to Inbox",
                };
                self.apply_to(keys, |n| format!("{done} {n} conversations"), |s, a, k| s.move_thread(a, k, role));
            }
            DropTarget::Folder(f) => {
                // Folders belong to one account.
                let (mine, other): (Vec<_>, Vec<_>) = keys.into_iter().partition(|(a, _)| *a == f.account_id);
                if !other.is_empty() {
                    self.toast(format!("{} of them {} in another account, so stayed put", other.len(), if other.len() == 1 { "is" } else { "are" }), true);
                }
                if mine.is_empty() || self.viewed_folder(f.account_id) == Some(f.id) {
                    return;
                }
                let name = folder_label(&f);
                match f.role {
                    FolderRole::Inbox | FolderRole::Archive | FolderRole::Spam | FolderRole::Trash => {
                        let role = f.role;
                        self.apply_to(mine, |n| format!("Moved {n} conversations to {name}"), |s, a, k| s.move_thread(a, k, role))
                    }
                    _ => self.apply_to(mine, |n| format!("Moved {n} conversations to {name}"), |s, a, k| s.move_to(a, k, &f)),
                }
            }
        }
    }

    /// The drag in progress: a label under the pointer, and the drop on release.
    fn drag_overlay(&mut self, ctx: &egui::Context) {
        let Some(keys) = &self.dragging else { return };
        let n = keys.len();
        let (pos, released) = ctx.input(|i| (i.pointer.interact_pos(), i.pointer.any_released() || !i.pointer.any_down()));
        if released {
            let keys = self.dragging.take().unwrap_or_default();
            if let Some(t) = self.drop_target.take() {
                self.drop_on(keys, t);
            }
            return;
        }
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(pos) = pos {
            let what = if n == 1 { "1 conversation".to_string() } else { format!("{n} conversations") };
            let text = match &self.drop_target {
                Some(DropTarget::Role(r)) => format!("Move {what} to {}", r.label()),
                Some(DropTarget::Folder(f)) => format!("Move {what} to {}", folder_label(f)),
                None => format!("Move {what}"),
            };
            egui::Area::new(egui::Id::new("drag_mail")).fixed_pos(pos + egui::vec2(14.0, 10.0)).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
                egui::Frame::new().fill(egui::Color32::from_rgb(40, 44, 52)).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 7)).show(ui, |ui| {
                    ui.label(RichText::new(format!("✉  {text}")).color(egui::Color32::WHITE));
                });
            });
        }
        ctx.request_repaint();
    }

    /// "Show original": fetch the newest message's raw source.
    fn show_original(&mut self) {
        let Some(m) = self.open.as_ref().and_then(|o| o.messages.last()).cloned() else { return };
        self.toast("Fetching the original…", false);
        self.worker.send(Cmd::ShowOriginal(Box::new(m)));
    }

    /// The "Show original" windows: the raw message, selectable, to copy or save.
    fn original_windows(&mut self, ctx: &egui::Context) {
        let col = self.colors.clone();
        let mut close = vec![];
        let mut notice = None;
        for o in &self.originals {
            let title = format!("Original — {}", o.subject);
            let vid = egui::ViewportId::from_hash_of(("original", o.id));
            let builder = egui::ViewportBuilder::default().with_title(&title).with_inner_size([820.0, 640.0]);
            ctx.show_viewport_immediate(vid, builder, |ctx, class| {
                let body = |ui: &mut egui::Ui, notice: &mut Option<String>| {
                    ui.horizontal(|ui| {
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(o.text.clone());
                            *notice = Some("Copied the original".into());
                        }
                        if ui.button("Save as .eml…").clicked() {
                            let name = format!("{}.eml", owlpost_core::engine::safe_filename(&o.subject));
                            if let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() {
                                *notice = Some(match std::fs::write(&path, &o.text) {
                                    Ok(()) => format!("Saved {}", path.display()),
                                    Err(e) => format!("Couldn't save: {e}"),
                                });
                            }
                        }
                        ui.label(RichText::new(format!("{} KB", o.text.len() / 1024)).small().color(col.subtle));
                    });
                    ui.separator();
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut o.text.as_str()).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).code_editor());
                    });
                };
                if class == egui::ViewportClass::Embedded {
                    let mut open = true;
                    egui::Window::new(&title).id(egui::Id::new(("original", o.id))).open(&mut open).default_size([760.0, 560.0]).show(ctx, |ui| body(ui, &mut notice));
                    if !open {
                        close.push(o.id);
                    }
                    return;
                }
                egui::CentralPanel::default().frame(egui::Frame::new().fill(col.bg).inner_margin(egui::Margin::same(12))).show(ctx, |ui| body(ui, &mut notice));
                if ctx.input(|i| i.viewport().close_requested()) || ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::W)) {
                    close.push(o.id);
                }
            });
        }
        self.originals.retain(|o| !close.contains(&o.id));
        if let Some(n) = notice {
            self.toast(n, false);
        }
    }

    /// Open a saved draft in a composer (its attachments are fetched first).
    fn edit_draft(&mut self, m: Message) {
        if self.composers.iter().any(|c| c.draft.as_ref().is_some_and(|d| d.message_id == m.message_id)) {
            self.toast("That draft is already open", false);
            return;
        }
        if self.opening_drafts.insert(m.message_id.clone()) {
            self.worker.send(Cmd::OpenDraft(Box::new(m)));
        }
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
                Event::DraftSaved(id, r) => {
                    if let Some(c) = self.composers.iter_mut().chain(self.pending_sends.iter_mut().map(|p| &mut p.composer)).find(|c| c.id == id) {
                        c.saving = false;
                        match r {
                            Ok(m) => {
                                c.draft = Some(m);
                                c.status = Some(format!("Saved {}", chrono::Local::now().format("%H:%M")));
                            }
                            Err(_) => c.status = Some("Couldn't save the draft — it will try again as you type".into()),
                        }
                    }
                }
                Event::Original(subject, r) => match r {
                    Ok(text) => {
                        let id = self.next_composer_id;
                        self.next_composer_id += 1;
                        self.originals.push(Original { id, subject, text });
                    }
                    Err(e) => self.toast(format!("Couldn't show the original: {e}"), true),
                },
                Event::DraftReady(m, r) => {
                    self.opening_drafts.remove(&m.message_id);
                    match (r, self.accounts.iter().find(|a| a.id == m.account_id).cloned()) {
                        (Ok(parts), Some(acc)) => {
                            let id = self.next_composer_id;
                            self.next_composer_id += 1;
                            self.composers.push(Composer::from_draft(id, &acc, &m, parts, &self.sigs));
                        }
                        (Err(e), _) => self.toast(e, true),
                        (_, None) => {}
                    }
                }
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
            if self.multi() && !ctx.wants_keyboard_input() {
                self.clear_marks();
            }
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
        // ⌘A: select every conversation in the list.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::A)) {
            self.select_all();
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
                let gmail = gmail || (self.multi() && self.targets_account().and_then(|a| self.accounts.iter().find(|x| x.id == a)).is_some_and(|a| a.is_gmail()));
                if (self.open.is_some() || (self.multi() && self.targets_account().is_some())) && (text == "v" || gmail) {
                    let id = if text == "v" { "picker_open_move" } else { "picker_open_label" };
                    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new(id), true));
                }
            }
            "b" | "h" => {
                if self.open.is_some() || self.multi() {
                    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("snooze_open"), true));
                }
            }
            "j" => self.move_selection(1),
            "k" => self.move_selection(-1),
            "/" => self.focus_search = true,
            "g" => self.pending_g = Some(Instant::now()),
            _ => {}
        }
        let shift = ctx.input(|i| i.modifiers.shift);
        if key(Key::ArrowDown) {
            if shift { self.extend_marks(1) } else { self.move_selection(1) }
        }
        if key(Key::ArrowUp) {
            if shift { self.extend_marks(-1) } else { self.move_selection(-1) }
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
        let multi = self.multi();
        // With several selected, folders and labels need them all in one account.
        let current = if multi { self.targets_account().and_then(|a| self.marked.iter().find(|m| m.0 == a).cloned()) } else { self.current() };
        let single = self.current().is_some() && !multi;
        let open = single || multi;
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
                            Item::new(Icon::Reply, "Reply (r)").enabled(single),
                            Item::new(Icon::ReplyAll, "Reply all (a)").enabled(single),
                            Item::new(Icon::Forward, "Forward (f)").enabled(single),
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
                            Item::new(Icon::Folder, if current.is_none() && multi { "Move: select conversations from one account" } else { "Move to folder (v)" }).enabled(current.is_some()),
                            Item::new(Icon::Tag, if gmail || !open { "Label (l)" } else { "Labels are a Gmail feature — use Move on this account" }).enabled(current.is_some() && gmail),
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
            .filter(|f| !snooze_sync::is_internal_folder(&f.path))
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
            (Some((f, add)), _) => {
                let name = folder_label(&f);
                match kind {
                    FolderUse::Move => self.for_each_target(|n| format!("Moved {n} conversations to {name}"), |s, a, k| s.move_to(a, k, &f)),
                    FolderUse::Label => self.for_each_target(
                        |n| if add { format!("Labelled {n} conversations “{name}”") } else { format!("Removed “{name}” from {n} conversations") },
                        |s, a, k| s.set_label(a, k, &f, add),
                    ),
                }
            }
            (None, Some(name)) => {
                let keys: Vec<String> = self.targets().into_iter().map(|(_, k)| k).collect();
                let keys = if keys.is_empty() { vec![k.clone()] } else { keys };
                self.worker.send(Cmd::CreateFolder(a, name.clone(), Some((keys.clone(), kind))));
                if kind == FolderUse::Move {
                    for k in &keys {
                        self.remove_row(a, k);
                    }
                }
                self.clear_marks();
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
            if self.open.is_some() {
                ui.separator();
                if ui.add(egui::Button::new("Show original").frame(false).min_size(egui::vec2(190.0, 24.0))).on_hover_text("The newest message's raw source").clicked() {
                    action = Some(4);
                }
            }
        });
        match action {
            Some(0) => self.act_star(),
            Some(1) => self.act_unread(),
            Some(2) => self.act_move(FolderRole::Trash),
            Some(3) => self.act_move(FolderRole::Spam),
            Some(4) => self.show_original(),
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
                        // Dragged mail can land on Inbox, Archive, Spam and Trash.
                        let droppable = self.dragging.is_some() && matches!(role, FolderRole::Inbox | FolderRole::Archive | FolderRole::Spam | FolderRole::Trash);
                        let over = droppable && ui.rect_contains_pointer(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 31.0)));
                        if over {
                            self.drop_target = Some(DropTarget::Role(role));
                        }
                        if nav_item_r(ui, &col, role_icon(role), label, count, sel, over).clicked() {
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
                            let droppable = self.dragging.is_some() && matches!(f.role, FolderRole::Inbox | FolderRole::Archive | FolderRole::Spam | FolderRole::Trash | FolderRole::All);
                            let over = droppable && ui.rect_contains_pointer(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 31.0)));
                            if over {
                                self.drop_target = Some(DropTarget::Folder(f.clone()));
                            }
                            if nav_item_r(ui, &col, role_icon(f.role), &label, count, sel, over).clicked() {
                                self.set_selection(Selection::Folder(f.id));
                            }
                        }
                        // Custom folders as a tree: parents fold away their subfolders.
                        let others: Vec<&Folder> = folders.iter().filter(|f| f.role == FolderRole::Other && !snooze_sync::is_internal_folder(&f.path)).collect();
                        let tree = FolderTree::new(&others);
                        let mut toggle: Option<String> = None;
                        for (i, f) in others.iter().enumerate() {
                            if tree.ancestors[i].iter().any(|&p| !self.open_folders.contains(&folder_key(a.id, &others[p].path))) {
                                continue;
                            }
                            let key = folder_key(a.id, &f.path);
                            let parent = tree.has_children(i);
                            let expanded = self.open_folders.contains(&key);
                            // A folded parent counts the unread mail inside it too.
                            let count = if parent && !expanded { f.unread + tree.descendants(i).map(|d| others[d].unread).sum::<i64>() } else { f.unread };
                            let sel = self.selection == Selection::Folder(f.id) && self.search.is_empty();
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.add_space(tree.ancestors[i].len() as f32 * 14.0);
                                let (slot, r) = ui.allocate_exact_size(egui::vec2(16.0, 31.0), if parent { Sense::click() } else { Sense::hover() });
                                if parent {
                                    let c = slot.center() + egui::vec2(2.0, 0.0);
                                    let fg = if r.hovered() { col.sidebar_text } else { col.sidebar_text.gamma_multiply(0.6) };
                                    let pts = if expanded {
                                        vec![c + egui::vec2(-3.5, -1.5), c + egui::vec2(0.0, 2.0), c + egui::vec2(3.5, -1.5)]
                                    } else {
                                        vec![c + egui::vec2(-1.5, -3.5), c + egui::vec2(2.0, 0.0), c + egui::vec2(-1.5, 3.5)]
                                    };
                                    ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.4_f32, fg)));
                                    let r = r.on_hover_text(if expanded { "Hide subfolders" } else { "Show subfolders" });
                                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} {}", if expanded { "Collapse" } else { "Expand" }, f.name)));
                                    if r.clicked() {
                                        toggle = Some(key.clone());
                                    }
                                }
                                let over = self.dragging.is_some() && ui.rect_contains_pointer(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 31.0)));
                                if over {
                                    self.drop_target = Some(DropTarget::Folder((*f).clone()));
                                }
                                if nav_item_r(ui, &col, Icon::Folder, &f.name, count, sel, over).clicked() {
                                    self.set_selection(Selection::Folder(f.id));
                                }
                            });
                        }
                        if let Some(k) = toggle {
                            if !self.open_folders.remove(&k) {
                                self.open_folders.insert(k);
                            }
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
        let mut tab = None;
        let mut remove = None;
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
            if self.tabs_shown() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(18.0, 4.0);
                    for (c, name, unread) in &self.tabs {
                        let r = category_tab(ui, col, name, *unread, *c == self.category);
                        if r.clicked() && *c != self.category {
                            tab = Some(*c);
                        }
                        if owlpost_core::category::is_tab(*c) {
                            r.context_menu(|ui| {
                                if ui.button(format!("Remove the “{name}” tab")).on_hover_text("Its mail goes back to being sorted automatically").clicked() {
                                    remove = Some(*c);
                                }
                            });
                        }
                    }
                });
                ui.add_space(10.0);
            }
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
        if let Some(c) = remove {
            self.remove_tab(c);
        }
        if let Some(c) = tab {
            self.category = c;
            self.selected = None;
            self.open = None;
            self.clear_marks();
            self.reload();
        }
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
                            _ if self.tabs_shown() && self.category != owlpost_core::category::PRIMARY => ("📭", "Nothing here right now."),
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
                // Show a sender's mail, or sort it into a tab.
                let mut sender_menu: Option<(String, Option<Option<u8>>)> = None;
                let mut thread_menu: Option<(AccountId, String, Option<u8>)> = None;
                let mut new_tab: Option<NewTabFor> = None;
                let all_tabs: Vec<(u8, String)> = owlpost_core::category::ALL
                    .iter()
                    .map(|c| (*c, owlpost_core::category::label(*c).to_string()))
                    .chain(self.store.tabs().unwrap_or_default())
                    .collect();
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
                        // …or a mouse drag: sideways swipes, up/down picks the
                        // conversation up to drop on a folder in the sidebar.
                        if resp.dragged() && self.dragging.is_none() {
                            let dx = resp.drag_delta().x;
                            let (origin, at) = ui.ctx().input(|inp| (inp.pointer.press_origin(), inp.pointer.interact_pos()));
                            let moved = match (origin, at) {
                                (Some(o), Some(p)) => p - o,
                                _ => egui::Vec2::ZERO,
                            };
                            // Dragged out of the list into the sidebar: that's a move too.
                            let into_sidebar = self.show_sidebar && at.is_some_and(|p| p.x < self.sidebar_right);
                            let pick_up = |s: &mut Self| {
                                s.swipe = None;
                                let key = (t.account_id, t.thread_key.clone());
                                s.dragging = Some(if s.is_marked(key.0, &key.1) { s.marked.clone() } else { vec![key] });
                            };
                            match &mut self.swipe {
                                Some(s) if is_this(s) && into_sidebar => pick_up(self),
                                Some(s) if is_this(s) => {
                                    s.phase = SwipePhase::Drag;
                                    s.offset += dx;
                                }
                                None if moved.length() >= 6.0 && moved.y.abs() > moved.x.abs() => pick_up(self),
                                None if moved.length() >= 6.0 => self.swipe = Some(Swipe::new(&t, SwipePhase::Drag, moved.x)),
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

                        let selected = self.selected == Some(i) || self.is_marked(t.account_id, &t.thread_key);
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
                            None => style::short_date(t.list_date()),
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
                            // The newest sender who isn't you.
                            let sender = t.participants.iter().rev().find(|p| !p.email.is_empty() && !self.accounts.iter().any(|a| a.email.eq_ignore_ascii_case(&p.email)));
                            if let Some(p) = sender {
                                let email = p.email.to_lowercase();
                                if ui.button(format!("All mail from {email}")).clicked() {
                                    sender_menu = Some((email.clone(), None));
                                }
                                let rule = self.store.sender_rule(&email).ok().flatten();
                                ui.menu_button("Always put their mail in", |ui| {
                                    for (c, name) in &all_tabs {
                                        let mark = if rule == Some(*c) { "✓" } else { "  " };
                                        if ui.button(format!("{mark}  {name}")).clicked() {
                                            sender_menu = Some((email.clone(), Some(Some(*c))));
                                        }
                                    }
                                    if ui.button("     New tab…").clicked() {
                                        new_tab = Some(NewTabFor::Sender(email.clone()));
                                    }
                                    ui.separator();
                                    let mark = if rule.is_none() { "✓" } else { "  " };
                                    if ui.button(format!("{mark}  Sort automatically")).on_hover_text("OwlPost decides from each email (Gmail's own tabs on Gmail)").clicked() {
                                        sender_menu = Some((email.clone(), Some(None)));
                                    }
                                });
                            }
                            let here = self.store.thread_rule(t.account_id, &t.thread_key).ok().flatten();
                            ui.menu_button("Move this conversation to", |ui| {
                                for (c, name) in &all_tabs {
                                    let mark = if here == Some(*c) { "✓" } else { "  " };
                                    if ui.button(format!("{mark}  {name}")).clicked() {
                                        thread_menu = Some((t.account_id, t.thread_key.clone(), Some(*c)));
                                    }
                                }
                                if ui.button("     New tab…").clicked() {
                                    new_tab = Some(NewTabFor::Thread(t.account_id, t.thread_key.clone()));
                                }
                                ui.separator();
                                let mark = if here.is_none() { "✓" } else { "  " };
                                if ui.button(format!("{mark}  Sort like the rest")).on_hover_text("By the sender's rule, or automatically").clicked() {
                                    thread_menu = Some((t.account_id, t.thread_key.clone(), None));
                                }
                            });
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
                        let m = ctx.input(|i| i.modifiers);
                        if m.command {
                            self.toggle_mark(i);
                        } else if m.shift {
                            self.mark_range(i);
                        } else {
                            self.select(i);
                        }
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
                match sender_menu {
                    Some((email, None)) => self.show_sender(&email),
                    Some((email, Some(c))) => self.sort_sender(&email, c),
                    None => {}
                }
                if let Some((a, k, c)) = thread_menu {
                    self.sort_thread(a, &k, c);
                }
                if let Some(then) = new_tab {
                    self.new_tab = Some(NewTab { name: String::new(), then, error: None });
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

    /// The reading pane while several conversations are selected.
    fn selection_summary(&mut self, ui: &mut egui::Ui, col: &Colors) {
        let n = self.marked.len();
        let rows = self.target_rows();
        let unread = rows.iter().any(|t| t.unread);
        let starred = rows.iter().all(|t| t.starred);
        let pinned = rows.iter().all(|t| t.pinned);
        let mut act = None;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.3).max(40.0));
            ui.label(RichText::new(format!("{n} conversations selected")).size(26.0).color(col.heading));
            ui.add_space(18.0);
            let buttons: [(Icon, &str, u8); 6] = [
                (Icon::Done, "Archive", 0),
                (Icon::Trash, "Trash", 1),
                (Icon::Done, if unread { "Mark as read" } else { "Mark as unread" }, 2),
                (Icon::Star, if starred { "Unstar" } else { "Star" }, 3),
                (if pinned { Icon::PinFilled } else { Icon::Pin }, if pinned { "Unpin" } else { "Pin" }, 4),
                (Icon::Snooze, "Snooze…", 5),
            ];
            // Centre the row of buttons (each is its label plus 50 points).
            let gap = 8.0;
            let w: f32 = buttons
                .iter()
                .map(|(_, l, _)| ui.painter().layout_no_wrap(l.to_string(), egui::FontId::proportional(13.5), col.text).size().x + 50.0 + gap)
                .sum::<f32>()
                - gap;
            let avail = ui.available_width();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.add_space(((avail - w) / 2.0).max(0.0));
                for (icon, label, id) in buttons {
                    if icons::labeled(ui, icon, label, col.text, col.bg).clicked() {
                        act = Some(id);
                    }
                }
            });
            ui.add_space(16.0);
            ui.label(RichText::new("⌘-click to add or remove · Shift-click for a range · ⌘A for all · Esc to clear").small().color(col.very_subtle));
            ui.add_space(6.0);
            if ui.add(egui::Button::new(RichText::new("Clear selection").color(col.link)).frame(false)).clicked() {
                act = Some(9);
            }
        });
        match act {
            Some(0) => self.act_move(FolderRole::Archive),
            Some(1) => self.act_move(FolderRole::Trash),
            Some(2) => self.act_unread(),
            Some(3) => self.act_star(),
            Some(4) => self.act_pin(),
            Some(5) => ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new("snooze_open"), true)),
            Some(9) => self.clear_marks(),
            _ => {}
        }
    }

    fn reader(&mut self, ctx: &egui::Context) {
        let col = self.colors.clone();
        // The conversation reads like a page: messages sit straight on the
        // background, separated by rules.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(col.bg))
            .show(ctx, |ui| {
                if self.multi() {
                    self.selection_summary(ui, &col);
                    return;
                }
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
                    .map(|fs| fs.iter().filter(|f| f.role == FolderRole::Other && in_folders.contains(&f.id) && !snooze_sync::is_internal_folder(&f.path)).cloned().collect())
                    .unwrap_or_default();
                let thread_ref = (open.account_id, open.thread_key.clone());
                let mut remove_label: Option<Folder> = None;
                let mut attachment_req: Option<(Message, usize, AttachmentClick)> = None;
                // The newest draft of ours in the conversation, to finish.
                let me = self.accounts.iter().find(|a| a.id == open.account_id).map(|a| a.email.clone()).unwrap_or_default();
                let draft = open
                    .messages
                    .iter()
                    .rev()
                    .filter(|m| m.from.email.eq_ignore_ascii_case(&me))
                    .find_map(|m| self.store.copy_in_role(open.account_id, &m.message_id, FolderRole::Drafts).ok().flatten());
                let mut edit_draft: Option<Message> = None;
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
                        if let Some(d) = &draft {
                            ui.add_space(8.0);
                            if icons::labeled(ui, Icon::Pencil, "Edit draft", col.text, col.bg).on_hover_text("Finish this draft in a new window").clicked() {
                                edit_draft = Some(d.clone());
                            }
                        }
                        if web {
                            return;
                        }
                        ui.add_space(16.0);
                        let n = open.messages.len();
                        // Newest first, like the web view.
                        for (i, m) in open.messages.iter().enumerate().rev() {
                            let expanded = open.expanded.get(i).copied().unwrap_or(true);
                            if i + 1 < n {
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
                if let Some(d) = edit_draft {
                    self.edit_draft(d);
                }
                if let Some(mode) = ctx.memory_mut(|m| m.data.remove_temp::<u8>(egui::Id::new("reply_click"))) {
                    self.compose_reply(mode);
                }
            });
    }

    /// Act on clicks and keys from the HTML message view.
    fn mail_view_events(&mut self, ctx: &egui::Context) {
        use crate::mail_view::ViewEvent;
        let events = self.mail_view.as_mut().map(|v| v.events()).unwrap_or_default();
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
        // The macOS view is a native overlay, so it hides under menus and
        // windows; Linux draws the page inside egui, which layers correctly.
        let covered = cfg!(target_os = "macos") && (self.prefs.is_some() || self.setup.is_some() || self.new_tab.is_some() || egui::Popup::is_any_open(ctx));
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
            (&key, remote, &self.theme_id, col.p.dark, self.dark_emails, open.messages.iter().map(|m| (m.id, m.starred, m.attachments.len())).collect::<Vec<_>>(), faces).hash(&mut h);
            h.finish()
        };
        let faces = &self.faces;
        let dark_emails = self.dark_emails;
        view.show(ctx, rect, ctx.zoom_factor(), page_key, || {
            let nonce = format!("{:x}", page_key ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7));
            crate::mail_view::document(&crate::mail_view::Page { messages: &open.messages, expanded: &open.expanded, remote_images: remote, nonce: &nonce, avatar: &|e| faces.data_uri(e), dark_emails }, col)
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
        if toggle.is_some() && !egui::Popup::is_id_open(ui.ctx(), popup_id) {
            self.snooze_pick = SnoozePick::default();
        }
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
                let pick_label = format!("📆  Pick a date & time  {}", if self.snooze_pick.open { "⏶" } else { "⏷" });
                if ui.add(egui::Button::new(pick_label).frame(false).min_size(egui::vec2(240.0, 26.0))).clicked() {
                    self.snooze_pick.open = !self.snooze_pick.open;
                }
                if self.snooze_pick.open {
                    if let Some(t) = snooze_calendar(ui, col, &mut self.snooze_pick) {
                        chosen = Some(t);
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
        crate::composer::route_web_events();
        self.original_windows(ctx);
        let col = self.colors.clone();
        // Composers.
        let mut i = 0;
        while i < self.composers.len() {
            match self.composers[i].ui(ctx, &self.accounts, &self.store, &col, &self.sigs) {
                ComposerAction::None => {
                    // Autosave once typing pauses.
                    let c = &mut self.composers[i];
                    if c.wants_autosave() {
                        let r = draft_ref(c);
                        let out = c.begin_save();
                        self.worker.send(Cmd::SaveDraft(c.account_id, out, r, false));
                    }
                    // Wake up to save even if nothing else happens.
                    ctx.request_repaint_after(Duration::from_secs(1));
                    i += 1;
                }
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
                    let composer = self.composers.remove(i);
                    self.queue_send(acc, *out, composer);
                }
                ComposerAction::SaveDraft(acc, out) => {
                    let r = draft_ref(&self.composers[i]);
                    self.worker.send(Cmd::SaveDraft(acc, *out, r, true));
                    self.composers.remove(i);
                }
                ComposerAction::DeleteDraft => {
                    let r = draft_ref(&self.composers[i]);
                    self.worker.send(Cmd::DeleteDraft(r));
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
            prefs.dark_emails = self.dark_emails;
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
                PrefsAction::SetDarkEmails(on) => self.dark_emails = on,
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
        // Messages waiting to send (Undo Send).
        if !self.pending_sends.is_empty() {
            let screen = ctx.content_rect();
            let x = if self.list_right > self.sidebar_right { (self.sidebar_right + self.list_right) / 2.0 } else { screen.center().x };
            let lift = 20.0 + self.toasts.len() as f32 * 48.0;
            let mut undo = None;
            let mut now = None;
            egui::Area::new(egui::Id::new("pending_sends"))
                .pivot(egui::Align2::CENTER_BOTTOM)
                .fixed_pos(egui::pos2(x, screen.bottom() - lift))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    for p in &self.pending_sends {
                        let left = p.due.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u32;
                        let subject = if p.out.subject.trim().is_empty() { "(no subject)".to_string() } else { p.out.subject.clone() };
                        egui::Frame::new().fill(egui::Color32::from_rgb(40, 44, 52)).corner_radius(8).inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add(egui::Label::new(RichText::new(format!("Sending “{subject}” in {left} s")).color(egui::Color32::WHITE)).truncate());
                                ui.add_space(10.0);
                                let blue = egui::Color32::from_rgb(130, 180, 255);
                                if ui.add(egui::Button::new(RichText::new("Undo").strong().color(blue)).frame(false)).on_hover_text("⌘Z").clicked() {
                                    undo = Some(p.id);
                                }
                                if ui.add(egui::Button::new(RichText::new("Send now").color(blue)).frame(false)).clicked() {
                                    now = Some(p.id);
                                }
                            });
                        });
                        ui.add_space(6.0);
                    }
                });
            if let Some(id) = undo {
                self.cancel_send(id);
            }
            if let Some(id) = now {
                self.send_now(id);
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
/// An Inbox tab: its name, unread count, and an underline when chosen.
fn category_tab(ui: &mut egui::Ui, col: &Colors, label: &str, unread: i64, selected: bool) -> egui::Response {
    let font = egui::FontId::proportional(14.0);
    let fg = if selected { col.heading } else { col.subtle };
    let name = ui.painter().layout_no_wrap(label.to_string(), font.clone(), fg);
    let count = (unread > 0).then(|| ui.painter().layout_no_wrap(unread.to_string(), egui::FontId::proportional(11.5), if selected { col.accent } else { col.very_subtle }));
    let w = name.size().x + count.as_ref().map_or(0.0, |c| c.size().x + 6.0);
    let (rect, r) = ui.allocate_exact_size(egui::vec2(w, 24.0), Sense::click());
    let p = ui.painter();
    let top = rect.top() + 1.0;
    if selected {
        style::bold_line(p, egui::pos2(rect.left(), top), label, font, fg, w + 4.0);
    } else {
        p.galley(egui::pos2(rect.left(), top), name.clone(), fg);
    }
    if let Some(c) = count {
        p.galley(egui::pos2(rect.left() + name.size().x + 6.0, top + 2.0), c, fg);
    }
    if selected {
        p.line_segment([egui::pos2(rect.left(), rect.bottom() - 1.0), egui::pos2(rect.right(), rect.bottom() - 1.0)], egui::Stroke::new(2.0_f32, col.accent));
    } else if r.hovered() {
        p.line_segment([egui::pos2(rect.left(), rect.bottom() - 1.0), egui::pos2(rect.right(), rect.bottom() - 1.0)], egui::Stroke::new(1.0_f32, col.border));
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

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

/// How a folder is remembered as open in the sidebar.
fn folder_key(account: AccountId, path: &str) -> String {
    format!("{account}\t{path}")
}

/// Custom folders as a tree, by path. A folder's ancestors are the folders
/// above it that exist (a server may list `A/B/C` without `A/B`).
struct FolderTree {
    /// For each folder, the indexes of its ancestors, nearest first.
    ancestors: Vec<Vec<usize>>,
}

impl FolderTree {
    fn new(folders: &[&Folder]) -> Self {
        let index: HashMap<&str, usize> = folders.iter().enumerate().map(|(i, f)| (f.path.as_str(), i)).collect();
        let ancestors = folders
            .iter()
            .map(|f| {
                let mut out = vec![];
                let mut p = owlpost_core::folder_parent(&f.path);
                while let Some(path) = p {
                    if let Some(&i) = index.get(path.as_str()) {
                        out.push(i);
                    }
                    p = owlpost_core::folder_parent(&path);
                }
                out
            })
            .collect();
        Self { ancestors }
    }

    fn has_children(&self, i: usize) -> bool {
        self.ancestors.iter().any(|a| a.first() == Some(&i))
    }

    fn descendants(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.ancestors.iter().enumerate().filter(move |(_, a)| a.contains(&i)).map(|(j, _)| j)
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
    nav_item_r(ui, col, icon, label, count, selected, false).clicked()
}

/// A sidebar entry. `drop` highlights it as where dragged mail will land.
fn nav_item_r(ui: &mut egui::Ui, col: &Colors, icon: Icon, label: &str, count: i64, selected: bool, drop: bool) -> egui::Response {
    let h = 31.0;
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::click());
    let p = ui.painter_at(rect);
    if drop {
        p.rect_filled(rect, 8.0, style::blend(col.accent, col.sidebar, 0.25));
        p.rect_stroke(rect.shrink(0.5), 8.0, egui::Stroke::new(1.5_f32, col.accent), egui::StrokeKind::Inside);
    } else if selected {
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
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
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

/// A month calendar and a time, for "Pick a date & time" in the Snooze menu.
/// Returns the moment once Snooze is pressed.
fn snooze_calendar(ui: &mut egui::Ui, col: &Colors, pick: &mut SnoozePick) -> Option<DateTime<Utc>> {
    let now = chrono::Local::now();
    let today = now.date_naive();
    let this_month = today.with_day(1).unwrap_or(today);
    let cell = egui::vec2(34.0, 28.0);
    let width = cell.x * 7.0;
    let mut out = None;
    ui.add_space(2.0);
    // ‹  November 2026  ›
    ui.allocate_ui_with_layout(egui::vec2(width, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let back = ui.add_enabled(pick.month > this_month, egui::Button::new("‹").frame(false).min_size(egui::vec2(24.0, 22.0)));
        if back.clicked() {
            pick.month = snooze::add_months(pick.month, -1);
        }
        let title = pick.month.format("%B %Y").to_string();
        ui.add_sized(egui::vec2(width - 56.0, 22.0), egui::Label::new(RichText::new(title).strong().color(col.text)));
        if ui.add(egui::Button::new("›").frame(false).min_size(egui::vec2(24.0, 22.0))).clicked() {
            pick.month = snooze::add_months(pick.month, 1);
        }
    });
    let painter_font = egui::FontId::proportional(13.0);
    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
    ui.horizontal(|ui| {
        for d in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
            ui.add_sized(egui::vec2(cell.x, 20.0), egui::Label::new(RichText::new(d).small().color(col.very_subtle)));
        }
    });
    for week in snooze::month_grid(pick.month.year(), pick.month.month()) {
        ui.horizontal(|ui| {
            for day in week {
                let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
                let Some(d) = day else { continue };
                let past = d < today;
                let selected = pick.day == Some(d);
                let r = rect.shrink2(egui::vec2(3.0, 2.0));
                if selected {
                    ui.painter().rect_filled(r, 6.0, col.button_emphasis);
                } else if resp.hovered() && !past {
                    ui.painter().rect_filled(r, 6.0, style::blend(col.text, col.bg, 0.08));
                }
                if d == today && !selected {
                    ui.painter().rect_stroke(r, 6.0, egui::Stroke::new(1.0_f32, col.accent), egui::StrokeKind::Inside);
                }
                let color = if selected {
                    col.button_emphasis_text
                } else if past {
                    style::blend(col.very_subtle, col.bg, 0.4)
                } else {
                    col.text
                };
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, d.day().to_string(), painter_font.clone(), color);
                if !past {
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        pick.day = Some(d);
                    }
                    if resp.double_clicked() {
                        pick.day = Some(d);
                        out = snooze::at_local(d, pick.hour, pick.minute);
                    }
                }
            }
        });
    }
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
    ui.add_space(6.0);
    // Time: hour and minute, plus the usual times of day.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Time").color(col.subtle));
        ui.add(egui::DragValue::new(&mut pick.hour).range(0..=23).custom_formatter(|n, _| format!("{n:02}")));
        ui.label(":");
        ui.add(egui::DragValue::new(&mut pick.minute).range(0..=59).speed(0.3).custom_formatter(|n, _| format!("{n:02}")));
        for (label, h) in [("9:00", 9), ("13:00", 13), ("18:00", 18)] {
            let on = pick.hour == h && pick.minute == 0;
            if ui.add(egui::Button::new(RichText::new(label).small()).selected(on)).clicked() {
                pick.hour = h;
                pick.minute = 0;
            }
        }
    });
    ui.add_space(4.0);
    let when = pick.day.and_then(|d| snooze::at_local(d, pick.hour, pick.minute));
    ui.horizontal(|ui| {
        match when {
            Some(t) if t <= Utc::now() => {
                ui.label(RichText::new("That time has passed").small().color(col.error));
            }
            Some(t) => {
                ui.label(RichText::new(snooze::describe(t)).small().color(col.success));
            }
            None => {
                ui.label(RichText::new("Choose a day").small().color(col.subtle));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ok = when.is_some_and(|t| t > Utc::now());
            if ui.add_enabled_ui(ok, |ui| style::primary_button(ui, col, "Snooze")).inner.clicked() {
                out = when;
            }
        });
    });
    out
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

impl OwlPostApp {
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
        self.handle_keys(ctx);
        self.toolbar(ctx);
        self.sidebar(ctx);
        self.thread_list(ctx);
        self.reader(ctx);
        self.windows(ctx);
        self.new_tab_dialog(ctx);
        self.drag_overlay(ctx);
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

    #[test]
    fn inbox_tabs_and_sender_rules() {
        use owlpost_core::category::{PRIMARY, PROMOTIONS, SOCIAL};
        let mut h = harness("owlpost-light");
        h.run();
        assert!(h.state().tabs_shown());
        let subjects = |h: &Harness<'static, OwlPostApp>| h.state().rows.iter().map(|r| r.t.subject.clone()).collect::<Vec<_>>();
        assert!(!subjects(&h).iter().any(|s| s.contains("November roast")), "the newsletter isn't in Primary");
        h.state_mut().category = PROMOTIONS;
        h.state_mut().reload();
        h.run();
        assert!(subjects(&h).iter().any(|s| s.contains("November roast")));
        save(&mut h, "tabs-promotions");
        // Always put the coffee shop in Social; ⌘Z puts it back.
        h.state_mut().sort_sender("hello@kowhai.demo", Some(SOCIAL));
        assert!(h.state().rows.is_empty());
        h.state_mut().category = SOCIAL;
        h.state_mut().reload();
        assert!(subjects(&h).iter().any(|s| s.contains("November roast")));
        h.state_mut().undo_last();
        h.state_mut().reload();
        assert!(!subjects(&h).iter().any(|s| s.contains("November roast")));
        // All mail from one sender, across folders and tabs.
        h.state_mut().category = PRIMARY;
        h.state_mut().show_sender("hello@kowhai.demo");
        assert!(!h.state().tabs_shown());
        assert_eq!(subjects(&h).len(), 1);
    }

    #[test]
    fn own_tabs() {
        use owlpost_core::category::{PRIMARY, PROMOTIONS};
        let mut h = harness("owlpost-light");
        h.run();
        let subjects = |h: &Harness<'static, OwlPostApp>| h.state().rows.iter().map(|r| r.t.subject.clone()).collect::<Vec<_>>();
        // Right-click → Always put their mail in → New tab… → "Invoices".
        h.state_mut().new_tab = Some(NewTab { name: String::new(), then: NewTabFor::Sender("hello@kowhai.demo".into()), error: None });
        h.run();
        save(&mut h, "new-tab-dialog");
        h.state_mut().create_tab("Invoices", &NewTabFor::Sender("hello@kowhai.demo".into())).unwrap();
        h.state_mut().new_tab = None;
        let inv = h.state().store.category_by_name("invoices").unwrap();
        assert!(owlpost_core::category::is_tab(inv));
        assert!(h.state().tabs.iter().any(|(c, n, _)| *c == inv && n == "Invoices"));
        assert!(h.state_mut().create_tab("promotions", &NewTabFor::Sender("x@y.z".into())).is_err(), "built-in names are taken");
        h.state_mut().category = inv;
        h.state_mut().reload();
        h.run();
        assert_eq!(subjects(&h).len(), 1);
        assert!(subjects(&h)[0].contains("November roast"));
        // One conversation by hand.
        h.state_mut().category = PRIMARY;
        h.state_mut().reload();
        let (a, k, subject) = h.state().rows.iter().map(|r| (r.t.account_id, r.t.thread_key.clone(), r.t.subject.clone())).next().unwrap();
        h.state_mut().sort_thread(a, &k, Some(inv));
        assert!(!subjects(&h).contains(&subject));
        h.state_mut().category = inv;
        h.state_mut().reload();
        h.run();
        assert!(subjects(&h).contains(&subject));
        save(&mut h, "tabs-invoices");
        h.state_mut().undo_last();
        h.state_mut().reload();
        assert!(!subjects(&h).contains(&subject));
        // Removing the tab sends its mail back where it came from; undo brings it back.
        h.state_mut().remove_tab(inv);
        assert_eq!(h.state().category, PRIMARY);
        assert!(!h.state().tabs.iter().any(|(c, ..)| *c == inv));
        h.state_mut().category = PROMOTIONS;
        h.state_mut().reload();
        assert!(subjects(&h).iter().any(|s| s.contains("November roast")));
        h.state_mut().undo_last();
        assert!(h.state().tabs.iter().any(|(c, ..)| *c == inv));
        assert!(!subjects(&h).iter().any(|s| s.contains("November roast")));
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
        // With Preferences open over a composer, ⌘W closes Preferences only.
        h.state_mut().compose_new();
        h.run_steps(2);
        h.state_mut().prefs = Some(Prefs::new(Tab::Appearance));
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
        h.run_steps(2);
        assert!(h.state().prefs.is_none());
        assert_eq!(h.state().composers.len(), 1);
        // One with text closes too, and is kept as a draft.
        h.state_mut().composers[0].subject = "Half-written thought".into();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
        h.run_steps(2);
        assert!(h.state().composers.is_empty());
        let mut saved = false;
        for _ in 0..100 {
            h.run_steps(1);
            if !h.state().store.threads(&MailboxView::Unified(FolderRole::Drafts), "subject:Half-written", 5).unwrap().is_empty() {
                saved = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(saved, "⌘W never drops a message: it's saved to Drafts");
    }

    #[test]
    fn drafts_reopen_and_save() {
        use egui_kittest::kittest::Queryable;
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        let gmail = h.state().accounts.iter().find(|a| a.is_gmail()).cloned().unwrap();
        let sig = h.state().store.save_signature(&Signature { id: 0, title: "Work".into(), body: "Alex Rivera\nNorthwind".into(), html: false }).unwrap();
        h.state().store.set_default_signature(gmail.id, Some(sig)).unwrap();
        h.state_mut().reload();
        // A reply draft, as the MCP server's save_draft makes it.
        let t = h.state().store.threads(&MailboxView::Everything, "subject:Dinner", 1).unwrap().remove(0);
        let acc = h.state().accounts.iter().find(|a| a.id == t.account_id).cloned().unwrap();
        let last = h.state().store.thread_messages(acc.id, &t.thread_key).unwrap().pop().unwrap();
        let mut out = owlpost_core::mcp::reply_draft(&acc, &last, false);
        out.body = format!("Sounds lovely!\n\nAlex Rivera\nNorthwind{}", owlpost_core::smtp::quote(&last));
        out.bcc = vec![Contact::new("", "sam@example.com")];
        owlpost_core::engine::save_draft(&h.state().store, &acc, &out, None).unwrap();
        h.state_mut().set_selection(Selection::Unified(FolderRole::Drafts));
        let i = h.state().rows.iter().position(|r| r.t.thread_key == t.thread_key).expect("draft listed in Drafts");
        h.state_mut().select(i);
        h.run_steps(3);
        assert!(h.query_by_label("Edit draft").is_some(), "a conversation with a draft offers Edit draft");
        save(&mut h, "thread-with-draft");

        // Reopened: the signature and quote are split off again.
        let m = h.state().store.thread_messages(acc.id, &t.thread_key).unwrap().into_iter().find(|m| m.body_text.starts_with("Sounds lovely")).unwrap();
        let parts = owlpost_core::engine::DraftParts { bcc: vec![Contact::new("", "sam@example.com")], attachments: vec![] };
        let c = Composer::from_draft(7, &acc, &m, parts, &h.state().sigs);
        assert_eq!(c.body, "Sounds lovely!");
        assert!(c.tail.contains("wrote:"), "{:?}", c.tail);
        assert_eq!(c.bcc, "sam@example.com");
        let again = c.draft();
        assert_eq!(again.body.trim_end(), m.body_text.replace("\r\n", "\n").trim_end(), "saving again keeps the text as it was");
        h.state_mut().composers.push(c);
        h.run_steps(3);
        // Unchanged: ⌘W closes without asking.
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
        h.run_steps(2);
        assert!(h.state().composers.is_empty(), "an unchanged draft closes straight away");
        // Changed: ⌘W saves the change, replacing the old copy.
        let mut c = Composer::from_draft(8, &acc, &m, Default::default(), &h.state().sigs);
        c.body.push_str(" See you at 7.");
        h.state_mut().composers.push(c);
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
        h.run_steps(2);
        assert!(h.state().composers.is_empty());
        let drafts = |h: &Harness<'static, OwlPostApp>| {
            h.state().store.thread_messages(acc.id, &t.thread_key).unwrap().into_iter().filter(|m| m.body_text.starts_with("Sounds lovely")).map(|m| m.body_text).collect::<Vec<_>>()
        };
        let mut ok = false;
        for _ in 0..100 {
            h.run_steps(1);
            let d = drafts(&h);
            if d.len() == 1 && d[0].starts_with("Sounds lovely! See you at 7.") {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ok, "one draft, with the change: {:?}", drafts(&h));
    }

    #[test]
    fn drafts_save_themselves() {
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        let acc = h.state().accounts[0].clone();
        let mut c = Composer::new(70, &acc);
        c.to = "ann@example.com".into();
        h.state_mut().composers.push(c);
        h.run_steps(2);
        // Typing, then a pause: saved without asking.
        h.state_mut().composers[0].subject = "Autosave check".into();
        h.run_steps(1);
        let found = |h: &Harness<'static, OwlPostApp>| h.state().store.threads(&MailboxView::Unified(FolderRole::Drafts), "subject:Autosave", 5).unwrap().len();
        assert_eq!(found(&h), 0, "not before the pause");
        let mut n = 0;
        for _ in 0..80 {
            h.run_steps(1);
            n = found(&h);
            if n > 0 && h.state().composers[0].draft.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        assert_eq!(n, 1, "saved after the pause");
        assert!(h.state().composers[0].status.as_deref().is_some_and(|s| s.starts_with("Saved")));
        save(&mut h, "composer-autosaved");
        // More typing replaces it rather than adding another copy.
        h.state_mut().composers[0].body = "Second version".into();
        for _ in 0..80 {
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(60));
            let d = h.state().composers[0].draft.as_ref().map(|d| d.body_text.clone()).unwrap_or_default();
            if d.starts_with("Second version") {
                break;
            }
        }
        assert_eq!(found(&h), 1, "still one draft");
        // Sending removes it.
        h.state_mut().list_settings.undo_send_secs = 0;
        let c = h.state_mut().composers.remove(0);
        let out = c.draft();
        h.state_mut().queue_send(acc.id, out, c);
        let mut gone = false;
        for _ in 0..100 {
            h.run_steps(1);
            if found(&h) == 0 {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gone, "sent, so no draft left");
    }

    #[test]
    fn drag_conversations_onto_folders() {
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        let before = h.state().rows.len();
        // Two selected, dropped on the All Accounts Archive.
        h.state_mut().toggle_mark(0);
        h.state_mut().toggle_mark(1);
        let keys = h.state().marked.clone();
        h.state_mut().dragging = Some(keys.clone());
        h.state_mut().drop_on(keys, DropTarget::Role(FolderRole::Archive));
        h.run_steps(2);
        assert_eq!(h.state().rows.len(), before - 2);
        assert!(h.state().toasts.iter().any(|t| t.text == "Archived 2 conversations"));
        // One conversation onto a folder of another account stays put.
        let row = h.state().rows[0].t.clone();
        let other = h.state().folders.iter().find(|(a, _)| **a != row.account_id).and_then(|(_, fs)| fs.iter().find(|f| f.role == FolderRole::Other).cloned()).unwrap();
        h.state_mut().drop_on(vec![(row.account_id, row.thread_key.clone())], DropTarget::Folder(other));
        assert!(h.state().rows.iter().any(|r| r.t.thread_key == row.thread_key));
        assert!(h.state().toasts.iter().any(|t| t.error && t.text.contains("another account")));
        // A real drag: press on the first row, move down (picks it up), then
        // left onto All Accounts → Archive, and let go.
        let rows_before = h.state().rows.len();
        let first = h.state().rows[0].t.thread_key.clone();
        let x = (h.state().sidebar_right + h.state().list_right) / 2.0;
        let start = egui::pos2(x, 280.0);
        let pointer = |h: &mut Harness<'static, OwlPostApp>, p: egui::Pos2| {
            h.event(egui::Event::PointerMoved(p));
            h.run_steps(1);
        };
        let button = |h: &mut Harness<'static, OwlPostApp>, p: egui::Pos2, pressed: bool| {
            h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            h.run_steps(1);
        };
        pointer(&mut h, start);
        button(&mut h, start, true);
        for dy in [4.0, 10.0, 20.0, 30.0] {
            pointer(&mut h, start + egui::vec2(0.0, dy));
        }
        assert!(h.state().dragging.is_some(), "picked up");
        let archive = egui::pos2(80.0, 301.0);
        for t in [0.3, 0.6, 1.0] {
            pointer(&mut h, start + (archive - start) * t);
        }
        assert!(matches!(h.state().drop_target, Some(DropTarget::Role(FolderRole::Archive))), "over Archive");
        save(&mut h, "dragging");
        button(&mut h, archive, false);
        h.run_steps(2);
        assert!(h.state().dragging.is_none());
        assert_eq!(h.state().rows.len(), rows_before - 1, "dropped on Archive");
        assert!(!h.state().rows.iter().any(|r| r.t.thread_key == first));
    }

    #[test]
    fn show_original_window() {
        use egui_kittest::kittest::Queryable;
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        h.state_mut().originals.push(Original { id: 900, subject: "Rates".into(), text: "From: rates@example.com\r\nSubject: Rates\r\n\r\nKia ora".into() });
        h.run_steps(3);
        assert!(h.query_by_label("Save as .eml…").is_some());
        save(&mut h, "show-original");
    }

    #[test]
    fn multi_select_and_batch_actions() {
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        let before = h.state().rows.len();
        // ⌘-click two conversations (the first one opens, the second joins it).
        h.state_mut().toggle_mark(0);
        assert!(!h.state().multi() && h.state().open.is_some(), "one marked: it just opens");
        h.state_mut().toggle_mark(1);
        assert_eq!(h.state().marked.len(), 2);
        assert!(h.state().open.is_none());
        // ⇧-click extends from the anchor.
        h.state_mut().mark_range(3);
        assert_eq!(h.state().marked.len(), 3, "rows 1 to 3");
        h.run_steps(3);
        save(&mut h, "multi-select");
        let keys: Vec<String> = h.state().marked.iter().map(|m| m.1.clone()).collect();
        h.state_mut().act_move(FolderRole::Archive);
        h.run_steps(2);
        assert_eq!(h.state().rows.len(), before - 3, "all three archived");
        assert!(h.state().toasts.iter().any(|t| t.undo && t.text == "Archived 3 conversations"), "one toast for the batch");
        assert!(h.state().marked.is_empty());
        // Let the moves finish, then one ⌘Z brings back all three.
        let back = |h: &mut Harness<'static, OwlPostApp>, n: usize| {
            for _ in 0..150 {
                h.run_steps(1);
                h.state_mut().reload();
                if h.state().rows.len() == n {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(back(&mut h, before - 3));
        h.state_mut().undo_last();
        assert!(back(&mut h, before), "⌘Z restores the whole batch");
        assert!(keys.iter().all(|k| h.state().rows.iter().any(|r| &r.t.thread_key == k)));
        // ⌘A, then Esc.
        h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run_steps(1);
        assert_eq!(h.state().marked.len(), before);
        h.key_press(Key::Escape);
        h.run_steps(1);
        assert!(h.state().marked.is_empty());
    }

    #[test]
    fn undo_send_holds_the_message() {
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        let acc = h.state().accounts[0].clone();
        let sent = |h: &Harness<'static, OwlPostApp>| h.state().store.threads(&MailboxView::Unified(FolderRole::Sent), "subject:Lunch", 50).unwrap().len();
        let compose = |id| {
            let mut c = Composer::new(id, &acc);
            c.to = "ann@example.com".into();
            c.subject = "Lunch?".into();
            c.body = "Free on Friday?".into();
            c
        };
        h.state_mut().list_settings.undo_send_secs = 10;
        let c = compose(50);
        let out = c.draft();
        h.state_mut().queue_send(acc.id, out.clone(), c);
        h.run_steps(2);
        assert_eq!(h.state().pending_sends.len(), 1, "held, not sent");
        save(&mut h, "undo-send");
        // ⌘Z brings the composer back.
        h.state_mut().undo_last();
        assert!(h.state().pending_sends.is_empty());
        assert_eq!(h.state().composers.len(), 1, "the message is back in its window");
        assert_eq!(h.state().composers[0].body, "Free on Friday?");
        // Send now goes straight away.
        let c = h.state_mut().composers.remove(0);
        h.state_mut().queue_send(acc.id, out, c);
        let id = h.state().pending_sends[0].id;
        h.state_mut().send_now(id);
        assert!(h.state().pending_sends.is_empty());
        let mut ok = false;
        for _ in 0..100 {
            h.run_steps(1);
            if sent(&h) > 0 {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ok, "sent");
    }

    #[test]
    fn sidebar_folders_fold() {
        use egui_kittest::kittest::Queryable;
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        let gmail = h.state().accounts.iter().find(|a| a.is_gmail()).unwrap().id;
        for path in ["Projects", "Projects/Website", "Projects/Website/2026", "timson.co"] {
            h.state().store.upsert_folder(gmail, path, FolderRole::Other).unwrap();
        }
        // A tall window and the other account folded, so the folders are on screen.
        h.set_size(egui::vec2(1280.0, 1100.0));
        let others_acc: Vec<AccountId> = h.state().accounts.iter().filter(|a| a.id != gmail).map(|a| a.id).collect();
        h.state_mut().collapsed_accounts = others_acc;
        h.state_mut().reload();
        h.run_steps(2);
        // Parents start folded; dotted names stay whole.
        let f = h.state().folders[&gmail].clone();
        assert_eq!(f.iter().find(|x| x.path == "timson.co").unwrap().name, "timson.co");
        let others: Vec<&Folder> = f.iter().filter(|x| x.role == FolderRole::Other).collect();
        let tree = FolderTree::new(&others);
        let at = |p: &str| others.iter().position(|x| x.path == p).unwrap();
        assert!(tree.has_children(at("Projects")) && !tree.has_children(at("timson.co")));
        assert_eq!(tree.ancestors[at("Projects/Website/2026")], vec![at("Projects/Website"), at("Projects")]);
        assert_eq!(tree.descendants(at("Projects")).count(), 3, "Website, 2026 and OwlPost");
        assert!(h.query_by_label("Expand Projects").is_some());
        assert!(h.query_by_label("Expand Website").is_none(), "hidden inside Projects");
        save(&mut h, "sidebar-folded");
        h.get_by_label("Expand Projects").click();
        h.run_steps(2);
        assert!(h.query_by_label("Collapse Projects").is_some());
        assert!(h.query_by_label("Expand Website").is_some(), "now showing");
        save(&mut h, "sidebar-unfolded");
    }

    #[test]
    fn snooze_calendar_picks_a_time() {
        use egui_kittest::kittest::Queryable;
        let mut h = harness("owlpost-light");
        h.run_steps(2);
        h.state_mut().select(0);
        h.run_steps(2);
        h.ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("snooze_open"), true));
        h.run_steps(2);
        h.state_mut().snooze_pick.open = true;
        let day = chrono::Local::now().date_naive() + chrono::Duration::days(3);
        h.state_mut().snooze_pick.day = Some(day);
        h.state_mut().snooze_pick.month = day.with_day(1).unwrap();
        h.state_mut().snooze_pick.hour = 13;
        h.run_steps(3);
        assert!(h.query_by_label(&day.format("%B %Y").to_string()).is_some(), "the month is shown");
        save(&mut h, "snooze-calendar");
        let before = h.state().rows.len();
        h.get_by_label("Snooze").click();
        h.run_steps(3);
        assert_eq!(h.state().rows.len(), before - 1, "snoozed from the calendar");
        let snoozed = h.state().store.snoozed_threads().unwrap();
        let until = snoozed[0].1.with_timezone(&chrono::Local);
        assert_eq!(until.format("%Y-%m-%d %H:%M").to_string(), format!("{} 13:00", day.format("%Y-%m-%d")));
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
        // The snooze may still be finishing in the background; the undo follows it.
        assert!(wait_for(&mut h, &key, true), "unsnoozed");
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
