//! Tests for the sync engine, with a pretend IMAP server (`Script`) that
//! plays back a server's replies and records what NightPost sent.

use super::tests_support::Script;

#[test]
fn gmail_labels_are_changed_silently() {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let script = Script { replies: std::io::Cursor::new(b"a1 OK logged in\r\na2 OK stored\r\na3 OK stored\r\n".to_vec()), sent: sent.clone() };
    let mut s = imap::Client::new(script).login("u", "p").map_err(|e| e.0).unwrap();
    super::gmail_label(&mut s, "7", "Invoices", true).unwrap();
    super::gmail_label(&mut s, "7", "Invoices", false).unwrap();
    let sent = String::from_utf8(sent.borrow().clone()).unwrap();
    assert!(sent.contains("a2 UID STORE 7 +X-GM-LABELS.SILENT (\"Invoices\")\r\n"), "{sent}");
    assert!(sent.contains("a3 UID STORE 7 -X-GM-LABELS.SILENT (\"Invoices\")\r\n"), "{sent}");
}

/// Why `gmail_label` uses `.SILENT`: the IMAP library can't read Gmail's
/// label echo. If this starts passing, a library update fixed it.
#[test]
fn imap_library_cant_read_gmail_label_echo() {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let replies = b"a1 OK logged in\r\n* 1 FETCH (X-GM-LABELS (\"\\\\Inbox\" Invoices) UID 7)\r\na2 OK stored\r\n".to_vec();
    let script = Script { replies: std::io::Cursor::new(replies), sent };
    let mut s = imap::Client::new(script).login("u", "p").map_err(|e| e.0).unwrap();
    let e = s.uid_store("7", "+X-GM-LABELS (Invoices)").err().map(|e| e.to_string());
    assert_eq!(e.as_deref(), Some("Unable to parse status response"));
}

#[test]
fn copy_quotes_folder_names_with_spaces() {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let script = Script { replies: std::io::Cursor::new(b"a1 OK logged in\r\na2 OK copied\r\n".to_vec()), sent: sent.clone() };
    let mut s = imap::Client::new(script).login("u", "p").map_err(|e| e.0).unwrap();
    super::uid_copy(&mut s, "7", "Deleted Messages").unwrap();
    let sent = String::from_utf8(sent.borrow().clone()).unwrap();
    assert!(sent.contains("a2 UID COPY 7 \"Deleted Messages\"\r\n"), "{sent}");
}

use super::*;

/// A label's first sync files mail that's already here from the local
/// copy, asks only for Message-IDs, and downloads just the rest.
#[test]
fn mail_already_here_isnt_downloaded_again() {
    let p = std::env::temp_dir().join(format!("nightpost-reuse-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let store = Store::open(&p).unwrap();
    crate::demo::seed(&store).unwrap();
    let row = store.threads(&crate::models::MailboxView::Unified(FolderRole::Inbox), "", 1).unwrap()[0].clone();
    let have = store.thread_messages(row.account_id, &row.thread_key).unwrap()[0].clone();
    let account = store.accounts().unwrap().into_iter().find(|a| a.id == have.account_id).unwrap();
    let label = store.upsert_folder(account.id, "Label", FolderRole::Other).unwrap();
    let folder = store.folders(account.id).unwrap().into_iter().find(|f| f.id == label).unwrap();

    let header = |id: &str| format!("Message-ID: <{id}>\r\n\r\n");
    let (known, unknown) = (header(&have.message_id), header("new@example.com"));
    let replies = format!(
        "a1 OK logged in\r\n\
         * 1 FETCH (UID 41 FLAGS (\\Seen) BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{known})\r\n\
         * 2 FETCH (UID 42 FLAGS () BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{unknown})\r\n\
         a2 OK fetched\r\n",
        known.len(),
        unknown.len()
    );
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let script = Script { replies: std::io::Cursor::new(replies.into_bytes()), sent: sent.clone() };
    let mut s = imap::Client::new(script).login("u", "p").map_err(|e| e.0).unwrap();

    // 43 isn't answered at all: it's downloaded the usual way.
    let download = reuse_known_mail(&store, &mut s, &account, &folder, &[43, 42, 41]).unwrap();
    assert_eq!(download, [43, 42]);
    let sent = String::from_utf8(sent.borrow().clone()).unwrap();
    assert!(sent.contains("BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)]") && !sent.contains("BODY.PEEK[]"), "{sent}");
    assert_eq!(store.known_uids(label).unwrap(), [41]);
    let copy = store.thread_copies(account.id, &have.thread_key).unwrap().into_iter().find(|m| m.folder_id == label).unwrap();
    assert_eq!((copy.subject, copy.body_text, copy.unread), (have.subject, have.body_text, false));
    let _ = std::fs::remove_file(&p);
}

/// Sync connections time out instead of waiting for ever on a server
/// that has gone quiet.
#[test]
fn sync_connections_have_timeouts() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Tls };
    let tcp = open_tcp(&server).unwrap();
    assert_eq!(tcp.read_timeout().unwrap(), Some(crate::idle::IO_TIMEOUT));
    assert_eq!(tcp.write_timeout().unwrap(), Some(crate::idle::IO_TIMEOUT));
}

