//! The reference: the Mac's own WKWebView, as the app uses today, drawn in
//! a borderless window placed far off screen and captured with WebKit's
//! snapshot call. Every other engine is compared with its pictures.
//!
//! How loads are routed: `cid:` goes through our [`Loader`] (a
//! `WKURLSchemeHandler`); `data:` is decoded by WebKit itself; `http:` is
//! fetched by WebKit directly, so it can't go through the loader, but the
//! local server logs it, and the page's Content Security Policy (as in the
//! app) blocks it unless remote images are on.
//!
//! Each page is loaded into a short view first, so its height can be read
//! from the layout (`scrollHeight` of a page taller than the view), then the
//! view is resized to the whole page and captured. Pages taller than
//! [`MAX_HEIGHT_PT`] are cut short, and the result says so.
//!
//! WebKit draws in its own WebContent process, so memory measured for this
//! process leaves most of WebKit's out (the results say so).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSBackingStoreType, NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSData, NSDate, NSDefaultRunLoopMode, NSDictionary, NSError, NSPoint, NSRect, NSRunLoop, NSSize, NSString, NSURLResponse};
use objc2_web_kit::{WKSnapshotConfiguration, WKURLSchemeHandler, WKURLSchemeTask, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore};

use crate::engine::{Engine, Page, Rect, RenderOpts, Rendered};
use crate::loader::Loader;

/// The tallest picture taken, in points. The 200-item digest is taller;
/// capturing all of it at 2× would need hundreds of megabytes.
const MAX_HEIGHT_PT: f64 = 12_000.0;
/// The short view used to measure a page's height.
const MEASURE_HEIGHT_PT: f64 = 40.0;

/// What the `cid:` handler needs: the current page's loader.
struct HandlerIvars {
    loader: Rc<RefCell<Option<Loader>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class only adds
    // the two WKURLSchemeHandler methods, called by WebKit on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NightPostTrialCidHandler"]
    #[ivars = HandlerIvars]
    struct CidHandler;

    unsafe impl NSObjectProtocol for CidHandler {}

    unsafe impl WKURLSchemeHandler for CidHandler {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.answer(task);
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop(&self, _web_view: &WKWebView, _task: &ProtocolObject<dyn WKURLSchemeTask>) {}
    }
);

