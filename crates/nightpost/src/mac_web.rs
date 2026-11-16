//! macOS: WKWebView set up the way NightPost's pages need it, for the
//! composer windows (`web_host.rs`) and the reading pane (`mail_view.rs`).
//!
//! This talks to WebKit directly through `objc2-web-kit`. It replaced `wry`,
//! whose Linux half pulled the unmaintained GTK 3 crates into `Cargo.lock`
//! (docs/gtk4-upgrade.md). It does only what NightPost used from `wry`:
//!
//! - Pages talk to the app with `window.ipc.postMessage(string)`, a script
//!   message handler named `ipc` ([`IPC_JS`]).
//! - Every navigation, in any frame, is put to the view's `navigate` rule
//!   (none: everything is allowed); pop-up windows are never opened, but
//!   their address goes to the same rule; downloads are cancelled.
//! - The page's background shows the theme's colour while it loads.
//! - A view laid over another window (the reading pane, `child`) takes the
//!   first click even when the window isn't focused, and leaves ⌘-shortcuts
//!   to the window (egui), as `wry`'s child views did.
//!
//! Everything here runs on the main thread (AppKit's rule).

use block2::DynBlock;
use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSColor, NSEvent, NSResponder, NSView};
use objc2_foundation::{NSNumber, NSRect, NSString};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKScriptMessage, WKScriptMessageHandler, WKUIDelegate, WKUserContentController,
    WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};

/// The page's side of `window.ipc.postMessage`, the name the pages use on
/// both systems (Linux: `web_host.rs`).
const IPC_JS: &str = "Object.defineProperty(window,'ipc',{value:Object.freeze({postMessage:function(s){window.webkit.messageHandlers.ipc.postMessage(String(s))}})});";

/// Called with each message a page posts.
pub type OnMessage = Box<dyn Fn(String)>;
/// Called with each address the page would go to: true lets it load.
pub type OnNavigate = Box<dyn Fn(String) -> bool>;

/// What a [`WebView`] is for.
pub struct Options {
    /// Laid over another window's view (the reading pane), not a window's
    /// whole content.
    pub child: bool,
    /// Shown behind the page while it loads.
    pub background: egui::Color32,
    /// Messages the page posts.
    pub on_message: OnMessage,
    /// The rule for where the page may go (none: anywhere).
    pub on_navigate: Option<OnNavigate>,
}

/// [`WebView`]'s own state.
pub struct ViewIvars {
    child: bool,
}

define_class!(
    // SAFETY: WKWebView may be subclassed; this only overrides two NSView
    // methods, with their AppKit signatures, and calls super otherwise.
    #[unsafe(super(WKWebView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NightPostWebView"]
    #[ivars = ViewIvars]
    /// WKWebView with NightPost's two tweaks for views laid over egui.
    pub struct WebView;

    impl WebView {
        /// A child view would otherwise claim every ⌘-key, so menu and egui
        /// shortcuts never fire while the pane has focus.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            if self.ivars().child {
                Bool::NO
            } else {
                // SAFETY: the superclass's own implementation, same arguments.
                unsafe { msg_send![super(self), performKeyEquivalent: event] }
            }
        }

        /// Clicking a link or fold in the pane works on the first click,
        /// even when NightPost isn't the active app.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> Bool {
            Bool::new(self.ivars().child)
        }
    }
);

/// [`Delegate`]'s own state: the view's callbacks.
pub struct DelegateIvars {
    on_message: OnMessage,
    on_navigate: Option<OnNavigate>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the methods match the
    // WebKit protocols' signatures and WebKit calls them on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NightPostWebDelegate"]
    #[ivars = DelegateIvars]
    /// Receives a view's messages, navigations and pop-up requests.
    pub struct Delegate;

