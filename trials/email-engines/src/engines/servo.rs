//! Servo, the browser engine, as a library: drawn in its software rendering
//! context (no window), one process, without developer tools. Built from
//! `servo-build/` (see there for why).
//!
//! Loading: Servo has no "load this HTML" call, so each page is served from
//! a scheme of our own, `trialpage:`, with a new address every time (so no
//! cache carries over between emails). `cid:` is a second scheme of ours,
//! answered by the page's [`Loader`]. `data:` is decoded by Servo; `http:`
//! is fetched by Servo itself, like WebKit, so the local server's log shows
//! it, and the page's Content Security Policy (as in the app) blocks it
//! unless remote images are on. JavaScript stays on, as in the app's
//! WebKit view: the policy is what keeps the email's own scripts from
//! running, and the script check tests exactly that.
//!
//! Each page is measured in a short view (as for webkit-mac), then the view
//! is resized to the whole page and captured.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use dpi::PhysicalSize;
use servo::protocol_handler::{DoneChannel, FetchContext, NetworkError, ProtocolHandler, ProtocolRegistry, Request, ResourceFetchTiming, Response, ResponseBody};
use servo::{
    EventLoopWaker, JSValue, LoadStatus, Preferences, RenderingContext, ServoBuilder, SoftwareRenderingContext, Theme, WebView,
    WebViewBuilder, WebViewDelegate,
};

use crate::engine::{Engine, Page, Rect, RenderOpts, Rendered};
use crate::loader::Loader;

/// The tallest picture taken, in points. Lower than for the other engines:
/// Servo draws through OpenGL, whose surfaces top out around 16,000 pixels.
const MAX_HEIGHT_PT: f64 = 8_000.0;
/// The short view used to measure a page's height.
const MEASURE_HEIGHT_PT: f64 = 40.0;

/// What our two schemes serve: the page being drawn and its loader.
#[derive(Default)]
struct Current {
    html: String,
    loader: Option<Loader>,
}

/// Answers `trialpage:` (the page) or `cid:` (its parts).
struct Scheme {
    current: Arc<Mutex<Current>>,
    page: bool,
}

impl ProtocolHandler for Scheme {
    fn load<'a>(
        &'a self,
        request: &'a mut Request,
        _done_chan: &mut DoneChannel,
        _context: &FetchContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'a>> {
        let url = request.current_url();
        let found = self.current.lock().ok().and_then(|c| {
            if self.page {
                Some(("text/html; charset=utf-8".to_string(), c.html.clone().into_bytes()))
            } else {
                c.loader.as_ref().and_then(|l| l.fetch(url.as_str())).map(|r| (r.mime, r.bytes))
            }
        });
        let response = match found {
            Some((mime, bytes)) => {
                let mut response = Response::new(url, ResourceFetchTiming::new(request.timing_type()));
                *response.body.lock() = ResponseBody::Done(bytes);
                if let Ok(v) = http::HeaderValue::from_str(&mime) {
                    response.headers.insert(http::header::CONTENT_TYPE, v);
                }
                response
            }
            None => Response::network_error(NetworkError::ResourceLoadError("refused".into())),
        };
        Box::pin(std::future::ready(response))
    }
}

/// Wakes nobody: we spin Servo's event loop ourselves.
#[derive(Clone)]
struct Waker;

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {}
}

/// Notices loads finishing and new frames, and logs `http:` loads.
#[derive(Default)]
struct Delegate {
    loaded: Cell<bool>,
    /// New frames painted so far (to wait for one after a resize).
    frames: Cell<u64>,
    crashed: Cell<bool>,
    loader: RefCell<Option<Loader>>,
}

impl WebViewDelegate for Delegate {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) {
            self.loaded.set(true);
        }
    }
    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.paint();
        self.frames.set(self.frames.get() + 1);
    }
    fn notify_crashed(&self, _webview: WebView, _reason: String, _backtrace: Option<String>) {
        self.crashed.set(true);
    }
    fn load_web_resource(&self, _webview: WebView, load: servo::WebResourceLoad) {
        // Not intercepted (Servo fetches it, as WebKit would); just noted.
        if let Some(l) = self.loader.borrow().as_ref() {
            l.note(load.request.url.as_str());
        }
    }
}

