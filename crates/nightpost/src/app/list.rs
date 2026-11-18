//! The message list: its header (title, filters, inbox tabs), the rows with
//! swipes, hover actions and context menus, date sections, and the summary
//! shown when several conversations are selected.

use super::*;

impl NightPostApp {
    pub(super) fn list_title(&self) -> String {
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
    pub(super) fn list_unread(&self) -> i64 {
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
    pub(super) fn list_header(&mut self, ui: &mut egui::Ui, col: &Colors) {
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
                        if nightpost_core::category::is_tab(*c) {
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
                // No filter is "All"; clicking the chosen chip again clears it.
                for f in ListFilter::ALL.into_iter().filter(|f| *f != ListFilter::All) {
                    // "With attachments" is a paperclip, so the chips fit on one line.
                    let r = if f == ListFilter::Attachments {
                        filter_chip(ui, col, "", f == self.filter).on_hover_text("With attachments")
                    } else {
                        filter_chip(ui, col, f.label(), f == self.filter)
                    };
                    if r.clicked() {
                        chosen = Some(if f == self.filter { ListFilter::All } else { f });
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

    pub(super) fn thread_list(&mut self, ctx: &egui::Context) {
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
                    self.empty_list(ui, &col);
                    return;
                }
                let width = ui.available_width();
                let threshold = (width * 0.3).clamp(80.0, 140.0);
                let settings = self.list_settings;
                let show_quick = settings.quick_actions && width >= QUICK_ACTIONS_MIN_WIDTH;

                commit = self.advance_swipe(ctx, wheel_x, now, threshold);
                let swiping = self.swipe.is_some();

                let row_h = ROW_H;
                let mut clicked = None;
                let mut row_pick: Option<RowPick> = None;
                let all_tabs: Vec<(u8, String)> = nightpost_core::category::ALL
                    .iter()
                    .map(|c| (*c, nightpost_core::category::label(*c).to_string()))
                    .chain(self.store.tabs().unwrap_or_default())
                    .collect();
                let mut toggle_pinned = false;
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
                            paint_swipe_under(&p, &col, rect, offset, action, offset.abs() >= threshold, &t);
                        }
                        let rect_c = rect.translate(egui::vec2(offset, 0.0));

                        let selected = self.selected == Some(i) || self.is_marked(t.account_id, &t.thread_key);
                        let hovered = resp.contains_pointer() && !swiping;
                        let look = row_look(&col, selected, hovered, &t);
                        // Full-width rows with a hairline between them.
                        p.rect_filled(rect_c, 0.0, look.bg);
                        p.hline(rect_c.x_range(), rect_c.bottom() - style::DIVIDER_WIDTH / 2.0, egui::Stroke::new(style::DIVIDER_WIDTH, col.list_border));
                        let rect = rect_c;
                        self.paint_row(&p, rect, &t, snoozed_until, &look);

                        // Quick actions on hover, in a wide enough list.
                        if show_quick && hovered {
                            if let Some(act) = quick_actions(ui, &col, rect, &t, i, &look) {
                                quick = Some((t.account_id, t.thread_key.clone(), act));
                            }
                        }

                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if let Some(pick) = self.row_menu(ui, &t, &all_tabs) {
                                row_pick = Some(pick);
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
                if let Some(pick) = row_pick {
                    self.apply_row_pick(pick);
                }
            });
        self.list_right = panel.response.rect.right();
        if let Some((a, k, act)) = commit.or(quick) {
            self.thread_action(ctx, a, &k, act);
        }
    }


    /// What the list says when it has nothing to show.
    fn empty_list(&self, ui: &mut egui::Ui, col: &Colors) {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            let (icon, text) = match (&self.selection, self.search.is_empty()) {
                (_, false) => ("🔍", "No conversations match your search."),
                _ if self.filter == ListFilter::Pinned => ("📌", "Nothing pinned. Press p on a conversation to keep it at the top."),
                _ if self.filter != ListFilter::All => ("🔍", "No conversations match this filter."),
                (Selection::Snoozed, _) => ("⏰", "Nothing snoozed. Press b on a conversation to snooze it."),
                _ if self.tabs_shown() && self.category != ALL_TAB => ("📭", "Nothing here right now."),
                (Selection::Unified(FolderRole::Inbox), _) => ("🌱", "Inbox zero. Nice work!"),
                _ => ("📭", "No conversations here."),
            };
            ui.label(RichText::new(icon).size(36.0));
            ui.label(RichText::new(text).color(col.subtle));
        });
    }

    /// Move the current swipe along: follow the trackpad, release after a
    /// short pause (when the fingers, and momentum, stop), then settle back.
    /// Returns the action to run when a release passed the threshold.
    fn advance_swipe(&mut self, ctx: &egui::Context, wheel_x: f32, now: Instant, threshold: f32) -> Option<(AccountId, String, SwipeAction)> {
        let settings = self.list_settings;
        let mut commit = None;
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
        commit
    }

    /// Draw a conversation's row: the sender's picture, names and date, the
    /// subject with its badges, and a two-line preview.
    fn paint_row(&self, p: &egui::Painter, rect: egui::Rect, t: &ThreadSummary, snoozed_until: Option<DateTime<Utc>>, look: &RowLook) {
        let col = &self.colors;
        let (bg, fg, sub, selected) = (look.bg, look.fg, look.sub, look.selected);
        let multi = self.accounts.len() > 1;
        let top = rect.top();
        let latest = t.participants.last().cloned().unwrap_or_default();
        self.faces.paint(p, egui::pos2(rect.left() + 36.0, top + 29.0), &latest.name, &latest.email, 34.0);
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
            icons::paint(p, egui::pos2(nx + 6.0, top + 19.0), 12.0, Icon::Reply, sub);
            nx += 17.0;
        }
        let names_max = right - date_w - 10.0;
        let name_font = egui::FontId::proportional(14.5);
        let w = style::bold_line(p, egui::pos2(nx, top + 10.0), &first, name_font.clone(), fg, names_max - nx);
        if !rest.is_empty() && nx + w < names_max {
            style::line(p, egui::pos2(nx + w, top + 10.0), &rest, name_font, sub, names_max - nx - w);
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
            badge(p, w, &|p, c| {
                p.rect_filled(egui::Rect::from_center_size(c, egui::vec2(w, 16.0)), 3.0, boxed);
                p.galley(egui::pos2(c.x - g.size().x / 2.0, c.y - g.size().y / 2.0), g.clone(), fg);
            });
        }
        if t.has_attachments {
            badge(p, 12.0, &|p, c| {
                p.text(c, egui::Align2::CENTER_CENTER, "📎", egui::FontId::proportional(12.0), sub);
            });
        }
        if t.starred {
            badge(p, 12.0, &|p, c| {
                p.text(c, egui::Align2::CENTER_CENTER, "★", egui::FontId::proportional(13.0), col.star);
            });
        }
        if t.pinned {
            let pin = if selected { fg } else { col.warning };
            badge(p, 12.0, &|p, c| icons::paint(p, c, 12.0, Icon::PinFilled, pin));
        }
        let subject_font = egui::FontId::proportional(13.5);
        if t.unread {
            style::bold_line(p, egui::pos2(x, top + 31.0), &t.subject, subject_font, fg, ix - x);
        } else {
            style::line(p, egui::pos2(x, top + 31.0), &t.subject, subject_font, fg, ix - x);
        }

        // Lines 3–4: a two-line preview.
        style::paragraph(p, egui::pos2(x, top + 51.0), &t.snippet, egui::FontId::proportional(12.5), sub, rect.right() - 20.0 - x, 2);
    }

