//! Planning a sync: which folders go first, and which can be left alone
//! because nothing in them has changed since the last sync.
//!
//! A full check of a folder (SELECT, every UID, every flag) costs several
//! round trips and a few kilobytes per hundred emails; across a hundred
//! Gmail labels that is most of an ordinary sync. One `STATUS` question
//! per folder says whether anything changed. With CONDSTORE (Gmail,
//! iCloud, Fastmail) its HIGHESTMODSEQ moves on every change, flags
//! included, so an unchanged answer is the whole truth about the server.
//! Without it, the counts miss a star added or a read balanced by an
//! unread, so those folders are still checked in full every
//! [`RECHECK_SECS`]. Every folder is checked in full once a day
//! ([`RECHECK_CONDSTORE_SECS`]) anyway, in case this copy went astray: a
//! change made here that never reached the server moves no counter.

use crate::models::FolderRole;
use anyhow::{Context, Result};

/// Without CONDSTORE, check a folder in full at least this often (seconds),
/// for the flag changes the counts can't see.
pub const RECHECK_SECS: i64 = 60 * 60;

/// With CONDSTORE, check a folder in full at least this often (seconds).
pub const RECHECK_CONDSTORE_SECS: i64 = 24 * 60 * 60;

/// Where a folder comes in a sync: the ones you look at first (Inbox,
/// Sent, Drafts, Archive and Gmail's All Mail), then Trash and Spam, then
/// the rest. Sent and Drafts before All Mail costs Gmail nothing: All Mail
/// reuses their copies (`reuse_known_mail`) as they would its.
pub fn sync_rank(role: FolderRole) -> u8 {
    match role {
        FolderRole::Inbox => 0,
        FolderRole::Sent => 1,
        FolderRole::Drafts => 2,
        FolderRole::Archive => 3,
        FolderRole::All => 4,
        FolderRole::Trash => 5,
        FolderRole::Spam => 6,
        FolderRole::Starred | FolderRole::Other => 7,
    }
}

/// What the server says about a folder without opening it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderStatus {
    pub uid_validity: u32,
    pub uid_next: u32,
    pub messages: u32,
    pub unseen: u32,
    /// CONDSTORE's change counter; `None` when the server has none.
    pub modseq: Option<u64>,
}

impl FolderStatus {
    /// Read a `STATUS` reply. `None` if it isn't one we understand, which
    /// the caller treats as "changed" (a full check is always safe).
    pub fn parse(reply: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(reply);
        let start = text.find("* STATUS ")?;
        // The values are the last bracketed list on the reply's line. A
        // folder name can hold brackets ("Receipts (old)") and can come as
        // a literal, ending its line with `{n}`, so take the first line
        // from here that ends with `)`, and its last `(`.
        let line = text[start..].lines().find(|l| l.trim_end().ends_with(')'))?.trim_end();
        let open = line.rfind('(')?;
        let inner = line.get(open + 1..line.len() - 1)?;
        let mut words = inner.split_ascii_whitespace();
        let (mut validity, mut next, mut messages, mut unseen, mut modseq) = (None, None, None, None, None);
        while let (Some(k), Some(v)) = (words.next(), words.next()) {
            match k.to_ascii_uppercase().as_str() {
                "UIDVALIDITY" => validity = v.parse().ok(),
                "UIDNEXT" => next = v.parse().ok(),
                "MESSAGES" => messages = v.parse().ok(),
                "UNSEEN" => unseen = v.parse().ok(),
                "HIGHESTMODSEQ" => modseq = v.parse().ok(),
                _ => {}
            }
        }
        Some(Self { uid_validity: validity?, uid_next: next?, messages: messages?, unseen: unseen?, modseq })
    }

    /// The status as saved after a sync, to compare with the next one.
    pub fn key(&self) -> String {
        let m = self.modseq.map(|m| m.to_string()).unwrap_or_default();
        format!("v{} n{} m{} u{} h{m}", self.uid_validity, self.uid_next, self.messages, self.unseen)
    }
}

/// Whether a folder can be left alone: it has been synced before, the
/// server's status is what it was after that sync, and the last full
/// check was recent enough (a day with CONDSTORE, an hour without).
pub fn unchanged(saved: Option<&(String, i64)>, now: &FolderStatus, now_secs: i64) -> bool {
    let Some((key, checked_at)) = saved else { return false };
    let limit = if now.modseq.is_some() { RECHECK_CONDSTORE_SECS } else { RECHECK_SECS };
    *key == now.key() && now_secs - checked_at < limit
}

/// Ask the server for a folder's status, without opening it.
pub fn folder_status<T: std::io::Read + std::io::Write>(session: &mut imap::Session<T>, path: &str, condstore: bool) -> Result<Option<FolderStatus>> {
    let items = if condstore { "(UIDVALIDITY UIDNEXT MESSAGES UNSEEN HIGHESTMODSEQ)" } else { "(UIDVALIDITY UIDNEXT MESSAGES UNSEEN)" };
    let reply = session.run_command_and_read_response(format!("STATUS {} {items}", crate::engine::imap_quote(path))).with_context(|| format!("asking about {path}"))?;
    Ok(FolderStatus::parse(&reply))
}

