//! Slim downloads on a made-up email: text, HTML, a PDF and an inline
//! picture. The slim copy must read exactly as the whole email does.

use super::*;

const HEADER: &str = "From: Ann <ann@example.com>\r\nTo: Bob <bob@example.com>\r\nSubject: The report\r\nMessage-ID: <r1@example.com>\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed;\r\n boundary=\"outer\"\r\n\r\n";
const PLAIN_MIME: &str = "Content-Type: text/plain; charset=utf-8\r\n\r\n";
const PLAIN: &str = "Here's the report.";
const HTML_MIME: &str = "Content-Type: text/html; charset=utf-8\r\n\r\n";
const HTML: &str = "<p>Here's the <b>report</b>.</p>";
const PDF_MIME: &str = "Content-Type: application/pdf; name=\"report.pdf\"\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\n";
const PNG_MIME: &str = "Content-Type: image/png; name=\"logo.png\"\r\nContent-Disposition: inline; filename=\"logo.png\"\r\nContent-ID: <logo>\r\nContent-Transfer-Encoding: base64\r\n\r\n";

/// The whole email, as a full download brings it.
fn whole() -> String {
    format!(
        "{HEADER}--outer\r\nContent-Type: multipart/alternative; boundary=\"inner\"\r\n\r\n--inner\r\n{PLAIN_MIME}{PLAIN}\r\n--inner\r\n{HTML_MIME}{HTML}\r\n--inner--\r\n--outer\r\n{PDF_MIME}JVBERi0xLjQKJcfsj6IK\r\n--outer\r\n{PNG_MIME}iVBORw0KGgo=\r\n--outer--\r\n"
    )
}

/// Its structure, as Gmail describes it.
const STRUCTURE: &str = "* 3 FETCH (UID 41 RFC822.SIZE 250000 BODYSTRUCTURE (((\"TEXT\" \"PLAIN\" (\"CHARSET\" \"utf-8\") NIL NIL \"7BIT\" 18 1 NIL NIL NIL)(\"TEXT\" \"HTML\" (\"CHARSET\" \"utf-8\") NIL NIL \"7BIT\" 33 1 NIL NIL NIL) \"ALTERNATIVE\" (\"BOUNDARY\" \"inner\") NIL NIL)(\"APPLICATION\" \"PDF\" (\"NAME\" \"report.pdf\") NIL NIL \"BASE64\" 249000 NIL (\"ATTACHMENT\" (\"FILENAME\" \"report.pdf\")) NIL)(\"IMAGE\" \"PNG\" (\"NAME\" \"logo.png\") \"<logo>\" NIL \"BASE64\" 16 NIL (\"INLINE\" (\"FILENAME\" \"logo.png\")) NIL) \"MIXED\" (\"BOUNDARY\" \"outer\") NIL NIL))\r\n";

fn lit(s: &str) -> String {
    format!("{{{}}}\r\n{s}", s.len())
}

/// The server's answer to the slim fetch.
fn slim_reply() -> String {
    format!(
        "* 3 FETCH (UID 41 FLAGS (\\Seen \\Flagged) BODY[HEADER] {} BODY[1.1.MIME] {} BODY[1.1] {} BODY[1.2.MIME] {} BODY[1.2] {} BODY[2.MIME] {} BODY[3.MIME] {})\r\n",
        lit(HEADER),
        lit(PLAIN_MIME),
        lit(PLAIN),
        lit(HTML_MIME),
        lit(HTML),
        lit(PDF_MIME),
        lit(PNG_MIME)
    )
}

fn structure() -> Part {
    Part::parse(&fetch_responses(STRUCTURE.as_bytes())[0]["BODYSTRUCTURE"]).unwrap()
}

#[test]
fn reads_the_structure() {
    let leaf = |ty: &str, sub: &str, enc: &str, octets| Part::Leaf { ty: ty.into(), subtype: sub.into(), encoding: enc.into(), octets };
    let alt = Part::Multi { subtype: "alternative".into(), parts: vec![leaf("text", "plain", "7bit", 18), leaf("text", "html", "7bit", 33)] };
    let want = Part::Multi { subtype: "mixed".into(), parts: vec![alt, leaf("application", "pdf", "base64", 249000), leaf("image", "png", "base64", 16)] };
    assert_eq!(structure(), want);
    let f = &fetch_responses(STRUCTURE.as_bytes())[0];
    assert_eq!((f["UID"].number(), f["RFC822.SIZE"].number()), (Some(41), Some(250000)));
}

