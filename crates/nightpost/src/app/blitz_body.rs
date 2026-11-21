//! HTML email drawn by Blitz, on Linux, when Preferences → Appearance →
//! Email engine is set to Blitz (experimental).
//!
//! Blitz (DioxusLabs: Stylo for CSS, Taffy for layout, Parley for text) lays
//! an email out and paints it on the CPU with vello_cpu. The picture is shown
//! as egui textures in the message card. Unlike the WebKitGTK view
//! (`mail_view.rs`, a picture of a hidden window), the text can be selected:
//! a drag sets Blitz's own text selection, which it paints, and Ctrl+C or the
//! right-click menu copies it. Blitz runs no JavaScript.
//!
//! Every picture, style sheet or font an email asks for comes through
//! `blitz_net::Net`: pictures written into the email, and web content only
//! when it's allowed for the conversation, fetched off the UI thread.
//!
//! Each body is laid out once per width, scale, theme and remote setting,
//! kept while its conversation is open, laid out again when a fetched
//! picture arrives, and repainted when that or its selection changes.
//! Blitz is a pre-release, so a panic inside it is caught: that email falls
//! back to its plain text, and the app carries on.

use crate::style::Colors;
use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{local_name, BaseDocument, DocumentConfig, FontContext};
use blitz_html::HtmlDocument;
use super::blitz_net::{Fetch, Net, Pool};
use blitz_traits::net::NetProvider;
use blitz_traits::shell::{ColorScheme, Viewport};
use eframe::egui;
use nightpost_core::{Message, MessageId};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The tallest email drawn, in points. Longer ones are cut, with a note.
const MAX_HEIGHT: f32 = 20_000.0;
/// At most this many pixels in one painted email (64 MB of RGBA), so a long
/// email on a HiDPI screen can't take hundreds of megabytes.
const MAX_PIXELS: f32 = 16_000_000.0;
/// The short viewport an email is first laid out in, to find its height.
const MEASURE_HEIGHT: f32 = 40.0;
/// Layout passes while resources arrive. Loads are answered at once, so two
/// or three settle it; more would mean something keeps changing.
const MAX_PASSES: usize = 6;
/// Space around an email shown on white "paper" in a dark theme.
const PAPER_PAD: f32 = 12.0;

/// What the user did in an email that the app acts on.
pub(super) enum Click {
    /// A link: open it in the browser.
    Link(String),
    /// "Load images" for this conversation.
    LoadImages,
}

/// What a body's picture was made for: change any of them and it's redrawn.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    width_px: u32,
    scale_bits: u32,
    dark: bool,
    /// Web pictures allowed (laid out again with them when this changes).
    remote: bool,
}

/// One email, laid out, with its picture.
struct Body {
    key: Key,
    doc: BaseDocument,
    /// Texture, top and height, in points within the email.
    tiles: Vec<(egui::TextureHandle, f32, f32)>,
    /// The email's size in points (after any cut).
    size: egui::Vec2,
    /// It was taller than [`MAX_HEIGHT`] and is cut short.
    cut: bool,
    /// It asks for content from the web.
    remote: bool,
    /// The selection changed, so the picture needs painting again.
    dirty: bool,
    /// Where the email was drawn last frame, on screen.
    shown_at: egui::Rect,
}

/// Every HTML email in the open conversation drawn by Blitz.
pub(super) struct Bodies {
    /// How web content is fetched (`email_view::fetch`, or a test's stand-in).
    fetch: Fetch,
    /// The fetcher threads, started the first time web content is allowed.
    pool: Option<Arc<Pool>>,
    /// Fetched resources handed to Blitz so far, and how many were laid out.
    arrived: Arc<AtomicU64>,
    seen: u64,
    /// The system's fonts, found once (the slow part of the first email).
    fonts: Option<FontContext>,
    bodies: HashMap<MessageId, Body>,
    /// Emails Blitz couldn't draw: shown as text from then on.
    failed: Vec<MessageId>,
    /// Clicks for the app to act on, collected with [`Bodies::take_clicks`].
    clicks: Vec<Click>,
}

impl Bodies {
    pub(super) fn new() -> Self {
        Self::with_fetch(Arc::new(|url: &str| nightpost_core::email_view::fetch(url)))
    }

    fn with_fetch(fetch: Fetch) -> Self {
        Self { fetch, pool: None, arrived: Arc::default(), seen: 0, fonts: None, bodies: HashMap::new(), failed: Vec::new(), clicks: Vec::new() }
    }

