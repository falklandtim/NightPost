//! Actions on conversations: archive, move, star, pin, snooze, read state,
//! compose and drag-and-drop, plus multi-select, undo, Undo Send and the
//! inbox tab rules. Each changes the local store at once and sends a
//! `worker::Cmd` to bring the server into line.

use super::*;

impl NightPostApp {
    pub(super) fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, at: Instant::now(), undo: false });
    }

    /// Remember an action for ⌘Z and say what happened, with an Undo button.
    pub(super) fn did(&mut self, text: impl Into<String>, undo: Undo) {
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
    pub(super) fn undo_last(&mut self) {
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

    pub(super) fn undo_apply(&mut self, u: &Undo) {
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
                self.worker.send(Cmd::SyncPins(*a));
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
            Undo::TabRemoved { tab } => self.save_tab_change(nightpost_core::snooze_sync::TabChange::Tab(nightpost_core::snooze_sync::restored_tab(tab)), None),
        }
    }

    /// Send now, or after the Undo Send delay (the composer is kept until then).
    pub(super) fn queue_send(&mut self, account: AccountId, out: OutgoingMessage, composer: Composer) {
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
    pub(super) fn flush_sends(&mut self, all: bool) {
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
    pub(super) fn cancel_send(&mut self, id: u64) {
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
    pub(super) fn send_pending_now(&mut self) {
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
    pub(super) fn send_now(&mut self, id: u64) {
        if let Some(p) = self.pending_sends.iter_mut().find(|p| p.id == id) {
            p.due = Instant::now();
        }
        self.flush_sends(false);
    }

    pub(super) fn multi(&self) -> bool {
        self.marked.len() >= 2
    }

    pub(super) fn row_key(&self, i: usize) -> Option<(AccountId, String)> {
        self.rows.get(i).map(|r| (r.t.account_id, r.t.thread_key.clone()))
    }

    pub(super) fn is_marked(&self, a: AccountId, k: &str) -> bool {
        self.multi() && self.marked.iter().any(|(ma, mk)| *ma == a && mk == k)
    }

    /// ⌘-click: add a conversation to the selection, or take it out.
    pub(super) fn toggle_mark(&mut self, i: usize) {
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
    pub(super) fn mark_range(&mut self, i: usize) {
        let from = self.anchor.or(self.selected).unwrap_or(i);
        let (lo, hi) = (from.min(i), from.max(i));
        self.marked = (lo..=hi).filter_map(|j| self.row_key(j)).collect();
        self.anchor = Some(from);
        self.marks_changed();
    }

    /// ⇧↑ / ⇧↓: grow or shrink the selection from the anchor.
    pub(super) fn extend_marks(&mut self, d: i64) {
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

    pub(super) fn select_all(&mut self) {
        self.marked = (0..self.rows.len()).filter_map(|j| self.row_key(j)).collect();
        self.anchor = Some(0);
        self.marks_changed();
    }

    pub(super) fn clear_marks(&mut self) {
        self.marked.clear();
    }

    /// With several marked the reading pane shows a summary; back to one,
    /// that conversation opens.
    pub(super) fn marks_changed(&mut self) {
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
    pub(super) fn targets(&self) -> Vec<(AccountId, String)> {
        if self.multi() {
            self.marked.clone()
        } else {
            self.current().into_iter().collect()
        }
    }

    /// Run `f` on the open conversation, or on every selected one. With
    /// several, their undos become one ⌘Z and one toast sums them up.
    pub(super) fn for_each_target(&mut self, text: impl FnOnce(usize) -> String, mut f: impl FnMut(&mut Self, AccountId, &str)) {
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
    pub(super) fn apply_to(&mut self, targets: Vec<(AccountId, String)>, text: impl FnOnce(usize) -> String, mut f: impl FnMut(&mut Self, AccountId, &str)) {
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
    pub(super) fn target_rows(&self) -> Vec<&ThreadSummary> {
        let t = self.targets();
        self.rows.iter().map(|r| &r.t).filter(|r| t.iter().any(|(a, k)| *a == r.account_id && *k == r.thread_key)).collect()
    }

    /// The account every selected conversation belongs to, if they share one
    /// (folders and labels are per account).
    pub(super) fn targets_account(&self) -> Option<AccountId> {
        let t = self.targets();
        let a = t.first()?.0;
        t.iter().all(|(x, _)| *x == a).then_some(a)
    }

    /// Keep a tab change here at once and on the servers in the background
    /// (`label`: a conversation that moved, and the tab it left, for Gmail's labels).
    pub(super) fn save_tab_change(&mut self, change: nightpost_core::snooze_sync::TabChange, label: Option<(AccountId, String, u8)>) {
        use nightpost_core::snooze_sync::TabChange;
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
    pub(super) fn put_sender_rule(&mut self, email: &str, category: Option<u8>) {
        match nightpost_core::snooze_sync::sender_rule(&self.store, email, category) {
            Ok(r) => self.save_tab_change(nightpost_core::snooze_sync::TabChange::Sender(r), None),
            Err(e) => self.toast(format!("{e:#}"), true),
        }
    }

    /// Sort a sender (undoable), from the list's context menu.
    pub(super) fn sort_sender(&mut self, email: &str, category: Option<u8>) {
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
    pub(super) fn put_thread_rule(&mut self, a: AccountId, k: &str, category: Option<u8>) {
        let before = self.store.thread_category(a, k).unwrap_or(0);
        match nightpost_core::snooze_sync::thread_rule(&self.store, a, k, category) {
            Ok(r) => self.save_tab_change(nightpost_core::snooze_sync::TabChange::Thread(r), Some((a, k.to_string(), before))),
            Err(e) => self.toast(format!("{e:#}"), true),
        }
    }

    /// Put a conversation in a tab (undoable), from the list's context menu.
    pub(super) fn sort_thread(&mut self, a: AccountId, k: &str, category: Option<u8>) {
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
    pub(super) fn create_tab(&mut self, name: &str, then: &NewTabFor) -> Result<(), String> {
        let tab = nightpost_core::snooze_sync::new_tab(&self.store, name).map_err(|e| format!("{e:#}"))?;
        let existed = self.store.all_tabs().unwrap_or_default().iter().any(|t| t.id == tab.id);
        if !existed {
            self.save_tab_change(nightpost_core::snooze_sync::TabChange::Tab(tab.clone()), None);
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
    pub(super) fn remove_tab(&mut self, c: u8) {
        let Ok(tab) = nightpost_core::snooze_sync::removed_tab(&self.store, c) else { return };
        let name = tab.name.clone();
        self.save_tab_change(nightpost_core::snooze_sync::TabChange::Tab(tab.clone()), None);
        self.did(format!("Removed the “{name}” tab"), Undo::TabRemoved { tab });
    }

    /// List everything from one address (a search, so Esc or clearing it goes back).
    pub(super) fn show_sender(&mut self, email: &str) {
        self.search = format!("from:{email}");
        self.clear_marks();
        self.selected = None;
        self.open = None;
        self.reload();
    }

    /// Remove the open thread from the list (archive/trash/snooze) and move on.
    pub(super) fn remove_current_and_advance(&mut self) {
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

    pub(super) fn act_move(&mut self, role: FolderRole) {
        let done = match role {
            FolderRole::Archive => "Archived",
            FolderRole::Trash => "Moved to Trash",
            FolderRole::Spam => "Reported as spam",
            FolderRole::Inbox => "Moved to Inbox",
            _ => "Moved",
        };
        self.for_each_target(|n| format!("{done} {n} conversations"), |s, a, k| s.move_thread(a, k, role));
    }

    pub(super) fn move_thread(&mut self, a: AccountId, k: &str, role: FolderRole) {
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
    pub(super) fn remove_row(&mut self, a: AccountId, k: &str) {
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

    pub(super) fn act_star(&mut self) {
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

    pub(super) fn act_unread(&mut self) {
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

    pub(super) fn act_snooze(&mut self, until: DateTime<Utc>) {
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

    pub(super) fn act_unsnooze(&mut self) {
        let Some((a, k)) = self.current() else { return };
        let _ = self.store.unsnooze(a, &k);
        self.worker.send(Cmd::Unsnooze(a, k.clone()));
        self.toast("Returned to Inbox", false);
        self.reload();
    }

    pub(super) fn toggle_pin(&mut self, a: AccountId, k: &str) {
        let pinned = self.store.is_pinned(a, k).unwrap_or(false);
        let r = if pinned { self.store.unpin(a, k) } else { self.store.pin(a, k) };
        if let Err(e) = r {
            self.toast(format!("Couldn't pin: {e:#}"), true);
            return;
        }
        self.worker.send(Cmd::SyncPins(a));
        self.did(if pinned { "Unpinned" } else { "Pinned to top" }, Undo::Pin { a, k: k.to_string() });
        self.reload();
    }

    pub(super) fn act_pin(&mut self) {
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

    pub(super) fn set_row_unread(&mut self, a: AccountId, k: &str, unread: bool) {
        let _ = self.store.set_thread_unread(a, k, unread);
        self.worker.send(Cmd::SetUnread(a, k.to_string(), unread));
    }

    pub(super) fn mark_all_read(&mut self) {
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

    pub(super) fn sync_now(&mut self) {
        self.syncing = true;
        self.status = "Checking for mail…".into();
        self.worker.send(Cmd::SyncAll);
    }

    pub(super) fn compose_new(&mut self) {
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

    pub(super) fn compose_reply(&mut self, mode: u8) {
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
    pub(super) fn drop_on(&mut self, keys: Vec<(AccountId, String)>, target: DropTarget) {
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
                // Mail from other accounts is copied across into this folder.
                let (mine, other): (Vec<_>, Vec<_>) = keys.into_iter().partition(|(a, _)| *a == f.account_id);
                if !other.is_empty() {
                    let name = self.choice_label(0, &f);
                    self.apply_to(other, |n| format!("Moving {n} conversations to {name}. The originals go to Trash."), |s, a, k| s.move_to(a, k, &f));
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

    /// "Show original": fetch the newest message's raw source.
    pub(super) fn show_original(&mut self) {
        let Some(m) = self.open.as_ref().and_then(|o| o.messages.last()).cloned() else { return };
        self.toast("Fetching the original…", false);
        self.worker.send(Cmd::ShowOriginal(Box::new(m)));
    }

    /// Open a saved draft in a composer (its attachments are fetched first).
    pub(super) fn edit_draft(&mut self, m: Message) {
        if self.composers.iter().any(|c| c.draft.as_ref().is_some_and(|d| d.message_id == m.message_id)) {
            self.toast("That draft is already open", false);
            return;
        }
        if self.opening_drafts.insert(m.message_id.clone()) {
            self.worker.send(Cmd::OpenDraft(Box::new(m)));
        }
    }
}
