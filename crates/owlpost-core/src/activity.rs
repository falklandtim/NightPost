//! Sync health: whether each account is connecting, and a log of recent sync
//! activity. Both live in the database, so the app, its sync threads and the
//! MCP server (a separate process) all see the same thing.

use crate::{AccountId, Store};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, OptionalExtension};

/// How many activity entries to keep (a few days of normal syncing).
const KEEP: i64 = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "error" => Level::Error,
            "warn" => Level::Warn,
            _ => Level::Info,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub at: DateTime<Utc>,
    pub account: Option<AccountId>,
    pub level: Level,
    pub text: String,
}

/// An account's connection, in the four states the sidebar dot shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Not synced yet (just added, or the app hasn't run).
    Unknown,
    /// Syncing fine.
    Ok,
    /// One failed sync or push reconnecting: probably a blip.
    Trouble,
    /// Can't connect: failed repeatedly, or the server refused the sign-in.
    Offline,
}

impl Health {
    pub fn as_str(self) -> &'static str {
        match self {
            Health::Unknown => "unknown",
            Health::Ok => "ok",
            Health::Trouble => "trouble",
            Health::Offline => "offline",
        }
    }
}

/// Push (IMAP IDLE) state, as the app last saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    Connecting,
    Live,
    Retrying,
    Unavailable,
}

impl Push {
    fn as_str(self) -> &'static str {
        match self {
            Push::Connecting => "connecting",
            Push::Live => "live",
            Push::Retrying => "retrying",
            Push::Unavailable => "unavailable",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "connecting" => Push::Connecting,
            "live" => Push::Live,
            "retrying" => Push::Retrying,
            "unavailable" => Push::Unavailable,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub account: AccountId,
    /// The last sync that reached the server.
    pub last_ok: Option<DateTime<Utc>>,
    pub last_try: Option<DateTime<Utc>>,
    /// Failed syncs in a row.
    pub failures: u32,
    /// Why the last sync failed (while `failures > 0`).
    pub error: Option<String>,
    pub push: Option<Push>,
    pub push_detail: Option<String>,
}

impl Status {
    pub fn health(&self) -> Health {
        if self.failures >= 2 || (self.failures >= 1 && self.error.as_deref().is_some_and(is_sign_in_error)) {
            Health::Offline
        } else if self.failures == 1 || self.push == Some(Push::Retrying) {
            Health::Trouble
        } else if self.last_ok.is_some() {
            Health::Ok
        } else {
            Health::Unknown
        }
    }

    /// The server turned down the password or Google sign-in.
    pub fn needs_sign_in(&self) -> bool {
        self.failures > 0 && self.error.as_deref().is_some_and(is_sign_in_error)
    }