    /// Forget emails that aren't in the open conversation any more.
    pub(super) fn keep_only(&mut self, messages: &[Message]) {
        self.bodies.retain(|id, _| messages.iter().any(|m| m.id == *id));
    }

    /// Clicks since last asked: links to open, "Load images".
    pub(super) fn take_clicks(&mut self) -> Vec<Click> {
        std::mem::take(&mut self.clicks)
    }

    /// Draw `m`'s HTML body at the full width available, with web content
    /// if `remote`. False when it has none or Blitz couldn't draw it: the
    /// caller shows the text.
    pub(super) fn show(&mut self, ui: &mut egui::Ui, col: &Colors, m: &Message, remote: bool) -> bool {
        self.take_arrivals();
        let Some(html) = m.body_html.as_deref().filter(|h| !h.trim().is_empty()) else { return false };
        if self.failed.contains(&m.id) {
            return false;
        }
        let paper = col.p.dark;
        let pad = if paper { PAPER_PAD } else { 0.0 };
        let width = (ui.available_width() - 2.0 * pad).max(40.0);
        let scale = ui.ctx().pixels_per_point();
        let key = Key { width_px: (width * scale).round() as u32, scale_bits: scale.to_bits(), dark: col.p.dark, remote };
        if self.bodies.get(&m.id).is_none_or(|b| b.key != key) {
            match self.build(ui.ctx(), m.id, html, width, scale, key) {
                Some(b) => {
                    self.bodies.insert(m.id, b);
                }
                None => {
                    self.failed.push(m.id);
                    return false;
                }
            }
        }
        match self.interact(ui, col, m.id, pad, paper) {
            Some(click) => {
                self.clicks.extend(click);
                true
            }
            None => {
                self.bodies.remove(&m.id);
                self.failed.push(m.id);
                false
            }
        }
    }

    /// Fetched pictures have arrived: lay the emails out again, so they grow
    /// to fit them, and paint them again.
    fn take_arrivals(&mut self) {
        let now = self.arrived.load(Ordering::SeqCst);
        if now == self.seen {
            return;
        }
        self.seen = now;
        let mut broken = Vec::new();
        for (id, b) in &mut self.bodies {
            if !relayout(b) {
                broken.push(*id);
            }
        }
        for id in broken {
            log::warn!("Blitz couldn't lay out message {id} again; showing its text");
            self.bodies.remove(&id);
            self.failed.push(id);
        }
    }

    /// Lay out and paint a new body, or `None` if Blitz failed.
    fn build(&mut self, ctx: &egui::Context, id: MessageId, html: &str, width: f32, scale: f32, key: Key) -> Option<Body> {
        let (clean, _trackers) = nightpost_core::html_clean::clean(html);
        let remote = nightpost_core::html_clean::has_remote_content(&clean);
        let page = page_html(&clean);
        let fonts = self.fonts.get_or_insert_with(FontContext::default).clone();
        let pool = if key.remote { Some(self.pool.get_or_insert_with(|| Arc::new(Pool::new(self.fetch.clone()))).clone()) } else { None };
        let net = Arc::new(Net { remote: pool, arrived: self.arrived.clone(), ctx: ctx.clone() });
        // Laid out once, in a short viewport to find its height, then grown
        // to it: a second layout would fetch every web picture twice.
        let laid = catch_unwind(AssertUnwindSafe(|| {
            let mut doc = layout(&page, width, MEASURE_HEIGHT, scale, &net, &fonts);
            let full = doc.root_element().scroll_height().max(1.0);
            let limit = height_limit(width, scale);
            let height = full.min(limit);
            set_height(&mut doc, width, height, scale);
            (doc, height, full > limit)
        }));
        let Ok((doc, height, cut)) = laid else {
            log::warn!("Blitz couldn't lay out message {id}; showing its text");
            return None;
        };
        let mut body = Body { key, doc, tiles: Vec::new(), size: egui::vec2(width, height), cut, remote, dirty: true, shown_at: egui::Rect::NOTHING };
        repaint(ctx, id, &mut body, scale).then_some(body)
    }

