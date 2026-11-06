//! Turning a resolved Mailspring-style [`Palette`] into egui visuals, plus
//! small drawing helpers shared by the views.

use eframe::egui::{self, Color32, CornerRadius, Stroke};
use owlpost_core::theme::{Palette, Rgba};

pub fn c(rgba: Rgba) -> Color32 {
    let [r, g, b, a] = rgba.to_u8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub fn hex(s: &str) -> Color32 {
    Rgba::hex(s).map(c).unwrap_or(Color32::GRAY)
}

/// Every colour the views need, pre-converted.
#[derive(Clone)]
pub struct Colors {
    pub p: Palette,
    pub accent: Color32,
    pub bg: Color32,
    pub bg_secondary: Color32,
    pub toolbar: Color32,
    pub toolbar_text: Color32,
    pub text: Color32,
    pub subtle: Color32,
    pub very_subtle: Color32,
    pub inverse: Color32,
    pub heading: Color32,
    pub link: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub sidebar: Color32,
    pub sidebar_text: Color32,
    pub sidebar_active: Color32,
    pub sidebar_active_text: Color32,
    pub list: Color32,
    #[allow(dead_code)] // kept for themes; the Canary-style list has no dividers
    pub list_border: Color32,
    pub list_hover: Color32,
    pub list_selected: Color32,
    #[allow(dead_code)]
    pub list_selected_text: Color32,
    pub list_focused: Color32,
    pub list_focused_text: Color32,
    pub unread: Color32,
    pub button_emphasis: Color32,
    pub button_emphasis_text: Color32,
    pub star: Color32,
    pub snooze: Color32,
    pub error: Color32,
    pub success: Color32,
    pub warning: Color32,
}

impl Colors {
    pub fn new(p: Palette) -> Self {
        Self {
            accent: c(p.accent),
            bg: c(p.background),
            bg_secondary: c(p.background_secondary),
            toolbar: c(p.toolbar),
            toolbar_text: c(p.toolbar_text),
            text: c(p.text),
            subtle: c(p.text_subtle),
            very_subtle: c(p.text_very_subtle),
            inverse: c(p.text_inverse),
            heading: c(p.heading),
            link: c(p.link),
            border: c(p.border),
            divider: c(p.divider),
            sidebar: c(p.sidebar),
            sidebar_text: c(p.sidebar_text),
            sidebar_active: c(p.sidebar_active),
            sidebar_active_text: c(p.sidebar_active_text),
            list: c(p.list),
            list_border: c(p.list_border),
            list_hover: c(p.list_hover),
            list_selected: c(p.list_selected),
            list_selected_text: c(p.list_selected_text),
            list_focused: c(p.list_focused),
            list_focused_text: c(p.list_focused_text),
            unread: c(p.unread),
            button_emphasis: c(p.button_emphasis),
            button_emphasis_text: c(p.button_emphasis_text),
            star: c(p.star),
            snooze: c(p.snooze),
            error: c(p.error),
            success: c(p.success),
            warning: c(p.warning),
            p,
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let p = &self.p;
        let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.dark_mode = p.dark;
        v.override_text_color = None;
        v.panel_fill = self.bg;
        v.window_fill = c(p.background_off);
        v.window_stroke = Stroke::new(1.0_f32, self.border);
        v.extreme_bg_color = c(p.input);
        v.faint_bg_color = self.bg_secondary;
        v.code_bg_color = self.bg_secondary;
        v.hyperlink_color = self.link;
        v.warn_fg_color = self.warning;
        v.error_fg_color = self.error;
        v.selection.bg_fill = self.accent.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0_f32, self.accent);
        v.text_cursor.stroke = Stroke::new(2.0_f32, self.accent);
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(6);

        let r = CornerRadius::same(5);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.bg;
        w.noninteractive.weak_bg_fill = self.bg;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.divider);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text);
        w.inactive.bg_fill = c(p.button);
        w.inactive.weak_bg_fill = c(p.button);
        w.inactive.bg_stroke = Stroke::new(1.0_f32, c(p.input_border));
        w.inactive.fg_stroke = Stroke::new(1.0_f32, c(p.button_text));
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = self.list_hover;
        w.hovered.weak_bg_fill = self.list_hover;
        w.hovered.bg_stroke = Stroke::new(1.0_f32, self.accent.gamma_multiply(0.7));
        w.hovered.fg_stroke = Stroke::new(1.5_f32, self.text);
        w.hovered.corner_radius = r;
        // egui uses `active.fg_stroke` as the "strong text" colour too, so the
        // pressed state keeps the theme's text colour on a tinted accent fill.
        let pressed = blend(self.accent, self.bg, 0.3);
        w.active.bg_fill = pressed;
        w.active.weak_bg_fill = pressed;
        w.active.bg_stroke = Stroke::new(1.0_f32, self.accent);
        w.active.fg_stroke = Stroke::new(1.5_f32, c(p.heading));
        w.active.corner_radius = r;
        w.open.bg_fill = self.list_selected;
        w.open.weak_bg_fill = self.list_selected;
        w.open.fg_stroke = Stroke::new(1.0_f32, self.text);
        w.open.corner_radius = r;
        // egui keeps separate light/dark styles and new windows follow the OS
        // setting, so pin the theme and give both styles the same visuals.
        ctx.set_theme(if p.dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
        ctx.set_visuals_of(egui::Theme::Dark, v.clone());
        ctx.set_visuals_of(egui::Theme::Light, v);

        ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(10.0, 5.0);
            s.spacing.interact_size.y = 26.0;
            s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
            s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(13.5));
            s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.5));
            s.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
        });
    }
}

