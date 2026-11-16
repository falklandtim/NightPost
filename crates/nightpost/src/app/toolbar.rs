//! The toolbar across the top, its pop-ups (Move, Label, More, Snooze) and
//! the snooze calendar.

use super::*;

impl NightPostApp {
    /// One bar across the top, split to line up with the columns below:
    /// sidebar toggle | list title + list actions | message actions + search.
    pub(super) fn toolbar(&mut self, ctx: &egui::Context) {
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
                egui::Image::new(egui::include_image!("../../assets/logo-128.png")).corner_radius(8).paint_at(ui, logo);
                if col.p.dark {
                    p.rect_stroke(logo, 8.0, egui::Stroke::new(1.0_f32, col.border), egui::StrokeKind::Inside);
                }
                let toggle_w = 32.0;
                let a_right = if self.show_sidebar { self.sidebar_right.max(logo.right() + toggle_w + 16.0) } else { logo.right() + toggle_w + 20.0 };
                let toggle_x = a_right - toggle_w - 10.0;
                let name_font = egui::FontId::proportional(19.0);
                let name_w = p.layout_no_wrap("NightPost".into(), name_font.clone(), col.heading).size().x;
                if self.show_sidebar && logo.right() + 10.0 + name_w + 4.0 < toggle_x {
                    style::bold_line(&p, egui::pos2(logo.right() + 10.0, full.center().y - 11.0), "NightPost", name_font, col.heading, name_w + 2.0);
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
    pub(super) fn viewed_folder(&self, account: AccountId) -> Option<FolderId> {
        match self.selection {
            Selection::Folder(f) if self.search.is_empty() => self.folders.get(&account)?.iter().find(|x| x.id == f).map(|x| x.id),
            _ => None,
        }
    }

    /// Folders a conversation can be moved to / labelled with. Moves list
    /// the other accounts' folders after its own.
    pub(super) fn folder_choices(&self, account: AccountId, kind: FolderUse) -> Vec<Folder> {
        let all = self.folders.get(&account).cloned().unwrap_or_default();
        let has_archive = all.iter().any(|f| f.role == FolderRole::Archive);
        let from = self.viewed_folder(account);
        let mut out: Vec<Folder> = all
            .into_iter()
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
            .collect();
        if kind == FolderUse::Move {
            for acc in self.accounts.iter().filter(|x| x.id != account) {
                let theirs = self.folders.get(&acc.id).cloned().unwrap_or_default();
                let has_archive = theirs.iter().any(|f| f.role == FolderRole::Archive);
                out.extend(theirs.into_iter().filter(|f| {
                    !snooze_sync::is_internal_folder(&f.path)
                        && match f.role {
                            FolderRole::Inbox | FolderRole::Archive | FolderRole::Other => true,
                            FolderRole::All => !has_archive,
                            _ => false,
                        }
                }));
            }
        }
        out
    }

    /// A folder's name in the Move picker: another account's folders say
    /// which account they're in.
    pub(super) fn choice_label(&self, account: AccountId, f: &Folder) -> String {
        if f.account_id == account {
            return folder_label(f);
        }
        let email = self.accounts.iter().find(|a| a.id == f.account_id).map(|a| a.email.as_str()).unwrap_or("another account");
        format!("{} · {email}", folder_label(f))
    }

    pub(super) fn move_to(&mut self, a: AccountId, k: &str, folder: &Folder) {
        let from = self.viewed_folder(a);
        if folder.account_id != a {
            // Into another account: copied across, originals to Trash. No
            // undo (the copy would have to be deleted from the other server).
            self.worker.send(Cmd::MoveAcross(a, k.to_string(), from, folder.id));
            self.remove_row(a, k);
            self.toast(format!("Moving to {}. The original goes to Trash.", self.choice_label(a, folder)), false);
            return;
        }
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

    pub(super) fn set_label(&mut self, a: AccountId, k: &str, folder: &Folder, add: bool) {
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

    /// The searchable list in the Move / Label pickers (toolbar and
    /// right-click): type to filter, ↑ ↓ to choose, Return to pick.
    /// Returns the folder picked (and, for a label, whether to add it) or a
    /// name to create.
    pub(super) fn folder_picker_ui(&mut self, ui: &mut egui::Ui, kind: FolderUse, a: AccountId, k: &str, opening: bool) -> (Option<(Folder, bool)>, Option<String>) {
        if opening {
            self.picker_query.clear();
            self.picker_index = 0;
        }
        let choices = self.folder_choices(a, kind);
        let applied: Vec<FolderId> = if kind == FolderUse::Label { self.store.thread_folder_ids(a, k).unwrap_or_default() } else { vec![] };
        let mut pick: Option<(Folder, bool)> = None;
        let mut create: Option<String> = None;
        let col = self.colors.clone();
        ui.set_min_width(260.0);
        // Arrow keys move the highlight; take them before the text
        // field does (it would move the cursor).
        let (down, up) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, Key::ArrowUp)));
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.picker_query)
                .hint_text(if kind == FolderUse::Move { "Move to…" } else { "Label as…" })
                .desired_width(250.0),
        );
        if opening {
            r.request_focus();
        }
        if r.changed() {
            self.picker_index = 0;
        }
        let q = self.picker_query.trim().to_lowercase();
        // Names starting with what's typed first, then the rest that contain it.
        let mut matches: Vec<&Folder> = choices.iter().filter(|f| q.is_empty() || self.choice_label(a, f).to_lowercase().contains(&q)).collect();
        // Names starting with what's typed first; this account's before others.
        matches.sort_by_key(|f| (!folder_label(f).to_lowercase().starts_with(&q), f.account_id != a));
        let exact = choices.iter().any(|f| f.account_id == a && (folder_label(f).to_lowercase() == q || f.path.to_lowercase() == q));
        let can_create = !q.is_empty() && !exact;
        // The rows you can pick: the matches, then "Create …".
        let rows = matches.len() + can_create as usize;
        if rows > 0 {
            if down {
                self.picker_index = (self.picker_index + 1).min(rows - 1);
            }
            if up {
                self.picker_index = self.picker_index.saturating_sub(1);
            }
        }
        self.picker_index = self.picker_index.min(rows.saturating_sub(1));
        let hi = self.picker_index;
        ui.add_space(4.0);
        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
            for (i, f) in matches.iter().enumerate() {
                if f.account_id != a && (i == 0 || matches[i - 1].account_id == a) {
                    ui.add_space(2.0);
                    ui.label(RichText::new("Other accounts").small().color(col.subtle));
                }
                let on = applied.contains(&f.id);
                let icon = match kind {
                    FolderUse::Label => if on { "☑" } else { "☐" },
                    FolderUse::Move => f.role.icon(),
                };
                let b = ui.add(egui::Button::selectable(i == hi, format!("{icon}  {}", self.choice_label(a, f))).min_size(egui::vec2(250.0, 24.0)));
                if i == hi && (up || down) {
                    b.scroll_to_me(None);
                }
                if b.clicked() {
                    pick = Some(((*f).clone(), !on));
                }
            }
            if matches.is_empty() && q.is_empty() {
                ui.label(RichText::new(if kind == FolderUse::Label { "No labels yet — type a name to create one." } else { "No other folders." }).small().color(col.subtle));
            }
        });
        if can_create {
            ui.separator();
            let what = if kind == FolderUse::Move { "folder" } else { "label" };
            if ui.add(egui::Button::selectable(hi == matches.len(), format!("+  Create {what} “{}”", self.picker_query.trim())).min_size(egui::vec2(250.0, 24.0))).clicked() {
                create = Some(self.picker_query.trim().to_string());
            }
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            if let Some(f) = matches.get(hi) {
                pick = Some(((*f).clone(), !applied.contains(&f.id)));
            } else if can_create {
                create = Some(self.picker_query.trim().to_string());
            }
        }
        (pick, create)
    }

    /// Searchable Move / Label picker under a toolbar button (`v` / `l`).
    pub(super) fn folder_popup(&mut self, btn: &egui::Response, kind: FolderUse, target: &(AccountId, String)) {
        let (a, k) = (target.0, target.1.clone());
        let (open_key, popup_id) = match kind {
            FolderUse::Move => ("picker_open_move", "picker_move"),
            FolderUse::Label => ("picker_open_label", "picker_label"),
        };
        let forced = btn.ctx.memory_mut(|m| m.data.remove_temp::<bool>(egui::Id::new(open_key))).unwrap_or(false);
        let opening = btn.clicked() || forced;
        if opening {
            self.picker_query.clear();
            self.picker_index = 0;
        }
        let mut pick: Option<(Folder, bool)> = None;
        let mut create: Option<String> = None;
        let popup_id = egui::Id::new(popup_id);
        egui::Popup::from_response(btn)
            .id(popup_id)
            .open_memory(opening.then_some(egui::SetOpenCommand::Toggle))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                (pick, create) = self.folder_picker_ui(ui, kind, a, &k, opening);
            });
        if pick.is_some() || create.is_some() {
            egui::Popup::close_id(&btn.ctx, popup_id);
            self.picker_query.clear();
        }
        match (pick, create) {
            (Some((f, add)), _) => {
                let name = self.choice_label(a, &f);
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

    pub(super) fn more_popup(&mut self, btn: &egui::Response, starred: bool) {
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
            ui.separator();
            if ui.add(egui::Button::new("Keyboard shortcuts").shortcut_text("?").frame(false).min_size(egui::vec2(190.0, 24.0))).clicked() {
                action = Some(5);
            }
            if ui.add(egui::Button::new("Sync activity").shortcut_text(ACTIVITY_KEY).frame(false).min_size(egui::vec2(190.0, 24.0))).clicked() {
                action = Some(6);
            }
            if self.open.is_some()
                && ui.add(egui::Button::new("Show original").frame(false).min_size(egui::vec2(190.0, 24.0))).on_hover_text("The newest message's raw source").clicked()
            {
                action = Some(4);
            }
        });
        match action {
            Some(0) => self.act_star(),
            Some(1) => self.act_unread(),
            Some(2) => self.act_move(FolderRole::Trash),
            Some(3) => self.act_move(FolderRole::Spam),
            Some(4) => self.show_original(),
            Some(5) => self.show_keys = true,
            Some(6) => self.toggle_activity(None),
            _ => {}
        }
    }

    pub(super) fn snooze_popup(&mut self, ui: &mut egui::Ui, btn: &egui::Response, col: &Colors, snoozed: Option<DateTime<Utc>>) {
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
}

/// A month calendar and a time, for "Pick a date & time" in the Snooze menu.
/// Returns the moment once Snooze is pressed.
pub(super) fn snooze_calendar(ui: &mut egui::Ui, col: &Colors, pick: &mut SnoozePick) -> Option<DateTime<Utc>> {
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
