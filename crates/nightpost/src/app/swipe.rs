//! Swiping a conversation sideways in the list, much as on the iPhone.
//!
//! A short swipe springs back. A medium one leaves the row open on its
//! buttons (up to three a side, chosen in Preferences → Swipe actions), and
//! a long one runs the first button's action. Two-finger trackpad swipes
//! arrive as horizontal scroll with no "fingers lifted" event, so a pause of
//! [`RELEASE_AFTER`] counts as letting go.
//!
//! Motion is time-based: the drawn row eases towards where it should be by
//! the frame's real duration, so it moves at the same speed at 60 and
//! 120 Hz and doesn't lurch when a frame is late. The old version shrank the
//! offset by a fixed share each frame, and snapped a row that was leaving
//! back into place before removing it. Now a leaving row (archive, trash,
//! spam) slides fully out first.

use super::*;
use super::list::RowPick;

/// The pop-up a swipe's More button opens: the row's right-click menu.
pub(super) const MORE_MENU: &str = "swipe-more-menu";
/// The width each revealed button takes.
pub(super) const BUTTON_W: f32 = 78.0;
/// A pause in trackpad scrolling this long counts as letting go.
const RELEASE_AFTER: Duration = Duration::from_millis(140);
/// How closely the row follows the fingers: the time it takes to close
/// about two thirds of the gap. Small enough to feel attached, large enough
/// to smooth the uneven bursts trackpad scrolling arrives in.
const FOLLOW: f32 = 0.02;
/// The same, for settling back, opening on the buttons, or sliding out.
const SETTLE: f32 = 0.06;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SwipePhase {
    /// Following two-finger trackpad scrolling.
    Trackpad,
    /// Following a mouse drag.
    Drag,
    /// Fingers lifted / mouse released: decide what happens.
    Released,
    /// Resting open on its buttons, waiting for a click.
    Open,
    /// Sliding out of the list before its action removes it.
    Leaving,
    /// Sliding back into place.
    Settling,
}

/// A conversation being swiped sideways in the list.
pub(super) struct Swipe {
    pub(super) account_id: AccountId,
    pub(super) thread_key: String,
    /// Where the fingers (or the mouse, or the outcome) put the row.
    pub(super) target: f32,
    /// Where it's drawn: eases towards `target`.
    pub(super) shown: f32,
    pub(super) phase: SwipePhase,
    /// The last trackpad movement, to notice the fingers lifting.
    pub(super) last: Instant,
    /// What to do once a leaving row is out of sight.
    leaving: Option<SwipeAction>,
    /// Closed by a click: that click doesn't also open a conversation.
    pub(super) tapped_shut: bool,
    /// Where its buttons were last drawn, so clicking one doesn't count as
    /// clicking away.
    buttons: Vec<egui::Rect>,
}

impl Swipe {
    pub(super) fn new(t: &ThreadSummary, phase: SwipePhase, offset: f32) -> Self {
        Self { account_id: t.account_id, thread_key: t.thread_key.clone(), target: offset, shown: offset, phase, last: Instant::now(), leaving: None, tapped_shut: false, buttons: Vec::new() }
    }

    pub(super) fn is(&self, t: &ThreadSummary) -> bool {
        self.account_id == t.account_id && self.thread_key == t.thread_key
    }

    /// Spring back into place.
    pub(super) fn close(&mut self) {
        self.phase = SwipePhase::Settling;
        self.target = 0.0;
    }
}

/// What letting go of a swiped row does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Release {
    /// Spring back.
    Close,
    /// Rest open on the buttons.
    Open,
    /// Run the first button's action.
    Full,
}

/// How far the buttons on one side take, open.
pub(super) fn open_width(n: usize) -> f32 {
    BUTTON_W * n as f32
}

/// How far a swipe must go to run the first action: past the open buttons
/// with room to spare, and over half the row.
pub(super) fn full_at(width: f32, n: usize) -> f32 {
    (width * 0.55).max(open_width(n) + 48.0)
}