/// Seconds since 1970, for [`unchanged`].
pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folders_you_look_at_come_first() {
        let mut roles = vec![FolderRole::Other, FolderRole::Spam, FolderRole::All, FolderRole::Trash, FolderRole::Drafts, FolderRole::Starred, FolderRole::Sent, FolderRole::Inbox, FolderRole::Archive];
        roles.sort_by_key(|r| sync_rank(*r));
        assert_eq!(&roles[..7], &[FolderRole::Inbox, FolderRole::Sent, FolderRole::Drafts, FolderRole::Archive, FolderRole::All, FolderRole::Trash, FolderRole::Spam]);
    }

    #[test]
    fn reads_a_status_reply() {
        let r = b"* STATUS \"[Gmail]/All Mail\" (UIDVALIDITY 11 UIDNEXT 44292 MESSAGES 231 UNSEEN 3 HIGHESTMODSEQ 7011231777)\r\na4 OK Success\r\n";
        let s = FolderStatus::parse(r).unwrap();
        assert_eq!(s, FolderStatus { uid_validity: 11, uid_next: 44292, messages: 231, unseen: 3, modseq: Some(7011231777) });
    }

    #[test]
    fn brackets_and_literals_in_folder_names() {
        let r = b"* STATUS \"Receipts (old)\" (MESSAGES 2 UIDNEXT 9 UIDVALIDITY 1 UNSEEN 0)\r\na4 OK\r\n";
        assert_eq!(FolderStatus::parse(r).map(|s| (s.messages, s.modseq)), Some((2, None)));
        let r = b"* STATUS {7}\r\nInvoice (MESSAGES 5 UIDNEXT 6 UIDVALIDITY 1 UNSEEN 1)\r\na4 OK\r\n";
        assert_eq!(FolderStatus::parse(r).map(|s| s.messages), Some(5));
    }

    #[test]
    fn a_reply_missing_a_count_isnt_trusted() {
        assert_eq!(FolderStatus::parse(b"* STATUS INBOX (MESSAGES 5 UIDNEXT 6)\r\na4 OK\r\n"), None);
        assert_eq!(FolderStatus::parse(b"a4 NO no such folder\r\n"), None);
        assert_eq!(FolderStatus::parse(b"* STATUS INBOX (MESSAGES x UIDNEXT 6 UIDVALIDITY 1 UNSEEN 0)\r\n"), None);
    }

    #[test]
    fn only_an_unchanged_folder_is_left_alone() {
        let s = FolderStatus { uid_validity: 1, uid_next: 10, messages: 9, unseen: 2, modseq: Some(500) };
        let saved = (s.key(), 1_000);
        // Never synced: check it.
        assert!(!unchanged(None, &s, 1_000));
        // Same counter, within the day.
        assert!(unchanged(Some(&saved), &s, 1_000 + RECHECK_CONDSTORE_SECS - 1));
        // A day on, a full check all the same.
        assert!(!unchanged(Some(&saved), &s, 1_000 + RECHECK_CONDSTORE_SECS));
        // A flag changed: only the counter moved.
        assert!(!unchanged(Some(&saved), &FolderStatus { modseq: Some(501), ..s.clone() }, 1_000));
        // New mail.
        assert!(!unchanged(Some(&saved), &FolderStatus { uid_next: 11, messages: 10, ..s.clone() }, 1_000));
    }

    #[test]
    fn without_condstore_it_is_checked_in_full_now_and_then() {
        let s = FolderStatus { uid_validity: 1, uid_next: 10, messages: 9, unseen: 2, modseq: None };
        let saved = (s.key(), 1_000);
        assert!(unchanged(Some(&saved), &s, 1_000 + RECHECK_SECS - 1));
        assert!(!unchanged(Some(&saved), &s, 1_000 + RECHECK_SECS));
    }

    #[test]
    fn asks_for_the_change_counter_only_where_there_is_one() {
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let replies = b"a1 OK logged in\r\n* STATUS \"My \\\"Box\\\"\" (UIDVALIDITY 1 UIDNEXT 6 MESSAGES 5 UNSEEN 0 HIGHESTMODSEQ 9)\r\na2 OK done\r\n* STATUS INBOX (UIDVALIDITY 1 UIDNEXT 6 MESSAGES 5 UNSEEN 0)\r\na3 OK done\r\n";
        let script = crate::engine::tests_support::Script::new(replies, sent.clone());
        let mut s = imap::Client::new(script).login("u", "p").map_err(|e| e.0).unwrap();
        assert_eq!(folder_status(&mut s, "My \"Box\"", true).unwrap().and_then(|s| s.modseq), Some(9));
        assert_eq!(folder_status(&mut s, "INBOX", false).unwrap().map(|s| s.modseq), Some(None));
        let sent = String::from_utf8(sent.borrow().clone()).unwrap();
        assert!(sent.contains("a2 STATUS \"My \\\"Box\\\"\" (UIDVALIDITY UIDNEXT MESSAGES UNSEEN HIGHESTMODSEQ)\r\n"), "{sent}");
        assert!(sent.contains("a3 STATUS \"INBOX\" (UIDVALIDITY UIDNEXT MESSAGES UNSEEN)\r\n"), "{sent}");
    }
}