/// Servo with its software rendering context.
pub struct ServoEngine {
    servo: servo::Servo,
    context: Rc<SoftwareRenderingContext>,
    current: Arc<Mutex<Current>>,
    delegate: Rc<Delegate>,
    webview: Option<WebView>,
    pages: u64,
}

impl ServoEngine {
    /// Start Servo. Slow the first time (that's the cold start).
    pub fn new() -> Result<Self> {
        let context = Rc::new(SoftwareRenderingContext::new(PhysicalSize { width: 1200, height: 80 }).map_err(|e| anyhow!("no software rendering context: {e:?}"))?);
        context.make_current().map_err(|e| anyhow!("rendering context won't activate: {e:?}"))?;
        let current = Arc::new(Mutex::new(Current::default()));
        let mut protocols = ProtocolRegistry::default();
        protocols.register("trialpage", Scheme { current: current.clone(), page: true }).map_err(|e| anyhow!("{e:?}"))?;
        protocols.register("cid", Scheme { current: current.clone(), page: false }).map_err(|e| anyhow!("{e:?}"))?;
        // Nothing should go through a proxy: the only server is on 127.0.0.1.
        let prefs = Preferences { network_http_proxy_uri: String::new(), network_https_proxy_uri: String::new(), ..Default::default() };
        let servo = ServoBuilder::default().preferences(prefs).protocol_registry(protocols).event_loop_waker(Box::new(Waker)).build();
        Ok(Self { servo, context, current, delegate: Rc::new(Delegate::default()), webview: None, pages: 0 })
    }