/// What letting go at `offset` does, with `n` buttons on that side.
pub(super) fn release(offset: f32, n: usize, width: f32) -> Release {
    let d = offset.abs();
    if n == 0 {
        Release::Close
    } else if d >= full_at(width, n) {
        Release::Full
    } else if d >= BUTTON_W * 0.5 {
        Release::Open
    } else {
        Release::Close
    }
}

/// Move `shown` towards `target` as `dt` seconds pass, closing about two
/// thirds of the gap every `tau` seconds. Depends only on time passed, not
/// on how many frames it took. Snaps to the target once within half a point.
pub(super) fn ease(shown: f32, target: f32, dt: f32, tau: f32) -> f32 {
    let k = 1.0 - (-dt.max(0.0) / tau).exp();
    let v = shown + (target - shown) * k;
    if (target - v).abs() < 0.5 {
        target
    } else {
        v
    }
}

/// The buttons on the side a row moving by `offset` reveals, outermost
/// first. None when that side is off (its first action is Nothing).
pub(super) fn side_actions(settings: &ListSettings, offset: f32) -> Vec<SwipeAction> {
    let side = if offset > 0.0 { settings.swipe_right } else { settings.swipe_left };
    if side[0] == SwipeAction::Nothing {
        return Vec::new();
    }
    side.into_iter().filter(|a| *a != SwipeAction::Nothing).collect()
}

/// Actions that take the conversation out of the list, so its row slides
/// out instead of springing back.
fn removes_row(a: SwipeAction) -> bool {
    matches!(a, SwipeAction::Archive | SwipeAction::Trash | SwipeAction::Spam)
}

pub(super) fn swipe_color(col: &Colors, a: SwipeAction) -> egui::Color32 {
    match a {
        SwipeAction::Archive => col.success,
        SwipeAction::Trash => col.error,
        SwipeAction::Snooze => col.snooze,
        SwipeAction::Pin => col.accent,
        SwipeAction::ToggleRead => col.unread,
        SwipeAction::Move => style::blend(col.accent, col.text, 0.35),
        SwipeAction::More => style::blend(col.subtle, col.text, 0.2),
        SwipeAction::Star => col.star,
        SwipeAction::Spam => col.warning,
        SwipeAction::Reply => col.link,
        SwipeAction::Nothing => col.list,
    }
}

pub(super) fn swipe_icon(a: SwipeAction) -> Icon {
    match a {
        SwipeAction::Archive => Icon::Done,
        SwipeAction::Trash => Icon::Trash,
        SwipeAction::Snooze => Icon::Snooze,
        SwipeAction::Pin => Icon::PinFilled,
        SwipeAction::Move => Icon::Folder,
        SwipeAction::More => Icon::More,
        SwipeAction::Star => Icon::Star,
        SwipeAction::Spam => Icon::Warning,
        SwipeAction::Reply => Icon::Reply,
        SwipeAction::ToggleRead | SwipeAction::Nothing => Icon::Envelope,
    }
}

pub(super) fn swipe_label(a: SwipeAction, t: &ThreadSummary) -> &'static str {
    match a {
        SwipeAction::Archive => "Archive",
        SwipeAction::Trash => "Trash",
        SwipeAction::Snooze => "Snooze",
        SwipeAction::Pin if t.pinned => "Unpin",
        SwipeAction::Pin => "Pin",
        SwipeAction::ToggleRead if t.unread => "Read",
        SwipeAction::ToggleRead => "Unread",
        SwipeAction::Move => "Move",
        SwipeAction::More => "More",
        SwipeAction::Star if t.starred => "Unstar",
        SwipeAction::Star => "Star",
        SwipeAction::Spam => "Spam",
        SwipeAction::Reply => "Reply",
        SwipeAction::Nothing => "",
    }
}

