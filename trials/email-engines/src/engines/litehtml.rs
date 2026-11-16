//! litehtml, the small C++ HTML and CSS engine, through the `litehtml`
//! crate's CPU renderer (`pixbuf`: tiny-skia for shapes, cosmic-text for
//! text). No JavaScript at all.
//!
//! The crate's `PixbufContainer` does the drawing. It's wrapped in
//! [`Shared`], which forwards every call to it but answers picture loads
//! itself, through our [`Loader`], the moment layout asks: so every load is
//! logged and nothing else fetches. The container (and its fonts) is kept
//! between emails, as the app would; a picture it remembers from an earlier
//! email is only drawn if this page's loader allowed it.
//!
//! Before parsing, the page goes through the crate's own clean-up for
//! email (`preprocess_attrs`: old attributes such as `bgcolor` to CSS;
//! `sanitize_html`: scripts, forms and event handlers removed), as the app
//! would with this engine. litehtml's standard stylesheet is used, not the
//! crate's email one, which zeroes paragraph margins unlike any browser.
//!
//! litehtml can't be interrupted, so `opts.timeout` isn't enforced inside a
//! drawing; the crash check runs risky emails in a child process instead.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use litehtml::pixbuf::PixbufContainer;
use litehtml::selection::Selection;
use litehtml::{
    BackgroundLayer, BorderRadiuses, Borders, Color, ConicGradient, DocumentContainer, DrawContext, FontDescription, FontHandle, FontMetrics,
    LinearGradient, ListMarker, MediaFeatures, Position, RadialGradient, Size, TextTransform,
};

use crate::engine::{over_white, Engine, Page, Rect, RenderOpts, Rendered};
use crate::loader::Loader;

/// The tallest picture taken, in points (as for webkit-mac).
const MAX_HEIGHT_PT: f32 = 12_000.0;
/// The viewport height while laying out (media queries see this).
const LAYOUT_HEIGHT_PT: u32 = 800;

/// The container, shared between the document (which holds `&mut` to the
/// wrapper while it's alive) and us (between document calls).
struct Shared {
    inner: Rc<RefCell<PixbufContainer>>,
    /// This page's loader, and the pictures it allowed.
    page: Rc<RefCell<PageState>>,
}

#[derive(Default)]
struct PageState {
    loader: Option<Loader>,
    allowed: HashSet<String>,
}

impl Shared {
    fn allowed(&self, url: &str) -> bool {
        self.page.borrow().allowed.contains(url)
    }
}

