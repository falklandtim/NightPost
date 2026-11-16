//! WebKitGTK 6.0 (GTK 4), the system's WebKit on Linux: the route the app's
//! composer has to take once the GTK 3 bindings go (docs/gtk4-upgrade.md).
//!
//! The open question this answers: does WebKit lay out and draw a page in a
//! GTK 4 window that is never shown? GTK 4 has no off-screen window (GTK 3's
//! `OffscreenWindow`, which the app uses today, is gone). So the web view
//! sits in a `gtk::Window` that is never presented, and the page is captured
//! with WebKit's own full-document snapshot. `TRIAL_GTK_PRESENT=1` shows the
//! window instead, to compare.
//!
//! As for webkit-mac: a fresh, ephemeral web view per email (so nothing is
//! cached between emails), `cid:` answered by our [`Loader`] through a URI
//! scheme, `http:` fetched by WebKit itself (the local server logs it, and
//! the page's Content Security Policy blocks it unless remote images are
//! on), JavaScript on as in the app, and the questions after drawing asked
//! with JavaScript.
//!
//! It needs a running Wayland (or X11) session: over SSH, borrow the
//! desktop's `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` (see the README).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use webkit6::gdk;
use webkit6::glib;
use webkit6::gtk;
use webkit6::gtk::prelude::*;
use webkit6::prelude::*;
use webkit6::{LoadEvent, NetworkSession, SnapshotOptions, SnapshotRegion, URISchemeRequest, WebContext, WebView};

use crate::engine::{Engine, Page, Rect, RenderOpts, Rendered};
use crate::loader::Loader;

/// The tallest picture kept, in points (as for webkit-mac).
const MAX_HEIGHT_PT: f32 = 12_000.0;
/// The view's height while laying out (the snapshot takes the whole page).
const VIEW_HEIGHT_PT: i32 = 800;

/// WebKitGTK 6.0 in a GTK 4 window that's never shown.
pub struct WebKitGtk4 {
    window: gtk::Window,
    context: WebContext,
    loader: Rc<RefCell<Option<Loader>>>,
    view: Option<WebView>,
    loaded: Rc<Cell<bool>>,
}

impl WebKitGtk4 {
    /// Start GTK and set up the window and the `cid:` scheme. Must be
    /// called on the main thread.
    pub fn new() -> Result<Self> {
        gtk::init().map_err(|e| anyhow!("GTK won't start (is there a Wayland or X11 session? see the README): {e}"))?;
        let loader: Rc<RefCell<Option<Loader>>> = Rc::new(RefCell::new(None));
        let context = WebContext::new();
        let scheme_loader = loader.clone();
        context.register_uri_scheme("cid", move |req: &URISchemeRequest| answer(req, &scheme_loader));
        let window = gtk::Window::new();
        window.set_decorated(false);
        window.set_title(Some("NightPost engine trial"));
        if std::env::var_os("TRIAL_GTK_PRESENT").is_some() {
            window.present();
        }
        Ok(Self { window, context, loader, view: None, loaded: Rc::new(Cell::new(false)) })
    }

    /// Run GTK's main loop in short slices until `done` or `timeout`.
    fn spin(&self, done: impl Fn() -> bool, timeout: Duration) -> bool {
        let ctx = glib::MainContext::default();
        let end = Instant::now() + timeout;
        while !done() {
            if Instant::now() > end {
                return false;
            }
            if !ctx.iteration(false) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        true
    }

    /// Run JavaScript in the page; its result as a string.
    fn eval(&self, js: &str, timeout: Duration) -> Option<String> {
        let view = self.view.as_ref()?;
        let out: Rc<RefCell<Option<Option<String>>>> = Rc::new(RefCell::new(None));
        let slot = out.clone();
        view.evaluate_javascript(js, None, None, None::<&webkit6::gio::Cancellable>, move |r| {
            *slot.borrow_mut() = Some(r.ok().filter(|v| v.is_string()).map(|v| v.to_str().to_string()));
        });
        self.spin(|| out.borrow().is_some(), timeout);
        let got = out.borrow_mut().take();
        got.flatten()
    }

    /// Wait until every picture has decoded (as for webkit-mac).
    fn settle(&self, timeout: Duration) -> bool {
        const JS: &str = "(function(){if(document.readyState!=='complete')return 'no';if(window.__trialReady)return 'yes';\
            if(!window.__trialWait){window.__trialWait=1;Promise.all(Array.from(document.images).map(function(i){return i.decode().catch(function(){})}))\
            .then(function(){window.__trialReady=1})}return 'no'})()";
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if self.eval(JS, end.saturating_duration_since(Instant::now())).as_deref() == Some("yes") {
                return true;
            }
            self.spin(|| false, Duration::from_millis(5));
        }
        false
    }