/// `a` over `b` with `t` opacity, as an opaque colour.
pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 * t + y as f32 * (1.0 - t)).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Stable colour per address, for avatars.
pub fn avatar_color(seed: &str) -> Color32 {
    let mut h: u32 = 2166136261;
    for b in seed.to_ascii_lowercase().bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.62, 0.8, 1.0).into()
}

pub fn avatar(ui: &mut egui::Ui, name: &str, email: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint_avatar(ui.painter(), rect.center(), name, email, size);
}

pub fn paint_avatar(painter: &egui::Painter, center: egui::Pos2, name: &str, email: &str, size: f32) {
    painter.circle_filled(center, size / 2.0, avatar_color(email));
    // One letter on a solid circle, like Canary.
    let src = if name.trim().is_empty() { email } else { name };
    let initial: String = src.chars().find(|c| c.is_alphanumeric()).into_iter().flat_map(char::to_uppercase).collect();
    let font = egui::FontId::proportional(size * 0.46);
    painter.text(center, egui::Align2::CENTER_CENTER, &initial, font.clone(), Color32::WHITE);
    painter.text(center + egui::vec2(0.5, 0.0), egui::Align2::CENTER_CENTER, &initial, font, Color32::WHITE);
}

/// Mailspring-style relative timestamps.
pub fn short_date(d: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::{Datelike, Local};
    let l = d.with_timezone(&Local);
    let now = Local::now();
    if l.date_naive() == now.date_naive() {
        l.format("%-I:%M %p").to_string()
    } else if l.year() == now.year() {
        l.format("%-d %b").to_string()
    } else {
        l.format("%-d %b %Y").to_string()
    }
}

pub fn long_date(d: chrono::DateTime<chrono::Utc>) -> String {
    d.with_timezone(&chrono::Local).format("%a, %b %-d, %Y at %-I:%M %p").to_string()
}

/// A single-line, ellipsised text galley.
pub fn line(
    painter: &egui::Painter,
    pos: egui::Pos2,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> f32 {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(1.0));
    let galley = painter.layout_job(job);
    let w = galley.size().x;
    painter.galley(pos, galley, color);
    w
}

/// [`line`] drawn twice, half a point apart: egui's bundled font has no bold.
pub fn bold_line(painter: &egui::Painter, pos: egui::Pos2, text: &str, font: egui::FontId, color: Color32, max_width: f32) -> f32 {
    line(painter, pos + egui::vec2(0.5, 0.0), text, font.clone(), color, max_width - 0.5);
    line(painter, pos, text, font, color, max_width - 0.5) + 0.5
}

/// Wrapped text, cut off with an ellipsis after `max_rows` lines.
pub fn paragraph(painter: &egui::Painter, pos: egui::Pos2, text: &str, font: egui::FontId, color: Color32, max_width: f32, max_rows: usize) {
    let mut job = egui::text::LayoutJob::single_section(text.split_whitespace().collect::<Vec<_>>().join(" "), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping { max_width: max_width.max(1.0), max_rows, break_anywhere: false, overflow_character: Some('…') };
    let galley = painter.layout_job(job);
    painter.galley(pos, galley, color);
}

pub fn primary_button(ui: &mut egui::Ui, col: &Colors, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).color(col.button_emphasis_text).strong())
            .fill(col.button_emphasis)
            .stroke(Stroke::NONE),
    )
}
