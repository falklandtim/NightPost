//! Windows holding a web page (the formatted composer), outside egui.
//!
//! The same HTML runs everywhere, each in the system's own web engine:
//! WebKit in a native window on macOS, WebKitGTK in a GTK window on Linux.
//! A GTK window is its own toplevel, so this works on Wayland as well as
//! X11 (an egui window can only host a web view under X11).
//!
//! Pages talk to the app with `window.ipc.postMessage(json)`; the app
//! answers by running script in the page ([`eval`]). Messages come back
//! through [`poll`], tagged with the window's id.
//!
//! On macOS everything runs on the main thread, where egui runs too. On
//! Linux, GTK has a thread of its own running its main loop, and commands
//! are posted to it.

use eframe::egui;

/// Something a window said or did.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// A message the page posted (JSON).
    Message(u64, String),
    /// The window's close button was clicked (the window stays until [`close`]).
    CloseRequested(u64),
}

/// What a new window looks like.
pub struct WindowSpec {
    /// The window's title bar text.
    pub title: String,
    /// Size in points (logical pixels).
    pub width: f64,
    pub height: f64,
    /// The whole page, loaded as is.
    pub html: String,
    /// Shown behind the page while it loads, so a dark theme doesn't flash white.
    pub background: (u8, u8, u8),
}

/// Start the host (once). Returns false where web windows aren't available,
/// and callers then use the egui composer instead.
pub fn init(ctx: &egui::Context) -> bool {
    imp::init(ctx)
}

/// Remember the main window, so new web windows can be brought to the
/// front on Wayland (the compositor wants permission from the window that
/// has focus). Call once the window exists.
pub fn set_main_window(window: &(impl raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle)) {
    #[cfg(target_os = "linux")]
    activation::remember(window);
    #[cfg(not(target_os = "linux"))]
    let _ = window;
}

/// Is the host running?
pub fn available() -> bool {
    imp::available()
}

/// Open a new window, hidden until [`show`]. `id` is the caller's: events
/// from the window carry it, and the other calls take it.
pub fn open(id: u64, spec: WindowSpec) -> anyhow::Result<()> {
    imp::open(id, spec)
}

/// Run script in a window's page.
pub fn eval(id: u64, js: &str) {
    imp::eval(id, js.to_string())
}

/// Change a window's title (the composer shows its subject).
pub fn set_title(id: u64, title: &str) {
    imp::set_title(id, title.to_string())
}

/// Show a window and bring it to the front.
pub fn show(id: u64) {
    imp::show(id)
}

/// Hide a window without closing it (its page keeps its state).
pub fn hide(id: u64) {
    imp::hide(id)
}

/// Close a window and forget it.
pub fn close(id: u64) {
    imp::close(id)
}

/// Events since the last call.
pub fn poll() -> Vec<HostEvent> {
    imp::poll()
}

/// Linux: run `f` on the GTK thread (for other GTK users, such as the
/// reading view). Returns false when GTK isn't running.
#[cfg(target_os = "linux")]
pub fn on_gtk_thread(f: impl FnOnce() + Send + 'static) -> bool {
    if !imp::available() {
        return false;
    }
    gtk::glib::MainContext::default().invoke(f);
    true
}

/// A JavaScript string literal for `s`.
pub fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Events from window callbacks to the app, and a nudge to repaint.
mod events {
    use super::HostEvent;
    use eframe::egui;
    use std::sync::{Mutex, OnceLock};

    static QUEUE: Mutex<Vec<HostEvent>> = Mutex::new(Vec::new());
    static CTX: OnceLock<egui::Context> = OnceLock::new();

    /// Remember egui's context, so new events can wake the UI.
    pub(super) fn set_ctx(ctx: &egui::Context) {
        let _ = CTX.set(ctx.clone());
    }

