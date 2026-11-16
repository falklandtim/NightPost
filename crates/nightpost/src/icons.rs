//! Thin line icons drawn as vector strokes (so they look the same on every
//! platform and in every theme), and Canary-style capsule button groups.

use crate::style::{blend, Colors};
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Check,
    Refresh,
    Reply,
    ReplyAll,
    Forward,
    Done,
    Snooze,
    Pin,
    PinFilled,
    More,
    Search,
    Sidebar,
    Gear,
    Trash,
    Envelope,
    Folder,
    Tag,
    Paperclip,
    Inbox,
    Star,
    Pencil,
    Send,
    Archive,
    Warning,
}

fn quad(a: Pos2, c: Pos2, b: Pos2, n: usize) -> impl Iterator<Item = Pos2> {
    (1..=n).map(move |i| {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        pos2(u * u * a.x + 2.0 * u * t * c.x + t * t * b.x, u * u * a.y + 2.0 * u * t * c.y + t * t * b.y)
    })
}

/// Paint `icon` centred on `center`, `size` points across.
pub fn paint(p: &egui::Painter, center: Pos2, size: f32, icon: Icon, color: Color32) {
    let h = size / 2.0;
    let pt = |x: f32, y: f32| pos2(center.x + x * h, center.y + y * h);
    let w = (size / 11.0).clamp(1.25, 2.0);
    let st = Stroke::new(w, color);
    let line = |pts: Vec<Pos2>| {
        p.add(Shape::line(pts, st));
    };
    let ring = |r: f32| {
        p.circle_stroke(center, r * h, st);
    };
    // A pushpin, upright, then tilted 45° so the head points up-right.
    let pin = |filled: bool| {
        let (s, c) = (std::f32::consts::FRAC_PI_4.sin(), std::f32::consts::FRAC_PI_4.cos());
        let rot = |x: f32, y: f32| pt((x * c - y * s) * 0.95 + 0.05, (x * s + y * c) * 0.95 + 0.05);
        let head = [(-0.3, -0.9), (0.3, -0.9), (0.2, -0.15), (-0.2, -0.15)];
        let base = [(-0.2, -0.15), (0.2, -0.15), (0.55, 0.22), (-0.55, 0.22)];
        if filled {
            for poly in [head, base] {
                p.add(Shape::convex_polygon(poly.iter().map(|&(x, y)| rot(x, y)).collect(), color, Stroke::NONE));
            }
        }
        let outline = [(-0.3, -0.9), (0.3, -0.9), (0.2, -0.15), (0.55, 0.22), (-0.55, 0.22), (-0.2, -0.15)];
        p.add(Shape::closed_line(outline.iter().map(|&(x, y)| rot(x, y)).collect(), st));
        line(vec![rot(0.0, 0.22), rot(0.0, 0.95)]);
    };
    match icon {
        Icon::Check => line(vec![pt(-0.7, 0.05), pt(-0.2, 0.55), pt(0.75, -0.55)]),
        Icon::Done => {
            ring(0.85);
            line(vec![pt(-0.38, 0.02), pt(-0.1, 0.3), pt(0.4, -0.3)]);
        }
        Icon::Refresh => {
            let r = 0.72;
            let (a0, a1) = (0.0_f32, 290.0_f32.to_radians());
            let pts: Vec<Pos2> = (0..=28).map(|i| a0 + (a1 - a0) * i as f32 / 28.0).map(|a| pt(r * a.cos(), r * a.sin())).collect();
            let tip = *pts.last().unwrap();
            line(pts);
            // Arrowhead along the tangent at the end of the arc.
            let d = vec2(-a1.sin(), a1.cos());
            let back = -d * 0.42 * h;
            let rot = |v: egui::Vec2, deg: f32| {
                let (s, c) = deg.to_radians().sin_cos();
                vec2(v.x * c - v.y * s, v.x * s + v.y * c)
            };
            line(vec![tip + rot(back, 40.0), tip, tip + rot(back, -40.0)]);
        }
        Icon::Reply | Icon::Forward | Icon::ReplyAll => {
            let flip = if icon == Icon::Forward { -1.0 } else { 1.0 };
            let q = |x: f32, y: f32| pt(x * flip, y);
            let shift = if icon == Icon::ReplyAll { 0.22 } else { 0.0 };
            line(vec![q(-0.25 + shift, -0.6), q(-0.78 + shift, -0.08), q(-0.25 + shift, 0.44)]);
            if icon == Icon::ReplyAll {
                line(vec![q(-0.5, -0.6), q(-1.02, -0.08), q(-0.5, 0.44)]);
            }
            let start = q(-0.78 + shift, -0.08);
            let mut body = vec![start, q(0.1 + shift / 2.0, -0.08)];
            body.extend(quad(q(0.1 + shift / 2.0, -0.08), q(0.82, -0.08), q(0.82, 0.72), 10));
            line(body);
        }
        Icon::Snooze => {
            ring(0.85);
            line(vec![pt(0.0, -0.5), pt(0.0, 0.0), pt(0.36, 0.24)]);
        }
        Icon::Pin => pin(false),
        Icon::PinFilled => pin(true),
        Icon::More => {
            ring(0.85);
            for x in [-0.4, 0.0, 0.4] {
                p.circle_filled(pt(x, 0.0), w * 0.8, color);
            }
        }
        Icon::Search => {
            p.circle_stroke(pt(-0.15, -0.15), 0.55 * h, st);
            line(vec![pt(0.25, 0.25), pt(0.75, 0.75)]);
        }
        Icon::Sidebar => {
            p.rect_stroke(Rect::from_min_max(pt(-0.9, -0.72), pt(0.9, 0.72)), 2.5, st, StrokeKind::Middle);
            line(vec![pt(-0.3, -0.72), pt(-0.3, 0.72)]);
        }
        Icon::Trash => {
            line(vec![pt(-0.8, -0.55), pt(0.8, -0.55)]);
            line(vec![pt(-0.28, -0.55), pt(-0.22, -0.85), pt(0.22, -0.85), pt(0.28, -0.55)]);
            line(vec![pt(-0.58, -0.55), pt(-0.46, 0.85), pt(0.46, 0.85), pt(0.58, -0.55)]);
            line(vec![pt(-0.16, -0.22), pt(-0.12, 0.52)]);
            line(vec![pt(0.16, -0.22), pt(0.12, 0.52)]);
        }
        Icon::Envelope => {
            p.rect_stroke(Rect::from_min_max(pt(-0.88, -0.62), pt(0.88, 0.62)), 2.0, st, StrokeKind::Middle);
            line(vec![pt(-0.85, -0.58), pt(0.0, 0.08), pt(0.85, -0.58)]);
        }
        Icon::Folder => {
            line(vec![pt(-0.85, -0.55), pt(-0.85, 0.7), pt(0.85, 0.7), pt(0.85, -0.35), pt(-0.05, -0.35), pt(-0.25, -0.6), pt(-0.85, -0.6), pt(-0.85, -0.55)]);
        }
        Icon::Tag => {
            p.add(Shape::closed_line(vec![pt(-0.85, -0.85), pt(0.0, -0.85), pt(0.85, 0.0), pt(0.0, 0.85), pt(-0.85, 0.0)], st));
            p.circle_filled(pt(-0.42, -0.42), w * 0.9, color);
        }
        Icon::Paperclip => {
            // Wire in two loops: up the middle, over a small bend, down the
            // left, round the bottom and up the right. Tilted like a clip.
            use std::f32::consts::PI;
            let (sn, cs) = (0.45_f32.sin(), 0.45_f32.cos());
            let rot = |(x, y): (f32, f32)| pt(x * cs - y * sn, x * sn + y * cs);
            let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| (0..=10).map(move |i| {
                let a = from + (to - from) * i as f32 / 10.0;
                (cx + r * a.cos(), cy + r * a.sin())
            });
            let mut pts = vec![(0.05, 0.3), (0.05, -0.5)];
            pts.extend(arc(-0.125, -0.5, 0.175, 0.0, -PI)); // small bend at the top
            pts.push((-0.3, 0.5));
            pts.extend(arc(0.05, 0.5, 0.35, PI, 0.0)); // round the bottom
            pts.push((0.4, -0.6));
            line(pts.into_iter().map(rot).collect());
        }
        Icon::Inbox => {
            p.rect_stroke(Rect::from_min_max(pt(-0.85, -0.72), pt(0.85, 0.72)), 2.5, st, StrokeKind::Middle);
            line(vec![pt(-0.85, 0.12), pt(-0.32, 0.12), pt(-0.2, 0.38), pt(0.2, 0.38), pt(0.32, 0.12), pt(0.85, 0.12)]);
        }
        Icon::Star => {
            let pts = (0..10)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    let r = if i % 2 == 0 { 0.92 } else { 0.4 };
                    pt(r * a.cos(), r * a.sin() + 0.08)
                })
                .collect();
            p.add(Shape::closed_line(pts, st));
        }
        Icon::Pencil => {
            p.add(Shape::closed_line(vec![pt(-0.8, 0.8), pt(-0.72, 0.38), pt(0.42, -0.76), pt(0.76, -0.42), pt(-0.38, 0.72)], st));
            line(vec![pt(0.18, -0.52), pt(0.52, -0.18)]);
        }
        Icon::Send => {
            p.add(Shape::closed_line(vec![pt(-0.88, -0.06), pt(0.86, -0.82), pt(0.16, 0.86), pt(-0.06, 0.12)], st));
            line(vec![pt(-0.06, 0.12), pt(0.86, -0.82)]);
        }
        Icon::Archive => {
            p.rect_stroke(Rect::from_min_max(pt(-0.88, -0.78), pt(0.88, -0.36)), 1.5, st, StrokeKind::Middle);
            line(vec![pt(-0.72, -0.36), pt(-0.72, 0.78), pt(0.72, 0.78), pt(0.72, -0.36)]);
            line(vec![pt(-0.24, 0.02), pt(0.24, 0.02)]);
        }
        Icon::Warning => {
            p.add(Shape::closed_line(vec![pt(0.0, -0.82), pt(0.92, 0.76), pt(-0.92, 0.76)], st));
            line(vec![pt(0.0, -0.24), pt(0.0, 0.2)]);
            p.circle_filled(pt(0.0, 0.47), w * 0.75, color);
        }
        Icon::Gear => {
            ring(0.28);
            ring(0.62);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                line(vec![pt(0.62 * a.cos(), 0.62 * a.sin()), pt(0.9 * a.cos(), 0.9 * a.sin())]);
            }
        }
    }
}