    /// A conversation's right-click menu. Returns what was picked; the
    /// caller applies it with [`Self::apply_row_pick`] once the list is drawn.
    fn row_menu(&mut self, ui: &mut egui::Ui, t: &ThreadSummary, all_tabs: &[(u8, String)]) -> Option<RowPick> {
        let mut pick = None;
        if ui.button(if t.pinned { "Unpin" } else { "Pin to top" }).clicked() {
            pick = Some(RowPick::TogglePin(t.account_id, t.thread_key.clone()));
        }
        if ui.button(if t.unread { "Mark as read" } else { "Mark as unread" }).clicked() {
            pick = Some(RowPick::SetUnread(t.account_id, t.thread_key.clone(), !t.unread));
        }
        ui.separator();
        // The newest sender who isn't you.
        let sender = t.participants.iter().rev().find(|p| !p.email.is_empty() && !self.accounts.iter().any(|a| a.email.eq_ignore_ascii_case(&p.email)));
        if let Some(p) = sender {
            let email = p.email.to_lowercase();
            if ui.button(format!("All mail from {email}")).clicked() {
                pick = Some(RowPick::ShowSender(email.clone()));
            }
            let rule = self.store.sender_rule(&email).ok().flatten();
            ui.menu_button("Always put their mail in", |ui| {
                for (c, name) in all_tabs {
                    let mark = if rule == Some(*c) { "✓" } else { "  " };
                    if ui.button(format!("{mark}  {name}")).clicked() {
                        pick = Some(RowPick::SortSender(email.clone(), Some(*c)));
                    }
                }
                if ui.button("     New tab…").clicked() {
                    pick = Some(RowPick::NewTab(NewTabFor::Sender(email.clone())));
                }
                ui.separator();
                let mark = if rule.is_none() { "✓" } else { "  " };
                if ui.button(format!("{mark}  Sort automatically")).on_hover_text("NightPost decides from each email (Gmail's own tabs on Gmail)").clicked() {
                    pick = Some(RowPick::SortSender(email.clone(), None));
                }
            });
        }
        let here = self.store.thread_rule(t.account_id, &t.thread_key).ok().flatten();
        ui.menu_button("Put this conversation in", |ui| {
            for (c, name) in all_tabs {
                let mark = if here == Some(*c) { "✓" } else { "  " };
                if ui.button(format!("{mark}  {name}")).clicked() {
                    pick = Some(RowPick::SortThread(t.account_id, t.thread_key.clone(), Some(*c)));
                }
            }
            if ui.button("     New tab…").clicked() {
                pick = Some(RowPick::NewTab(NewTabFor::Thread(t.account_id, t.thread_key.clone())));
            }
            ui.separator();
            let mark = if here.is_none() { "✓" } else { "  " };
            if ui.button(format!("{mark}  Sort like the rest")).on_hover_text("By the sender's rule, or automatically").clicked() {
                pick = Some(RowPick::SortThread(t.account_id, t.thread_key.clone(), None));
            }
        });
        ui.separator();
        // The same type-to-find lists as the toolbar's Move and Label.
        let gmail = self.accounts.iter().any(|a| a.id == t.account_id && a.is_gmail());
        for kind in [FolderUse::Move, FolderUse::Label] {
            if kind == FolderUse::Label && !gmail {
                continue;
            }
            let title = if kind == FolderUse::Move { "Move to" } else { "Label" };
            ui.menu_button(title, |ui| {
                let opening = first_pass(ui, egui::Id::new(("row_picker", title)));
                let (p, c) = self.folder_picker_ui(ui, kind, t.account_id, &t.thread_key, opening);
                if let Some((f, add)) = p {
                    pick = Some(RowPick::Folder(t.account_id, t.thread_key.clone(), f, kind, add));
                    ui.close();
                }
                if let Some(name) = c {
                    pick = Some(RowPick::CreateFolder(t.account_id, t.thread_key.clone(), name, kind));
                    ui.close();
                }
            });
        }
        pick
    }

