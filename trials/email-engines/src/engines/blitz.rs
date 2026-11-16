//! Blitz (DioxusLabs): Stylo for CSS (Firefox's style engine), Taffy for
//! layout, Parley for text, painted on the CPU with vello_cpu through
//! anyrender. No JavaScript at all.
//!
//! Every resource goes through our [`Loader`] by way of a `NetProvider`.
//! A refused load is answered with no bytes, so the resource fails cleanly:
//! Blitz waits for render-blocking resources (style sheets, fonts) before
//! laying out, and a load that never answers would leave the page blank.
//!
//! The font collection is built once and shared between emails, as the app
//! would. Each page is laid out in a short viewport to find its height (as
//! for webkit-mac), then in one that height and painted.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Result};
use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{local_name, BaseDocument, DocumentConfig, FontContext};
use blitz_html::HtmlDocument;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use blitz_traits::shell::{ColorScheme, Viewport};

use crate::engine::{over_white, Engine, Page, Rect, RenderOpts, Rendered};
use crate::loader::Loader;

/// The tallest picture taken, in points (as for webkit-mac).
const MAX_HEIGHT_PT: f32 = 12_000.0;
/// The short viewport used to measure a page's height.
const MEASURE_HEIGHT_PT: f32 = 40.0;
/// Layout passes while resources arrive. Loads are answered at once, so two
/// or three settle it; more would mean something keeps changing.
const MAX_PASSES: usize = 6;

/// Hands every request to the current page's loader.
struct TrialNet {
    loader: Mutex<Option<Loader>>,
}

impl NetProvider for TrialNet {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str().to_string();
        let found = self.loader.lock().ok().and_then(|l| l.as_ref().and_then(|l| l.fetch(&url)));
        let bytes = found.map(|r| Bytes::from(r.bytes)).unwrap_or_default();
        handler.bytes(url, bytes);
    }
}

/// Blitz with its CPU painter.
pub struct Blitz {
    net: Arc<TrialNet>,
    fonts: Option<FontContext>,
    /// The last page, laid out at its full height, for questions after drawing.
    last: Option<BaseDocument>,
}

impl Blitz {
    /// The fonts are found on the first drawing (that's the cold start).
    pub fn new() -> Result<Self> {
        Ok(Self { net: Arc::new(TrialNet { loader: Mutex::new(None) }), fonts: None, last: None })
    }

    /// Parse the page and lay it out in a viewport `w` × `h` points.
    fn document(&mut self, html: &str, w: f32, h: f32, opts: &RenderOpts) -> BaseDocument {
        let scheme = if opts.dark { ColorScheme::Dark } else { ColorScheme::Light };
        let viewport = Viewport::new((w * opts.scale) as u32, (h * opts.scale) as u32, opts.scale, scheme);
        let config = DocumentConfig {
            viewport: Some(viewport),
            net_provider: Some(self.net.clone() as Arc<dyn NetProvider>),
            font_ctx: self.fonts.clone(),
            ..Default::default()
        };
        let mut doc = HtmlDocument::from_html(html, config).into_inner();
        settle(&mut doc);
        doc
    }
}

/// Resolve styles and layout until loaded resources stop changing things.
fn settle(doc: &mut BaseDocument) {
    for _ in 0..MAX_PASSES {
        doc.resolve(0.0);
        if !doc.has_changes() && !doc.has_pending_critical_resources() {
            break;
        }
    }
}

impl Engine for Blitz {
    fn name(&self) -> &'static str {
        "blitz"
    }

    fn render(&mut self, page: &Page, opts: &RenderOpts) -> Result<Rendered> {
        let mut notes = Vec::new();
        if self.fonts.is_none() {
            let t = Instant::now();
            self.fonts = Some(FontContext::default());
            notes.push(format!("finding the system's fonts took {:.0} ms (once per app, not counted)", t.elapsed().as_secs_f64() * 1000.0));
        }
        let started = Instant::now();
        if let Ok(mut l) = self.net.loader.lock() {
            *l = Some(page.loader.clone());
        }
        let w = opts.width_pt as f32;
        let measure = self.document(&page.html, w, MEASURE_HEIGHT_PT, opts);
        let full = measure.root_element().scroll_height();
        let h = if full > MAX_HEIGHT_PT {
            notes.push(format!("cut short at {MAX_HEIGHT_PT} of {full} points"));
            MAX_HEIGHT_PT
        } else {
            full.max(1.0)
        };
        let mut doc = self.document(&page.html, w, h, opts);
        let (pw, ph) = ((w * opts.scale).round() as u32, (h * opts.scale).ceil() as u32);
        let mut renderer = VelloCpuImageRenderer::new(pw, ph);
        let mut buf = Vec::with_capacity((pw * ph * 4) as usize);
        renderer.render_to_vec(|scene| blitz_paint::paint_scene(scene, &mut doc, opts.scale as f64, pw, ph, 0, 0), &mut buf);
        let first_draw = started.elapsed();
        let image = over_white(buf, pw, ph).ok_or_else(|| anyhow!("pixel buffer the wrong size"))?;
        self.last = Some(doc);
        Ok(Rendered { image, height_pt: full, first_draw, requests: page.loader.take_log(), notes })
    }

    fn link_at(&mut self, x: f32, y: f32) -> Option<String> {
        let doc = self.last.as_ref()?;
        let mut id = Some(doc.hit(x, y)?.node_id);
        while let Some(node) = id.and_then(|i| doc.get_node(i)) {
            if node.element_data().is_some_and(|e| e.name.local == local_name!("a")) {
                return node.attr(local_name!("href")).map(String::from);
            }
            id = node.parent;
        }
        None
    }

    fn text_between(&mut self, from: (f32, f32), to: (f32, f32)) -> Option<String> {
        let doc = self.last.as_mut()?;
        let (a, ao) = doc.find_text_position(from.0, from.1)?;
        let (b, bo) = doc.find_text_position(to.0, to.1)?;
        doc.set_text_selection(a, ao, b, bo);
        doc.get_selected_text()
    }

    fn element_box(&mut self, id: &str) -> Option<Rect> {
        let doc = self.last.as_ref()?;
        let node = doc.query_selector(&format!("#{id}")).ok()??;
        let r = doc.get_client_bounding_rect(node)?;
        Some(Rect { x: r.x as f32, y: r.y as f32, w: r.width as f32, h: r.height as f32 })
    }
}