impl CidHandler {
    fn new(mtm: MainThreadMarker, loader: Rc<RefCell<Option<Loader>>>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(HandlerIvars { loader });
        // SAFETY: NSObject's plain `init`, on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }

    /// Answer one `cid:` request from the loader, or fail it.
    fn answer(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        // SAFETY: WebKit hands us a live task for the duration of this call.
        let request = unsafe { task.request() };
        let Some(url) = request.URL() else { return };
        let text = url.absoluteString().map(|s| s.to_string()).unwrap_or_default();
        let found = self.ivars().loader.borrow().as_ref().and_then(|l| l.fetch(&text));
        match found {
            Some(r) => {
                let mime = NSString::from_str(&r.mime);
                let response = NSURLResponse::initWithURL_MIMEType_expectedContentLength_textEncodingName(
                    NSURLResponse::alloc(),
                    &url,
                    Some(&mime),
                    r.bytes.len() as isize,
                    None,
                );
                // SAFETY: the task is live; response, data and finish are sent once, in order.
                unsafe {
                    task.didReceiveResponse(&response);
                    task.didReceiveData(&NSData::with_bytes(&r.bytes));
                    task.didFinish();
                }
            }
            None => {
                // NSURLErrorFileDoesNotExist: the email has no such part.
                let error = NSError::new(-1100, &NSString::from_str("NSURLErrorDomain"));
                // SAFETY: the task is live and hasn't been answered.
                unsafe { task.didFailWithError(&error) };
            }
        }
    }
}

/// The Mac's WKWebView, in a window kept off screen.
pub struct WebKitMac {
    mtm: MainThreadMarker,
    window: Retained<NSWindow>,
    view: Retained<WKWebView>,
    loader: Rc<RefCell<Option<Loader>>>,
    _handler: Retained<CidHandler>,
}

impl WebKitMac {
    /// Set up the app (no Dock icon), the window and the web view. Must be
    /// called on the main thread.
    pub fn new() -> Result<Self> {
        let mtm = MainThreadMarker::new().ok_or_else(|| anyhow!("webkit-mac must run on the main thread"))?;
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let loader = Rc::new(RefCell::new(None));
        let handler = CidHandler::new(mtm, loader.clone());
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, MEASURE_HEIGHT_PT));
        // SAFETY: plain AppKit set-up on the main thread (mtm).
        let window = unsafe {
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            // Far off any screen, so nothing flashes up while it draws.
            window.setFrameOrigin(NSPoint::new(-30_000.0, -30_000.0));
            window.orderFront(None);
            window
        };
        let view = Self::new_view(mtm, &handler, rect);
        window.setContentView(Some(&view));
        Ok(Self { mtm, window, view, loader, _handler: handler })
    }

    /// A web view with its own empty, in-memory data store. A fresh one for
    /// every page: WebKit otherwise reuses pictures from the last page by
    /// their `cid:` address, so later pages' requests would go unlogged.
    fn new_view(mtm: MainThreadMarker, handler: &CidHandler, rect: NSRect) -> Retained<WKWebView> {
        // SAFETY: plain WebKit set-up on the main thread (mtm).
        unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            config.setURLSchemeHandler_forURLScheme(Some(ProtocolObject::from_ref(handler)), &NSString::from_str("cid"));
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config)
        }
    }

    /// Wait until every picture has decoded, so the snapshot shows what a
    /// reader would see (without this, a picture can be fetched but still
    /// blank). Not frames: WebKit pauses `requestAnimationFrame` in a window
    /// off screen; the snapshot forces its own paint instead.
    fn settle(&self, timeout: Duration) -> bool {
        const JS: &str = "(function(){if(document.readyState!=='complete')return 'no';if(window.__trialReady)return 'yes';if(!window.__trialWait){window.__trialWait=1;\
            Promise.all(Array.from(document.images).map(function(i){return i.decode().catch(function(){})}))\
            .then(function(){window.__trialReady=1})}return 'no'})()";
        let end = Instant::now() + timeout;
        let _ = self.eval("window.__trialReady=0;window.__trialWait=0;''", timeout);
        while Instant::now() < end {
            let got = self.eval(JS, end.saturating_duration_since(Instant::now()));
            if std::env::var_os("TRIAL_DEBUG").is_some() {
                eprintln!("settle: {got:?}");
            }
            if got.as_deref() == Some("yes") {
                return true;
            }
            pump(|| false, Duration::from_millis(5));
        }
        false
    }

    fn resize(&self, w: f64, h: f64) {
        let size = NSSize::new(w, h);
        self.window.setContentSize(size);
        self.view.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), size));
    }

    /// Run JavaScript in the page (not subject to its policy, which only
    /// governs the email's own scripts) and return its string result.
    fn eval(&self, js: &str, timeout: Duration) -> Option<String> {
        let out: Rc<RefCell<Option<Option<String>>>> = Rc::new(RefCell::new(None));
        let slot = out.clone();
        let block = RcBlock::new(move |value: *mut AnyObject, _err: *mut NSError| {
            // SAFETY: WebKit passes a valid object or null for the call's duration.
            let s = unsafe { value.as_ref() }.and_then(|v| v.downcast_ref::<NSString>()).map(|s| s.to_string());
            *slot.borrow_mut() = Some(s);
        });
        // SAFETY: main thread; the block lives until WebKit calls it.
        unsafe { self.view.evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&block)) };
        pump(|| out.borrow().is_some(), timeout);
        let got = out.borrow_mut().take();
        got.flatten()
    }

    /// Capture the view as PNG bytes.
    fn snapshot(&self, timeout: Duration) -> Result<Vec<u8>> {
        let out: Rc<RefCell<Option<Result<Vec<u8>>>>> = Rc::new(RefCell::new(None));
        let slot = out.clone();
        let block = RcBlock::new(move |img: *mut NSImage, err: *mut NSError| {
            // SAFETY: WebKit passes a valid image or error (or null) for the call's duration.
            let result = match unsafe { img.as_ref() } {
                Some(img) => png_bytes(img),
                None => Err(anyhow!("snapshot failed: {}", unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_default())),
            };
            *slot.borrow_mut() = Some(result);
        });
        // SAFETY: main thread; the configuration and block outlive the call.
        unsafe {
            let config = WKSnapshotConfiguration::new(self.mtm);
            config.setAfterScreenUpdates(true);
            self.view.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
        }
        if !pump(|| out.borrow().is_some(), timeout) {
            bail!("no snapshot within {timeout:?}");
        }
        let got = out.borrow_mut().take();
        got.unwrap_or_else(|| Err(anyhow!("no snapshot")))
    }
}

