//! Where Blitz gets an email's pictures, style sheets and fonts (Linux, with
//! Blitz as the email engine; see `blitz_body.rs`).
//!
//! Blitz asks [`Net`] for every resource. It answers at once for a `data:`
//! picture written into the email, and hands a web address to a small pool
//! of fetcher threads, but only when remote content is allowed for the
//! conversation (the "Load remote images" setting, or "Load images"). That's
//! what WebKit's Content Security Policy allows too (`mail_html::csp`).
//! Everything else is refused with an empty answer, so Blitz never waits on
//! it. `cid:` pictures aren't loaded yet.
//!
//! Fetches go through `nightpost_core::email_view::fetch`: never this
//! machine or the local network (`net_guard`), at most 8 MB, a generic
//! User-Agent, and never on the UI thread. Blitz decodes a picture on the
//! fetcher thread that delivers it and posts it to the email. The fetcher
//! then bumps a counter and asks egui for a frame, and the email is laid
//! out again with the picture in it.

use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use eframe::egui;
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Fetcher threads. Pictures in a newsletter come from a handful of hosts,
/// and a few at a time is plenty without hammering them.
const THREADS: usize = 4;
/// The largest picture decoded, in pixels (a 5,000 × 5,000 photo). A small
/// file can claim to be enormous and take gigabytes once decoded.
const MAX_PICTURE_PIXELS: u64 = 25_000_000;

/// Gets the bytes at a web address (blocking). The real one is
/// `email_view::fetch`; tests use a stand-in.
pub(super) type Fetch = Arc<dyn Fn(&str) -> anyhow::Result<Vec<u8>> + Send + Sync>;

/// A web resource to fetch and hand to Blitz.
struct Job {
    url: String,
    handler: Box<dyn NetHandler>,
    arrived: Arc<AtomicU64>,
    ctx: egui::Context,
}

/// The fetcher threads. They stop when the pool is dropped (switching the
/// engine back to WebKit), once the jobs already queued are done.
pub(super) struct Pool {
    tx: crossbeam_channel::Sender<Job>,
}

impl Pool {
    pub(super) fn new(fetch: Fetch) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<Job>();
        for i in 0..THREADS {
            let (rx, fetch) = (rx.clone(), fetch.clone());
            let spawned = std::thread::Builder::new().name(format!("blitz-fetch-{i}")).spawn(move || {
                for job in rx {
                    let bytes = fetch(&job.url).ok().filter(|b| picture_ok(b)).unwrap_or_default();
                    let Job { url, handler, arrived, ctx } = job;
                    // Blitz decodes here; a bad picture mustn't take the thread down.
                    if catch_unwind(AssertUnwindSafe(|| handler.bytes(url, Bytes::from(bytes)))).is_err() {
                        log::warn!("Blitz couldn't take a picture it was sent");
                    }
                    arrived.fetch_add(1, Ordering::SeqCst);
                    ctx.request_repaint();
                }
            });
            // Fewer threads only means slower pictures; none means none load
            // (each request is then refused, below).
            if let Err(e) = spawned {
                log::warn!("couldn't start an image fetcher: {e}");
            }
        }
        Self { tx }
    }
}

/// Answers one email's requests (see the module docs).
pub(super) struct Net {
    /// The fetchers, when remote content is allowed for this conversation.
    pub(super) remote: Option<Arc<Pool>>,
    /// Bumped each time a fetched resource is handed to Blitz.
    pub(super) arrived: Arc<AtomicU64>,
    pub(super) ctx: egui::Context,
}

impl NetProvider for Net {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str().to_string();
        if url.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("data:")) {
            let bytes = nightpost_core::email_view::data_url(&url).ok().filter(|b| picture_ok(b)).unwrap_or_default();
            handler.bytes(url, Bytes::from(bytes));
            return;
        }
        let web = matches!(request.url.scheme(), "http" | "https");
        match self.remote.as_ref().filter(|_| web) {
            Some(pool) => {
                let job = Job { url, handler, arrived: self.arrived.clone(), ctx: self.ctx.clone() };
                // No fetcher threads left: refuse, so Blitz doesn't wait.
                if let Err(crossbeam_channel::SendError(job)) = pool.tx.send(job) {
                    job.handler.bytes(job.url, Bytes::new());
                }
            }
            None => handler.bytes(url, Bytes::new()),
        }
    }
}