    /// Queue an event (from any thread) and ask egui to repaint.
    pub(super) fn push(e: HostEvent) {
        if let Ok(mut q) = QUEUE.lock() {
            q.push(e);
        }
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint();
        }
    }

    /// Everything queued since the last call.
    pub(super) fn take() -> Vec<HostEvent> {
        QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod imp {
    use super::{events, HostEvent, WindowSpec};
    use eframe::egui;
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use raw_window_handle::{AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::ptr::NonNull;

    struct Win {
        window: Retained<NSWindow>,
        webview: wry::WebView,
        /// Hidden on purpose (Undo Send); not a close.
        hidden: bool,
    }

    thread_local! {
        static WINS: RefCell<HashMap<u64, Win>> = RefCell::new(HashMap::new());
        static READY: Cell<bool> = const { Cell::new(false) };
    }

    /// A window's content view, as the handle wry builds into.
    struct ViewHandle(NonNull<std::ffi::c_void>);

    impl HasWindowHandle for ViewHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
            // SAFETY: the view outlives the builder call that borrows it.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    pub(super) fn init(ctx: &egui::Context) -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        events::set_ctx(ctx);
        super::edit_menu::install();
        READY.set(true);
        true
    }

    pub(super) fn available() -> bool {
        READY.get()
    }

    pub(super) fn open(id: u64, spec: WindowSpec) -> anyhow::Result<()> {
        let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("not on the main thread"))?;
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::Resizable;
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(spec.width, spec.height));
        // SAFETY: plain AppKit window creation on the main thread.
        let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false) };
        // Closing only hides it; the app decides when it goes.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&spec.title));
        window.setMinSize(NSSize::new(520.0, 380.0));
        window.center();
        let view = window.contentView().ok_or_else(|| anyhow::anyhow!("window has no content view"))?;
        let handle = ViewHandle(NonNull::from(&*view).cast());
        let (r, g, b) = spec.background;
        let webview = wry::WebViewBuilder::new()
            .with_html(spec.html)
            .with_background_color((r, g, b, 255))
            .with_ipc_handler(move |req| events::push(HostEvent::Message(id, req.into_body())))
            .build(&handle)?;
        window.makeKeyAndOrderFront(None);
        WINS.with(|w| w.borrow_mut().insert(id, Win { window, webview, hidden: false }));
        Ok(())
    }

    fn with(id: u64, f: impl FnOnce(&mut Win)) {
        WINS.with(|w| {
            if let Some(win) = w.borrow_mut().get_mut(&id) {
                f(win)
            }
        })
    }

    pub(super) fn eval(id: u64, js: String) {
        with(id, |w| {
            let _ = w.webview.evaluate_script(&js);
        })
    }

    pub(super) fn set_title(id: u64, title: String) {
        with(id, |w| w.window.setTitle(&NSString::from_str(&title)))
    }

    pub(super) fn show(id: u64) {
        with(id, |w| {
            w.hidden = false;
            w.window.makeKeyAndOrderFront(None);
        })
    }

    pub(super) fn hide(id: u64) {
        with(id, |w| {
            w.hidden = true;
            w.window.orderOut(None);
        })
    }

    pub(super) fn close(id: u64) {
        if let Some(w) = WINS.with(|w| w.borrow_mut().remove(&id)) {
            w.window.orderOut(None);
            w.window.close();
        }
    }

    pub(super) fn poll() -> Vec<HostEvent> {
        let mut out = events::take();
        // The close button hides the window (it's kept): report it.
        WINS.with(|w| {
            for (id, win) in w.borrow_mut().iter_mut() {
                if !win.hidden && !win.window.isVisible() && !win.window.isMiniaturized() {
                    win.hidden = true;
                    out.push(HostEvent::CloseRequested(*id));
                }
            }
        });
        out
    }
}