impl DocumentContainer for Shared {
    fn create_font(&mut self, descr: &FontDescription) -> (FontHandle, FontMetrics) {
        self.inner.borrow_mut().create_font(descr)
    }
    fn delete_font(&mut self, font: FontHandle) {
        self.inner.borrow_mut().delete_font(font)
    }
    fn text_width(&self, text: &str, font: FontHandle) -> f32 {
        self.inner.borrow().text_width(text, font)
    }
    fn draw_text(&mut self, hdc: DrawContext, text: &str, font: FontHandle, color: Color, pos: Position) {
        self.inner.borrow_mut().draw_text(hdc, text, font, color, pos)
    }
    fn draw_list_marker(&mut self, hdc: DrawContext, marker: &ListMarker) {
        self.inner.borrow_mut().draw_list_marker(hdc, marker)
    }
    /// Answer the load now, through the loader. Asked once per page, so the
    /// log shows every picture each email wants.
    fn load_image(&mut self, src: &str, _baseurl: &str, _redraw_on_ready: bool) {
        let mut page = self.page.borrow_mut();
        if src.is_empty() || page.allowed.contains(src) {
            return;
        }
        let Some(found) = page.loader.as_ref().and_then(|l| l.fetch(src)) else { return };
        // Decoding is litehtml's job: it skips what it can't read.
        self.inner.borrow_mut().load_image_data(src, &found.bytes);
        page.allowed.insert(src.to_string());
    }
    fn get_image_size(&self, src: &str, baseurl: &str) -> Size {
        if self.allowed(src) {
            self.inner.borrow().get_image_size(src, baseurl)
        } else {
            Size::default()
        }
    }
    fn draw_image(&mut self, hdc: DrawContext, layer: &BackgroundLayer, url: &str, base_url: &str) {
        if self.allowed(url) {
            self.inner.borrow_mut().draw_image(hdc, layer, url, base_url)
        }
    }
    fn draw_solid_fill(&mut self, hdc: DrawContext, layer: &BackgroundLayer, color: Color) {
        self.inner.borrow_mut().draw_solid_fill(hdc, layer, color)
    }
    fn draw_linear_gradient(&mut self, hdc: DrawContext, layer: &BackgroundLayer, gradient: &LinearGradient) {
        self.inner.borrow_mut().draw_linear_gradient(hdc, layer, gradient)
    }
    fn draw_radial_gradient(&mut self, hdc: DrawContext, layer: &BackgroundLayer, gradient: &RadialGradient) {
        self.inner.borrow_mut().draw_radial_gradient(hdc, layer, gradient)
    }
    fn draw_conic_gradient(&mut self, hdc: DrawContext, layer: &BackgroundLayer, gradient: &ConicGradient) {
        self.inner.borrow_mut().draw_conic_gradient(hdc, layer, gradient)
    }
    fn draw_borders(&mut self, hdc: DrawContext, borders: &Borders, draw_pos: Position, root: bool) {
        self.inner.borrow_mut().draw_borders(hdc, borders, draw_pos, root)
    }
    fn set_caption(&mut self, caption: &str) {
        self.inner.borrow_mut().set_caption(caption)
    }
    fn set_base_url(&mut self, base_url: &str) {
        self.inner.borrow_mut().set_base_url(base_url)
    }
    fn on_anchor_click(&mut self, url: &str) {
        self.inner.borrow_mut().on_anchor_click(url)
    }
    fn set_cursor(&mut self, cursor: &str) {
        self.inner.borrow_mut().set_cursor(cursor)
    }
    fn set_clip(&mut self, pos: Position, radius: BorderRadiuses) {
        self.inner.borrow_mut().set_clip(pos, radius)
    }
    fn del_clip(&mut self) {
        self.inner.borrow_mut().del_clip()
    }
    fn get_viewport(&self) -> Position {
        self.inner.borrow().get_viewport()
    }
    fn get_media_features(&self) -> MediaFeatures {
        self.inner.borrow().get_media_features()
    }
    fn transform_text(&self, text: &str, tt: TextTransform) -> String {
        self.inner.borrow().transform_text(text, tt)
    }
    // Style sheets by address (`import_css`) keep the default: none. The
    // sanitiser removes `<link rel=stylesheet>` anyway.
}

/// litehtml with its CPU renderer.
pub struct LiteHtml {
    inner: Option<Rc<RefCell<PixbufContainer>>>,
    page: Rc<RefCell<PageState>>,
    /// The last page drawn (cleaned HTML and width), for the questions asked
    /// after drawing, which lay it out again.
    last: Option<(String, f32)>,
}

impl LiteHtml {
    /// Nothing is loaded until the first drawing (that's the cold start).
    pub fn new() -> Result<Self> {
        Ok(Self { inner: None, page: Rc::new(RefCell::new(PageState::default())), last: None })
    }

    fn container(&mut self, width: u32, height: u32, scale: f32) -> Rc<RefCell<PixbufContainer>> {
        let c = self.inner.get_or_insert_with(|| Rc::new(RefCell::new(PixbufContainer::new_with_scale(width, height, scale))));
        c.borrow_mut().resize_with_scale(width, height, scale);
        c.clone()
    }