    /// One line in plain words, e.g. "Synced 2 min ago · new mail arrives instantly".
    pub fn summary(&self, now: DateTime<Utc>) -> String {
        let synced = match self.last_ok {
            Some(t) => format!("Synced {}", ago(t, now)),
            None => "Not synced yet".to_string(),
        };
        match self.health() {
            Health::Offline | Health::Trouble if self.failures > 0 => {
                let why = explain(self.error.as_deref().unwrap_or("unknown error"));
                let when = if self.failures > 1 { format!("{} tries in a row", self.failures) } else { "trying again".into() };
                format!("Can't connect: {why} ({when}). {synced}.")
            }
            _ => {
                let push = match self.push {
                    Some(Push::Live) => " · new mail arrives instantly",
                    Some(Push::Retrying) => " · push reconnecting",
                    Some(Push::Unavailable) => " · checked every 5 minutes",
                    _ => "",
                };
                format!("{synced}{push}")
            }
        }
    }
}

/// "just now", "4 min ago", "2 h ago", "3 days ago".
pub fn ago(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let s = (now - t).num_seconds().max(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => {
            let d = s / 86_400;
            format!("{d} day{} ago", if d == 1 { "" } else { "s" })
        }
    }
}

fn is_sign_in_error(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    ["authenticationfailed", "authentication failed", "invalid credentials", "login failed", "invalid_grant", "sign in again", "no password", "password not found", "[auth]", "username and password not accepted"]
        .iter()
        .any(|k| e.contains(k))
}

/// A connection error in plain words.
pub fn explain(e: &str) -> String {
    let l = e.to_ascii_lowercase();
    let has = |ks: &[&str]| ks.iter().any(|k| l.contains(k));
    if is_sign_in_error(e) {
        "the server turned down the sign-in; sign in again in Preferences → Accounts".into()
    } else if has(&["failed to lookup address", "nodename nor servname", "name or service not known", "temporary failure in name resolution", "no address associated"]) {
        "can't find the server; there's probably no internet connection".into()
    } else if has(&["network is unreachable", "no route to host", "network is down"]) {
        "no internet connection".into()
    } else if has(&["timed out", "timeout"]) {
        "the server didn't answer in time".into()
    } else if has(&["connection refused"]) {
        "the server refused the connection".into()
    } else if has(&["connection reset", "broken pipe", "unexpected eof", "connection aborted", "closed the connection"]) {
        "the connection dropped".into()
    } else if has(&["certificate", "tls", "handshake"]) {
        "a secure connection couldn't be set up".into()
    } else {
        e.lines().next().unwrap_or(e).chars().take(160).collect()
    }
}

fn from_ts(t: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(t, 0).single().unwrap_or_else(Utc::now)
}

impl Store {
    /// Add an activity entry (and write it to the log).
    pub fn note(&self, account: Option<AccountId>, level: Level, text: impl Into<String>) {
        let text = text.into();
        let who = account
            .and_then(|a| self.conn.query_row("SELECT email FROM accounts WHERE id=?1", [a], |r| r.get::<_, String>(0)).ok())
            .map(|e| format!("{e}: "))
            .unwrap_or_default();
        let lvl = match level {
            Level::Info => log::Level::Info,
            Level::Warn => log::Level::Warn,
            Level::Error => log::Level::Error,
        };
        log::log!(lvl, "{who}{text}");
        let _ = self.conn.execute(
            "INSERT INTO activity (at, account_id, level, text) VALUES (?1, ?2, ?3, ?4)",
            params![Utc::now().timestamp(), account, level.as_str(), text],
        );
        let _ = self.conn.execute("DELETE FROM activity WHERE id <= (SELECT MAX(id) FROM activity) - ?1", [KEEP]);
    }