#[test]
fn asks_for_the_text_and_every_parts_header() {
    assert_eq!(
        sections(&structure()),
        ["BODY.PEEK[HEADER]", "BODY.PEEK[1.1.MIME]", "BODY.PEEK[1.1]", "BODY.PEEK[1.2.MIME]", "BODY.PEEK[1.2]", "BODY.PEEK[2.MIME]", "BODY.PEEK[3.MIME]"]
    );
}

/// The point of it all: the slim copy parses to the same email, with the
/// same attachments in the same order (opening one finds it by position).
#[test]
fn a_slim_email_reads_like_the_whole_one() {
    let full = crate::parse::parse_message(whole().as_bytes(), 1, 1, 41).unwrap();
    let got = fetch_responses(slim_reply().as_bytes()).remove(0);
    let part = structure();
    let slim_raw = assemble(&part, &got).unwrap();
    let mut slim = crate::parse::parse_message(&slim_raw, 1, 1, 41).unwrap();
    fill_in_sizes(&mut slim, &part);
    assert_eq!(slim.subject, full.subject);
    assert_eq!(slim.message_id, full.message_id);
    assert_eq!(slim.from.email, full.from.email);
    assert_eq!(slim.body_text, full.body_text);
    assert_eq!(slim.body_html, full.body_html);
    assert_eq!(slim.snippet, full.snippet);
    let names = |m: &Message| m.attachments.iter().map(|a| (a.filename.clone(), a.mime_type.clone())).collect::<Vec<_>>();
    assert_eq!(names(&slim), names(&full));
    assert_eq!(names(&slim).len(), 2, "the PDF and the picture");
    // The PDF's size from the structure: 249,000 base64 characters.
    assert!((180_000..=190_000).contains(&slim.attachments[0].size), "{}", slim.attachments[0].size);
}

#[test]
fn small_or_all_text_emails_come_whole() {
    assert!(worth_slimming(250_000, &structure()));
    assert!(!worth_slimming(SLIM_ABOVE, &structure()), "small");
    let text = |octets| Part::Leaf { ty: "text".into(), subtype: "html".into(), encoding: "7bit".into(), octets };
    assert!(!worth_slimming(250_000, &Part::Multi { subtype: "alternative".into(), parts: vec![text(200_000)] }), "nothing to leave behind");
    assert!(!worth_slimming(250_000, &text(250_000)), "not multipart");
}

#[test]
fn a_missing_section_means_a_whole_download() {
    let mut got = fetch_responses(slim_reply().as_bytes()).remove(0);
    got.remove("BODY[2.MIME]");
    assert_eq!(assemble(&structure(), &got), None);
}

/// Text inside an email that looks like a reply isn't read as one.
#[test]
fn a_literal_that_looks_like_a_reply_isnt_one() {
    let body = "hi\r\n* 9 FETCH (UID 99 FLAGS ())\r\n";
    let raw = format!("* 1 FETCH (UID 4 BODY[1] {})\r\n* 2 FETCH (UID 5)\r\n", lit(body));
    let r = fetch_responses(raw.as_bytes());
    assert_eq!(r.iter().map(|f| f["UID"].number()).collect::<Vec<_>>(), [Some(4), Some(5)]);
    assert_eq!(r[0]["BODY[1]"], Value::Str(body.as_bytes().to_vec()));
}

/// Replies are from outside: nonsense gives nothing, never a crash.
#[test]
fn nonsense_is_survived() {
    let deep = format!("* 1 FETCH ({}", "(".repeat(10_000));
    let cases: [&[u8]; 7] = [
        b"* 1 FETCH (UID 4 BODY[1] {999}\r\nshort)\r\n",
        b"* 1 FETCH (UID 4 BODY[1] {18446744073709551615}\r\n)\r\n",
        b"* 1 FETCH (UID 4 BODY[1 \"x\")\r\n",
        b"* 1 FETCH (UID \"unterminated)\r\n",
        b"* 1 FETCH ",
        deep.as_bytes(),
        b"",
    ];
    for c in cases {
        let r = fetch_responses(c);
        assert!(r.iter().all(|f| f.get("BODY[1]").is_none()), "{}", String::from_utf8_lossy(c));
    }
    assert_eq!(Part::parse(&Value::List(vec![Value::List(vec![])])), None);
    assert_eq!(Part::parse(&Value::Atom("x".into())), None);
}