/// macOS: an Edit menu, so ⌘C, ⌘V, ⌘X, ⌘A and ⌘Z work in web windows
/// (WebKit takes them from menu items, which an egui app doesn't have).
/// egui's own windows don't answer these menu actions, so the keys still
/// reach egui there.
#[cfg(target_os = "macos")]
mod edit_menu {
    use objc2::{sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
    use objc2_foundation::NSString;

    pub(super) fn install() {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let main = match app.mainMenu() {
            Some(m) => m,
            None => {
                let m = NSMenu::new(mtm);
                app.setMainMenu(Some(&m));
                m
            }
        };
        let title = NSString::from_str("Edit");
        if main.itemWithTitle(&title).is_some() {
            return;
        }
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &title);
        let item = |name: &str, action: objc2::runtime::Sel, key: &str| {
            // SAFETY: standard responder actions with key equivalents.
            unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(name), Some(action), &NSString::from_str(key)) }
        };
        for (name, action, key) in [
            ("Undo", sel!(undo:), "z"),
            ("Redo", sel!(redo:), "Z"),
            ("Cut", sel!(cut:), "x"),
            ("Copy", sel!(copy:), "c"),
            ("Paste", sel!(paste:), "v"),
            ("Select All", sel!(selectAll:), "a"),
        ] {
            menu.addItem(&item(name, action, key));
        }
        let top = NSMenuItem::new(mtm);
        top.setTitle(&title);
        top.setSubmenu(Some(&menu));
        main.addItem(&top);
    }
}

// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod imp {
    use super::{events, HostEvent, WindowSpec};
    use eframe::egui;
    use gtk::prelude::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    static READY: AtomicBool = AtomicBool::new(false);

    struct Win {
        window: gtk::Window,
        webview: wry::WebView,
    }

    thread_local! {
        // Only touched on the GTK thread.
        static WINS: RefCell<HashMap<u64, Win>> = RefCell::new(HashMap::new());
    }

    pub(super) fn init(ctx: &egui::Context) -> bool {
        if READY.load(Ordering::SeqCst) {
            return true;
        }
        events::set_ctx(ctx);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("gtk".into()).spawn(move || {
            // The launcher's id, so GTK windows group with OwlPost's.
            gtk::glib::set_prgname(Some("co.timson.OwlPost"));
            if gtk::init().is_err() {
                let _ = tx.send(false);
                return;
            }
            let _ = tx.send(true);
            gtk::main();
        });
        let ok = spawned.is_ok() && rx.recv().unwrap_or(false);
        READY.store(ok, Ordering::SeqCst);
        if !ok {
            log::warn!("GTK didn't start; the composer falls back to plain text");
        }
        ok
    }

    pub(super) fn available() -> bool {
        READY.load(Ordering::SeqCst)
    }

    /// Run `f` on the GTK thread.
    fn on_gtk(f: impl FnOnce() + Send + 'static) {
        gtk::glib::MainContext::default().invoke(f);
    }

    pub(super) fn open(id: u64, spec: WindowSpec) -> anyhow::Result<()> {
        if !available() {
            anyhow::bail!("GTK isn't running");
        }
        let (tx, rx) = std::sync::mpsc::channel();
        // Asked for here, on the main window's thread, before GTK shows it.
        let token = super::activation::token();
        on_gtk(move || {
            let _ = tx.send(build(id, spec, token).map_err(|e| format!("{e:#}")));
        });
        rx.recv().map_err(|_| anyhow::anyhow!("the GTK thread stopped"))?.map_err(|e| anyhow::anyhow!(e))
    }

    fn build(id: u64, spec: WindowSpec, token: Option<String>) -> anyhow::Result<()> {
        use wry::WebViewBuilderExtUnix;
        let window = gtk::Window::new(gtk::WindowType::Toplevel);

        window.set_title(&spec.title);
        window.set_default_size(spec.width as i32, spec.height as i32);
        window.set_position(gtk::WindowPosition::Center);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&content);
        let (r, g, b) = spec.background;
        let webview = wry::WebViewBuilder::new()
            .with_html(spec.html)
            .with_background_color((r, g, b, 255))
            .with_ipc_handler(move |req| events::push(HostEvent::Message(id, req.into_body())))
            .build_gtk(&content)?;
        spell_check(&webview);
        window.connect_delete_event(move |_, _| {
            events::push(HostEvent::CloseRequested(id));
            gtk::glib::Propagation::Stop
        });
        window.show_all();
        window.present();
        // Bring it to the front with the main window's permission (else KDE
        // opens it behind the main window).
        if let Some(t) = token {
            // Once the compositor has mapped it.
            let w = window.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || super::activation::activate_gtk(&w, &t));
        }
        log::info!("web window {id}: realized={} visible={} mapped={} size={:?}", window.is_realized(), window.is_visible(), window.is_mapped(), window.size());
        WINS.with(|w| w.borrow_mut().insert(id, Win { window, webview }));
        Ok(())
    }

    /// Check spelling in the system's language (Hunspell dictionaries).
    fn spell_check(webview: &wry::WebView) {
        use webkit2gtk::{WebContextExt, WebViewExt};
        use wry::WebViewExtUnix;
        if let Some(ctx) = webview.webview().context() {
            let lang = std::env::var("LANG").ok().and_then(|l| l.split('.').next().map(str::to_string)).filter(|l| !l.is_empty() && l != "C").unwrap_or_else(|| "en_US".into());
            ctx.set_spell_checking_enabled(true);
            ctx.set_spell_checking_languages(&[lang.as_str()]);
        }
    }

    fn with(id: u64, f: impl FnOnce(&Win) + Send + 'static) {
        on_gtk(move || {
            WINS.with(|w| {
                if let Some(win) = w.borrow().get(&id) {
                    f(win)
                }
            })
        });
    }

    pub(super) fn eval(id: u64, js: String) {
        with(id, move |w| {
            let _ = w.webview.evaluate_script(&js);
        })
    }

    pub(super) fn set_title(id: u64, title: String) {
        with(id, move |w| w.window.set_title(&title))
    }

    pub(super) fn show(id: u64) {
        let token = super::activation::token();
        with(id, move |w| {
            w.window.show_all();
            w.window.present();
            if let Some(t) = token {
                let win = w.window.clone();
                gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || super::activation::activate_gtk(&win, &t));
            }
        })
    }

    pub(super) fn hide(id: u64) {
        with(id, |w| w.window.hide())
    }

    pub(super) fn close(id: u64) {
        on_gtk(move || {
            if let Some(w) = WINS.with(|w| w.borrow_mut().remove(&id)) {
                drop(w.webview);
                // SAFETY: the window is ours and nothing else holds it.
                unsafe { w.window.destroy() };
            }
        });
    }

    pub(super) fn poll() -> Vec<HostEvent> {
        events::take()
    }
}