    /// Paint the body, handle the pointer and Copy, and return what was
    /// clicked. `None` if a repaint failed (the caller shows the text).
    fn interact(&mut self, ui: &mut egui::Ui, col: &Colors, id: MessageId, pad: f32, paper: bool) -> Option<Option<Click>> {
        let scale = ui.ctx().pixels_per_point();
        let size = self.bodies.get(&id)?.size;
        let (outer, resp) = ui.allocate_exact_size(size + egui::vec2(2.0 * pad, 2.0 * pad), egui::Sense::click_and_drag());
        let rect = outer.shrink(pad);
        let at = |p: egui::Pos2| (p.x - rect.min.x, p.y - rect.min.y);
        let mut link = None;
        if resp.drag_started() {
            // Start from where the button went down, not where the drag was noticed.
            let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
            self.clear_selections();
            if let (Some(p), Some(b)) = (origin, self.bodies.get_mut(&id)) {
                let (x, y) = at(p);
                if let Some((node, offset)) = b.doc.find_text_position(x, y) {
                    b.doc.set_text_selection(node, offset, node, offset);
                }
            }
        }
        let b = self.bodies.get_mut(&id)?;
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (x, y) = at(p);
                b.dirty |= b.doc.extend_text_selection_to_point(x, y);
            }
        }
        let hovered_link = resp.hover_pos().and_then(|p| link_at(&b.doc, at(p)));
        if hovered_link.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if resp.clicked() {
            link = hovered_link.map(Click::Link);
            if b.doc.has_text_selection() {
                b.doc.clear_text_selection();
                b.dirty = true;
            }
        }
        let selected = b.doc.get_selected_text().filter(|t| !t.is_empty());
        if let Some(text) = &selected {
            if ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
                ui.ctx().copy_text(text.clone());
            }
        }
        resp.context_menu(|ui| {
            if ui.add_enabled(selected.is_some(), egui::Button::new("Copy")).clicked() {
                if let Some(text) = &selected {
                    ui.ctx().copy_text(text.clone());
                }
                ui.close();
            }
        });
        if b.dirty && !repaint(ui.ctx(), id, b, scale) {
            return None;
        }
        b.shown_at = rect;
        if draw(ui, col, b, outer, rect, paper) {
            link = Some(Click::LoadImages);
        }
        Some(link)
    }

    /// Tests: where message `id`'s email was drawn last frame.
    #[cfg(test)]
    pub(super) fn shown_at(&self, id: MessageId) -> Option<egui::Rect> {
        self.bodies.get(&id).map(|b| b.shown_at)
    }

    /// Only one email holds a selection at a time, as in a browser.
    fn clear_selections(&mut self) {
        for b in self.bodies.values_mut() {
            if b.doc.has_text_selection() {
                b.doc.clear_text_selection();
                b.dirty = true;
            }
        }
    }
}

/// The email as a page of its own. The colours match the WebKit view with
/// "Match emails to a dark theme" off: dark text, on white paper in a dark
/// theme. The policy is only for the record: Blitz runs no scripts and gets
/// nothing that [`Net`] doesn't hand it.
fn page_html(clean: &str) -> String {
    let policy = nightpost_core::mail_html::csp("", false);
    nightpost_core::mail_html::email_frame(clean, &policy, "body{color:#1d1d1f;}", false)
}

/// The tallest an email `width` points wide is drawn, in points: at most
/// [`MAX_HEIGHT`] and [`MAX_PIXELS`].
fn height_limit(width: f32, scale: f32) -> f32 {
    MAX_HEIGHT.min(MAX_PIXELS / (width * scale * scale)).max(1.0)
}

/// Give a laid-out email a viewport `height` points tall, so it's painted
/// whole, and lay it out in it.
fn set_height(doc: &mut BaseDocument, width: f32, height: f32, scale: f32) {
    doc.set_viewport(Viewport::new((width * scale).round() as u32, (height * scale).ceil() as u32, scale, ColorScheme::Light));
    doc.resolve(0.0);
}

/// Lay a body out again after pictures arrived, growing it to fit them
/// (it never shrinks: the page fills at least its viewport), and mark it for
/// painting. False if Blitz panicked.
fn relayout(b: &mut Body) -> bool {
    let scale = f32::from_bits(b.key.scale_bits);
    let (width, old) = (b.size.x, b.size.y);
    let grown = catch_unwind(AssertUnwindSafe(|| {
        b.doc.resolve(0.0);
        let full = b.doc.root_element().scroll_height();
        let limit = height_limit(width, scale);
        if full > old + 0.5 && old < limit {
            let height = full.min(limit);
            set_height(&mut b.doc, width, height, scale);
            return Some((height, full > limit));
        }
        None
    }));
    match grown {
        Ok(Some((height, cut))) => {
            b.size.y = height;
            b.cut = cut;
        }
        Ok(None) => {}
        Err(_) => return false,
    }
    b.dirty = true;
    true
}