    /// Spin Servo until `done` or `timeout`. Returns whether `done` became true.
    fn spin(&self, done: impl Fn() -> bool, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while !done() {
            if Instant::now() > end || self.delegate.crashed.get() {
                return false;
            }
            self.servo.spin_event_loop();
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }

    /// Run JavaScript in the page and return its result.
    fn eval(&self, js: &str, timeout: Duration) -> Option<JSValue> {
        let webview = self.webview.as_ref()?;
        let out: Rc<RefCell<Option<JSValue>>> = Rc::new(RefCell::new(None));
        let slot = out.clone();
        webview.evaluate_javascript(js, move |r| *slot.borrow_mut() = Some(r.unwrap_or(JSValue::Undefined)));
        self.spin(|| out.borrow().is_some(), timeout);
        let got = out.borrow_mut().take();
        got
    }

    fn eval_string(&self, js: &str) -> Option<String> {
        match self.eval(js, Duration::from_secs(5))? {
            JSValue::String(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    fn resize(&self, w: f64, h: f64, scale: f32) {
        let size = PhysicalSize { width: (w * scale as f64).round() as u32, height: (h * scale as f64).ceil() as u32 };
        // Through the WebView when there is one: it resizes the context and
        // the page's viewport together. Resizing the context first would
        // make Servo see no change and leave the viewport as it was.
        match &self.webview {
            Some(v) => v.resize(size),
            None => self.context.resize(size),
        }
    }

    fn screenshot(&self, timeout: Duration) -> Result<image::RgbaImage> {
        let webview = self.webview.as_ref().ok_or_else(|| anyhow!("no page"))?;
        let out: Rc<RefCell<Option<Result<image::RgbaImage>>>> = Rc::new(RefCell::new(None));
        let slot = out.clone();
        webview.take_screenshot(None, move |r| *slot.borrow_mut() = Some(r.map_err(|e| anyhow!("screenshot failed: {e:?}"))));
        if !self.spin(|| out.borrow().is_some(), timeout) {
            bail!("no screenshot within {timeout:?}");
        }
        let got = out.borrow_mut().take();
        got.unwrap_or_else(|| Err(anyhow!("no screenshot")))
    }
}

impl Engine for ServoEngine {
    fn name(&self) -> &'static str {
        "servo"
    }

    fn render(&mut self, page: &Page, opts: &RenderOpts) -> Result<Rendered> {
        let started = Instant::now();
        if let Ok(mut c) = self.current.lock() {
            *c = Current { html: page.html.clone(), loader: Some(page.loader.clone()) };
        }
        *self.delegate.loader.borrow_mut() = Some(page.loader.clone());
        self.delegate.loaded.set(false);
        let w = opts.width_pt as f64;
        self.resize(w, MEASURE_HEIGHT_PT, opts.scale);
        self.pages += 1;
        let url = url::Url::parse(&format!("trialpage://email/{}", self.pages))?;
        // A new view for every page, so nothing (pictures included) carries over.
        let webview = WebViewBuilder::new(&self.servo, self.context.clone())
            .delegate(self.delegate.clone())
            .hidpi_scale_factor(euclid::Scale::new(opts.scale))
            .url(url)
            .build();
        webview.notify_theme_change(if opts.dark { Theme::Dark } else { Theme::Light });
        self.webview = Some(webview);
        if !self.spin(|| self.delegate.loaded.get(), opts.timeout) {
            bail!(if self.delegate.crashed.get() { "Servo crashed".to_string() } else { format!("page still loading after {:?}", opts.timeout) });
        }
        let full = match self.eval("Math.ceil(document.documentElement.scrollHeight)", opts.timeout.saturating_sub(started.elapsed())) {
            Some(JSValue::Number(n)) => n,
            other => bail!("couldn't read the page height: {other:?}"),
        };
        let mut notes = Vec::new();
        let h = if full > MAX_HEIGHT_PT {
            notes.push(format!("cut short at {MAX_HEIGHT_PT} of {full} points"));
            MAX_HEIGHT_PT
        } else {
            full.max(1.0)
        };
        // Servo applies a resize on its next frame: wait for one, or the
        // screenshot is of the old, measuring size.
        let frames = self.delegate.frames.get();
        self.resize(w, h, opts.scale);
        if !self.spin(|| self.delegate.frames.get() > frames, opts.timeout.saturating_sub(started.elapsed())) {
            notes.push("no new frame after resizing, by the time limit".to_string());
        }
        let image = self.screenshot(opts.timeout.saturating_sub(started.elapsed()))?;
        let first_draw = started.elapsed();
        Ok(Rendered { image, height_pt: full as f32, first_draw, requests: page.loader.take_log(), notes })
    }

    fn link_at(&mut self, x: f32, y: f32) -> Option<String> {
        self.eval_string(&format!("(function(){{var e=document.elementFromPoint({x},{y});var a=e&&e.closest('a');return a?a.getAttribute('href'):''}})()"))
    }

    fn text_between(&mut self, from: (f32, f32), to: (f32, f32)) -> Option<String> {
        // The standard caretPositionFromPoint, or WebKit's caretRangeFromPoint.
        self.eval_string(&format!(
            "(function(){{function at(x,y){{if(document.caretPositionFromPoint){{var p=document.caretPositionFromPoint(x,y);return p&&[p.offsetNode,p.offset]}}\
             if(document.caretRangeFromPoint){{var r=document.caretRangeFromPoint(x,y);return r&&[r.startContainer,r.startOffset]}}return null}}\
             var a=at({},{}),b=at({},{});if(!a||!b)return '';var r=document.createRange();r.setStart(a[0],a[1]);r.setEnd(b[0],b[1]);return r.toString()}})()",
            from.0, from.1, to.0, to.1
        ))
    }

    fn element_box(&mut self, id: &str) -> Option<Rect> {
        let s = self.eval_string(&format!(
            "(function(){{var e=document.getElementById({});if(!e)return '';var r=e.getBoundingClientRect();return [r.left,r.top,r.width,r.height].join(',')}})()",
            serde_json::to_string(id).ok()?
        ))?;
        let v: Vec<f32> = s.split(',').filter_map(|p| p.parse().ok()).collect();
        match v.as_slice() {
            [x, y, w, h] => Some(Rect { x: *x, y: *y, w: *w, h: *h }),
            _ => None,
        }
    }
}