/// Under a row moved `shown` points sideways: its buttons, outermost first,
/// sharing the space it uncovers. Past the full-swipe point (`full`), the
/// first one fills it all. Returns each button's rectangle, for clicks.
pub(super) fn paint_under(p: &egui::Painter, col: &Colors, rect: egui::Rect, shown: f32, acts: &[SwipeAction], full: bool, t: &ThreadSummary) -> Vec<(egui::Rect, SwipeAction)> {
    let area = if shown > 0.0 {
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + shown, rect.bottom()))
    } else {
        egui::Rect::from_min_max(egui::pos2(rect.right() + shown, rect.top()), rect.max)
    };
    p.rect_filled(area, 0.0, col.list);
    let shown_acts: &[SwipeAction] = if full { &acts[..acts.len().min(1)] } else { acts };
    let n = shown_acts.len().max(1) as f32;
    let slot = area.width() / n;
    let mut out = Vec::new();
    for (i, a) in shown_acts.iter().enumerate() {
        // Outermost first: from the row's edge inwards.
        let x0 = if shown > 0.0 { area.left() + slot * i as f32 } else { area.right() - slot * (i as f32 + 1.0) };
        let cell = egui::Rect::from_min_size(egui::pos2(x0, area.top()), egui::vec2(slot, area.height()));
        let pill = egui::Rect::from_center_size(egui::pos2(cell.center().x, cell.center().y - 9.0), egui::vec2((slot - 10.0).max(0.0), 38.0));
        if pill.width() < 8.0 {
            continue;
        }
        p.rect_filled(pill, 19.0, swipe_color(col, *a));
        // Full: the icon stays by the row, as if the row pulls it along.
        let icon_x = match (full, shown > 0.0) {
            (true, true) => (pill.right() - 24.0).max(pill.center().x),
            (true, false) => (pill.left() + 24.0).min(pill.center().x),
            _ => pill.center().x,
        };
        if pill.width() >= 26.0 {
            icons::paint(p, egui::pos2(icon_x, pill.center().y), 17.0, swipe_icon(*a), egui::Color32::WHITE);
        }
        if slot >= 44.0 {
            p.text(egui::pos2(icon_x, pill.bottom() + 11.0), egui::Align2::CENTER_CENTER, swipe_label(*a, t), egui::FontId::proportional(11.5), col.subtle);
        }
        out.push((pill.expand2(egui::vec2(4.0, 14.0)), *a));
    }
    out
}

impl NightPostApp {
    /// Move the current swipe along: follow the trackpad, notice the fingers
    /// lifting, decide what letting go does, and ease the row to where it
    /// belongs. Returns an action to run now (a full swipe).
    pub(super) fn advance_swipe(&mut self, ctx: &egui::Context, wheel: egui::Vec2, now: Instant, width: f32) -> Option<(AccountId, String, SwipeAction)> {
        let settings = self.list_settings;
        // The swiped conversation has gone (synced away, or acted on).
        if let Some(s) = &self.swipe {
            if !self.rows.iter().any(|r| s.is(&r.t)) {
                self.swipe = None;
            }
        }
        let s = self.swipe.as_mut()?;
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let mut commit = None;
        match s.phase {
            SwipePhase::Trackpad if wheel.x != 0.0 => {
                s.target += wheel.x;
                s.last = now;
            }
            SwipePhase::Trackpad if now.duration_since(s.last) > RELEASE_AFTER => s.phase = SwipePhase::Released,
            // Scrolling the list shuts an open row.
            SwipePhase::Open if wheel.y.abs() > 2.0 && wheel.y.abs() > wheel.x.abs() => s.close(),
            _ => {}
        }
        // A click anywhere but on its buttons shuts an open row, and only
        // does that (see `tapped_shut`).
        if s.phase == SwipePhase::Open && ctx.input(|i| i.pointer.any_click()) {
            let at = ctx.input(|i| i.pointer.interact_pos());
            if !at.is_some_and(|p| s.buttons.iter().any(|r| r.contains(p))) {
                s.close();
                s.tapped_shut = true;
            }
        }
        if matches!(s.phase, SwipePhase::Trackpad | SwipePhase::Drag | SwipePhase::Released) {
            // A side with no buttons doesn't move; no side moves past the row.
            if side_actions(&settings, s.target).is_empty() {
                s.target = 0.0;
            }
            s.target = s.target.clamp(-width * 0.92, width * 0.92);
        }
        if s.phase == SwipePhase::Released {
            let acts = side_actions(&settings, s.target);
            let sign = s.target.signum();
            match release(s.target, acts.len(), width) {
                Release::Full if removes_row(acts[0]) => {
                    s.phase = SwipePhase::Leaving;
                    s.leaving = Some(acts[0]);
                    s.target = sign * width;
                }
                Release::Full => {
                    commit = Some((s.account_id, s.thread_key.clone(), acts[0]));
                    s.close();
                }
                Release::Open => {
                    s.phase = SwipePhase::Open;
                    s.target = sign * open_width(acts.len());
                }
                Release::Close => s.close(),
            }
        }
        let tau = if matches!(s.phase, SwipePhase::Trackpad | SwipePhase::Drag) { FOLLOW } else { SETTLE };
        s.shown = ease(s.shown, s.target, dt, tau);
        // Out of sight (nearly: the last few points of an ease are slow).
        if s.phase == SwipePhase::Leaving && (s.target - s.shown).abs() < width * 0.04 {
            if let Some(a) = s.leaving {
                commit = Some((s.account_id, s.thread_key.clone(), a));
            }
            self.swipe = None;
        } else if s.phase == SwipePhase::Settling && s.shown == 0.0 {
            self.swipe = None;
        }
        if self.swipe.as_ref().is_some_and(|s| s.phase != SwipePhase::Open || s.shown != s.target) {
            ctx.request_repaint();
        }
        commit
    }