    /// Lay out the last page again and ask it something.
    fn with_last<R>(&mut self, f: impl FnOnce(&mut litehtml::Document, &Rc<RefCell<PixbufContainer>>) -> R) -> Option<R> {
        let (html, width) = self.last.clone()?;
        let inner = self.inner.clone()?;
        let mut shared = Shared { inner: inner.clone(), page: self.page.clone() };
        let mut doc = litehtml::Document::from_html(&html, &mut shared, None, None).ok()?;
        let _ = doc.render(width);
        Some(f(&mut doc, &inner))
    }
}

impl Engine for LiteHtml {
    fn name(&self) -> &'static str {
        "litehtml"
    }

    fn render(&mut self, page: &Page, opts: &RenderOpts) -> Result<Rendered> {
        let mut notes = Vec::new();
        let cold = self.inner.is_none();
        let fonts = Instant::now();
        let inner = self.container(opts.width_pt, LAYOUT_HEIGHT_PT, opts.scale);
        if cold {
            notes.push(format!("loading the system's fonts took {:.0} ms (once per app, not counted)", fonts.elapsed().as_secs_f64() * 1000.0));
        }
        let started = Instant::now();
        *self.page.borrow_mut() = PageState { loader: Some(page.loader.clone()), allowed: HashSet::new() };
        let html = litehtml::html::sanitize_html(&litehtml::html::preprocess_attrs(&page.html));
        let w = opts.width_pt as f32;
        let mut shared = Shared { inner: inner.clone(), page: self.page.clone() };
        let full = {
            let mut doc = litehtml::Document::from_html(&html, &mut shared, None, None).map_err(|e| anyhow!("litehtml couldn't parse it: {e}"))?;
            let _ = doc.render(w);
            doc.height()
        };
        let h = if full > MAX_HEIGHT_PT {
            notes.push(format!("cut short at {MAX_HEIGHT_PT} of {full} points"));
            MAX_HEIGHT_PT
        } else {
            full.max(1.0)
        };
        // A pixmap the page's size, then lay out and draw into it.
        inner.borrow_mut().resize_with_scale(opts.width_pt, h.ceil() as u32, opts.scale);
        {
            let mut doc = litehtml::Document::from_html(&html, &mut shared, None, None).map_err(|e| anyhow!("litehtml couldn't parse it: {e}"))?;
            let _ = doc.render(w);
            doc.draw(DrawContext::default(), 0.0, 0.0, Some(Position { x: 0.0, y: 0.0, width: w, height: h }));
        }
        let first_draw = started.elapsed();
        let c = inner.borrow();
        let image = over_white(c.pixels().to_vec(), c.width(), c.height()).ok_or_else(|| anyhow!("pixel buffer the wrong size"))?;
        drop(c);
        self.last = Some((html, w));
        Ok(Rendered { image, height_pt: full, first_draw, requests: page.loader.take_log(), notes })
    }

    fn link_at(&mut self, x: f32, y: f32) -> Option<String> {
        self.with_last(|doc, inner| {
            doc.on_mouse_over(x, y, x, y);
            doc.on_lbutton_down(x, y, x, y);
            doc.on_lbutton_up(x, y, x, y);
            // The document holds the wrapper, not the container itself, so
            // the click can be read back while it's still alive.
            inner.borrow_mut().take_anchor_click()
        })
        .flatten()
    }

    fn text_between(&mut self, from: (f32, f32), to: (f32, f32)) -> Option<String> {
        self.with_last(|doc, inner| {
            let measure = inner.borrow().text_measure_fn();
            let mut sel = Selection::for_document(doc);
            sel.start_at(doc, &measure, from.0, from.1, from.0, from.1);
            sel.extend_to(doc, &measure, to.0, to.1, to.0, to.1);
            sel.selected_text()
        })
        .flatten()
    }

    fn element_box(&mut self, id: &str) -> Option<Rect> {
        self.with_last(|doc, _| {
            let p = doc.root()?.select_one(&format!("#{}", litehtml::css_escape_ident(id)))?.placement();
            Some(Rect { x: p.x, y: p.y, w: p.width, h: p.height })
        })
        .flatten()
    }
}