/// Encode an NSImage as PNG through a bitmap representation.
fn png_bytes(img: &NSImage) -> Result<Vec<u8>> {
    let tiff = img.TIFFRepresentation().ok_or_else(|| anyhow!("snapshot has no bitmap"))?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or_else(|| anyhow!("snapshot bitmap unreadable"))?;
    // SAFETY: an empty properties dictionary is valid for PNG.
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
        .ok_or_else(|| anyhow!("snapshot couldn't be encoded"))?;
    Ok(png.to_vec())
}

/// Run the main run loop in short slices until `done` or `timeout`.
/// Returns whether `done` became true.
fn pump(done: impl Fn() -> bool, timeout: Duration) -> bool {
    let end = Instant::now() + timeout;
    let run_loop = NSRunLoop::currentRunLoop();
    while !done() {
        if Instant::now() > end {
            return false;
        }
        // SAFETY: NSDefaultRunLoopMode is a valid static mode name.
        unsafe { run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &NSDate::dateWithTimeIntervalSinceNow(0.005)) };
    }
    true
}

impl Engine for WebKitMac {
    fn name(&self) -> &'static str {
        "webkit-mac"
    }

    fn render(&mut self, page: &Page, opts: &RenderOpts) -> Result<Rendered> {
        let started = Instant::now();
        *self.loader.borrow_mut() = Some(page.loader.clone());
        let name = if opts.dark { unsafe { NSAppearanceNameDarkAqua } } else { unsafe { NSAppearanceNameAqua } };
        self.view.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
        let w = opts.width_pt as f64;
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, MEASURE_HEIGHT_PT));
        self.view = Self::new_view(self.mtm, &self._handler, rect);
        self.window.setContentView(Some(&self.view));
        self.view.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
        self.resize(w, MEASURE_HEIGHT_PT);
        // SAFETY: main thread; WebKit copies the string.
        unsafe { self.view.loadHTMLString_baseURL(&NSString::from_str(&page.html), None) };
        // Wait for the load to start, then to finish (subresources included).
        // SAFETY (both closures): reading a property on the main thread.
        // A cold WebKit can take well over half a second to start a load; if
        // we stopped waiting sooner, the first page could be measured (and
        // pictured) before it had loaded. `settle` also checks readyState.
        pump(|| unsafe { self.view.isLoading() }, opts.timeout.min(Duration::from_secs(5)));
        if !pump(|| !unsafe { self.view.isLoading() }, opts.timeout) {
            // SAFETY: main thread.
            unsafe { self.view.stopLoading() };
            bail!("page still loading after {:?}", opts.timeout);
        }
        let mut notes = Vec::new();
        if !self.settle(opts.timeout.saturating_sub(started.elapsed())) {
            notes.push("pictures hadn't all decoded by the time limit".to_string());
        }
        let left = opts.timeout.saturating_sub(started.elapsed());
        let full: f64 = self
            .eval("String(Math.ceil(document.documentElement.scrollHeight))", left)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow!("couldn't read the page height"))?;
        let h = if full > MAX_HEIGHT_PT {
            notes.push(format!("cut short at {MAX_HEIGHT_PT} of {full} points"));
            MAX_HEIGHT_PT
        } else {
            full.max(1.0)
        };
        self.resize(w, h);
        if !self.settle(opts.timeout.saturating_sub(started.elapsed())) {
            notes.push("pictures hadn't all decoded after resizing, by the time limit".to_string());
        }
        let png = self.snapshot(opts.timeout.saturating_sub(started.elapsed()))?;
        let first_draw = started.elapsed();
        let image = image::load_from_memory(&png)?.to_rgba8();
        let scale = image.width() as f32 / w as f32;
        if (scale - opts.scale).abs() > 0.01 {
            notes.push(format!("drawn at {scale}× (this screen's backing scale), not {}×", opts.scale));
        }
        let requests = page.loader.take_log();
        Ok(Rendered { image, height_pt: full as f32, first_draw, requests, notes })
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
            "(function(){{var e=document.getElementById({});if(!e)return '';var r=e.getBoundingClientRect();return [r.left,r.top,r.width,r.height].join(',')}})()",
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
