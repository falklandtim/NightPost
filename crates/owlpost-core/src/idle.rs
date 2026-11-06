//! Push mail with IMAP IDLE (RFC 2177).
//!
//! One extra connection per account sits on the Inbox and waits; the server
//! tells it the moment something changes (new mail, a message read or
//! deleted elsewhere), and the caller syncs just the Inbox. Other folders
//! still sync on the regular timer.

use crate::models::{Account, Security};
use anyhow::{anyhow, Context, Result};
use imap::extensions::idle::{SetReadTimeout, WaitOutcome};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// How long to wait in one IDLE before re-issuing it. RFC 2177 says at most
/// 29 minutes; home routers often drop quiet connections sooner, and each
/// re-issue doubles as a check that the connection is still alive.
pub const CYCLE: Duration = Duration::from_secs(8 * 60);

/// The longest any single read may block outside of IDLE itself, so a
/// connection that died (sleep, Wi-Fi change) is noticed instead of
/// hanging forever.
pub const IO_TIMEOUT: Duration = Duration::from_secs(60);

/// A stream that is never left without a read timeout. The `imap` crate
/// clears the timeout after an IDLE wait and then reads the server's reply
/// to DONE, which would block forever on a dead connection.
pub struct Guarded<S>(pub S);

impl<S: Read> Read for Guarded<S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl<S: Write> Write for Guarded<S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl<S: SetReadTimeout> SetReadTimeout for Guarded<S> {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::error::Result<()> {
        self.0.set_read_timeout(Some(timeout.unwrap_or(IO_TIMEOUT)))
    }
}

pub type IdleSession = imap::Session<Guarded<native_tls::TlsStream<TcpStream>>>;

/// Whether this account can use push. IDLE needs a direct TLS connection
/// (port 993, as iCloud and Gmail use); STARTTLS accounts keep polling.
pub fn can_push(account: &Account) -> bool {
    !account.demo && account.imap.security == Security::Tls
}

/// Open the long-lived IDLE connection and log in.
pub fn connect(account: &Account) -> Result<IdleSession> {
    if !can_push(account) {
        return Err(anyhow!("push needs a TLS (port 993) connection"));
    }
    let s = &account.imap;
    let addr = (s.host.as_str(), s.port)
        .to_socket_addrs()
        .with_context(|| format!("looking up {}", s.host))?
        .next()
        .ok_or_else(|| anyhow!("couldn't find {}", s.host))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(20)).with_context(|| format!("connecting to {}:{}", s.host, s.port))?;
    tcp.set_read_timeout(Some(IO_TIMEOUT))?;
    tcp.set_write_timeout(Some(IO_TIMEOUT))?;
    let tls = native_tls::TlsConnector::builder().build()?.connect(&s.host, tcp).map_err(|e| anyhow!("TLS with {}: {e}", s.host))?;
    let mut client = imap::Client::new(Guarded(tls));
    client.read_greeting()?;
    crate::oauth::imap_login(client, account)
}

/// What the server said while we waited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvent {
    /// Something changed in the mailbox: sync it.
    Changed,
    /// A quiet cycle passed; the connection is still alive.
    Quiet,
}

/// Whether the server supports IDLE (checked first: the `imap` crate
/// panics if the server refuses the command).
pub fn supports_idle<T: Read + Write>(session: &mut imap::Session<T>) -> Result<bool> {
    Ok(session.capabilities()?.has_str("IDLE"))
}

/// Select `mailbox` and wait on it until `on_event` returns false. Each
/// wait lasts at most `cycle`. Returns an error when the connection fails,
/// so the caller can reconnect.
pub fn watch<T: Read + Write + SetReadTimeout>(
    session: &mut imap::Session<T>,
    mailbox: &str,
    cycle: Duration,
    mut on_event: impl FnMut(IdleEvent) -> bool,
) -> Result<()> {
    if !supports_idle(session)? {
        return Err(anyhow!("this server doesn't support push (IMAP IDLE)"));
    }
    session.select(mailbox)?;
    loop {
        let outcome = session.idle()?.wait_with_timeout(cycle)?;
        // A NOOP collects whatever the server queued and proves the link is
        // up (the crate ignores errors when it ends the IDLE).
        session.noop()?;
        let event = match outcome {
            WaitOutcome::MailboxChanged => IdleEvent::Changed,
            WaitOutcome::TimedOut => IdleEvent::Quiet,
        };
        if !on_event(event) {
            return Ok(());
        }
    }
}

