//! Keyboard and worker events: what the app does with each key, and with
//! each event the background threads send back.

use super::*;

impl NightPostApp {
    pub(super) fn handle_events(&mut self) {
        let mut changed = false;
        while let Ok(ev) = self.worker.rx.try_recv() {
            match ev {
                // Progress text only: the toolbar spinner is for syncs you
                // asked for, not the quiet background ones.
                Event::Status(s) => self.status = s,
                Event::SyncFinished { new, errors } => {
                    self.syncing = false;
                    // Background failures show on the account's dot (and in
                    // Sync activity); a sync you asked for also says why.
                    let asked = self.syncing;
                    self.syncing = false;
                    self.status = if new > 0 { format!("{new} new message{}", if new == 1 { "" } else { "s" }) } else { String::new() };
                    if asked {
                        for e in errors {
                            self.toast(e, true);
                        }
                    }
                    self.health_read = None;
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
                    self.health_read = None;
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

    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
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
        // ⌘F: find (the search box), from anywhere.
        if self.setup.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F)) {
            self.focus_search = true;
        }
        // ⌘⇧L: Sync activity.
        if self.setup.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::L)) {
            self.toggle_activity(None);
        }
        if self.activity.is_some() {
            return;
        }
        // A sheet over the window: ? closes the shortcuts list; nothing
        // else reaches the list behind it.
        if self.show_keys {
            if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "?"))) {
                self.show_keys = false;
            }
            return;
        }
        // Single-key shortcuts only when nothing is being typed into.
        if ctx.wants_keyboard_input() || self.setup.is_some() || self.new_tab.is_some() {
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
            "?" => self.show_keys = !self.show_keys,
            "g" => self.pending_g = Some(Instant::now()),
            _ => {}
        }
        let shift = ctx.input(|i| i.modifiers.shift);
        // With a menu open, the arrows belong to it (see `menu_keys`).
        let key = |k: Key| key(k) && !egui::Popup::is_any_open(ctx);
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

    pub(super) fn move_selection(&mut self, d: i64) {
        if self.rows.is_empty() {
            return;
        }
        let next = match self.selected {
            Some(i) => (i as i64 + d).clamp(0, self.rows.len() as i64 - 1) as usize,
            None => 0,
        };
        self.select(next);
    }
}

/// ↑ ↓ through the items of an open menu (right-click menus, the toolbar's
/// ⋯ menu, drop-downs), Return to choose one or open a submenu, Esc to close.
/// egui's own arrow-key focus jumps to whatever is nearest on screen, which
/// can be the list behind the menu, so it's replaced here: the items are the
/// clickable widgets on the newest menu layer, top to bottom, wrapping round.
/// Text fields (the Move / Label search) keep the arrows to themselves.
pub(super) fn menu_keys(ctx: &egui::Context) {
    let pending = egui::Id::new("menu_keys_enter");
    if !egui::Popup::is_any_open(ctx) {
        ctx.data_mut(|d| d.remove_temp::<(usize, u64)>(pending));
        return;
    }
    let focused = ctx.memory(|m| m.focused());
    let order: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().filter(|l| l.order == egui::Order::Foreground).collect());
    // Each open menu (newest last) with its items, top to bottom.
    let (menus, typing) = ctx.viewport(|v| {
        let widgets = &v.prev_pass.widgets;
        let typing = focused.and_then(|id| widgets.get(id)).is_some_and(|w| w.sense.senses_drag() && w.sense.is_focusable());
        let menus: Vec<Vec<(egui::Id, egui::Rect)>> = order
            .iter()
            .map(|l| {
                // The menu's own background is clickable too; skip it.
                let items: Vec<&egui::WidgetRect> = widgets.get_layer(*l).filter(|w| w.id != l.id && w.enabled && w.sense.senses_click() && !w.sense.senses_drag()).collect();
                let bg = |w: &egui::WidgetRect| items.len() > 1 && items.iter().all(|o| w.rect.contains_rect(o.rect));
                let mut items: Vec<(egui::Id, egui::Rect)> = items.iter().filter(|w| !bg(w)).map(|w| (w.id, w.rect)).collect();
                items.sort_by(|a, b| (a.1.top(), a.1.left()).partial_cmp(&(b.1.top(), b.1.left())).unwrap_or(std::cmp::Ordering::Equal));
                items
            })
            .filter(|items| !items.is_empty())
            .collect();
        (menus, typing)
    });
    // Return on an item clicks it (egui does that for the focused widget).
    // Unless it opened a submenu, the menu's job is done: close it, as a
    // mouse click would.
    let pass = ctx.cumulative_pass_nr();
    if let Some((before, at)) = ctx.data(|d| d.get_temp::<(usize, u64)>(pending)) {
        if pass >= at + 2 {
            ctx.data_mut(|d| d.remove_temp::<(usize, u64)>(pending));
            if menus.len() <= before {
                egui::Popup::close_all(ctx);
                return;
            }
        }
    }
    let in_menu = focused.is_some_and(|f| menus.iter().any(|m| m.iter().any(|(id, _)| *id == f)));
    if in_menu && !typing && ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space)) {
        ctx.data_mut(|d| d.insert_temp(pending, (menus.len(), pass)));
    }
    let (down, up) = ctx.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp)));
    if typing || (!down && !up) {
        return;
    }
    let Some(items) = menus.last() else { return };
    // Stop egui's own move, then make ours.
    ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    let n = items.len();
    let next = match focused.and_then(|f| items.iter().position(|(id, _)| *id == f)) {
        Some(i) if down => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        None if down => 0,
        None => n - 1,
    };
    ctx.memory_mut(|m| m.request_focus(items[next].0));
}

/// Is this the first pass a piece of UI (like a submenu) has been shown
/// since it was last hidden?
pub(super) fn first_pass(ui: &egui::Ui, id: egui::Id) -> bool {
    let pass = ui.ctx().cumulative_pass_nr();
    let last = ui.ctx().data_mut(|d| {
        let last = d.get_temp::<u64>(id);
        d.insert_temp(id, pass);
        last
    });
    !matches!(last, Some(l) if l + 1 >= pass)
}
