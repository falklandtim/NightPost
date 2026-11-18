//! Tests for the store, on the demo accounts and made-up mail.

use super::*;
use crate::demo;

fn tmp() -> Store {
    let p = crate::test_db_path("sp-test");
    let _ = std::fs::remove_file(&p);
    Store::open(&p).unwrap()
}

#[test]
fn suggests_people_you_mail_but_not_yourself() {
    let s = tmp();
    demo::seed(&s).unwrap();
    let maya = s.suggest_contacts("may", 5).unwrap();
    assert_eq!(maya.first().map(|c| (c.name.as_str(), c.email.as_str())), Some(("Maya Chen", "maya@chen.family")));
    let me = s.accounts().unwrap()[0].email.clone();
    let mine = s.suggest_contacts(&me[..4], 20).unwrap();
    assert!(mine.iter().all(|c| !c.email.eq_ignore_ascii_case(&me)));
    // Sam is someone Alex wrote to, so sorts well even by first name.
    assert!(s.suggest_contacts("sam", 5).unwrap().iter().any(|c| c.email == "sam@northwind.demo"));
}

/// After "OwlPost Snoozed" is renamed on the server, the local folder
/// follows with its mail, so snoozed conversations aren't mistaken for
/// ones taken out of the folder elsewhere.
#[test]
fn renamed_folder_keeps_its_mail() {
    let s = tmp();
    demo::seed(&s).unwrap();
    let account = s.accounts().unwrap()[0].id;
    let old = s.upsert_folder(account, "OwlPost Snoozed", FolderRole::Other).unwrap();
    let t = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap().into_iter().find(|t| t.account_id == account).unwrap();
    let mut m = s.thread_messages(account, &t.thread_key).unwrap()[0].clone();
    m.folder_id = old;
    m.uid = 9_001;
    s.insert_message(&m).unwrap();

    s.rename_folder(account, "OwlPost Snoozed", "NightPost Snoozed").unwrap();
    let f = s.folders(account).unwrap().into_iter().find(|f| f.path == "NightPost Snoozed").expect("renamed");
    assert_eq!((f.id, f.name.as_str()), (old, "NightPost Snoozed"));
    assert_eq!(s.thread_keys_in_folder(old).unwrap(), [m.thread_key.clone()]);

    // A folder already called that is left alone.
    let other = s.upsert_folder(account, "Spare", FolderRole::Other).unwrap();
    s.rename_folder(account, "Spare", "NightPost Snoozed").unwrap();
    assert!(s.folder(other).unwrap().is_some_and(|f| f.path == "Spare"));
}

/// Real Gmail flags All Mail `\All` and has no Archive folder (unlike
/// the demo, which calls All Mail its Archive): Archive must still show
/// what's been archived, and not what's in the Inbox.
#[test]
fn gmail_archive_is_all_mail_minus_the_inbox() {
    let s = tmp();
    demo::seed(&s).unwrap();
    let mut gmail = s.accounts().unwrap()[1].clone();
    gmail.email = "real-gmail@example.com".into();
    gmail.id = s.add_account(&gmail).unwrap();
    let inbox = s.upsert_folder(gmail.id, "INBOX", FolderRole::Inbox).unwrap();
    let all = s.upsert_folder(gmail.id, "[Gmail]/All Mail", FolderRole::All).unwrap();
    let template = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 1).unwrap()[0].clone();
    let template = s.thread_messages(template.account_id, &template.thread_key).unwrap()[0].clone();
    let add = |id: &str, subject: &str, folder: FolderId, uid: u32| {
        let mut m = template.clone();
        m.account_id = gmail.id;
        m.folder_id = folder;
        m.uid = uid;
        m.message_id = id.into();
        m.subject = subject.into();
        m.in_reply_to = String::new();
        m.references = vec![];
        m.thread_key = s.resolve_thread_key(gmail.id, id, "", &[]).unwrap();
        s.insert_message(&m).unwrap();
    };
    // Still in the Inbox: a copy in both.
    add("waiting@x", "Still in the inbox", inbox, 1);
    add("waiting@x", "Still in the inbox", all, 1);
    // Archived: only in All Mail.
    add("done@x", "Archived already", all, 2);

    let archive = s.threads(&MailboxView::Unified(FolderRole::Archive), "", 100).unwrap();
    let mine: Vec<&str> = archive.iter().filter(|t| t.account_id == gmail.id).map(|t| t.subject.as_str()).collect();
    assert_eq!(mine, ["Archived already"]);
    // The iCloud-style account's real Archive still shows.
    let icloud = s.accounts().unwrap()[0].id;
    assert!(archive.iter().any(|t| t.account_id == icloud));
}