    /// One row's part in swiping: start or carry on a swipe from the
    /// trackpad or a mouse drag (an open row picks up where it rests), or
    /// pick the conversation up instead when it's dragged up or down, or
    /// into the sidebar, to drop on a folder. Returns how far to draw the
    /// row moved sideways.
    pub(super) fn swipe_input(&mut self, ui: &egui::Ui, resp: &egui::Response, t: &ThreadSummary, wheel: egui::Vec2, fresh_gesture: bool) -> f32 {
        let mine = self.swipe.as_ref().is_some_and(|s| s.is(t));
        // Another row's swipe that has finished moving can give way.
        let idle = self.swipe.as_ref().is_none_or(|s| matches!(s.phase, SwipePhase::Open | SwipePhase::Settling));
        if resp.contains_pointer() && fresh_gesture && wheel.x.abs() > wheel.y.abs() && wheel.x.abs() > 0.5 {
            match &mut self.swipe {
                Some(s) if mine && idle => {
                    s.phase = SwipePhase::Trackpad;
                    s.target += wheel.x;
                    s.last = Instant::now();
                    s.tapped_shut = false;
                }
                _ if idle && !mine => self.swipe = Some(Swipe::new(t, SwipePhase::Trackpad, wheel.x)),
                _ => {}
            }
        }
        if resp.dragged() && self.dragging.is_none() {
            let dx = resp.drag_delta().x;
            let (origin, at) = ui.ctx().input(|inp| (inp.pointer.press_origin(), inp.pointer.interact_pos()));
            let moved = match (origin, at) {
                (Some(o), Some(p)) => p - o,
                _ => egui::Vec2::ZERO,
            };
            let into_sidebar = self.show_sidebar && at.is_some_and(|p| p.x < self.sidebar_right);
            let pick_up = |s: &mut Self| {
                s.swipe = None;
                let key = (t.account_id, t.thread_key.clone());
                s.dragging = Some(if s.is_marked(key.0, &key.1) { s.marked.clone() } else { vec![key] });
            };
            match &mut self.swipe {
                Some(_) if mine && into_sidebar => pick_up(self),
                Some(s) if mine && s.phase != SwipePhase::Leaving => {
                    s.phase = SwipePhase::Drag;
                    s.target += dx;
                    s.tapped_shut = false;
                }
                _ if idle && !mine && moved.length() >= 6.0 && moved.y.abs() > moved.x.abs() => pick_up(self),
                _ if idle && !mine && moved.length() >= 6.0 => self.swipe = Some(Swipe::new(t, SwipePhase::Drag, moved.x)),
                _ => {}
            }
        }
        if let Some(s) = self.swipe.as_mut().filter(|s| s.is(t) && s.phase == SwipePhase::Drag) {
            if resp.drag_stopped() {
                s.phase = SwipePhase::Released;
            }
        }
        self.swipe.as_ref().filter(|s| s.is(t)).map(|s| s.shown).unwrap_or(0.0)
    }