/// Parse `page` and lay it out in a viewport `width` × `height` points.
fn layout(page: &str, width: f32, height: f32, scale: f32, net: &Arc<Net>, fonts: &FontContext) -> BaseDocument {
    let viewport = Viewport::new((width * scale).round() as u32, (height * scale).ceil() as u32, scale, ColorScheme::Light);
    let config = DocumentConfig { viewport: Some(viewport), net_provider: Some(net.clone() as Arc<dyn NetProvider>), font_ctx: Some(fonts.clone()), ..Default::default() };
    let mut doc = HtmlDocument::from_html(page, config).into_inner();
    for _ in 0..MAX_PASSES {
        doc.resolve(0.0);
        if !doc.has_changes() && !doc.has_pending_critical_resources() {
            break;
        }
    }
    doc
}

/// Paint `body` and cut the picture into texture-sized tiles. False if
/// Blitz panicked or handed back the wrong number of pixels.
fn repaint(ctx: &egui::Context, id: MessageId, body: &mut Body, scale: f32) -> bool {
    let (w, h) = ((body.size.x * scale).round().max(1.0) as u32, (body.size.y * scale).ceil().max(1.0) as u32);
    let doc = &mut body.doc;
    let painted = catch_unwind(AssertUnwindSafe(|| {
        let mut renderer = VelloCpuImageRenderer::new(w, h);
        let mut buf = Vec::with_capacity(w as usize * h as usize * 4);
        renderer.render_to_vec(|scene| blitz_paint::paint_scene(scene, doc, f64::from(scale), w, h, 0, 0), &mut buf);
        buf
    }));
    let Some(buf) = painted.ok().filter(|b| b.len() == w as usize * h as usize * 4) else {
        log::warn!("Blitz couldn't paint message {id}; showing its text");
        return false;
    };
    body.tiles = tiles(ctx, id, &buf, w as usize, h as usize, scale);
    body.dirty = false;
    true
}

/// The picture as textures no taller than the GPU allows.
fn tiles(ctx: &egui::Context, id: MessageId, rgba: &[u8], w: usize, h: usize, scale: f32) -> Vec<(egui::TextureHandle, f32, f32)> {
    let max = ctx.input(|i| i.max_texture_side).max(1024);
    let mut out = Vec::new();
    let mut top = 0;
    while top < h {
        let rows = (h - top).min(max);
        let Some(px) = rgba.get(top * w * 4..(top + rows) * w * 4) else { break };
        let image = egui::ColorImage::from_rgba_premultiplied([w, rows], px);
        let tex = ctx.load_texture(format!("blitz-{id}-{top}"), image, egui::TextureOptions::LINEAR);
        out.push((tex, top as f32 / scale, rows as f32 / scale));
        top += rows;
    }
    out
}

/// Paint the tiles (on white paper in a dark theme), and the notes below.
/// True if "Load images" was clicked.
fn draw(ui: &mut egui::Ui, col: &Colors, b: &Body, outer: egui::Rect, rect: egui::Rect, paper: bool) -> bool {
    let painter = ui.painter_at(outer);
    if paper {
        painter.rect_filled(outer, 6.0, egui::Color32::WHITE);
    }
    for (tex, top, height) in &b.tiles {
        let r = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, *top), egui::vec2(b.size.x, *height));
        if ui.is_rect_visible(r) {
            painter.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
    }
    let note = |ui: &mut egui::Ui, text: &str| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(text).small().color(col.very_subtle));
    };
    if b.cut {
        note(ui, "This email is very long, so the end isn't shown here.");
    }
    // Web content waits for "Load images", as in the WebKit view.
    if b.remote && !b.key.remote {
        ui.add_space(6.0);
        return ui
            .horizontal(|ui| {
                ui.label(egui::RichText::new("Pictures from the web are hidden to protect your privacy.").small().color(col.very_subtle));
                ui.link(egui::RichText::new("Load images").small()).clicked()
            })
            .inner;
    }
    false
}