/// Gmail files your reply in Sent, not the Inbox. The Inbox row's
/// preview should still show it, while the row keeps its place and its
/// time (when the last email reached the Inbox). Drafts don't count.
#[test]
fn preview_shows_your_reply_from_sent() {
    let s = tmp();
    demo::seed(&s).unwrap();
    let mut gmail = s.accounts().unwrap()[1].clone();
    gmail.email = "real-gmail@example.com".into();
    gmail.id = s.add_account(&gmail).unwrap();
    let inbox = s.upsert_folder(gmail.id, "INBOX", FolderRole::Inbox).unwrap();
    let sent = s.upsert_folder(gmail.id, "[Gmail]/Sent Mail", FolderRole::Sent).unwrap();
    let drafts = s.upsert_folder(gmail.id, "[Gmail]/Drafts", FolderRole::Drafts).unwrap();
    let template = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 1).unwrap()[0].clone();
    let template = s.thread_messages(template.account_id, &template.thread_key).unwrap()[0].clone();
    let asked = template.date;
    let add = |id: &str, folder: FolderId, uid: u32, mins: i64, snippet: &str| {
        let mut m = template.clone();
        m.account_id = gmail.id;
        m.folder_id = folder;
        m.uid = uid;
        m.message_id = id.into();
        m.subject = "Lunch?".into();
        m.snippet = snippet.into();
        m.date = asked + chrono::Duration::minutes(mins);
        m.in_reply_to = if id == "ask@x" { String::new() } else { "ask@x".into() };
        m.references = if id == "ask@x" { vec![] } else { vec!["ask@x".into()] };
        m.thread_key = s.resolve_thread_key(gmail.id, id, &m.in_reply_to, &m.references).unwrap();
        s.insert_message(&m).unwrap();
    };
    add("ask@x", inbox, 1, 0, "Are you free for lunch?");
    add("reply@x", sent, 1, 30, "Yes, see you at one.");
    add("draft@x", drafts, 1, 60, "Half-written follow-up");

    let row = |view: MailboxView| s.threads(&view, "", 100).unwrap().into_iter().find(|t| t.account_id == gmail.id).unwrap();
    let t = row(MailboxView::Folder(inbox));
    assert_eq!(t.snippet, "Yes, see you at one.");
    assert_eq!(t.last_date, asked, "the row keeps the Inbox email's time");
    // The Drafts folder still shows the draft itself.
    assert_eq!(row(MailboxView::Folder(drafts)).snippet, "Half-written follow-up");
}

#[test]
fn demo_threads_and_snooze() {
    let s = tmp();
    demo::seed(&s).unwrap();
    let inbox = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
    assert!(!inbox.is_empty());
    // Replies group into their original thread.
    assert!(inbox.iter().any(|t| t.count > 1));
    // Participants include your own replies from Sent, newest last.
    let dinner = inbox.iter().find(|t| t.subject.contains("Dinner")).unwrap();
    assert!(dinner.participants.len() >= 2, "{:?}", dinner.participants);

    let t = &inbox[0];
    s.snooze(t.account_id, &t.thread_key, Utc::now() + chrono::Duration::hours(3)).unwrap();
    let after = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
    assert_eq!(after.len(), inbox.len() - 1);
    assert_eq!(s.snoozed_threads().unwrap().len(), 1);

    // Expire it and wake.
    s.snooze(t.account_id, &t.thread_key, Utc::now() - chrono::Duration::seconds(1)).unwrap();
    let woke = s.wake_due_snoozes().unwrap();
    assert_eq!(woke.len(), 1);
    let back = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
    assert_eq!(back.len(), inbox.len());
    assert!(back.iter().find(|x| x.thread_key == t.thread_key).unwrap().unread);
    // It comes back at the top, as if it had just arrived.
    let woken = back.iter().find(|x| !x.pinned).unwrap();
    assert_eq!(woken.thread_key, t.thread_key);
    assert!(woken.woken.is_some() && woken.list_date() > woken.last_date - chrono::Duration::seconds(1));
}