/// Linux on Wayland: an xdg-activation token from the main window, so a new
/// window may take focus. Asked for over the main window's own connection
/// (winit's), since the compositor only grants tokens to the client whose
/// window is focused. X11 needs none.
#[cfg(target_os = "linux")]
mod activation {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use std::sync::Mutex;
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
    use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};

    /// The main window's wl_display and wl_surface.
    static MAIN: Mutex<Option<(usize, usize)>> = Mutex::new(None);

    pub(super) fn remember(window: &(impl HasWindowHandle + HasDisplayHandle)) {
        let (Ok(w), Ok(d)) = (window.window_handle(), window.display_handle()) else { return };
        if let (RawWindowHandle::Wayland(w), RawDisplayHandle::Wayland(d)) = (w.as_raw(), d.as_raw()) {
            if let Ok(mut m) = MAIN.lock() {
                *m = Some((d.display.as_ptr() as usize, w.surface.as_ptr() as usize));
            }
        }
    }

    struct State {
        token: Option<String>,
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<xdg_activation_v1::XdgActivationV1, ()> for State {
        fn event(_: &mut Self, _: &xdg_activation_v1::XdgActivationV1, _: xdg_activation_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for State {
        fn event(state: &mut Self, _: &xdg_activation_token_v1::XdgActivationTokenV1, ev: xdg_activation_token_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            if let xdg_activation_token_v1::Event::Done { token } = ev {
                state.token = Some(token);
            }
        }
    }

    extern "C" {
        fn gdk_wayland_window_get_wl_surface(window: *mut gtk::gdk::ffi::GdkWindow) -> *mut std::ffi::c_void;
        fn gdk_wayland_display_get_wl_display(display: *mut gtk::gdk::ffi::GdkDisplay) -> *mut std::ffi::c_void;
    }

    /// On the GTK thread: activate a shown GTK window with `token`, using
    /// GTK's own Wayland connection. Does nothing under X11.
    pub(super) fn activate_gtk(window: &gtk::Window, token: &str) {
        use gtk::glib::translate::ToGlibPtr;
        use gtk::prelude::*;
        let Some(gdk_window) = window.window() else { return };
        let display = gdk_window.display();
        if !display.type_().name().contains("Wayland") {
            return;
        }
        // SAFETY: both are GTK's live Wayland objects, checked above.
        let (wl_display, wl_surface) = unsafe {
            (gdk_wayland_display_get_wl_display(display.to_glib_none().0), gdk_wayland_window_get_wl_surface(gdk_window.to_glib_none().0))
        };
        if wl_display.is_null() || wl_surface.is_null() {
            return;
        }
        let r = (|| -> Option<()> {
            // SAFETY: GTK's display and the window's surface outlive this call.
            let backend = unsafe { wayland_backend::sys::client::Backend::from_foreign_display(wl_display.cast()) };
            let conn = Connection::from_backend(backend);
            let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
            let qh = queue.handle();
            let activation: xdg_activation_v1::XdgActivationV1 = globals.bind(&qh, 1..=1, ()).ok()?;
            // SAFETY: `wl_surface` is the window's live surface (non-null, checked above).
            let id = unsafe { wayland_backend::sys::client::ObjectId::from_ptr(WlSurface::interface(), wl_surface.cast()) }.ok()?;
            let surface = WlSurface::from_id(&conn, id).ok()?;
            activation.activate(token.to_string(), &surface);
            activation.destroy();
            queue.roundtrip(&mut State { token: None }).ok()?;
            Some(())
        })();
        log::info!("activated the new window: {}", r.is_some());
    }

    /// A fresh token, or None (not Wayland, or the compositor said no).
    pub(super) fn token() -> Option<String> {
        let (display, surface) = (*MAIN.lock().ok()?)?;
        // SAFETY: winit's display and surface live as long as the main window,
        // which outlives every composer; this only adds a queue of our own.
        let backend = unsafe { wayland_backend::sys::client::Backend::from_foreign_display(display as *mut _) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let activation: xdg_activation_v1::XdgActivationV1 = globals.bind(&qh, 1..=1, ()).ok()?;
        // SAFETY: as above; the pointer is winit's wl_surface.
        let id = unsafe { wayland_backend::sys::client::ObjectId::from_ptr(WlSurface::interface(), surface as *mut _) }.ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        let request = activation.get_activation_token(&qh, ());
        request.set_app_id("co.timson.OwlPost".into());
        request.set_surface(&surface);
        request.commit();
        let mut state = State { token: None };
        for _ in 0..4 {
            if queue.roundtrip(&mut state).is_err() || state.token.is_some() {
                break;
            }
        }
        request.destroy();
        activation.destroy();
        let _ = conn.flush();
        log::info!("activation token for a new window: {}", if state.token.is_some() { "granted" } else { "none" });
        state.token
    }
}

// ---------------------------------------------------------------------------
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use super::{HostEvent, WindowSpec};
    use eframe::egui;

    pub(super) fn init(_ctx: &egui::Context) -> bool {
        false
    }
    pub(super) fn available() -> bool {
        false
    }
    pub(super) fn open(_id: u64, _spec: WindowSpec) -> anyhow::Result<()> {
        anyhow::bail!("web windows aren't available on this system")
    }
    pub(super) fn eval(_id: u64, _js: String) {}
    pub(super) fn set_title(_id: u64, _title: String) {}
    pub(super) fn show(_id: u64) {}
    pub(super) fn hide(_id: u64) {}
    pub(super) fn close(_id: u64) {}
    pub(super) fn poll() -> Vec<HostEvent> {
        vec![]
    }
}