    /// Draw what a swiped row uncovers and, on an open row, make its buttons
    /// clickable. Returns the action of a button clicked; a button that
    /// takes the conversation away slides the row out first and acts after.
    pub(super) fn swipe_under(&mut self, ui: &egui::Ui, p: &egui::Painter, col: &Colors, rect: egui::Rect, shown: f32, t: &ThreadSummary) -> Option<SwipeAction> {
        let acts = side_actions(&self.list_settings, shown);
        let s = self.swipe.as_mut()?;
        let full = s.phase == SwipePhase::Leaving || shown.abs() >= full_at(rect.width(), acts.len());
        let buttons = paint_under(p, col, rect, shown, &acts, full, t);
        s.buttons = buttons.iter().map(|(r, _)| *r).collect();
        if s.phase != SwipePhase::Open {
            return None;
        }
        let id = ui.id().with(("swipe-button", t.account_id, &t.thread_key));
        let (_, act) = buttons.into_iter().enumerate().find(|(i, (r, _))| ui.interact(*r, id.with(i), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()).map(|(_, b)| b)?;
        if removes_row(act) {
            s.phase = SwipePhase::Leaving;
            s.leaving = Some(act);
            s.target = shown.signum() * rect.width();
            None
        } else {
            s.close();
            Some(act)
        }
    }

    /// The row's right-click menu, opened by a swipe's More button where it
    /// was clicked. Returns what was picked in it.
    pub(super) fn more_menu(&mut self, ui: &egui::Ui, resp: &egui::Response, t: &ThreadSummary, all_tabs: &[(u8, String)]) -> Option<RowPick> {
        let at = self.row_menu_for.as_ref().filter(|(a, k, _)| *a == t.account_id && *k == t.thread_key)?.2;
        let shown = egui::Popup::new(MORE_MENU.into(), ui.ctx().clone(), egui::PopupAnchor::Position(at), resp.layer_id)
            .kind(egui::PopupKind::Menu)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            // Look like the right-click menu it stands in for.
            .style(egui::containers::menu::menu_style)
            .open_memory(None)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.row_menu(ui, t, all_tabs));
        match shown.map(|r| r.inner) {
            None => {
                self.row_menu_for = None;
                None
            }
            Some(Some(pick)) => {
                egui::Popup::close_id(ui.ctx(), MORE_MENU.into());
                Some(pick)
            }
            Some(None) => None,
        }
    }