/// Seconds to wait before reconnecting after `failures` failures in a row:
/// 5 s, 15 s, 45 s… up to 5 minutes.
pub fn backoff(failures: u32) -> Duration {
    let secs = 5u64.saturating_mul(3u64.saturating_pow(failures.saturating_sub(1).min(10)));
    Duration::from_secs(secs.clamp(5, 300))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// A tiny scripted IMAP server: answers LOGIN, CAPABILITY, SELECT and
    /// NOOP, and for each IDLE plays the next item of `idles` (Some(line) =
    /// push that line, None = stay quiet).
    fn fake_server(caps: &'static str, idles: Vec<Option<&'static str>>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            let mut lines = BufReader::new(stream);
            let mut seen = vec![];
            let mut idles = idles.into_iter();
            out.write_all(b"* OK fake IMAP ready\r\n").unwrap();
            let mut line = String::new();
            let mut idle_tag: Option<String> = None;
            while lines.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                let l = line.trim_end().to_string();
                line.clear();
                seen.push(l.clone());
                if l == "DONE" {
                    let tag = idle_tag.take().unwrap();
                    out.write_all(format!("{tag} OK IDLE terminated\r\n").as_bytes()).unwrap();
                    continue;
                }
                let (tag, cmd) = l.split_once(' ').unwrap();
                let word = cmd.split(' ').next().unwrap().to_ascii_uppercase();
                let reply = match word.as_str() {
                    "LOGIN" => format!("{tag} OK logged in\r\n"),
                    "CAPABILITY" => format!("* CAPABILITY {}\r\n{tag} OK done\r\n", format!("IMAP4rev1 {caps}").trim_end()),
                    "SELECT" => format!("* 3 EXISTS\r\n* FLAGS (\\Seen)\r\n{tag} OK [READ-WRITE] selected\r\n"),
                    "NOOP" => format!("{tag} OK noop\r\n"),
                    "LOGOUT" => format!("* BYE\r\n{tag} OK bye\r\n"),
                    "IDLE" => {
                        idle_tag = Some(tag.to_string());
                        let mut r = "+ idling\r\n".to_string();
                        match idles.next() {
                            Some(Some(push)) => r.push_str(&format!("{push}\r\n")),
                            Some(None) => {}
                            None => return seen, // script over: hang up
                        }
                        r
                    }
                    _ => format!("{tag} BAD unknown\r\n"),
                };
                out.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        (port, handle)
    }

    fn session(port: u16) -> imap::Session<Guarded<TcpStream>> {
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut c = imap::Client::new(Guarded(tcp));
        c.read_greeting().unwrap();
        c.login("me", "pw").map_err(|(e, _)| e).unwrap()
    }

    #[test]
    fn push_reports_new_mail_and_quiet_cycles() {
        let (port, server) = fake_server("IDLE", vec![Some("* 4 EXISTS"), None, Some("* 2 EXPUNGE")]);
        let mut s = session(port);
        let mut events = vec![];
        watch(&mut s, "INBOX", Duration::from_millis(300), |e| {
            events.push(e);
            events.len() < 3
        })
        .unwrap();
        assert_eq!(events, [IdleEvent::Changed, IdleEvent::Quiet, IdleEvent::Changed]);
        drop(s);
        let seen = server.join().unwrap();
        assert!(seen.iter().any(|l| l.ends_with("SELECT \"INBOX\"")), "{seen:?}");
        assert_eq!(seen.iter().filter(|l| l.ends_with(" IDLE")).count(), 3);
        assert_eq!(seen.iter().filter(|l| *l == "DONE").count(), 3, "every IDLE is ended cleanly");
    }

    #[test]
    fn dropped_connection_is_an_error_to_reconnect_from() {
        // The server hangs up during the second IDLE.
        let (port, server) = fake_server("IDLE", vec![Some("* 4 EXISTS")]);
        let mut s = session(port);
        let mut events = vec![];
        let r = watch(&mut s, "INBOX", Duration::from_millis(300), |e| {
            events.push(e);
            true
        });
        assert!(r.is_err());
        assert_eq!(events, [IdleEvent::Changed]);
        server.join().unwrap();
    }

    #[test]
    fn servers_without_idle_are_refused_politely() {
        let (port, server) = fake_server("", vec![]);
        let mut s = session(port);
        let err = watch(&mut s, "INBOX", Duration::from_millis(300), |_| true).unwrap_err();
        assert!(err.to_string().contains("doesn't support push"), "{err}");
        drop(s);
        let seen = server.join().unwrap();
        assert!(!seen.iter().any(|l| l.ends_with(" IDLE")), "never sends IDLE: {seen:?}");
    }

    #[test]
    fn backoff_grows_then_caps() {
        let s: Vec<u64> = (1..=6).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(s, [5, 15, 45, 135, 300, 300]);
        assert_eq!(backoff(0).as_secs(), 5);
        assert_eq!(backoff(1000).as_secs(), 300);
    }

    #[test]
    fn guarded_streams_always_keep_a_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut g = Guarded(tcp);
        g.set_read_timeout(None).unwrap();
        assert_eq!(g.0.read_timeout().unwrap(), Some(IO_TIMEOUT));
        g.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        assert_eq!(g.0.read_timeout().unwrap(), Some(Duration::from_secs(3)));
    }
}