/// The address of the link at `(x, y)` (points within the email), if any.
fn link_at(doc: &BaseDocument, (x, y): (f32, f32)) -> Option<String> {
    let mut id = Some(doc.hit(x, y)?.node_id);
    while let Some(node) = id.and_then(|i| doc.get_node(i)) {
        if node.element_data().is_some_and(|e| e.name.local == local_name!("a")) {
            return node.attr(local_name!("href")).map(String::from);
        }
        id = node.parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out and paint `html` at 400 points wide, as `show` would.
    fn body(html: &str) -> (Bodies, Body) {
        build(Bodies::new(), html, false)
    }

    fn build(mut b: Bodies, html: &str, remote: bool) -> (Bodies, Body) {
        let ctx = egui::Context::default();
        let key = Key { width_px: 400, scale_bits: 1.0f32.to_bits(), dark: false, remote };
        let body = b.build(&ctx, 1, html, 400.0, 1.0, key).expect("Blitz draws it");
        (b, body)
    }

    /// A 300 × 200 PNG, as a web server would send it.
    fn photo() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(300, 200, image::Rgba([200, 40, 40, 255])).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).expect("test PNG");
        out
    }

    #[test]
    fn draws_an_email_at_its_height() {
        let (_, b) = body("<p style='margin:0;height:300px'>Hello</p>");
        assert_eq!(b.size.x, 400.0);
        assert!((299.0..=320.0).contains(&b.size.y), "height {}", b.size.y);
        assert!(!b.tiles.is_empty() && !b.cut && !b.remote);
    }

    #[test]
    fn finds_links_and_selects_text() {
        let (_, mut b) = body("<p style='margin:0;font-size:20px'><a href='https://example.com/x'>Example link</a> and more words</p>");
        assert_eq!(link_at(&b.doc, (10.0, 10.0)).as_deref(), Some("https://example.com/x"));
        assert_eq!(link_at(&b.doc, (390.0, 200.0)), None, "below the text");
        let (node, offset) = b.doc.find_text_position(1.0, 10.0).expect("text at the start");
        b.doc.set_text_selection(node, offset, node, offset);
        assert!(b.doc.extend_text_selection_to_point(395.0, 10.0));
        let text = b.doc.get_selected_text().unwrap_or_default();
        assert!(text.starts_with("Example link and more"), "selected {text:?}");
    }

    #[test]
    fn web_pictures_wait_for_load_images() {
        let fetched = Arc::new(AtomicU64::new(0));
        let f = fetched.clone();
        let bodies = Bodies::with_fetch(Arc::new(move |_| {
            f.fetch_add(1, Ordering::SeqCst);
            Ok(photo())
        }));
        let (bodies, b) = build(bodies, "<p>Hi</p><img src='https://pics.example/banner.png'>", false);
        assert!(b.remote && !b.key.remote, "the Load images note shows");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(fetched.load(Ordering::SeqCst), 0, "nothing fetched");
        assert!(bodies.pool.is_none(), "no fetcher threads started");
    }

    #[test]
    fn web_pictures_load_and_the_email_grows_to_fit() {
        let fetched = Arc::new(AtomicU64::new(0));
        let f = fetched.clone();
        let mut bodies = Bodies::with_fetch(Arc::new(move |url: &str| {
            assert_eq!(url, "https://pics.example/banner.png");
            f.fetch_add(1, Ordering::SeqCst);
            // Slower than the layout, as a real server is.
            std::thread::sleep(std::time::Duration::from_millis(300));
            Ok(photo())
        }));
        let key = Key { width_px: 400, scale_bits: 1.0f32.to_bits(), dark: false, remote: true };
        let html = "<p style='margin:0'>Hi</p><img src='https://pics.example/banner.png' style='display:block'>";
        let b = bodies.build(&egui::Context::default(), 1, html, 400.0, 1.0, key).expect("Blitz draws it");
        let before = b.size.y;
        bodies.bodies.insert(1, b);
        for _ in 0..300 {
            if bodies.arrived.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(bodies.arrived.load(Ordering::SeqCst) > 0, "the picture arrived");
        // The next frame lays it out again.
        bodies.take_arrivals();
        let b = bodies.bodies.get(&1).expect("still drawn");
        assert!(b.size.y >= before + 190.0, "grew from {before} to {} to fit the 200-pixel picture", b.size.y);
        assert!(b.dirty, "and will be painted again");
        assert_eq!(fetched.load(Ordering::SeqCst), 1, "fetched once");
    }

    #[test]
    fn cuts_very_long_emails() {
        let (_, b) = body("<div style='height:50000px'>Long</div>");
        assert!(b.cut);
        assert!(b.size.y <= MAX_HEIGHT);
    }
}