#[test]
fn signatures_and_defaults() {
    let s = tmp();
    demo::seed(&s).unwrap();
    // The two demo accounts share one "— Alex" signature.
    let sigs = s.signatures().unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(s.accounts().unwrap().iter().all(|a| a.signature == "— Alex"));
    let a = s.accounts().unwrap()[0].clone();
    let work = s.save_signature(&Signature { id: 0, title: "Work".into(), body: "Alex Rivera\nNorthwind".into(), html: false }).unwrap();
    s.set_default_signature(a.id, Some(work)).unwrap();
    assert_eq!(s.account(a.id).unwrap().unwrap().signature, "Alex Rivera\nNorthwind");
    s.set_default_signature(a.id, None).unwrap();
    assert_eq!(s.account(a.id).unwrap().unwrap().signature, "");
    // HTML signatures read as text on the account.
    let html = s.save_signature(&Signature { id: 0, title: "HTML".into(), body: "<p><b>Alex</b> · <a href=\"https://x.co\">x.co</a></p>".into(), html: true }).unwrap();
    s.set_default_signature(a.id, Some(html)).unwrap();
    let acc = s.account(a.id).unwrap().unwrap();
    assert!(acc.signature.contains("Alex") && acc.signature.contains("x.co") && !acc.signature.contains('<'), "{}", acc.signature);
    assert!(s.default_signature(a.id).unwrap().unwrap().html);
    s.set_default_signature(a.id, None).unwrap();
    // Deleting a signature clears it as a default.
    let other = s.accounts().unwrap()[1].id;
    s.delete_signature(sigs[0].id).unwrap();
    assert_eq!(s.account(other).unwrap().unwrap().signature, "");
    // Old per-account signatures are migrated on open.
    s.conn.execute("UPDATE accounts SET signature='Cheers, A' WHERE id=?1", [other]).unwrap();
    let s2 = Store::open(s.path()).unwrap();
    assert_eq!(s2.account(other).unwrap().unwrap().signature, "Cheers, A");
}

#[test]
fn pinned_threads_float_to_top() {
    let s = tmp();
    demo::seed(&s).unwrap();
    let inbox = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
    let last = inbox.last().unwrap();
    s.pin(last.account_id, &last.thread_key).unwrap();
    let after = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
    assert_eq!(after[0].thread_key, last.thread_key);
    assert!(after[0].pinned && !after[1].pinned);
    assert_eq!(after.len(), inbox.len());
    // Still pinned when the list is cut short.
    assert!(s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 1).unwrap().iter().any(|t| t.pinned));
    assert_eq!(s.threads(&MailboxView::Everything, "is:pinned", 100).unwrap().len(), 1);
    s.unpin(last.account_id, &last.thread_key).unwrap();
    assert!(!s.is_pinned(last.account_id, &last.thread_key).unwrap());
}

#[test]
fn search_filters() {
    let s = tmp();
    demo::seed(&s).unwrap();
    let all = s.threads(&MailboxView::Unified(FolderRole::Inbox), "", 100).unwrap();
    let unread = s.threads(&MailboxView::Unified(FolderRole::Inbox), "is:unread", 100).unwrap();
    assert!(unread.len() < all.len());
    let r = s.threads(&MailboxView::Unified(FolderRole::Inbox), "from:nonexistent-zzz", 100).unwrap();
    assert!(r.is_empty());
}