    /// Do what was picked in a row's right-click menu.
    fn apply_row_pick(&mut self, pick: RowPick) {
        match pick {
            RowPick::TogglePin(a, k) => self.toggle_pin(a, &k),
            RowPick::SetUnread(a, k, unread) => {
                self.set_row_unread(a, &k, unread);
                self.reload();
            }
            RowPick::ShowSender(email) => self.show_sender(&email),
            RowPick::SortSender(email, c) => self.sort_sender(&email, c),
            RowPick::SortThread(a, k, c) => self.sort_thread(a, &k, c),
            RowPick::NewTab(then) => self.new_tab = Some(NewTab { name: String::new(), then, error: None }),
            RowPick::Folder(a, k, f, FolderUse::Move, _) => self.move_to(a, &k, &f),
            RowPick::Folder(a, k, f, FolderUse::Label, add) => self.set_label(a, &k, &f, add),
            RowPick::CreateFolder(a, k, name, kind) => {
                self.worker.send(Cmd::CreateFolder(a, name.clone(), Some((vec![k.clone()], kind))));
                if kind == FolderUse::Move {
                    self.remove_row(a, &k);
                }
                self.toast(format!("Created “{name}”"), false);
            }
        }
    }

    /// Run a swipe / quick action on any conversation, not just the open one.
    pub(super) fn thread_action(&mut self, ctx: &egui::Context, a: AccountId, k: &str, act: SwipeAction) {
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
    pub(super) fn selection_summary(&mut self, ui: &mut egui::Ui, col: &Colors) {
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
}

/// The date group a conversation falls in, Canary-style.
pub(super) fn date_group(when: DateTime<Utc>, now: DateTime<Utc>) -> String {
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
pub(super) fn list_layout(rows: &[Row], grouped: bool, hidden_pinned: usize, now: DateTime<Utc>) -> Vec<(f32, f32, ListItem)> {
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

/// A rounded filter chip; the chosen one is filled with the accent.
pub(super) fn filter_chip(ui: &mut egui::Ui, col: &Colors, label: &str, on: bool) -> egui::Response {
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

pub(super) fn section_header(ui: &mut egui::Ui, col: &Colors, rect: egui::Rect, title: &str, pinned: Option<usize>) -> bool {
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

pub(super) fn swipe_color(col: &Colors, a: SwipeAction) -> egui::Color32 {
    match a {
        SwipeAction::Archive => col.success,
        SwipeAction::Trash => col.error,
        SwipeAction::Snooze => col.snooze,
        SwipeAction::Pin => col.accent,
        SwipeAction::ToggleRead => col.unread,
        SwipeAction::Nothing => col.list,
    }
}

pub(super) fn swipe_icon(a: SwipeAction) -> Icon {
    match a {
        SwipeAction::Archive => Icon::Done,
        SwipeAction::Trash => Icon::Trash,
        SwipeAction::Snooze => Icon::Snooze,
        SwipeAction::Pin => Icon::PinFilled,
        SwipeAction::ToggleRead | SwipeAction::Nothing => Icon::Envelope,
    }
}

pub(super) fn swipe_label(a: SwipeAction, unread: bool, pinned: bool) -> &'static str {
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

/// The list's "who" line, Canary style: the latest sender first (shown in
/// bold), then everyone else, e.g. ("Tim", ", Bee & Tom", true).
/// The flag is set when you sent the latest message.
pub(super) fn participants_parts(ps: &[Contact], me: Option<&Account>) -> (String, String, bool) {
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

/// A small letter-spaced heading in the sidebar ("ALL ACCOUNTS").
/// An Inbox tab: its name, unread count, and an underline when chosen.
pub(super) fn category_tab(ui: &mut egui::Ui, col: &Colors, label: &str, unread: i64, selected: bool) -> egui::Response {
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

/// Is the list dark (a dark theme)?
pub(super) fn list_is_dark(col: &Colors) -> bool {
    let c = col.list;
    (c.r() as u32 + c.g() as u32 + c.b() as u32) < 384
}

/// An unread row's background: the sidebar's highlight, lighter (full
/// strength would look selected). A touch stronger on dark themes.
pub(super) fn unread_wash(col: &Colors, hovered: bool) -> egui::Color32 {
    let base = if list_is_dark(col) { 0.60 } else { 0.55 };
    style::blend(col.sidebar_active, col.list, if hovered { base + 0.25 } else { base })
}

/// The colour pinned rows are washed with: a violet, the same in every
/// theme (themes have no colour of their own for it).
const PINNED_TINT: egui::Color32 = egui::Color32::from_rgb(0x8b, 0x5c, 0xf6);

/// A pinned row's background: a pale wash of [`PINNED_TINT`], deeper when
/// it has unread mail. Pinned rows don't get the unread wash, so without
/// this, new mail in a pinned conversation showed only as the small dot.
pub(super) fn pinned_wash(col: &Colors, hovered: bool, unread: bool) -> egui::Color32 {
    let base = match (list_is_dark(col), unread) {
        (true, false) => 0.08,
        (false, false) => 0.10,
        (true, true) => 0.18,
        (false, true) => 0.24,
    };
    style::blend(PINNED_TINT, col.list, if hovered { base + 0.06 } else { base })
}

/// What was picked in a conversation's right-click menu.
pub(super) enum RowPick {
    TogglePin(AccountId, String),
    /// Mark read (`false`) or unread (`true`).
    SetUnread(AccountId, String, bool),
    /// List all mail from this address.
    ShowSender(String),
    /// Always put this sender's mail in a tab (`None`: sort automatically).
    SortSender(String, Option<u8>),
    /// Put this conversation in a tab (`None`: sort like the rest).
    SortThread(AccountId, String, Option<u8>),
    NewTab(NewTabFor),
    /// Move to, or add or remove (the bool) a label.
    Folder(AccountId, String, Folder, FolderUse, bool),
    /// Make a folder or label, then move or label the conversation into it.
    CreateFolder(AccountId, String, String, FolderUse),
}

/// How a row looks: its background, text colours and whether it's drawn selected.
pub(super) struct RowLook {
    bg: egui::Color32,
    fg: egui::Color32,
    sub: egui::Color32,
    selected: bool,
}

/// A row's colours. A soft highlight (Moonlit) keeps the normal text
/// colours; a strong one (most Mailspring themes) inverts them. Pinned rows
/// get a pale wash of [`PINNED_TINT`] (deeper when unread), other unread
/// ones a lighter wash of the sidebar's highlight (full strength would look
/// like the selected row).
fn row_look(col: &Colors, selected: bool, hovered: bool, t: &ThreadSummary) -> RowLook {
    let soft = selected && col.p.list_focused.over(col.p.list).contrast(col.p.list) < 1.6;
    let (bg, fg, sub) = if selected && !soft {
        (col.list_focused, col.list_focused_text, col.list_focused_text.gamma_multiply(0.8))
    } else if selected {
        (col.list_focused, col.list_focused_text, col.subtle)
    } else if t.pinned {
        (pinned_wash(col, hovered, t.unread), col.text, col.subtle)
    } else if t.unread {
        (unread_wash(col, hovered), col.text, col.subtle)
    } else if hovered {
        (col.list_hover, col.text, col.subtle)
    } else {
        (col.list, col.text, col.subtle)
    };
    RowLook { bg, fg, sub, selected: selected && !soft }
}

/// The buttons shown over a hovered row (read, archive, trash, snooze, pin),
/// covering its right-hand end. Returns the one clicked.
fn quick_actions(ui: &mut egui::Ui, col: &Colors, rect: egui::Rect, t: &ThreadSummary, i: usize, look: &RowLook) -> Option<SwipeAction> {
    let (bg, fg, selected) = (look.bg, look.fg, look.selected);
    let p = ui.painter_at(rect);
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
    r.iter().zip(acts).find(|(resp, _)| resp.clicked()).map(|(_, act)| act)
}

/// Under a row being swiped: the action's colour (paler until it's far
/// enough to take effect), its icon and its name on the side it's going.
fn paint_swipe_under(p: &egui::Painter, col: &Colors, rect: egui::Rect, offset: f32, action: SwipeAction, armed: bool, t: &ThreadSummary) {
    let base = swipe_color(col, action);
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
    icons::paint(p, egui::pos2(icon_x, cy), 18.0, swipe_icon(action), fg);
    p.galley(egui::pos2(text_x, cy - galley.size().y / 2.0), galley, fg);
}
