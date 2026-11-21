//! Momentum scrolling for touchpads (Linux only).
//!
//! On a Mac the system adds momentum itself: after a flick it keeps sending
//! scroll events that slow down, so egui scrolls on. On Wayland that's the
//! app's job, and egui doesn't do it, so a list stopped dead the moment your
//! fingers left the touchpad.
//!
//! When fingers lift, the compositor sends a scroll "stop", which winit and
//! egui pass on as a touchpad scroll of zero. [`Momentum`] keeps the last
//! tenth of a second of touchpad scrolling, and on that zero works out how
//! fast you were going. It then adds a scroll event to each frame, slowing
//! as it goes, until it all but stops. Touching the touchpad again, a mouse
//! wheel, a click or a key press stops it at once.
//!
//! Only up and down coast: sideways scrolling swipes rows in the list
//! (`swipe.rs`), and a coasting swipe would run an action nobody chose.

use eframe::egui;
use std::collections::VecDeque;

/// How much recent scrolling sets the speed at lift, in seconds.
const WINDOW: f64 = 0.1;
/// Fingers held still this long before lifting means "stop here", no coast.
const STILL: f64 = 0.05;
/// Slower than this at lift (points a second) doesn't coast: a slow,
/// careful scroll should end where it was left.
const MIN_SPEED: f32 = 150.0;
/// A cap, so one wild flick can't fly through thousands of messages.
const MAX_SPEED: f32 = 6000.0;
/// How quickly coasting slows (per second). It travels speed ÷ this, about
/// 0.4 s of the lift speed, close to how the Mac feels.
const FRICTION: f32 = 2.5;
/// Coasting ends below this speed (points a second).
const STOP_SPEED: f32 = 20.0;
/// The time one touchpad event stands for, when only one frame was seen.
const FRAME: f64 = 1.0 / 60.0;

/// Touchpad momentum, fed every frame's input (see the module docs).
#[derive(Default)]
pub(super) struct Momentum {
    /// Recent touchpad scrolling: (time, sideways, up and down), in points.
    recent: VecDeque<(f64, f32, f32)>,
    /// Coasting: the speed (points a second) and when it was last applied.
    coast: Option<(f32, f64)>,
}