/// Whether bytes are safe to hand Blitz: a picture no bigger than
/// [`MAX_PICTURE_PIXELS`], or not a picture format we know (a style sheet, a
/// font, an SVG), which Blitz doesn't decode as a bitmap. A picture whose
/// size can't be read is refused.
pub(super) fn picture_ok(bytes: &[u8]) -> bool {
    let Ok(reader) = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format() else { return false };
    if reader.format().is_none() {
        return true;
    }
    matches!(reader.into_dimensions(), Ok((w, h)) if u64::from(w) * u64::from(h) <= MAX_PICTURE_PIXELS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what Blitz was answered.
    struct Got(Arc<std::sync::Mutex<Vec<(String, usize)>>>);
    impl NetHandler for Got {
        fn bytes(self: Box<Self>, url: String, bytes: Bytes) {
            self.0.lock().expect("test lock").push((url, bytes.len()));
        }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::new(w, h).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).expect("test PNG");
        out
    }

    fn ask(net: &Net, urls: &[&str]) -> Vec<(String, usize)> {
        let got = Arc::new(std::sync::Mutex::new(Vec::new()));
        for url in urls {
            let url = blitz_traits::net::Url::parse(url).expect("test URL");
            net.fetch(0, Request::get(url), Box::new(Got(got.clone())));
        }
        for _ in 0..200 {
            if got.lock().expect("test lock").len() == urls.len() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let got = got.lock().expect("test lock").clone();
        got
    }

    #[test]
    fn web_addresses_only_when_allowed() {
        let fetched = Arc::new(AtomicU64::new(0));
        let f = fetched.clone();
        let fetch: Fetch = Arc::new(move |_| {
            f.fetch_add(1, Ordering::SeqCst);
            Ok(png(2, 2))
        });
        let urls = ["https://pics.example/a.png", "http://pics.example/b.png", "file:///etc/passwd", "cid:logo@x"];
        let off = Net { remote: None, arrived: Arc::default(), ctx: egui::Context::default() };
        let got = ask(&off, &urls);
        assert_eq!(got.len(), 4, "every request is answered, so Blitz never waits");
        assert!(got.iter().all(|(_, n)| *n == 0), "nothing loads with remote content off: {got:?}");
        assert_eq!(fetched.load(Ordering::SeqCst), 0, "and nothing is fetched");

        let on = Net { remote: Some(Arc::new(Pool::new(fetch))), arrived: Arc::default(), ctx: egui::Context::default() };
        let got = ask(&on, &urls);
        assert_eq!(got.len(), 4);
        assert_eq!(fetched.load(Ordering::SeqCst), 2, "only the two web addresses are fetched");
        assert_eq!(got.iter().filter(|(_, n)| *n > 0).count(), 2, "{got:?}");
        assert_eq!(on.arrived.load(Ordering::SeqCst), 2, "each arrival is counted, for the relayout");
    }

    #[test]
    fn data_urls_answer_at_once() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png(3, 3));
        let net = Net { remote: None, arrived: Arc::default(), ctx: egui::Context::default() };
        let got = ask(&net, &[&format!("data:image/png;base64,{b64}")]);
        assert!(got[0].1 > 0, "the picture written into the email");
    }

    #[test]
    fn refuses_pictures_that_would_be_huge_once_decoded() {
        assert!(picture_ok(&png(40, 30)));
        // A GIF header claiming 65,535 × 65,535 pixels: tiny file, 17 GB decoded.
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b]);
        let claimed = image::ImageReader::new(Cursor::new(&gif)).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok());
        assert_eq!(claimed, Some((65_535, 65_535)), "refused for its size, not for being unreadable");
        assert!(!picture_ok(&gif), "an enormous picture is refused");
        assert!(!picture_ok(b"\x89PNG\r\n\x1a\nbroken"), "a picture whose size can't be read");
        assert!(picture_ok(b"body { color: red }"), "a style sheet isn't a picture");
    }
}