    // SAFETY: an NSObject subclass conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: the method below has the protocol's exact selector and types.
    unsafe impl WKScriptMessageHandler for Delegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            // SAFETY: WebKit hands us a live message for this call.
            let body = unsafe { message.body() };
            if let Some(s) = body.downcast_ref::<NSString>() {
                (self.ivars().on_message)(s.to_string());
            }
        }
    }

    // SAFETY: as above; the protocol's other methods are optional.
    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(&self, _view: &WKWebView, action: &WKNavigationAction, decide: &DynBlock<dyn Fn(WKNavigationActionPolicy)>) {
            // A download (a link with `download`, say) never starts.
            // SAFETY: a plain query of a live action (macOS 11.3+; NightPost needs 26).
            let allow = !unsafe { action.shouldPerformDownload() } && self.ivars().on_navigate.as_ref().is_none_or(|f| f(address(action)));
            decide.call((if allow { WKNavigationActionPolicy::Allow } else { WKNavigationActionPolicy::Cancel },));
        }
    }

    // SAFETY: as above; the protocol's other methods are optional.
    unsafe impl WKUIDelegate for Delegate {
        /// A link that wants a new window: never opened here; the app decides.
        #[unsafe(method(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create(&self, _view: &WKWebView, _config: &WKWebViewConfiguration, action: &WKNavigationAction, _features: &WKWindowFeatures) -> *mut WKWebView {
            if let Some(f) = &self.ivars().on_navigate {
                f(address(action));
            }
            // No new view: nil tells WebKit not to open one.
            std::ptr::null_mut()
        }
    }
);

/// Where a navigation goes, as text.
fn address(action: &WKNavigationAction) -> String {
    // SAFETY: plain queries of a live action and its request.
    let url = unsafe { action.request() }.URL();
    url.and_then(|u| u.absoluteString()).map(|s| s.to_string()).unwrap_or_default()
}

/// A web view and its delegate (kept alive with it: WebKit holds delegates
/// weakly).
pub struct View {
    /// The view itself, to place in a window.
    pub view: Retained<WebView>,
    _delegate: Retained<Delegate>,
}

impl View {
    /// A new, empty web view of size `frame`.
    pub fn new(mtm: MainThreadMarker, frame: NSRect, opts: Options) -> Self {
        let child = opts.child;
        let delegate = Delegate::alloc(mtm).set_ivars(DelegateIvars { on_message: opts.on_message, on_navigate: opts.on_navigate });
        // SAFETY: NSObject's plain `init` on a freshly allocated object.
        let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
        let c = opts.background;
        // SAFETY: plain WebKit set-up on the main thread (mtm). `drawsBackground`
        // is a private but long-standing key (wry used it too): without it the
        // view paints white before the page arrives, a flash in a dark theme.
        let view = unsafe {
            let content = WKUserContentController::new(mtm);
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(WKUserScript::alloc(mtm), &NSString::from_str(IPC_JS), WKUserScriptInjectionTime::AtDocumentStart, true);
            content.addUserScript(&script);
            content.addScriptMessageHandler_name(ProtocolObject::from_ref(&*delegate), &NSString::from_str("ipc"));
            let config = WKWebViewConfiguration::new(mtm);
            config.setUserContentController(&content);
            let no = NSNumber::new_bool(false);
            let _: () = msg_send![&config, setValue: &*no, forKey: &*NSString::from_str("drawsBackground")];
            let view = WebView::alloc(mtm).set_ivars(ViewIvars { child });
            let view: Retained<WebView> = msg_send![super(view), initWithFrame: frame, configuration: &*config];
            let bg = NSColor::colorWithSRGBRed_green_blue_alpha(f64::from(c.r()) / 255.0, f64::from(c.g()) / 255.0, f64::from(c.b()) / 255.0, 1.0);
            view.setUnderPageBackgroundColor(Some(&bg));
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view
        };
        Self { view, _delegate: delegate }
    }

    /// Load a whole page (no base address, so it's `about:blank`).
    pub fn load_html(&self, html: &str) {
        // SAFETY: a plain WebKit call on the main thread.
        let _ = unsafe { self.view.loadHTMLString_baseURL(&NSString::from_str(html), None) };
    }

    /// Run script in the page; its result is ignored.
    pub fn eval(&self, js: &str) {
        // SAFETY: a plain WebKit call on the main thread; no completion handler.
        unsafe { self.view.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
    }
}