/// IMAP connections (push and sync alike) send keepalives, so a quiet
/// IDLE isn't forgotten on the way to the server.
#[test]
fn imap_connections_keep_alive() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = ServerConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), security: Security::Tls };
    let tcp = open_tcp(&server).unwrap();
    let sock = socket2::SockRef::from(&tcp);
    assert!(sock.keepalive().unwrap());
    assert_eq!(sock.tcp_keepalive_time().unwrap(), KEEPALIVE_AFTER);
    assert_eq!(sock.tcp_keepalive_interval().unwrap(), KEEPALIVE_EVERY);
    assert!(KEEPALIVE_AFTER < crate::idle::CYCLE, "probes start well inside one IDLE wait");
}

/// A stand-in connection: `dead` once the server has hung up.
struct Fake {
    dead: bool,
    generation: u32,
}

#[test]
fn a_dropped_connection_is_replaced_and_the_folder_retried() {
    let mut s = Fake { dead: false, generation: 1 };
    let mut tries = 0;
    let (r, connected) = retry_on_new_connection(
        &mut s,
        |s| {
            tries += 1;
            if s.generation == 1 {
                s.dead = true;
                Err(anyhow!("Connection Lost"))
            } else {
                Ok(7)
            }
        },
        |s| !s.dead,
        || Ok(Fake { dead: false, generation: 2 }),
    );
    assert_eq!((r.unwrap(), connected, tries, s.generation), (7, true, 2, 2));
}

#[test]
fn a_folder_error_on_a_live_connection_isnt_retried() {
    let mut s = Fake { dead: false, generation: 1 };
    let mut reconnected = false;
    let (r, connected) = retry_on_new_connection(&mut s, |_| Err::<u32, _>(anyhow!("NO no such mailbox")), |s| !s.dead, || {
        reconnected = true;
        Ok(Fake { dead: false, generation: 2 })
    });
    assert!(r.is_err() && connected && !reconnected);
}

#[test]
fn no_way_back_stops_the_sync() {
    let mut s = Fake { dead: true, generation: 1 };
    let (r, connected) = retry_on_new_connection(&mut s, |_| Err::<u32, _>(anyhow!("Connection Lost")), |s| !s.dead, || Err(anyhow!("offline")));
    assert!(!connected);
    assert!(format!("{:#}", r.unwrap_err()).contains("reconnecting"));
}

