//! The interface every engine in the trial implements, and the types passed
//! to and from it. One engine at a time is built in (Cargo features), and
//! [`create`] makes it by name.
//!
//! The brief's sketch is followed, with one addition: [`Engine::element_box`].
//! The link and text checks need to know where a known link or paragraph
//! ended up, and that depends on each engine's layout, so the engine itself
//! is asked (by the element's `id`).

use std::time::Duration;

use crate::loader::Loader;

/// One way of drawing an email. Each module in `engines/` implements this.
pub trait Engine {
    /// Short name used in file names and the report, e.g. "litehtml".
    fn name(&self) -> &'static str;
    /// Lay out and draw `page` at `opts`. Must return within `opts.timeout`.
    fn render(&mut self, page: &Page, opts: &RenderOpts) -> anyhow::Result<Rendered>;
    /// After `render`: the link under a point (in points), if any.
    fn link_at(&mut self, x: f32, y: f32) -> Option<String>;
    /// After `render`: the text between two points, if the engine can say.
    fn text_between(&mut self, from: (f32, f32), to: (f32, f32)) -> Option<String>;
    /// After `render`: where the element with this `id` was laid out, in
    /// points from the top left of the page.
    fn element_box(&mut self, id: &str) -> Option<Rect>;
}

/// A rectangle in points.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// The middle of the rectangle.
    pub fn centre(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// One email, ready to draw.
pub struct Page {
    /// The email as the app would show it: tracking pixels stripped by
    /// `html_clean` (unless the test asks for it raw) and wrapped by
    /// `mail_html::email_frame` under the app's Content Security Policy.
    pub html: String,
    /// Answers every resource the engine asks for, and logs it.
    pub loader: Loader,
}

/// How to draw it.
#[derive(Debug, Clone)]
pub struct RenderOpts {
    /// Width of the reading pane in points (600 or 900).
    pub width_pt: u32,
    /// Pixels per point (2.0, as on a Retina display).
    pub scale: f32,
    /// Dark theme colours, through `email_frame`'s `css`.
    pub dark: bool,
    /// As after "Load images": the local "remote" server may be used.
    pub remote_images: bool,
    /// Give up after this long and report a failure.
    pub timeout: Duration,
}

/// What came back from drawing.
pub struct Rendered {
    pub image: image::RgbaImage,
    /// The page's full height in points.
    pub height_pt: f32,
    /// From starting to load the page to having its picture.
    pub first_draw: Duration,
    /// Every resource the engine tried to load, in order.
    pub requests: Vec<String>,
    /// Anything worth saying that isn't an error (e.g. the picture was cut
    /// short at the height limit).
    pub notes: Vec<String>,
}

/// Premultiplied RGBA laid over white (what a browser shows behind a page
/// with no background of its own). For the engines that draw on the CPU.
#[allow(dead_code)] // only used when a CPU engine is built in
pub fn over_white(mut px: Vec<u8>, w: u32, h: u32) -> Option<image::RgbaImage> {
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as u16;
        for c in &mut p[..3] {
            *c = (*c as u16 + (255 - a)).min(255) as u8;
        }
        p[3] = 255;
    }
    image::RgbaImage::from_raw(w, h, px)
}

/// The engines built into this binary, by name.
// One push per engine feature, so most builds push only some of them.
#[allow(clippy::vec_init_then_push)]
pub fn available() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut v = Vec::new();
    #[cfg(feature = "webkit-mac")]
    v.push("webkit-mac");
    #[cfg(feature = "litehtml")]
    v.push("litehtml");
    #[cfg(feature = "blitz")]
    v.push("blitz");
    #[cfg(feature = "servo")]
    v.push("servo");
    #[cfg(feature = "webkit-gtk4")]
    v.push("webkit-gtk4");
    v
}

/// Start the engine called `name`. Fails if it wasn't built in, or doesn't
/// run on this platform.
pub fn create(name: &str) -> anyhow::Result<Box<dyn Engine>> {
    match name {
        #[cfg(all(feature = "webkit-mac", target_os = "macos"))]
        "webkit-mac" => Ok(Box::new(crate::engines::webkit_mac::WebKitMac::new()?)),
        #[cfg(all(feature = "webkit-mac", not(target_os = "macos")))]
        "webkit-mac" => anyhow::bail!("webkit-mac runs on macOS only: it's the Mac's own WKWebView"),
        #[cfg(feature = "litehtml")]
        "litehtml" => Ok(Box::new(crate::engines::litehtml::LiteHtml::new()?)),
        #[cfg(feature = "blitz")]
        "blitz" => Ok(Box::new(crate::engines::blitz::Blitz::new()?)),
        #[cfg(feature = "servo")]
        "servo" => Ok(Box::new(crate::engines::servo::ServoEngine::new()?)),
        #[cfg(all(feature = "webkit-gtk4", target_os = "linux"))]
        "webkit-gtk4" => Ok(Box::new(crate::engines::webkit_gtk4::WebKitGtk4::new()?)),
        #[cfg(all(feature = "webkit-gtk4", not(target_os = "linux")))]
        "webkit-gtk4" => anyhow::bail!("webkit-gtk4 runs on Linux only: it's WebKitGTK 6.0"),
        other => anyhow::bail!("engine {other:?} isn't built in; built in: {:?} (turn one on with --features)", available()),
    }
}
