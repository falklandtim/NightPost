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
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub html: String,
    pub background: (u8, u8, u8),
}

/// Start the host (once). Returns false where web windows aren't available,
/// and callers then use the egui composer instead.
pub fn init(ctx: &egui::Context) -> bool {
    imp::init(ctx)
}

/// Is the host running?
pub fn available() -> bool {
    imp::available()
}

pub fn open(id: u64, spec: WindowSpec) -> anyhow::Result<()> {
    imp::open(id, spec)
}

/// Run script in a window's page.
pub fn eval(id: u64, js: &str) {
    imp::eval(id, js.to_string())
}

pub fn set_title(id: u64, title: &str) {
    imp::set_title(id, title.to_string())
}

pub fn show(id: u64) {
    imp::show(id)
}

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

    pub fn set_ctx(ctx: &egui::Context) {
        let _ = CTX.set(ctx.clone());
    }

    pub fn push(e: HostEvent) {
        if let Ok(mut q) = QUEUE.lock() {
            q.push(e);
        }
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint();
        }
    }

    pub fn take() -> Vec<HostEvent> {
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

    pub fn init(ctx: &egui::Context) -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        events::set_ctx(ctx);
        super::edit_menu::install();
        READY.set(true);
        true
    }

    pub fn available() -> bool {
        READY.get()
    }

    pub fn open(id: u64, spec: WindowSpec) -> anyhow::Result<()> {
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

    pub fn eval(id: u64, js: String) {
        with(id, |w| {
            let _ = w.webview.evaluate_script(&js);
        })
    }

    pub fn set_title(id: u64, title: String) {
        with(id, |w| w.window.setTitle(&NSString::from_str(&title)))
    }

    pub fn show(id: u64) {
        with(id, |w| {
            w.hidden = false;
            w.window.makeKeyAndOrderFront(None);
        })
    }

    pub fn hide(id: u64) {
        with(id, |w| {
            w.hidden = true;
            w.window.orderOut(None);
        })
    }

    pub fn close(id: u64) {
        if let Some(w) = WINS.with(|w| w.borrow_mut().remove(&id)) {
            w.window.orderOut(None);
            w.window.close();
        }
    }

    pub fn poll() -> Vec<HostEvent> {
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

    pub fn install() {
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

    pub fn init(ctx: &egui::Context) -> bool {
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

    pub fn available() -> bool {
        READY.load(Ordering::SeqCst)
    }

    /// Run `f` on the GTK thread.
    fn on_gtk(f: impl FnOnce() + Send + 'static) {
        gtk::glib::MainContext::default().invoke(f);
    }

    pub fn open(id: u64, spec: WindowSpec) -> anyhow::Result<()> {
        if !available() {
            anyhow::bail!("GTK isn't running");
        }
        let (tx, rx) = std::sync::mpsc::channel();
        on_gtk(move || {
            let _ = tx.send(build(id, spec).map_err(|e| format!("{e:#}")));
        });
        rx.recv().map_err(|_| anyhow::anyhow!("the GTK thread stopped"))?.map_err(|e| anyhow::anyhow!(e))
    }

    fn build(id: u64, spec: WindowSpec) -> anyhow::Result<()> {
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

    pub fn eval(id: u64, js: String) {
        with(id, move |w| {
            let _ = w.webview.evaluate_script(&js);
        })
    }

    pub fn set_title(id: u64, title: String) {
        with(id, move |w| w.window.set_title(&title))
    }

    pub fn show(id: u64) {
        with(id, |w| {
            w.window.show_all();
            w.window.present();
        })
    }

    pub fn hide(id: u64) {
        with(id, |w| w.window.hide())
    }

    pub fn close(id: u64) {
        on_gtk(move || {
            if let Some(w) = WINS.with(|w| w.borrow_mut().remove(&id)) {
                drop(w.webview);
                // SAFETY: the window is ours and nothing else holds it.
                unsafe { w.window.destroy() };
            }
        });
    }

    pub fn poll() -> Vec<HostEvent> {
        events::take()
    }
}

// ---------------------------------------------------------------------------
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use super::{HostEvent, WindowSpec};
    use eframe::egui;

    pub fn init(_ctx: &egui::Context) -> bool {
        false
    }
    pub fn available() -> bool {
        false
    }
    pub fn open(_id: u64, _spec: WindowSpec) -> anyhow::Result<()> {
        anyhow::bail!("web windows aren't available on this system")
    }
    pub fn eval(_id: u64, _js: String) {}
    pub fn set_title(_id: u64, _title: String) {}
    pub fn show(_id: u64) {}
    pub fn hide(_id: u64) {}
    pub fn close(_id: u64) {}
    pub fn poll() -> Vec<HostEvent> {
        vec![]
    }
}