/// One button in a [`group`].
pub struct Item {
    pub icon: Icon,
    pub tip: &'static str,
    pub enabled: bool,
    /// Drawn in the accent colour (e.g. the pin of a pinned conversation).
    pub active: bool,
}

impl Item {
    pub fn new(icon: Icon, tip: &'static str) -> Self {
        Self { icon, tip, enabled: true, active: false }
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn active(mut self, on: bool) -> Self {
        self.active = on;
        self
    }
}

pub const HEIGHT: f32 = 32.0;
const SEG: f32 = 32.0;


/// The colours a capsule is drawn with: foreground, the surface it sits on,
/// and the highlight for active items.
#[derive(Clone, Copy)]
pub struct Tone {
    pub fg: Color32,
    pub bg: Color32,
    pub accent: Color32,
}

impl Tone {
    /// Derived from the toolbar colours, so every theme (including Mailspring
    /// ones) gets matching chrome.
    pub fn toolbar(col: &Colors) -> Self {
        Self { fg: col.toolbar_text, bg: col.toolbar, accent: col.accent }
    }
    fn chrome(self) -> (Color32, Color32) {
        (blend(self.fg, self.bg, 0.045), blend(self.fg, self.bg, 0.17))
    }
}

fn paint_button(ui: &egui::Ui, tone: Tone, r: Rect, resp: &Response, it: &Item, icon_size: f32) {
    let p = ui.painter();
    if it.enabled && (resp.hovered() || resp.is_pointer_button_down_on()) {
        let t = if resp.is_pointer_button_down_on() { 0.2 } else { 0.11 };
        p.circle_filled(r.center(), r.height() / 2.0, blend(tone.fg, tone.bg, t));
    }
    let fg = if !it.enabled {
        blend(tone.fg, tone.bg, 0.35)
    } else if it.active {
        tone.accent
    } else {
        tone.fg
    };
    paint(p, r.center(), icon_size, it.icon, fg);
}

/// A row of icon buttons sharing one capsule. Returns one response per item.
pub fn group(ui: &mut egui::Ui, col: &Colors, items: &[Item]) -> Vec<Response> {
    group_with(ui, Tone::toolbar(col), items, SEG, HEIGHT, "pill")
}

/// [`group`] with explicit colours and size; `salt` keeps ids unique.
pub fn group_with(ui: &mut egui::Ui, tone: Tone, items: &[Item], seg: f32, height: f32, salt: impl std::hash::Hash + Copy) -> Vec<Response> {
    let pad = (height / 10.0).round();
    let size = vec2(items.len() as f32 * seg + pad * 2.0, height);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let (fill, stroke) = tone.chrome();
    ui.painter().rect(rect, height / 2.0, fill, Stroke::new(1.0_f32, stroke), StrokeKind::Inside);
    items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let r = Rect::from_min_size(pos2(rect.left() + pad + i as f32 * seg, rect.top() + pad), vec2(seg, height - pad * 2.0));
            let sense = if it.enabled { Sense::click() } else { Sense::hover() };
            let resp = ui.interact(r, ui.make_persistent_id((salt, it.tip)), sense);
            paint_button(ui, tone, r, &resp, it, height * 0.53);
            resp.on_hover_text(it.tip)
        })
        .collect()
}