    /// Run a swipe or quick action on any conversation, not just the open one.
    pub(super) fn thread_action(&mut self, ctx: &egui::Context, a: AccountId, k: &str, act: SwipeAction) {
        let row = self.rows.iter().position(|r| r.t.account_id == a && r.t.thread_key == k);
        match act {
            SwipeAction::Nothing => {}
            SwipeAction::Archive => self.move_thread(a, k, FolderRole::Archive),
            SwipeAction::Trash => self.move_thread(a, k, FolderRole::Trash),
            SwipeAction::Spam => self.move_thread(a, k, FolderRole::Spam),
            SwipeAction::Pin => self.toggle_pin(a, k),
            SwipeAction::Star => self.star_thread(a, k),
            SwipeAction::ToggleRead => {
                let unread = row.and_then(|i| self.rows.get(i)).is_some_and(|r| r.t.unread);
                self.set_row_unread(a, k, !unread);
                self.did(if unread { "Marked as read" } else { "Marked as unread" }, Undo::Unread { items: vec![(a, k.to_string(), unread)] });
                self.reload();
            }
            // These need the conversation open: open it, then pop the
            // toolbar's menu (as `b` and `v` do) or start the reply.
            SwipeAction::Snooze | SwipeAction::Move | SwipeAction::Reply => {
                let Some(i) = row else { return };
                self.select(i);
                match act {
                    SwipeAction::Snooze => ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("snooze_open"), true)),
                    SwipeAction::Move => ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("picker_open_move"), true)),
                    _ => self.compose_reply(0),
                }
            }
            // The row's right-click menu, opened where the pointer is when
            // the list draws that row next.
            SwipeAction::More => {
                let at = ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default();
                self.row_menu_for = Some((a, k.to_string(), at));
                egui::Popup::open_id(ctx, MORE_MENU.into());
            }
        }
    }

    /// Star or unstar one conversation, with undo.
    fn star_thread(&mut self, a: AccountId, k: &str) {
        let was = self.rows.iter().find(|r| r.t.account_id == a && r.t.thread_key == k).is_some_and(|r| r.t.starred);
        let _ = self.store.set_thread_starred(a, k, !was);
        self.worker.send(Cmd::SetStarred(a, k.to_string(), !was));
        self.did(if was { "Unstarred" } else { "Starred" }, Undo::Star { a, k: k.to_string(), was });
        self.reload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letting_go_closes_opens_or_acts() {
        let w = 500.0;
        assert_eq!(release(20.0, 3, w), Release::Close, "a nudge springs back");
        assert_eq!(release(-120.0, 3, w), Release::Open, "a short swipe shows the buttons");
        assert_eq!(release(-(full_at(w, 3) + 1.0), 3, w), Release::Full);
        assert_eq!(release(400.0, 0, w), Release::Close, "a side that's off never acts");
        // A full swipe always needs more than the open buttons take.
        assert!(full_at(300.0, 3) > open_width(3));
    }

    /// The jerkiness fix: the same time passing moves the row the same
    /// distance however many frames it's split into.
    #[test]
    fn easing_follows_time_not_frames() {
        let one = ease(200.0, 0.0, 1.0 / 60.0, SETTLE);
        let two = ease(ease(200.0, 0.0, 1.0 / 120.0, SETTLE), 0.0, 1.0 / 120.0, SETTLE);
        assert!((one - two).abs() < 0.01, "{one} vs {two}");
        assert!(one < 200.0 && one > 0.0);
        assert_eq!(ease(0.3, 0.0, 0.001, SETTLE), 0.0, "snaps when nearly there");
        assert_eq!(ease(50.0, 0.0, 1.0, SETTLE), 0.0, "a long pause lands");
    }

    #[test]
    fn sides_and_their_buttons() {
        let mut s = ListSettings::default();
        assert_eq!(side_actions(&s, -10.0), [SwipeAction::Trash, SwipeAction::Snooze, SwipeAction::More]);
        assert_eq!(side_actions(&s, 10.0), [SwipeAction::Archive, SwipeAction::ToggleRead]);
        s.swipe_left[0] = SwipeAction::Nothing;
        assert!(side_actions(&s, -10.0).is_empty(), "Nothing first turns the side off");
    }

    #[test]
    fn settings_keep_older_copies_working() {
        let d = ListSettings::default();
        let back = ListSettings::decode(&d.encode());
        assert_eq!(back, d);
        // Saved by an older build: each side's one action, nothing else.
        let old = ListSettings::decode("archive,trash,1,0,10");
        assert_eq!(old.swipe_left, [SwipeAction::Archive, SwipeAction::Snooze, SwipeAction::More], "the default extras fill in");
        assert_eq!(old.swipe_right, [SwipeAction::Trash, SwipeAction::ToggleRead, SwipeAction::Nothing]);
        let old = ListSettings::decode("snooze,archive,1,0,10");
        assert_eq!(old.swipe_left, [SwipeAction::Snooze, SwipeAction::More, SwipeAction::Nothing], "without repeating the first");
        // An older build reading ours sees the full-swipe actions first.
        let ours = ListSettings { swipe_left: [SwipeAction::Snooze, SwipeAction::Move, SwipeAction::Nothing], ..d }.encode();
        assert!(ours.starts_with("snooze,archive,"), "{ours}");
        assert_eq!(ListSettings::decode(&ours).swipe_left, [SwipeAction::Snooze, SwipeAction::Move, SwipeAction::Nothing]);
    }
}