    /// Recent activity, newest first.
    pub fn activity(&self, limit: usize, account: Option<AccountId>, problems_only: bool) -> Result<Vec<Entry>> {
        let mut st = self.conn.prepare(
            "SELECT at, account_id, level, text FROM activity
             WHERE (?1 IS NULL OR account_id = ?1) AND (?2 = 0 OR level != 'info')
             ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![account, problems_only, limit as i64], |r| {
            Ok(Entry { at: from_ts(r.get(0)?), account: r.get(1)?, level: Level::parse(&r.get::<_, String>(2)?), text: r.get(3)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn ensure_status(&self, account: AccountId) {
        let _ = self.conn.execute("INSERT OR IGNORE INTO sync_status (account_id) SELECT id FROM accounts WHERE id=?1", [account]);
    }

    /// A sync reached the server.
    pub fn sync_ok(&self, account: AccountId) {
        let was = self.sync_status(account).ok();
        self.ensure_status(account);
        let now = Utc::now().timestamp();
        let _ = self.conn.execute("UPDATE sync_status SET last_ok=?2, last_try=?2, failures=0, error=NULL WHERE account_id=?1", params![account, now]);
        if let Some(was) = was.filter(|w| w.failures > 0) {
            self.note(Some(account), Level::Info, format!("Connected again after {} failed {}", was.failures, if was.failures == 1 { "try" } else { "tries" }));
        }
    }

    /// A sync couldn't reach the server (or the server refused it).
    pub fn sync_failed(&self, account: AccountId, error: &str) {
        self.ensure_status(account);
        let now = Utc::now().timestamp();
        let _ = self.conn.execute(
            "UPDATE sync_status SET last_try=?2, failures=failures+1, error=?3 WHERE account_id=?1",
            params![account, now, error],
        );
        let failures = self.sync_status(account).map(|s| s.failures).unwrap_or(1);
        let level = if failures >= 2 || is_sign_in_error(error) { Level::Error } else { Level::Warn };
        self.note(Some(account), level, format!("Couldn't sync: {} ({error})", explain(error)));
    }

    /// Record the push connection's state (noting changes in the activity log).
    pub fn set_push(&self, account: AccountId, push: Push, detail: Option<&str>) {
        let was = self.sync_status(account).ok().and_then(|s| s.push);
        self.ensure_status(account);
        let _ = self.conn.execute("UPDATE sync_status SET push=?2, push_detail=?3 WHERE account_id=?1", params![account, push.as_str(), detail]);
        if was == Some(push) && push != Push::Retrying {
            return;
        }
        match push {
            Push::Live => self.note(Some(account), Level::Info, "Push connected: new mail arrives instantly"),
            Push::Retrying => self.note(Some(account), Level::Warn, format!("Push dropped: {}", detail.map(explain).unwrap_or_default())),
            Push::Unavailable => self.note(Some(account), Level::Info, format!("Push unavailable ({}); checking every 5 minutes", detail.unwrap_or("not supported"))),
            Push::Connecting => {}
        }
    }

    pub fn sync_status(&self, account: AccountId) -> Result<Status> {
        let s = self
            .conn
            .query_row(
                "SELECT last_ok, last_try, failures, error, push, push_detail FROM sync_status WHERE account_id=?1",
                [account],
                |r| {
                    Ok(Status {
                        account,
                        last_ok: r.get::<_, Option<i64>>(0)?.map(from_ts),
                        last_try: r.get::<_, Option<i64>>(1)?.map(from_ts),
                        failures: r.get::<_, i64>(2)? as u32,
                        error: r.get(3)?,
                        push: r.get::<_, Option<String>>(4)?.as_deref().and_then(Push::parse),
                        push_detail: r.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(s.unwrap_or(Status { account, ..Default::default() }))
    }

    /// Every account's status (demo accounts included; they never sync).
    pub fn sync_statuses(&self) -> Result<Vec<Status>> {
        self.accounts()?.iter().map(|a| self.sync_status(a.id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let s = Store::open(&crate::test_db_path("activity")).unwrap();
        crate::demo::seed(&s).unwrap();
        s
    }

    #[test]
    fn health_follows_failures_and_push() {
        let s = store();
        let a = s.accounts().unwrap()[0].id;
        let now = Utc::now();
        assert_eq!(s.sync_status(a).unwrap().health(), Health::Unknown);
        s.sync_ok(a);
        s.set_push(a, Push::Live, None);
        let st = s.sync_status(a).unwrap();
        assert_eq!(st.health(), Health::Ok);
        assert_eq!(st.summary(now), "Synced just now · new mail arrives instantly");
        // One blip is amber; two in a row is red.
        s.sync_failed(a, "connecting to imap.mail.me.com:993: failed to lookup address information: nodename nor servname provided");
        assert_eq!(s.sync_status(a).unwrap().health(), Health::Trouble);
        s.sync_failed(a, "connecting to imap.mail.me.com:993: failed to lookup address information");
        let st = s.sync_status(a).unwrap();
        assert_eq!(st.health(), Health::Offline);
        assert!(st.summary(now).starts_with("Can't connect: can't find the server"), "{}", st.summary(now));
        // Reconnecting clears it and says so.
        s.sync_ok(a);
        assert_eq!(s.sync_status(a).unwrap().health(), Health::Ok);
        let log = s.activity(10, Some(a), false).unwrap();
        assert!(log[0].text.starts_with("Connected again after 2 failed tries"), "{log:?}");
        assert_eq!(s.activity(10, None, true).unwrap().len(), 2, "problems only");
        // A refused password is red straight away.
        s.sync_failed(a, "LOGIN failed: [AUTHENTICATIONFAILED] Invalid credentials");
        let st = s.sync_status(a).unwrap();
        assert_eq!(st.health(), Health::Offline);
        assert!(st.needs_sign_in());
        // Push reconnecting is amber.
        s.sync_ok(a);
        s.set_push(a, Push::Retrying, Some("connection reset by peer"));
        assert_eq!(s.sync_status(a).unwrap().health(), Health::Trouble);
        assert_eq!(s.activity(1, None, false).unwrap()[0].text, "Push dropped: the connection dropped");
    }

    #[test]
    fn activity_is_trimmed() {
        let s = store();
        for i in 0..(KEEP + 5) {
            s.note(None, Level::Info, format!("n{i}"));
        }
        let n: i64 = s.conn.query_row("SELECT COUNT(*) FROM activity", [], |r| r.get(0)).unwrap();
        assert_eq!(n, KEEP);
        assert_eq!(s.activity(1, None, false).unwrap()[0].text, format!("n{}", KEEP + 4));
    }
}
