//! The resource loader: the only way an engine may get a picture, style
//! sheet or font. It answers `cid:` from the email's own parts, `data:`
//! inline, and the local "remote" server only when remote images are on.
//! Everything else is refused. Every request is logged, refused or not.
//!
//! Engines that can't route loads through here (WebKit fetches `http:` by
//! itself) still show up in the server's own log; see `server.rs`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;

/// A loaded resource.
pub struct Resource {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// One page's loader. Cheap to clone; clones share the log.
#[derive(Clone)]
pub struct Loader {
    /// The email's `cid:` parts by Content-ID (without angle brackets).
    parts: Arc<HashMap<String, (String, Vec<u8>)>>,
    /// `http://127.0.0.1:<port>`, the local "remote" server.
    remote_base: String,
    remote_allowed: bool,
    log: Arc<Mutex<Vec<String>>>,
}

impl Loader {
    /// A loader for one email.
    pub fn new(parts: HashMap<String, (String, Vec<u8>)>, remote_base: String, remote_allowed: bool) -> Self {
        Self { parts: Arc::new(parts), remote_base, remote_allowed, log: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Answer a request, or refuse it (`None`). Logged either way.
    pub fn fetch(&self, url: &str) -> Option<Resource> {
        self.note(url);
        let lower = url.get(..url.len().min(8)).unwrap_or("").to_ascii_lowercase();
        if lower.starts_with("cid:") {
            let id = percent_decode(url.get(4..).unwrap_or("")).trim_matches(['<', '>']).to_string();
            return self.parts.get(&id).map(|(m, b)| Resource { mime: m.clone(), bytes: b.clone() });
        }
        if lower.starts_with("data:") {
            return decode_data(url);
        }
        if self.remote_allowed && url.starts_with(&format!("{}/", self.remote_base)) {
            return crate::server::get(url).map(|(mime, bytes)| Resource { mime, bytes });
        }
        None
    }

    /// Log a request the engine made without asking us for the bytes (e.g.
    /// WebKit's own `cid:` scheme handler calls this before `fetch`).
    pub fn note(&self, url: &str) {
        // A `data:` URL can be megabytes; the log only needs to say what it was.
        let shown = if url.len() > 120 { format!("{}… ({} bytes)", url.get(..60).unwrap_or(""), url.len()) } else { url.to_string() };
        if let Ok(mut l) = self.log.lock() {
            l.push(shown);
        }
    }

    /// Everything requested so far, and clear the log.
    pub fn take_log(&self) -> Vec<String> {
        self.log.lock().map(|mut l| l.drain(..).collect()).unwrap_or_default()
    }

    /// Is `url` one the privacy rules allow with remote images off?
    pub fn is_local(url: &str) -> bool {
        let l = url.to_ascii_lowercase();
        l.starts_with("cid:") || l.starts_with("data:") || l.starts_with("about:")
    }
}

fn decode_data(url: &str) -> Option<Resource> {
    let rest = url.get(5..)?;
    let (head, body) = rest.split_once(',')?;
    let mime = head.split(';').next().filter(|m| !m.is_empty()).unwrap_or("text/plain").to_string();
    let bytes = if head.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD.decode(body.trim()).ok()?
    } else {
        percent_decode(body).into_bytes()
    };
    Some(Resource { mime, bytes })
}

/// `%40` → `@`, as Content-IDs in `cid:` URLs are often escaped.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loader(remote: bool) -> Loader {
        let parts = HashMap::from([("logo@trial".to_string(), ("image/png".to_string(), vec![1, 2, 3]))]);
        Loader::new(parts, "http://127.0.0.1:9".into(), remote)
    }

    #[test]
    fn answers_cid_and_data_refuses_the_rest() {
        let l = loader(false);
        assert_eq!(l.fetch("cid:logo%40trial").map(|r| r.bytes), Some(vec![1, 2, 3]));
        assert_eq!(l.fetch("data:text/plain;base64,aGk=").map(|r| r.bytes), Some(b"hi".to_vec()));
        assert!(l.fetch("http://127.0.0.1:9/images/x.png").is_none(), "remote is off");
        assert!(l.fetch("https://example.com/x.png").is_none());
        assert_eq!(l.take_log().len(), 4);
        assert!(l.take_log().is_empty());
    }

    #[test]
    fn remote_is_only_the_local_server() {
        let l = loader(true);
        assert!(l.fetch("https://example.com/x.png").is_none());
        assert!(l.fetch("http://127.0.0.1:99/x.png").is_none(), "another port is not ours");
    }
}