    /// Capture the whole page as RGBA.
    fn snapshot(&self, timeout: Duration) -> Result<image::RgbaImage> {
        let view = self.view.as_ref().ok_or_else(|| anyhow!("no page"))?;
        let out: Rc<RefCell<Option<Result<gdk::Texture, glib::Error>>>> = Rc::new(RefCell::new(None));
        let slot = out.clone();
        view.snapshot(SnapshotRegion::FullDocument, SnapshotOptions::NONE, None::<&webkit6::gio::Cancellable>, move |r| *slot.borrow_mut() = Some(r));
        if !self.spin(|| out.borrow().is_some(), timeout) {
            bail!("no snapshot within {timeout:?}");
        }
        let texture = out.borrow_mut().take().ok_or_else(|| anyhow!("no snapshot"))?.map_err(|e| anyhow!("snapshot failed: {e}"))?;
        let mut down = gdk::TextureDownloader::new(&texture);
        down.set_format(gdk::MemoryFormat::R8g8b8a8);
        let (bytes, stride) = down.download_bytes();
        let (w, h) = (texture.width() as u32, texture.height() as u32);
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for row in bytes.chunks(stride).take(h as usize) {
            px.extend_from_slice(row.get(..(w * 4) as usize).ok_or_else(|| anyhow!("short row in snapshot"))?);
        }
        image::RgbaImage::from_raw(w, h, px).ok_or_else(|| anyhow!("snapshot the wrong size"))
    }
}

/// Answer one `cid:` request from the page's loader, or refuse it.
fn answer(req: &URISchemeRequest, loader: &RefCell<Option<Loader>>) {
    let url = req.uri().map(|u| u.to_string()).unwrap_or_default();
    match loader.borrow().as_ref().and_then(|l| l.fetch(&url)) {
        Some(r) => {
            let len = r.bytes.len() as i64;
            let stream = webkit6::gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(r.bytes));
            req.finish(&stream, len, Some(&r.mime));
        }
        None => {
            let mut err = glib::Error::new(webkit6::gio::IOErrorEnum::NotFound, "refused");
            req.finish_error(&mut err);
        }
    }
}