#[test]
fn fetches_one_email_slim() {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let replies = format!("a1 OK logged in\r\n{}a2 OK done\r\n", slim_reply());
    let script = crate::engine::tests_support::Script::new(replies.as_bytes(), sent.clone());
    let mut s = imap::Client::new(script).login("u", "p").map_err(|e| e.0).unwrap();
    let (raw, unread, starred) = fetch(&mut s, 41, &structure()).unwrap();
    assert!(!unread && starred);
    assert!(crate::parse::parse_message(&raw, 1, 1, 41).is_some());
    let sent = String::from_utf8(sent.borrow().clone()).unwrap();
    assert!(sent.contains("a2 UID FETCH 41 (UID FLAGS BODY.PEEK[HEADER] BODY.PEEK[1.1.MIME] BODY.PEEK[1.1] BODY.PEEK[1.2.MIME] BODY.PEEK[1.2] BODY.PEEK[2.MIME] BODY.PEEK[3.MIME])\r\n"), "{sent}");
}

/// A folder with the big email in it, on a database of its own.
fn reports_folder(name: &str) -> (crate::store::Store, crate::models::Account, crate::models::Folder, std::path::PathBuf) {
    let p = std::env::temp_dir().join(format!("nightpost-slim-{name}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let store = crate::store::Store::open(&p).unwrap();
    crate::demo::seed(&store).unwrap();
    let account = store.accounts().unwrap().remove(0);
    let id = store.upsert_folder(account.id, "Reports", crate::models::FolderRole::Other).unwrap();
    let folder = store.folders(account.id).unwrap().into_iter().find(|f| f.id == id).unwrap();
    (store, account, folder, p)
}

/// The server's side of a first sync of that folder, up to the download.
fn first_sync_replies() -> String {
    let id = "Message-ID: <r1@example.com>\r\n\r\n";
    format!(
        "a1 OK logged in\r\n\
         * 1 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\na2 OK [READ-WRITE] done\r\n\
         * SEARCH 41\r\na3 OK done\r\n\
         * 1 FETCH (UID 41 FLAGS () BODY[HEADER.FIELDS (MESSAGE-ID)] {})\r\na4 OK done\r\n",
        lit(id)
    )
}

fn session(replies: String, sent: &std::rc::Rc<std::cell::RefCell<Vec<u8>>>) -> imap::Session<crate::engine::tests_support::Script> {
    let script = crate::engine::tests_support::Script::new(replies.as_bytes(), sent.clone());
    imap::Client::new(script).login("u", "p").map_err(|e| e.0).unwrap()
}

/// A sync brings the big email down without its PDF, and it reads as it
/// should, attachments and all.
#[test]
fn a_sync_leaves_the_attachments_behind() {
    let (store, account, folder, p) = reports_folder("sync");
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let replies = format!("{}{STRUCTURE}a5 OK done\r\n{}a6 OK done\r\n", first_sync_replies(), slim_reply());
    let mut s = session(replies, &sent);
    let n = crate::engine::sync_folder_slim_first(&store, &mut s, &account, &folder, &|| Err(anyhow!("no reconnecting in a test"))).unwrap();
    assert_eq!(n, 1);
    let sent = String::from_utf8(sent.borrow().clone()).unwrap();
    assert!(sent.contains("BODYSTRUCTURE") && sent.contains("BODY.PEEK[1.1]"), "{sent}");
    assert!(!sent.contains("BODY.PEEK[]"), "never the whole email: {sent}");
    let m = store.thread_messages(account.id, "r1@example.com").unwrap().remove(0);
    assert_eq!((m.subject.as_str(), m.body_text.as_str()), ("The report", PLAIN));
    assert_eq!(m.attachments.iter().map(|a| a.filename.as_str()).collect::<Vec<_>>(), ["report.pdf", "logo.png"]);
    assert!(m.attachments[0].size > 100_000);
    assert!(!m.unread && m.starred);
    let _ = std::fs::remove_file(&p);
}

/// If asking for the structure fails, the connection is dropped and the
/// folder synced again on a new one, with whole emails, as before.
#[test]
fn a_failed_slim_download_falls_back_to_whole_emails() {
    let (store, account, folder, p) = reports_folder("fallback");
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut s = session(format!("{}a5 BAD not today\r\n", first_sync_replies()), &sent);
    let again = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let whole_reply = format!("{}* 1 FETCH (UID 41 FLAGS (\\Seen) BODY[] {})\r\na5 OK done\r\n", first_sync_replies(), lit(&whole()));
    let reconnect = || Ok(session(whole_reply.clone(), &again));
    let n = crate::engine::sync_folder_slim_first(&store, &mut s, &account, &folder, &reconnect).unwrap();
    assert_eq!(n, 1);
    let again = String::from_utf8(again.borrow().clone()).unwrap();
    assert!(again.contains("BODY.PEEK[]") && !again.contains("BODYSTRUCTURE"), "{again}");
    let m = store.thread_messages(account.id, "r1@example.com").unwrap().remove(0);
    assert_eq!(m.attachments.len(), 2);
    let _ = std::fs::remove_file(&p);
}
