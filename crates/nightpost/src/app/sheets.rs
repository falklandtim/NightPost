//! Sheets and windows over the main view: keyboard shortcuts, Sync activity,
//! the folder and new-tab dialogs, Show original, the drag overlay, and the
//! composer, preferences and setup windows.

use super::*;

impl NightPostApp {
    /// The keyboard shortcuts sheet (? or More ▸ Keyboard shortcuts).
    pub(super) fn keys_sheet(&mut self, ctx: &egui::Context) {
        if !self.show_keys {
            return;
        }
        let col = self.colors.clone();
        let cmd = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
        let groups: [(&str, Vec<(String, &str)>); 4] = [
            ("Write", vec![("c".into(), "New message"), ("r".into(), "Reply"), ("a".into(), "Reply all"), ("f".into(), "Forward")]),
            (
                "Act on a conversation",
                vec![
                    ("e".into(), "Done (archive)"),
                    ("b".into(), "Snooze"),
                    ("v".into(), "Move to folder"),
                    ("l".into(), "Label (Gmail)"),
                    ("p".into(), "Pin to top"),
                    ("s".into(), "Star"),
                    ("u".into(), "Mark read / unread"),
                    ("# or Delete".into(), "Move to Trash"),
                    ("!".into(), "Report spam"),
                    (format!("{cmd}Z"), "Undo"),
                ],
            ),
            (
                "Get around",
                vec![
                    ("j  k".into(), "Next / previous"),
                    ("↓  ↑".into(), "Next / previous"),
                    ("Shift ↓  ↑".into(), "Select several"),
                    (format!("{cmd}A"), "Select all"),
                    (format!("{cmd}F  or  /"), "Search"),
                    ("Esc".into(), "Clear the search"),
                ],
            ),
            (
                "Go to",
                vec![
                    ("g  i".into(), "Inbox"),
                    ("g  s".into(), "Starred"),
                    ("g  t".into(), "Sent"),
                    ("g  d".into(), "Drafts"),
                    ("g  z".into(), "Snoozed"),
                    (format!("{cmd},"), "Preferences"),
                    (format!("{cmd}Shift+L"), "Sync activity"),
                    ("?".into(), "This list"),
                ],
            ),
        ];
        let r = egui::Modal::new(egui::Id::new("keys-sheet")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Keyboard shortcuts").size(17.0).strong().color(col.heading));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Done").clicked() {
                        self.show_keys = false;
                    }
                });
            });
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                for (i, (title, keys)) in groups.iter().enumerate() {
                    let ui = &mut cols[if i < 2 { 0 } else { 1 }];
                    ui.label(RichText::new(*title).strong().color(col.text));
                    ui.add_space(2.0);
                    egui::Grid::new(("keys", i)).num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                        for (k, what) in keys {
                            ui.label(RichText::new(k).monospace().color(col.accent));
                            ui.label(RichText::new(*what).color(col.subtle));
                            ui.end_row();
                        }
                    });
                    ui.add_space(10.0);
                }
                cols[1].label(RichText::new("In any menu, use the arrow keys to choose and Return to pick. In the Move and Label lists, type to find a folder first.").small().color(col.subtle));
            });
        });
        if r.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.show_keys = false;
        }
    }

    /// Re-read each account's sync health every few seconds (and straight
    /// after a sync or push change).
    pub(super) fn refresh_health(&mut self) {
        if self.health_read.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return;
        }
        self.health_read = Some(Instant::now());
        self.health = self.store.sync_statuses().unwrap_or_default().into_iter().map(|s| (s.account, s)).collect();
    }

    pub(super) fn toggle_activity(&mut self, account: Option<AccountId>) {
        self.activity = match self.activity {
            Some(_) if account.is_none() => None,
            _ => Some(ActivityView::new(account)),
        };
    }

    /// The colour of an account's status dot.
    pub(super) fn health_color(&self, h: activity::Health) -> egui::Color32 {
        match h {
            activity::Health::Ok => self.colors.success,
            activity::Health::Trouble => self.colors.warning,
            activity::Health::Offline => self.colors.error,
            activity::Health::Unknown => self.colors.sidebar_text.gamma_multiply(0.35),
        }
    }

    pub(super) fn health_of(&self, a: AccountId) -> activity::Status {
        self.health.get(&a).cloned().unwrap_or(activity::Status { account: a, ..Default::default() })
    }

    /// One line for the sidebar footer: the account in the worst state, or
    /// when everything last synced.
    pub(super) fn health_line(&self) -> Option<(String, activity::Health)> {
        let real: Vec<&Account> = self.accounts.iter().filter(|a| !a.demo).collect();
        let short = |a: &Account| a.email.clone();
        for want in [activity::Health::Offline, activity::Health::Trouble] {
            let bad: Vec<&&Account> = real.iter().filter(|a| self.health_of(a.id).health() == want).collect();
            if let Some(a) = bad.first() {
                let more = if bad.len() > 1 { format!(" (+{})", bad.len() - 1) } else { String::new() };
                let what = if want == activity::Health::Offline { "Can't connect" } else { "Reconnecting" };
                return Some((format!("{what}: {}{more}", short(a)), want));
            }
        }
        let oldest = real.iter().map(|a| self.health_of(a.id).last_ok).collect::<Option<Vec<_>>>()?.into_iter().min()?;
        Some((format!("Synced {}", activity::ago(oldest, Utc::now())), activity::Health::Ok))
    }

    /// Sync activity: each account's connection, then what happened, newest first.
    pub(super) fn activity_sheet(&mut self, ctx: &egui::Context) {
        let Some(view) = self.activity.as_mut() else { return };
        if view.read.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            view.entries = self.store.activity(400, view.account, view.problems).unwrap_or_default();
            view.read = Some(Instant::now());
        }
        ctx.request_repaint_after(Duration::from_secs(2));
        let col = self.colors.clone();
        let accounts: Vec<Account> = self.accounts.iter().filter(|a| !a.demo).cloned().collect();
        let email = |id: Option<AccountId>| id.and_then(|id| self.accounts.iter().find(|a| a.id == id)).map(|a| a.email.clone());
        let now = Utc::now();
        let today = chrono::Local::now().date_naive();
        let when = |t: DateTime<Utc>| {
            let l = t.with_timezone(&chrono::Local);
            if l.date_naive() == today { l.format("%H:%M:%S").to_string() } else { l.format("%-d %b %H:%M").to_string() }
        };
        let mut close = false;
        let mut sync: Option<AccountId> = None;
        let mut sign_in = false;
        let mut copy = false;
        let mut show_log = false;
        let mut pick: Option<Option<AccountId>> = None;
        let mut problems = view.problems;
        let filter = view.account;
        let entries = view.entries.clone();
        let r = egui::Modal::new(egui::Id::new("activity-sheet")).show(ctx, |ui| {
            ui.set_width(680.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Sync activity").size(17.0).strong().color(col.heading));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Done").clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            if accounts.is_empty() {
                ui.label(RichText::new("Only demo accounts: nothing syncs.").color(col.subtle));
            }
            for a in &accounts {
                let st = self.health_of(a.id);
                let h = st.health();
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(14.0, 18.0), Sense::hover());
                    paint_health(ui.painter(), dot.center(), h, self.health_color(h), false);
                    ui.label(RichText::new(&a.email).strong().color(col.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Sync now").clicked() {
                            sync = Some(a.id);
                        }
                        if st.needs_sign_in() && ui.button("Sign in again…").on_hover_text("Preferences → Accounts").clicked() {
                            sign_in = true;
                        }
                        let tone = match h {
                            activity::Health::Offline => col.error,
                            activity::Health::Trouble => col.warning,
                            _ => col.subtle,
                        };
                        let mut tip = st.summary(now);
                        if let Some(e) = st.error.as_deref().filter(|_| st.failures > 0) {
                            tip.push_str(&format!("\n\n{e}"));
                        }
                        if let (Some(activity::Push::Retrying), Some(d)) = (st.push, st.push_detail.as_deref()) {
                            tip.push_str(&format!("\n\nPush: {d}"));
                        }
                        ui.add(egui::Label::new(RichText::new(st.summary(now)).color(tone)).truncate()).on_hover_text(tip);
                    });
                });
            }
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                let label = filter.and_then(|f| email(Some(f))).unwrap_or_else(|| "All accounts".into());
                egui::ComboBox::from_id_salt("activity-account").selected_text(label).width(200.0).show_ui(ui, |ui| {
                    if ui.selectable_label(filter.is_none(), "All accounts").clicked() {
                        pick = Some(None);
                    }
                    for a in &accounts {
                        if ui.selectable_label(filter == Some(a.id), &a.email).clicked() {
                            pick = Some(Some(a.id));
                        }
                    }
                });
                ui.checkbox(&mut problems, "Problems only");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Show log file").on_hover_text(nightpost_core::log_path().display().to_string()).clicked() {
                        show_log = true;
                    }
                    if ui.button("Copy").on_hover_text("Copy these lines, to paste into a message").clicked() {
                        copy = true;
                    }
                });
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, false]).show(ui, |ui| {
                if entries.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new(if problems { "No problems recorded." } else { "Nothing yet: activity appears as NightPost syncs." }).color(col.subtle));
                }
                for e in &entries {
                    ui.horizontal_top(|ui| {
                        let cell = |ui: &mut egui::Ui, w: f32, l: egui::Label| {
                            ui.allocate_ui_with_layout(egui::vec2(w, 16.0), egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                ui.set_width(w);
                                ui.add(l)
                            });
                        };
                        cell(ui, 70.0, egui::Label::new(RichText::new(when(e.at)).monospace().small().color(col.very_subtle)));
                        if filter.is_none() {
                            let who = email(e.account).unwrap_or_default();
                            cell(ui, 150.0, egui::Label::new(RichText::new(who).small().color(col.subtle)).truncate());
                        }
                        let tone = match e.level {
                            activity::Level::Error => col.error,
                            activity::Level::Warn => col.warning,
                            activity::Level::Info => col.text,
                        };
                        ui.add(egui::Label::new(RichText::new(&e.text).color(tone)).wrap());
                    });
                }
            });
        });
        if r.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        let view = self.activity.as_mut().expect("open");
        if problems != view.problems {
            view.problems = problems;
            view.read = None;
        }
        if let Some(a) = pick {
            view.account = a;
            view.read = None;
        }
        if copy {
            let text: String = entries
                .iter()
                .rev()
                .map(|e| format!("{}  {}  {}\n", e.at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S"), email(e.account).unwrap_or_else(|| "-".into()), e.text))
                .collect();
            ctx.copy_text(text);
            self.toast(format!("Copied {} lines", entries.len()), false);
        }
        if show_log {
            let path = nightpost_core::log_path();
            if let Err(e) = crate::worker::open_file(&path, true) {
                self.toast(format!("Couldn't show the log file ({}): {e:#}", path.display()), true);
            }
        }
        if let Some(a) = sync {
            self.syncing = true;
            self.worker.send(Cmd::SyncAccount(a));
            self.toast("Syncing…", false);
        }
        if sign_in {
            self.prefs = Some(Prefs::new(Tab::Accounts));
            close = true;
        }
        if close {
            self.activity = None;
        }
    }

    /// The sidebar's New folder and Delete folder dialogs.
    pub(super) fn folder_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.folder_dialog.as_mut() else { return };
        let col = self.colors.clone();
        let mut done: Option<bool> = None;
        egui::Modal::new(egui::Id::new("folder-dialog")).show(ctx, |ui| {
            ui.set_width(340.0);
            match dialog {
                FolderDialog::New { parent, name, error, .. } => {
                    ui.label(RichText::new("New folder").size(17.0).strong().color(col.heading));
                    if let Some(p) = parent {
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("Inside “{}”", p.name)).color(col.subtle));
                    }
                    ui.add_space(8.0);
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("Receipts").desired_width(f32::INFINITY));
                    if !r.has_focus() && error.is_none() && name.is_empty() {
                        r.request_focus();
                    }
                    if r.changed() {
                        *error = None;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        done = Some(true);
                    }
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(col.error));
                    }
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Create")).clicked() {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                }
                FolderDialog::Delete { folder, gmail } => {
                    ui.label(RichText::new(format!("Delete “{}”?", folder.name)).size(17.0).strong().color(col.heading));
                    ui.add_space(6.0);
                    let what = if *gmail {
                        "The label is removed. Its mail isn't deleted: it stays in All Mail."
                    } else {
                        "The folder is deleted from the server. Any mail in it is moved to Trash first."
                    };
                    ui.label(RichText::new(what).color(col.subtle));
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new("Delete folder").color(col.error)).clicked() {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
        match done {
            Some(true) => match self.folder_dialog.take() {
                Some(FolderDialog::New { account, parent, name, .. }) => {
                    let name = name.trim().to_string();
                    let parent_path = parent.as_ref().map(|p| p.path.clone());
                    let taken = self.folders.get(&account).into_iter().flatten().any(|f| {
                        f.name.eq_ignore_ascii_case(&name) && nightpost_core::folder_parent(&f.path) == parent_path && f.role == FolderRole::Other
                    });
                    if taken || (parent.is_none() && name.eq_ignore_ascii_case("inbox")) {
                        let error = Some(format!("There's already a folder called “{name}” there."));
                        self.folder_dialog = Some(FolderDialog::New { account, parent, name, error });
                        return;
                    }
                    if let Some(p) = &parent {
                        // Open the parent so the new folder shows.
                        self.open_folders.insert(folder_key(account, &p.path));
                    }
                    self.worker.send(Cmd::NewFolder(account, parent.map(|p| p.id), name));
                }
                Some(FolderDialog::Delete { folder, .. }) => {
                    if self.selection == Selection::Folder(folder.id) {
                        self.set_selection(Selection::Unified(FolderRole::Inbox));
                    }
                    self.toast(format!("Deleting “{}”…", folder.name), false);
                    self.worker.send(Cmd::DeleteFolder(folder.id));
                }
                None => {}
            },
            Some(false) => self.folder_dialog = None,
            None => {}
        }
    }

    /// The "New tab" dialog.
    pub(super) fn new_tab_dialog(&mut self, ctx: &egui::Context) {
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

    /// "Open Anyway?" for an attachment that could run something: a program,
    /// script, installer, archive or document with macros. Cancel is the
    /// default (Escape or Enter).
    pub(super) fn risky_open_dialog(&mut self, ctx: &egui::Context) {
        let Some((m, index)) = &self.risky_open else { return };
        let name = m.attachments.get(*index).map(|a| a.filename.clone()).unwrap_or_default();
        let kind = nightpost_core::file_safety::risky_kind(&name).unwrap_or("file");
        let article = if kind.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
        let col = self.colors.clone();
        let mut open: Option<bool> = None;
        egui::Modal::new(egui::Id::new("risky-open")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(format!("Open \u{201c}{name}\u{201d}?")).size(17.0).strong().color(col.heading));
            ui.add_space(4.0);
            ui.label(RichText::new(format!("It's {article} {kind}. Files from email can be harmful. Open it only if you were expecting it.")).color(col.subtle));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        open = Some(false);
                    }
                    if ui.button("Open Anyway").clicked() {
                        open = Some(true);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) {
                open = Some(false);
            }
        });
        if let Some(yes) = open {
            if let Some((m, index)) = self.risky_open.take() {
                if yes {
                    self.fetch_attachment(m, index, crate::worker::AttachmentDo::Open);
                }
            }
        }
    }

    /// The drag in progress: a label under the pointer, and the drop on release.
    pub(super) fn drag_overlay(&mut self, ctx: &egui::Context) {
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

    /// The "Show original" windows: the raw message, selectable, to copy or save.
    pub(super) fn original_windows(&mut self, ctx: &egui::Context) {
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
                            let name = format!("{}.eml", nightpost_core::engine::safe_filename(&o.subject));
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

    pub(super) fn windows(&mut self, ctx: &egui::Context) {
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
        let blitz = self.blitz_engine();
        if let Some(prefs) = &mut self.prefs {
            prefs.push = self.push.clone();
            prefs.google_ready = self.google_ready;
            prefs.html_email = self.html_email;
            prefs.load_images = self.load_images;
            prefs.dark_emails = self.dark_emails;
            prefs.faces = self.faces.enabled.get();
            prefs.blitz = blitz;
            prefs.google_waiting = self.google.as_ref().map(|g| g.1.clone());
            let act = prefs.ui(ctx, &col, &self.themes, &self.theme_id, &self.accounts, &mut self.list_settings, &self.sigs);
            match act {
                PrefsAction::None => {}
                PrefsAction::Close => self.prefs = None,
                PrefsAction::SetTheme(id) => self.set_theme(ctx, &id),
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
                    let r = nightpost_core::oauth::GoogleClient { id, secret }.save();
                    self.google_ready = r.is_ok() || self.google_ready;
                    if let Some(p) = &mut self.prefs {
                        p.message = Some(match r {
                            Ok(()) => (true, "Google sign-in is set up — use Sign in with Google to add or switch an account".into()),
                            Err(e) => (false, format!("{e:#}")),
                        });
                    }
                }
                PrefsAction::ClearGoogleClient => {
                    nightpost_core::oauth::GoogleClient::clear();
                    self.google_ready = false;
                }
                PrefsAction::GoogleSignIn(target) => self.google_sign_in(target),
                PrefsAction::CancelGoogle => self.cancel_google(),
                PrefsAction::SetHtmlEmail(on) => self.html_email = on,
                PrefsAction::SetLoadImages(on) => self.load_images = on,
                PrefsAction::SetDarkEmails(on) => self.dark_emails = on,
                PrefsAction::SetBlitz(on) => self.set_blitz_engine(on),
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
                        let c = nightpost_core::parse::clean_signature_html(&sig.body);
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

/// An account's status mark: a small dot (green, or grey before the first
/// sync), amber for a blip, a larger red one with "!" when it can't connect.
pub(super) fn paint_health(p: &egui::Painter, c: egui::Pos2, h: activity::Health, color: egui::Color32, hovered: bool) {
    let grow = if hovered { 1.0 } else { 0.0 };
    match h {
        activity::Health::Offline => {
            p.circle_filled(c, 6.0 + grow, color);
            p.text(c + egui::vec2(0.0, 0.5), egui::Align2::CENTER_CENTER, "!", egui::FontId::proportional(10.0), egui::Color32::WHITE);
        }
        activity::Health::Trouble => {
            p.circle_filled(c, 4.5 + grow, color);
        }
        activity::Health::Ok => {
            p.circle_filled(c, 3.0 + grow, color);
        }
        activity::Health::Unknown => {
            p.circle_stroke(c, 3.0 + grow, egui::Stroke::new(1.2_f32, color));
        }
    }
}