impl Engine for WebKitGtk4 {
    fn name(&self) -> &'static str {
        "webkit-gtk4"
    }

    fn render(&mut self, page: &Page, opts: &RenderOpts) -> Result<Rendered> {
        let started = Instant::now();
        *self.loader.borrow_mut() = Some(page.loader.clone());
        // The page's `prefers-color-scheme` follows GTK's dark preference.
        if let Some(s) = gtk::Settings::default() {
            s.set_gtk_application_prefer_dark_theme(opts.dark);
        }
        // A fresh view and session per email, so nothing carries over.
        let view = WebView::builder().web_context(&self.context).network_session(&NetworkSession::new_ephemeral()).build();
        view.set_size_request(opts.width_pt as i32, VIEW_HEIGHT_PT);
        self.window.set_default_size(opts.width_pt as i32, VIEW_HEIGHT_PT);
        self.window.set_child(Some(&view));
        self.loaded.set(false);
        let loaded = self.loaded.clone();
        view.connect_load_changed(move |_, ev| {
            if ev == LoadEvent::Finished {
                loaded.set(true);
            }
        });
        // A window that's never shown is never given a size, so WebKit would
        // lay the page out at 0 × 0. Realise it and size the view by hand.
        if std::env::var_os("TRIAL_GTK_PRESENT").is_none() {
            WidgetExt::realize(&self.window);
            WidgetExt::realize(&view);
            view.allocate(opts.width_pt as i32, VIEW_HEIGHT_PT, -1, None);
        }
        view.load_html(&page.html, None);
        self.view = Some(view);
        if !self.spin(|| self.loaded.get(), opts.timeout) {
            bail!("page still loading after {:?}", opts.timeout);
        }
        let mut notes = Vec::new();
        if !self.settle(opts.timeout.saturating_sub(started.elapsed())) {
            notes.push("pictures hadn't all decoded by the time limit".to_string());
        }
        if std::env::var_os("TRIAL_DEBUG").is_some() {
            eprintln!("viewport: {:?}", self.eval("[innerWidth,innerHeight,document.documentElement.clientWidth,devicePixelRatio].join('x')", Duration::from_secs(5)));
        }
        let full: f32 = self
            .eval("String(Math.ceil(document.documentElement.scrollHeight))", Duration::from_secs(5))
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow!("couldn't read the page height"))?;
        let scale: f32 = self.eval("String(window.devicePixelRatio)", Duration::from_secs(5)).and_then(|s| s.parse().ok()).unwrap_or(1.0);
        let mut image = self.snapshot(opts.timeout.saturating_sub(started.elapsed()))?;
        let first_draw = started.elapsed();
        if (scale - opts.scale).abs() > 0.01 {
            notes.push(format!("drawn at {scale}× (this display's scale), not {}×", opts.scale));
        }
        // The full-document snapshot includes anything wider than the pane;
        // keep the pane's width, as the Mac's snapshot does.
        let pane_px = (opts.width_pt as f32 * scale).round() as u32;
        if image.width() > pane_px {
            image = image::imageops::crop_imm(&image, 0, 0, pane_px, image.height()).to_image();
        }
        let max_px = (MAX_HEIGHT_PT * scale).round() as u32;
        if image.height() > max_px {
            notes.push(format!("cut short at {MAX_HEIGHT_PT} of {full} points"));
            image = image::imageops::crop_imm(&image, 0, 0, image.width(), max_px).to_image();
        }
        Ok(Rendered { image, height_pt: full, first_draw, requests: page.loader.take_log(), notes })
    }

    fn link_at(&mut self, x: f32, y: f32) -> Option<String> {
        let js = format!("(function(){{var e=document.elementFromPoint({x},{y});var a=e&&e.closest('a');return a?a.getAttribute('href'):''}})()");
        self.eval(&js, Duration::from_secs(5)).filter(|s| !s.is_empty())
    }

    fn text_between(&mut self, from: (f32, f32), to: (f32, f32)) -> Option<String> {
        let js = format!(
            "(function(){{var a=document.caretRangeFromPoint({},{}),b=document.caretRangeFromPoint({},{});if(!a||!b)return '';\
             var r=document.createRange();r.setStart(a.startContainer,a.startOffset);r.setEnd(b.startContainer,b.startOffset);return r.toString()}})()",
            from.0, from.1, to.0, to.1
        );
        self.eval(&js, Duration::from_secs(5)).filter(|s| !s.is_empty())
    }

    fn element_box(&mut self, id: &str) -> Option<Rect> {
        let js = format!(
            "(function(){{var e=document.getElementById({});if(!e)return '';var r=e.getBoundingClientRect();return [r.left,r.top+window.scrollY,r.width,r.height].join(',')}})()",
            serde_json::to_string(id).ok()?
        );
        let s = self.eval(&js, Duration::from_secs(5))?;
        let v: Vec<f32> = s.split(',').filter_map(|p| p.parse().ok()).collect();
        match v.as_slice() {
            [x, y, w, h] => Some(Rect { x: *x, y: *y, w: *w, h: *h }),
            _ => None,
        }
    }
}