impl Momentum {
    /// Looks at a frame's input, before egui does, and adds the coasting
    /// scroll to it. Returns true while coasting, so the caller asks for
    /// another frame.
    pub(super) fn process(&mut self, raw: &mut egui::RawInput) -> bool {
        // eframe always sets the time; without it there's no speed to judge.
        let Some(now) = raw.time else { return false };
        let mut lifted = false;
        for e in &raw.events {
            match e {
                egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. } => {
                    self.coast = None;
                    if *delta == egui::Vec2::ZERO {
                        lifted = true;
                    } else {
                        lifted = false;
                        self.recent.push_back((now, delta.x, delta.y));
                    }
                }
                // A mouse wheel, a click, a key or the pointer leaving: stop.
                egui::Event::MouseWheel { .. } | egui::Event::PointerButton { pressed: true, .. } | egui::Event::Key { pressed: true, .. } | egui::Event::PointerGone => {
                    self.coast = None;
                    self.recent.clear();
                }
                _ => {}
            }
        }
        while self.recent.front().is_some_and(|(t, ..)| *t < now - WINDOW) {
            self.recent.pop_front();
        }
        if lifted {
            self.coast = self.speed_at_lift(now).map(|v| (v, now));
            self.recent.clear();
        } else if let Some((v, last)) = self.coast {
            // Capped, so a stalled frame can't jump the list a long way.
            let dt = (now - last).clamp(0.0, 0.1) as f32;
            if dt > 0.0 {
                raw.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, v * dt), modifiers: raw.modifiers });
            }
            let v = v * (-FRICTION * dt).exp();
            self.coast = (v.abs() >= STOP_SPEED).then_some((v, now));
        }
        self.coast.is_some()
    }

    /// How fast fingers were scrolling up or down as they lifted, if that's
    /// a flick worth carrying on.
    fn speed_at_lift(&self, now: f64) -> Option<f32> {
        let (first, last) = (self.recent.front()?, self.recent.back()?);
        if last.0 < now - STILL {
            return None;
        }
        let (dx, dy) = self.recent.iter().fold((0.0, 0.0), |(x, y), (_, dx, dy)| (x + dx, y + dy));
        if dx.abs() >= dy.abs() {
            return None;
        }
        let v = dy / (now - first.0 + FRAME) as f32;
        (v.abs() >= MIN_SPEED).then(|| v.clamp(-MAX_SPEED, MAX_SPEED))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(unit: egui::MouseWheelUnit, x: f32, y: f32) -> egui::Event {
        egui::Event::MouseWheel { unit, delta: egui::vec2(x, y), modifiers: egui::Modifiers::NONE }
    }
    fn pad(x: f32, y: f32) -> egui::Event {
        wheel(egui::MouseWheelUnit::Point, x, y)
    }

    /// Runs one frame at `t` seconds; returns the scroll momentum added.
    fn frame(m: &mut Momentum, t: f64, events: Vec<egui::Event>) -> f32 {
        let given = events.len();
        let mut raw = egui::RawInput { time: Some(t), events, ..Default::default() };
        m.process(&mut raw);
        raw.events[given..].iter().map(|e| if let egui::Event::MouseWheel { delta, .. } = e { delta.y } else { 0.0 }).sum()
    }

    /// Scrolls down at 20 points a frame for five frames, ending at `t`.
    fn flick(m: &mut Momentum, t: f64) {
        for i in 0..5 {
            frame(m, t - FRAME * f64::from(4 - i), vec![pad(0.0, 20.0)]);
        }
    }

    /// Frames after the lift, until coasting ends: (total added, frames).
    fn coast(m: &mut Momentum, from: f64) -> (f32, usize) {
        let (mut total, mut frames) = (0.0, 0);
        while frames < 600 {
            let added = frame(m, from + FRAME * (frames + 1) as f64, vec![]);
            if added == 0.0 {
                break;
            }
            total += added;
            frames += 1;
        }
        (total, frames)
    }

    #[test]
    fn a_flick_carries_on_slows_and_stops() {
        let mut m = Momentum::default();
        flick(&mut m, 1.0);
        assert_eq!(frame(&mut m, 1.0, vec![pad(0.0, 0.0)]), 0.0, "nothing extra on the lift itself");
        let first = frame(&mut m, 1.0 + FRAME, vec![]);
        assert!(first > 0.0, "carries on the way it was going");
        let (rest, frames) = coast(&mut m, 1.0 + FRAME);
        assert!(frames > 10 && frames < 300, "coasts a while, then stops ({frames} frames)");
        // About 1,000 points a second at lift, so about 400 points of coast.
        let total = first + rest;
        assert!((250.0..550.0).contains(&total), "coasted {total} points");
        assert_eq!(frame(&mut m, 9.0, vec![]), 0.0, "and stays stopped");
    }

    #[test]
    fn scrolling_up_coasts_up() {
        let mut m = Momentum::default();
        for i in 0..5 {
            frame(&mut m, 1.0 - FRAME * f64::from(4 - i), vec![pad(0.0, -20.0)]);
        }
        frame(&mut m, 1.0, vec![pad(0.0, 0.0)]);
        assert!(frame(&mut m, 1.0 + FRAME, vec![]) < 0.0);
    }

    #[test]
    fn no_coast_after_a_pause_or_a_slow_scroll() {
        let mut m = Momentum::default();
        flick(&mut m, 1.0);
        frame(&mut m, 1.2, vec![pad(0.0, 0.0)]);
        assert_eq!(coast(&mut m, 1.2).1, 0, "fingers held still, then lifted");

        for i in 0..5 {
            frame(&mut m, 2.0 - FRAME * f64::from(4 - i), vec![pad(0.0, 1.0)]);
        }
        frame(&mut m, 2.0, vec![pad(0.0, 0.0)]);
        assert_eq!(coast(&mut m, 2.0).1, 0, "a slow, careful scroll ends where it was left");
    }

    #[test]
    fn touching_again_or_clicking_stops_it() {
        for stop in [pad(0.0, 3.0), wheel(egui::MouseWheelUnit::Line, 0.0, 1.0), egui::Event::PointerGone, egui::Event::PointerButton { pos: egui::Pos2::ZERO, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }] {
            let mut m = Momentum::default();
            flick(&mut m, 1.0);
            frame(&mut m, 1.0, vec![pad(0.0, 0.0)]);
            assert!(frame(&mut m, 1.0 + FRAME, vec![]) > 0.0);
            frame(&mut m, 1.0 + 2.0 * FRAME, vec![stop.clone()]);
            assert_eq!(frame(&mut m, 1.0 + 3.0 * FRAME, vec![]), 0.0, "stopped by {stop:?}");
        }
    }

    #[test]
    fn mouse_wheels_and_sideways_swipes_never_coast() {
        let mut m = Momentum::default();
        for i in 0..5 {
            frame(&mut m, 1.0 - FRAME * f64::from(4 - i), vec![wheel(egui::MouseWheelUnit::Line, 0.0, 3.0)]);
        }
        assert_eq!(coast(&mut m, 1.0).1, 0, "a mouse wheel has no lift");

        for i in 0..5 {
            frame(&mut m, 2.0 - FRAME * f64::from(4 - i), vec![pad(30.0, 5.0)]);
        }
        frame(&mut m, 2.0, vec![pad(0.0, 0.0)]);
        assert_eq!(coast(&mut m, 2.0).1, 0, "a sideways swipe on a row mustn't carry on");
    }
}