#[test]
fn only_new_mail_is_fetched_after_the_first_sync() {
    let server: HashSet<u32> = (1..=1000).collect();
    // First sync: the newest window.
    let first = uids_to_fetch(&server, &HashSet::new(), 0);
    assert_eq!(first.len(), INITIAL_WINDOW);
    assert_eq!((first[0], *first.last().unwrap()), (1000, 701));
    // Next sync, nothing new: nothing to download (older mail stays put).
    let known: HashSet<u32> = (701..=1000).collect();
    assert!(uids_to_fetch(&server, &known, 1000).is_empty());
    // Three new messages arrive (one moved in from elsewhere).
    let mut server2 = server.clone();
    server2.extend([1001, 1002, 1005]);
    assert_eq!(uids_to_fetch(&server2, &known, 1000), vec![1005, 1002, 1001]);
}

#[test]
fn special_use_flags_beat_folder_names() {
    let f = |p: &str, a: &[&str]| (p.to_string(), a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    // Gmail, with labels left behind by Apple Mail.
    let gmail = folder_roles(&[f("INBOX", &[]), f("Drafts", &[]), f("Deleted Messages", &[]), f("[Gmail]/Drafts", &["\\Drafts"]), f("[Gmail]/Trash", &["\\Trash"])]);
    let role = |p: &str| gmail.iter().find(|(q, _)| q == p).unwrap().1;
    assert_eq!(role("INBOX"), FolderRole::Inbox);
    assert_eq!(role("Drafts"), FolderRole::Other);
    assert_eq!(role("Deleted Messages"), FolderRole::Other);
    assert_eq!(role("[Gmail]/Drafts"), FolderRole::Drafts);
    assert_eq!(role("[Gmail]/Trash"), FolderRole::Trash);
    // A server without SPECIAL-USE still gets roles from names.
    let plain = folder_roles(&[f("INBOX", &[]), f("Trash", &[]), f("Sent", &[])]);
    assert_eq!(plain[1].1, FolderRole::Trash);
    assert_eq!(plain[2].1, FolderRole::Sent);
}

#[test]
fn attachments_download_once_and_cache() {
    let p = crate::test_db_path("sp-att");
    let store = Store::open(&p).unwrap();
    crate::demo::seed(&store).unwrap();
    let account = store.accounts().unwrap().into_iter().find(|a| a.demo).unwrap();
    let inbox = store.folder_by_role(account.id, FolderRole::Inbox).unwrap().unwrap();
    let threads = store.threads(&MailboxView::Folder(inbox.id), "has:attachment", 10).unwrap();
    let m = threads
        .iter()
        .flat_map(|t| store.thread_messages(account.id, &t.thread_key).unwrap())
        .find(|m| m.attachments.iter().any(|a| a.filename.ends_with(".pdf")))
        .expect("a demo message with a PDF");
    let cache = std::env::temp_dir().join(format!("sp-att-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    let path = attachment_file_in(&cache, &store, &account, &m, 0).unwrap();
    assert_eq!(path.file_name().unwrap().to_string_lossy(), safe_filename(&m.attachments[0].filename));
    assert!(std::fs::read(&path).unwrap().starts_with(b"%PDF-1.4"));
    // Second time comes from the cache.
    std::fs::write(&path, b"cached").unwrap();
    assert_eq!(std::fs::read(attachment_file_in(&cache, &store, &account, &m, 0).unwrap()).unwrap(), b"cached");
    assert!(attachment_file_in(&cache, &store, &account, &m, 5).is_err());
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn drafts_keep_bcc_and_attachments() {
    let store = Store::open(&crate::test_db_path("sp-draft")).unwrap();
    crate::demo::seed(&store).unwrap();
    let account = store.accounts().unwrap().remove(0);
    let out = OutgoingMessage {
        bcc: vec![Contact::new("Sam", "sam@example.com")],
        subject: "Plan".into(),
        body: "Draft text".into(),
        attachments: vec![smtp::OutgoingAttachment::new("notes.txt", b"hello".to_vec())],
        ..Default::default()
    };
    // No recipients at all: fine for a draft, not for sending.
    let empty = OutgoingMessage { subject: "Later".into(), ..Default::default() };
    assert!(smtp::build(&account, &empty).is_err() && smtp::build_draft(&account, &empty).is_ok());
    let (_, raw) = smtp::build_draft(&account, &out).unwrap();
    let dir = std::env::temp_dir().join(format!("sp-draft-{}", std::process::id()));
    let parts = draft_parts(&raw, &dir).unwrap();
    assert_eq!(parts.bcc[0].email, "sam@example.com");
    assert_eq!(parts.attachments.len(), 1);
    assert_eq!(std::fs::read(&parts.attachments[0]).unwrap(), b"hello");
}

#[test]
fn finds_attachments_in_raw_messages() {
    use crate::smtp::{build, OutgoingAttachment, OutgoingMessage};
    let account = Account {
        id: 1,
        display_name: "A".into(),
        email: "a@example.com".into(),
        username: "a@example.com".into(),
        imap: ServerConfig { host: "imap.example.com".into(), port: 993, security: Security::Tls },
        smtp: ServerConfig { host: "smtp.example.com".into(), port: 465, security: Security::Tls },
        color: "#000".into(),
        signature: String::new(),
        demo: false,
        auth: Auth::Password,
    };
    let out = OutgoingMessage {
        to: vec![Contact::new("B", "b@example.com")],
        body: "hi".into(),
        attachments: vec![OutgoingAttachment::new("one.txt", b"first".to_vec()), OutgoingAttachment::new("two.bin", vec![0, 1, 2, 255])],
        ..Default::default()
    };
    let raw = build(&account, &out).unwrap().1;
    assert_eq!(attachment_from_raw(&raw, 1, "two.bin").unwrap(), [0, 1, 2, 255]);
    assert_eq!(attachment_from_raw(&raw, 0, "one.txt").unwrap(), b"first");
    // Order changed on the server: found by name.
    assert_eq!(attachment_from_raw(&raw, 0, "two.bin").unwrap(), [0, 1, 2, 255]);
    assert!(attachment_from_raw(&raw, 0, "missing.pdf").is_err());
    assert_eq!(safe_filename("../../etc/passwd"), "_.._etc_passwd");
    assert_eq!(safe_filename("Q3: plan?.pdf"), "Q3_ plan_.pdf");
    assert_eq!(safe_filename(""), "attachment");
}

#[test]
fn moves_can_be_undone() {
    let p = crate::test_db_path("sp-undo");
    let store = Store::open(&p).unwrap();
    crate::demo::seed(&store).unwrap();
    for gmail in [false, true] {
        let account = store.accounts().unwrap().into_iter().find(|a| a.is_gmail() == gmail).unwrap();
        let inbox = store.folder_by_role(account.id, FolderRole::Inbox).unwrap().unwrap();
        let key = store.threads(&MailboxView::Folder(inbox.id), "", 10).unwrap().remove(0).thread_key;
        for role in [FolderRole::Archive, FolderRole::Trash] {
            let target = folder_for_role(&store, &account, role).unwrap();
            let plan = move_plan(&store, &account, &key, target.id, None).unwrap();
            assert_eq!(plan, [inbox.id], "{role:?} takes it out of the Inbox");
            move_thread(&store, &account, &key, role).unwrap();
            assert!(!store.thread_folder_ids(account.id, &key).unwrap().contains(&inbox.id));
            for f in plan {
                restore_thread(&store, &account, &key, &store.folder(f).unwrap().unwrap(), &target).unwrap();
            }
            let now = store.thread_folder_ids(account.id, &key).unwrap();
            assert!(now.contains(&inbox.id), "undo puts it back in the Inbox ({role:?}, gmail={gmail})");
            if role == FolderRole::Trash {
                assert!(!now.contains(&target.id), "and out of the Trash");
            }
        }
    }
}

#[test]
fn gmail_archive_keeps_labels() {
    let p = crate::test_db_path("sp-eng");
    let store = Store::open(&p).unwrap();
    crate::demo::seed(&store).unwrap();
    let gmail = store.accounts().unwrap().into_iter().find(|a| a.is_gmail()).unwrap();
    let t = store.threads(&MailboxView::Folder(store.folder_by_role(gmail.id, FolderRole::Inbox).unwrap().unwrap().id), "", 10).unwrap().remove(0);
    let label = store.folders(gmail.id).unwrap().into_iter().find(|f| f.role == FolderRole::Other).unwrap();
    set_label(&store, &gmail, &t.thread_key, &label, true).unwrap();
    move_thread(&store, &gmail, &t.thread_key, FolderRole::Archive).unwrap();
    let ids = store.thread_folder_ids(gmail.id, &t.thread_key).unwrap();
    assert!(ids.contains(&label.id), "archiving on Gmail must keep other labels");
    assert!(!ids.contains(&store.folder_by_role(gmail.id, FolderRole::Inbox).unwrap().unwrap().id));
}

#[test]
fn folders_can_be_made_and_deleted() {
    let store = Store::open(&crate::test_db_path("sp-folders")).unwrap();
    crate::demo::seed(&store).unwrap();
    let account = store.accounts().unwrap().into_iter().find(|a| !a.is_gmail()).unwrap();
    let top = create_folder_in(&store, &account, None, "Receipts").unwrap();
    let top = store.folder(top).unwrap().unwrap();
    let sub = create_folder_in(&store, &account, Some(&top), "2026").unwrap();
    let sub = store.folder(sub).unwrap().unwrap();
    assert_eq!(sub.path, "Receipts/2026");
    assert_eq!(create_folder_in(&store, &account, Some(&top), "2026").unwrap(), sub.id, "making it twice finds the first");

    let inbox = store.folder_by_role(account.id, FolderRole::Inbox).unwrap().unwrap();
    assert!(cant_delete_folder(&store, &inbox).unwrap().is_some(), "the Inbox stays");
    assert!(cant_delete_folder(&store, &top).unwrap().is_some(), "not while it has a folder inside");
    assert!(delete_folder(&store, &account, &top).is_err());
    delete_folder(&store, &account, &sub).unwrap();
    assert!(store.folder(sub.id).unwrap().is_none());

    // Mail in a deleted folder ends up in Trash, not gone.
    let key = store.threads(&MailboxView::Folder(inbox.id), "", 10).unwrap().remove(0).thread_key;
    move_thread_to(&store, &account, &key, &top, None).unwrap();
    delete_folder(&store, &account, &top).unwrap();
    let trash = store.folder_by_role(account.id, FolderRole::Trash).unwrap().unwrap();
    assert!(store.thread_folder_ids(account.id, &key).unwrap().contains(&trash.id));
    assert!(store.folder(top.id).unwrap().is_none());
}

#[test]
fn moves_between_accounts() {
    let store = Store::open(&crate::test_db_path("sp-across")).unwrap();
    crate::demo::seed(&store).unwrap();
    let accounts = store.accounts().unwrap();
    let from = accounts.iter().find(|a| !a.is_gmail()).unwrap();
    let to = accounts.iter().find(|a| a.is_gmail()).unwrap();
    let inbox = store.folder_by_role(from.id, FolderRole::Inbox).unwrap().unwrap();
    let key = store.threads(&MailboxView::Folder(inbox.id), "", 10).unwrap().remove(0).thread_key;
    let messages = store.thread_copies(from.id, &key).unwrap().iter().filter(|m| m.folder_id == inbox.id).count();
    let label = store.folders(to.id).unwrap().into_iter().find(|f| f.role == FolderRole::Other).unwrap();

    let wrong = move_thread_across(&store, from, &key, None, to, &store.folder_by_role(from.id, FolderRole::Archive).unwrap().unwrap());
    assert!(wrong.is_err(), "the folder must be in the other account");
    assert_eq!(move_thread_across(&store, from, &key, None, to, &label).unwrap(), messages);

    let copied = store.threads(&MailboxView::Folder(label.id), "", 50).unwrap();
    assert!(copied.iter().any(|t| t.account_id == to.id), "it arrives in the other account");
    let left = store.thread_folder_ids(from.id, &key).unwrap();
    assert!(!left.contains(&inbox.id), "and leaves the first account's Inbox");
    assert!(left.contains(&store.folder_by_role(from.id, FolderRole::Trash).unwrap().unwrap().id), "for its Trash");
}

#[test]
fn compacts_uid_sets() {
    assert_eq!(super::uid_set(&[7, 1, 2, 3, 9, 10]), "1:3,7,9:10");
    assert_eq!(super::uid_set(&[5]), "5");
}

/// A folder the server says hasn't changed is left alone: one STATUS
/// question, no SELECT, no flags. A changed one is checked in full, and
/// its status saved for next time.
#[test]
fn unchanged_folders_are_left_alone() {
    let p = std::env::temp_dir().join(format!("nightpost-unchanged-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let store = Store::open(&p).unwrap();
    crate::demo::seed(&store).unwrap();
    let account = store.accounts().unwrap().remove(0);
    let same = store.upsert_folder(account.id, "Label A", FolderRole::Other).unwrap();
    let moved = store.upsert_folder(account.id, "Label B", FolderRole::Other).unwrap();
    let status = |modseq: u64| crate::sync_plan::FolderStatus { uid_validity: 1, uid_next: 5, messages: 0, unseen: 0, modseq: Some(modseq) };
    store.set_folder_status(same, &status(7).key(), crate::sync_plan::now_secs()).unwrap();
    store.set_folder_status(moved, &status(6).key(), crate::sync_plan::now_secs()).unwrap();
    let folders: Vec<Folder> = store.folders(account.id).unwrap().into_iter().filter(|f| f.id == same || f.id == moved).collect();

    let replies = b"a1 OK logged in\r\n\
        * STATUS \"Label A\" (UIDVALIDITY 1 UIDNEXT 5 MESSAGES 0 UNSEEN 0 HIGHESTMODSEQ 7)\r\na2 OK done\r\n\
        * STATUS \"Label B\" (UIDVALIDITY 1 UIDNEXT 5 MESSAGES 0 UNSEEN 0 HIGHESTMODSEQ 7)\r\na3 OK done\r\n\
        * 0 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\na4 OK [READ-WRITE] done\r\n";
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut s = imap::Client::new(Script::new(replies, sent.clone())).login("u", "p").map_err(|e| e.0).unwrap();
    let (new, failed, unchanged) = sync_folders(&store, &mut s, &account, &folders, true, || Err(anyhow!("no reconnecting in a test")), &mut |_| {}).unwrap();
    assert_eq!((new, failed, unchanged), (0, 0, 1));
    let sent = String::from_utf8(sent.borrow().clone()).unwrap();
    assert!(!sent.contains("SELECT \"Label A\""), "{sent}");
    assert!(sent.contains("SELECT \"Label B\""), "{sent}");
    assert_eq!(store.folder_status(moved).unwrap().map(|(k, _)| k), Some(status(7).key()), "saved for next time");
    let _ = std::fs::remove_file(&p);
}

/// Every account gets its own sync and its own answer, in order (the demo
/// accounts never touch the network, so each finds nothing new).
#[test]
fn accounts_sync_side_by_side() {
    let p = std::env::temp_dir().join(format!("nightpost-side-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let store = Store::open(&p).unwrap();
    crate::demo::seed(&store).unwrap();
    let accounts = store.accounts().unwrap();
    assert!(accounts.len() > 1);
    let results = sync_accounts(&store, accounts.clone(), |_| {});
    let ids: Vec<_> = results.iter().map(|(a, _)| a.id).collect();
    assert_eq!(ids, accounts.iter().map(|a| a.id).collect::<Vec<_>>());
    assert!(results.iter().all(|(_, r)| matches!(r, Ok(0))));
    let _ = std::fs::remove_file(&p);
}
