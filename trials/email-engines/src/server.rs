//! The local "remote" server: stands in for the web, on `127.0.0.1` only,
//! so the trial never touches the internet. It serves the trial's pictures
//! under `/images/`, a font under `/fonts/`, and answers `/marker/…` (what
//! a script in `14-scripts` asks for if it runs).
//!
//! Every request it sees is logged. That log is the ground truth for the
//! privacy checks: an engine that fetches something on its own, behind the
//! loader's back, shows up here.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::images::Picture;

/// A running server. It stops when the process ends.
#[derive(Clone)]
pub struct Server {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    /// Start serving `pictures` on a free port.
    pub fn start(pictures: BTreeMap<String, Picture>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let files = Arc::new(Files { pictures, font: system_font() });
        let thread_log = log.clone();
        std::thread::Builder::new().name("remote-server".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, files) = (thread_log.clone(), files.clone());
                // One thread per connection: WebKit opens several at once.
                let _ = std::thread::Builder::new().spawn(move || serve(stream, &log, &files));
            }
        })?;
        Ok(Self { port, log })
    }

    /// `http://127.0.0.1:<port>`, which replaces `{{REMOTE}}` in the corpus.
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Everything requested so far, as full URLs, and clear the log.
    pub fn take_log(&self) -> Vec<String> {
        let base = self.base();
        let mut log = self.log.lock().expect("server log lock is never poisoned: no panics while held");
        log.drain(..).map(|p| format!("{base}{p}")).collect()
    }
}

struct Files {
    pictures: BTreeMap<String, Picture>,
    font: Option<Vec<u8>>,
}

fn serve(stream: TcpStream, log: &Mutex<Vec<String>>, files: &Files) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    // Drain the headers; nothing in them matters here.
    let mut header = String::new();
    while reader.read_line(&mut header).map(|n| n > 2).unwrap_or(false) {
        header.clear();
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    if let Ok(mut l) = log.lock() {
        l.push(path.clone());
    }
    let clean = path.split(['?', '#']).next().unwrap_or("");
    let found: Option<(&str, &[u8])> = if let Some(name) = clean.strip_prefix("/images/") {
        files.pictures.get(name).map(|p| (p.mime, p.bytes.as_slice()))
    } else if clean.starts_with("/fonts/") {
        files.font.as_deref().map(|f| ("font/ttf", f))
    } else if clean.starts_with("/marker/") || clean.starts_with("/track/") {
        Some(("image/gif", crate::images::PIXEL_GIF))
    } else {
        None
    };
    let mut out = &stream;
    let _ = match found {
        Some((mime, body)) => write!(out, "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
            .and_then(|_| out.write_all(body)),
        None => out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    };
}

/// A display font from the system, for the "web font" in `09-styled`.
/// Nothing is committed: the font differs between machines, which the
/// results say.
fn system_font() -> Option<Vec<u8>> {
    [
        "/System/Library/Fonts/Supplemental/Georgia.ttf",
        "/System/Library/Fonts/NewYork.ttf",
        "/usr/share/fonts/dejavu-serif-fonts/DejaVuSerif.ttf",
        "/usr/share/fonts/TTF/DejaVuSerif.ttf",
    ]
    .iter()
    .find_map(|p| std::fs::read(p).ok())
}

/// Fetch `url` from the server with a plain HTTP GET (so it's logged like
/// any other request). Only for `http://127.0.0.1:<port>/…`.
pub fn get(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    if !host.starts_with("127.0.0.1:") {
        return None;
    }
    let mut s = TcpStream::connect(host).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").ok()?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut s, &mut buf).ok()?;
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(buf.get(..split)?).to_string();
    if !head.starts_with("HTTP/1.1 200") {
        return None;
    }
    let mime = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Type: "))
        .unwrap_or("application/octet-stream")
        .to_string();
    Some((mime, buf.get(split + 4..)?.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_and_logs() {
        let s = Server::start(crate::images::all().unwrap()).unwrap();
        let (mime, bytes) = get(&format!("{}/images/logo.png", s.base())).unwrap();
        assert_eq!(mime, "image/png");
        assert!(!bytes.is_empty());
        assert!(get(&format!("{}/nothing", s.base())).is_none());
        assert_eq!(s.take_log(), vec![format!("{}/images/logo.png", s.base()), format!("{}/nothing", s.base())]);
        assert!(s.take_log().is_empty());
    }
}