/// A borderless icon button (used on the sidebar edge).
pub fn bare(ui: &mut egui::Ui, icon: Icon, tip: &'static str, fg: Color32, bg: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, blend(fg, bg, 0.12));
    }
    paint(ui.painter(), rect.center(), 17.0, icon, fg);
    resp.on_hover_text(tip)
}

/// A capsule button with an icon and a label, for use outside the toolbar.
pub fn labeled(ui: &mut egui::Ui, icon: Icon, label: &str, fg: Color32, bg: Color32) -> Response {
    let font = egui::FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let size = vec2(galley.size().x + 50.0, HEIGHT);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let t = if resp.is_pointer_button_down_on() { 0.16 } else if resp.hovered() { 0.09 } else { 0.04 };
    let p = ui.painter();
    p.rect(rect, HEIGHT / 2.0, blend(fg, bg, t), Stroke::new(1.0_f32, blend(fg, bg, 0.17)), StrokeKind::Inside);
    paint(p, pos2(rect.left() + 21.0, rect.center().y), 15.0, icon, fg);
    p.galley(pos2(rect.left() + 36.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    // Name the button for VoiceOver (and UI tests).
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    resp
}

/// A filled capsule with an icon and a label: the toolbar's main action.
pub fn filled(ui: &mut egui::Ui, icon: Icon, label: &str, fill: Color32, fg: Color32) -> Response {
    let font = egui::FontId::proportional(14.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let size = vec2(galley.size().x + 52.0, HEIGHT);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let bg = ui.visuals().panel_fill;
    let fill = if resp.is_pointer_button_down_on() { blend(fill, bg, 0.8) } else if resp.hovered() { blend(fill, bg, 0.9) } else { fill };
    let p = ui.painter();
    p.rect_filled(rect, HEIGHT / 2.0, fill);
    paint(p, pos2(rect.left() + 22.0, rect.center().y), 14.0, icon, fg);
    // Drawn twice, half a point apart, for weight (egui's font has no bold).
    p.galley(pos2(rect.left() + 37.5, rect.center().y - galley.size().y / 2.0), galley.clone(), fg);
    p.galley(pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A capsule-shaped search box with a magnifier.
pub fn search_field(ui: &mut egui::Ui, col: &Colors, text: &mut String, width: f32, hint: &str) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());
    let (fill, stroke) = Tone::toolbar(col).chrome();
    ui.painter().rect(rect, HEIGHT / 2.0, fill, Stroke::new(1.0_f32, stroke), StrokeKind::Inside);
    let icon_c = pos2(rect.left() + 17.0, rect.center().y);
    paint(ui.painter(), icon_c, 13.0, Icon::Search, blend(col.toolbar_text, col.toolbar, 0.6));
    let inner = Rect::from_min_max(pos2(rect.left() + 30.0, rect.top() + 6.0), pos2(rect.right() - 12.0, rect.bottom() - 6.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.add(
        egui::TextEdit::singleline(text)
            .frame(false)
            .hint_text(egui::RichText::new(hint).color(blend(col.toolbar_text, col.toolbar, 0.5)))
            .text_color(col.toolbar_text)
            .desired_width(inner.width())
            .vertical_align(egui::Align::Center),
    )
}
