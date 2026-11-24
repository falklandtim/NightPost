//! The phone's calls against the demo accounts (never the network).

use super::*;

fn demo() -> (Arc<Mail>, PathBuf) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("nightpost-ffi-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    (Mail::open_demo(path.to_string_lossy().into_owned()).unwrap(), path)
}

fn inbox(mail: &Mail) -> Vec<ThreadRow> {
    mail.threads(Mailbox::Inbox, String::new(), 200).unwrap()
}

#[test]
fn demo_inbox_lists_every_account() {
    let (mail, _) = demo();
    let accounts = mail.accounts().unwrap();
    assert!(accounts.len() > 1 && accounts.iter().all(|a| a.demo));
    let rows = inbox(&mail);
    assert!(!rows.is_empty());
    for a in &accounts {
        assert!(rows.iter().any(|r| r.account_id == a.id), "{} has inbox mail", a.email);
    }
    assert!(rows.windows(2).all(|w| w[0].pinned >= w[1].pinned), "pinned rows come first");
    assert!(rows.iter().all(|r| !r.people.is_empty() && r.date > 0));

    let unread = mail.threads(Mailbox::Inbox, "is:unread".into(), 200).unwrap();
    assert!(!unread.is_empty() && unread.iter().all(|r| r.unread));
}

#[test]
fn reading_a_conversation() {
    let (mail, _) = demo();
    let row = inbox(&mail).into_iter().max_by_key(|r| r.count).unwrap();
    let msgs = mail.messages(row.account_id, row.thread_key.clone()).unwrap();
    assert_eq!(msgs.len() as i64, row.count);
    assert!(msgs.windows(2).all(|w| w[0].date <= w[1].date), "oldest first");
    assert!(msgs.iter().all(|m| !m.from_email.is_empty()));
    assert!(mail.messages(row.account_id, "no such thread".into()).unwrap().is_empty());
}

#[test]
fn actions_change_the_lists() {
    let (mail, _) = demo();
    let row = inbox(&mail).into_iter().find(|r| r.unread && !r.pinned).unwrap();
    let (acc, key) = (row.account_id, row.thread_key.clone());
    let find = |mail: &Mail| inbox(mail).into_iter().find(|r| r.thread_key == key && r.account_id == acc);

    mail.set_read(acc, key.clone(), true).unwrap();
    assert!(!find(&mail).unwrap().unread);
    mail.set_starred(acc, key.clone(), true).unwrap();
    assert!(find(&mail).unwrap().starred);
    mail.set_pinned(acc, key.clone(), true).unwrap();
    assert!(inbox(&mail)[0].pinned && find(&mail).unwrap().pinned);
    mail.set_pinned(acc, key.clone(), false).unwrap();
    assert!(!find(&mail).unwrap().pinned);

    mail.archive(acc, key.clone()).unwrap();
    assert!(find(&mail).is_none(), "archived out of the Inbox");
    assert!(mail.threads(Mailbox::Archive, String::new(), 500).unwrap().iter().any(|r| r.thread_key == key));
}

#[test]
fn spam_goes_to_the_spam_folder() {
    let (mail, _) = demo();
    let row = inbox(&mail).into_iter().find(|r| !r.pinned).unwrap();
    let (acc, key) = (row.account_id, row.thread_key.clone());
    mail.spam(acc, key.clone()).unwrap();
    assert!(!inbox(&mail).iter().any(|r| r.thread_key == key && r.account_id == acc), "out of the Inbox");
    let spam = mail.folders(acc).unwrap().into_iter().find(|f| f.kind == FolderKind::Spam).unwrap();
    assert!(mail.folder_threads(spam.id, String::new(), 500).unwrap().iter().any(|r| r.thread_key == key));
}

#[test]
fn snooze_and_wake() {
    let (mail, _) = demo();
    let row = inbox(&mail).into_iter().find(|r| !r.pinned).unwrap();
    let (acc, key) = (row.account_id, row.thread_key.clone());
    let later = chrono::Utc::now().timestamp() + 3600;

    assert!(mail.snooze(acc, key.clone(), chrono::Utc::now().timestamp() - 60).is_err(), "not in the past");
    mail.snooze(acc, key.clone(), later).unwrap();
    assert!(!inbox(&mail).iter().any(|r| r.thread_key == key), "hidden while snoozed");
    let snoozed = mail.threads(Mailbox::Snoozed, String::new(), 50).unwrap();
    assert_eq!(snoozed.iter().find(|r| r.thread_key == key).unwrap().snoozed_until, Some(later));

    mail.unsnooze(acc, key.clone()).unwrap();
    assert!(inbox(&mail).iter().any(|r| r.thread_key == key));
    assert!(mail.threads(Mailbox::Snoozed, String::new(), 50).unwrap().is_empty());
}

#[test]
fn unknown_things_are_errors_not_crashes() {
    let (mail, _) = demo();
    let msg = |r: Result<()>| r.unwrap_err().to_string();
    assert!(msg(mail.archive(9999, "x".into())).contains("account"));
    let acc = mail.accounts().unwrap()[0].id;
    assert!(msg(mail.archive(acc, "no such thread".into())).contains("conversation"));
    assert!(msg(mail.set_pinned(acc, "no such thread".into(), true)).contains("conversation"));
    assert!(msg(mail.snooze(acc, "x".into(), i64::MAX)).contains("time"));
    assert!(mail.add_password_account(String::new(), "not an address".into(), "pw".into(), MailProvider::Detect).is_err());
    let own_domain = mail.add_password_account(String::new(), "me@example.org".into(), "pw".into(), MailProvider::Detect).unwrap_err();
    assert!(own_domain.to_string().contains("pick the provider"), "{own_domain}");
}

#[test]
fn own_domains_use_the_provider_picked() {
    // Google Workspace on your own domain uses Gmail's servers.
    let google = MailProvider::Google.resolve("me@example.org");
    assert_eq!(google, Provider::Gmail);
    assert_eq!(google.imap().unwrap().host, "imap.gmail.com");
    assert_eq!(MailProvider::Detect.resolve("me@icloud.com"), Provider::ICloud);
    assert_eq!(MailProvider::Detect.resolve("me@example.org"), Provider::Custom);
    assert_eq!(MailProvider::ICloud.resolve("me@example.org"), Provider::ICloud);
}

#[test]
fn sync_of_demo_accounts_stays_offline() {
    let (mail, _) = demo();
    let results = mail.sync(None).unwrap();
    assert_eq!(results.len(), mail.accounts().unwrap().len());
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
}

#[test]
fn removing_an_account_takes_its_mail() {
    let (mail, _) = demo();
    let gone = mail.accounts().unwrap()[0].id;
    mail.remove_account(gone).unwrap();
    assert!(!mail.accounts().unwrap().iter().any(|a| a.id == gone));
    assert!(!inbox(&mail).iter().any(|r| r.account_id == gone));
}

#[test]
fn google_sign_in_returns_to_the_app() {
    let (mail, _) = demo();
    let id = "123-abc.apps.googleusercontent.com";
    let s = mail.start_google_sign_in(id.into(), Some("tim@example.com".into())).unwrap();
    assert_eq!(s.callback_scheme(), "com.googleusercontent.apps.123-abc");
    let url = s.url();
    assert!(url.starts_with(nightpost_core::oauth::AUTH_URL), "{url}");
    assert!(url.contains("client_id=123-abc.apps.googleusercontent.com"));
    assert!(url.contains("redirect_uri=com.googleusercontent.apps.123-abc%3A%2Foauth2redirect"));
    assert!(url.contains("login_hint=tim%40example.com"));
    assert!(!url.contains("client_secret"), "iOS clients have no secret");
}

#[test]
fn google_sign_in_rejects_bad_client_ids() {
    let (mail, _) = demo();
    for bad in ["", "not-a-client", ".apps.googleusercontent.com", "x.apps.example.com"] {
        assert!(mail.start_google_sign_in(bad.into(), None).is_err(), "{bad:?}");
    }
}

#[test]
fn google_sign_in_refuses_links_that_are_not_its_own() {
    // These fail before any network call or keychain write.
    let (mail, _) = demo();
    let before = mail.accounts().unwrap().len();
    let s = mail.start_google_sign_in("123-abc.apps.googleusercontent.com".into(), None).unwrap();
    let back = "com.googleusercontent.apps.123-abc:/oauth2redirect";
    for link in [
        format!("{back}?state=forged&code=evil"),
        format!("{back}?code=evil"),
        back.to_string(),
        "garbage".to_string(),
    ] {
        assert!(mail.finish_google_sign_in(s.clone(), link.clone()).is_err(), "{link}");
    }
    let state = s.url().split("state=").nth(1).unwrap().split('&').next().unwrap().to_string();
    let MailError::Failed { message } = mail.finish_google_sign_in(s, format!("{back}?state={state}&error=access_denied")).unwrap_err();
    assert!(message.contains("cancelled"), "{message}");
    assert_eq!(mail.accounts().unwrap().len(), before, "no account added");
}

#[test]
fn folders_are_listed_as_a_tree() {
    let (mail, _) = demo();
    let acc = mail.accounts().unwrap()[0].clone();
    {
        let store = mail.store().unwrap();
        // Out of order, a missing middle level, a name that sorts between a
        // parent and its child by raw path, and NightPost's hidden folders.
        for path in ["Work/Receipts/2026", "Work", "Work B", "Work/Clients", "Archive Old", "NightPost Sync", "NightPost Snoozed"] {
            store.upsert_folder(acc.id, path, nightpost_core::FolderRole::Other).unwrap();
        }
    }
    let folders = mail.folders(acc.id).unwrap();
    let names: Vec<(&str, u32)> = folders.iter().map(|f| (f.path.as_str(), f.depth)).collect();
    let first_other = folders.iter().position(|f| f.kind == FolderKind::Other).unwrap();
    assert!(folders[..first_other].iter().all(|f| f.kind != FolderKind::Other && f.depth == 0), "standard folders first: {names:?}");
    assert_eq!(folders[0].kind, FolderKind::Inbox);
    assert_eq!(folders[0].name, "Inbox");
    // The demo's own folders sit among them, so only look at ours.
    let ours: Vec<(&str, u32)> = names[first_other..].iter().copied().filter(|(p, _)| p.starts_with("Work") || p.starts_with("Archive Old") || p.starts_with("NightPost")).collect();
    assert_eq!(
        ours,
        [("Archive Old", 0), ("Work", 0), ("Work/Clients", 1), ("Work/Receipts/2026", 1), ("Work B", 0)],
        "each folder after its parent; hidden folders left out"
    );
    let id = |p: &str| folders.iter().find(|f| f.path == p).unwrap().id;
    let deep = folders.iter().find(|f| f.path == "Work/Receipts/2026").unwrap();
    assert_eq!((deep.name.as_str(), deep.parent_id), ("2026", Some(id("Work"))), "nearest folder that exists");
    assert!(mail.folders(9999).is_err());
}

#[test]
fn a_folder_lists_its_conversations() {
    let (mail, _) = demo();
    let acc = mail.accounts().unwrap()[0].clone();
    let folders = mail.folders(acc.id).unwrap();
    let inbox = folders.iter().find(|f| f.kind == FolderKind::Inbox).unwrap();
    let rows = mail.folder_threads(inbox.id, String::new(), 200).unwrap();
    assert!(!rows.is_empty() && rows.iter().all(|r| r.account_id == acc.id), "only this account's mail");
    let unified = inbox_rows_for(&mail, acc.id);
    assert_eq!(rows.len(), unified, "same conversations as this account's part of the Inbox");
    assert!(mail.folder_threads(9999, String::new(), 10).is_err());
}

fn inbox_rows_for(mail: &Mail, account_id: i64) -> usize {
    inbox(mail).iter().filter(|r| r.account_id == account_id).count()
}

/// Downloads in these tests go to a scratch cache, not the real one.
fn scratch_cache() {
    std::env::set_var("NIGHTPOST_CACHE", std::env::temp_dir().join(format!("nightpost-ffi-cache-{}", std::process::id())));
}

fn message_ids(mail: &Mail, row: &ThreadRow) -> Vec<MessageInfo> {
    mail.messages(row.account_id, row.thread_key.clone()).unwrap()
}

fn draft(account_id: i64, to: &str, html: &str) -> Draft {
    Draft {
        account_id,
        to: to.into(),
        cc: String::new(),
        bcc: String::new(),
        subject: "Phone test".into(),
        html: html.into(),
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
        replaces: None,
    }
}

#[test]
fn html_email_gets_a_safe_page() {
    let (mail, _) = demo();
    let html = inbox(&mail).into_iter().flat_map(|r| message_ids(&mail, &r)).find(|m| m.has_html).expect("the demo has an HTML newsletter");
    let page = mail.message_page(html.id, false).unwrap().unwrap();
    assert!(page.html.contains("Content-Security-Policy") && page.html.contains("default-src 'none'"));
    assert!(page.html.contains("width=device-width") && page.html.contains("<base target=\"_top\">"));
    let open = mail.message_page(html.id, true).unwrap().unwrap();
    assert!(open.html.contains("img-src data: cid: https: http:") && !open.remote_hidden);
    let text = inbox(&mail).into_iter().flat_map(|r| message_ids(&mail, &r)).find(|m| !m.has_html).unwrap();
    assert_eq!(mail.message_page(text.id, false).unwrap(), None, "plain text has no page");
    assert!(mail.message_page(999_999, false).is_err());
}

#[test]
fn trackers_and_remote_pictures_are_held_back() {
    let raw = b"From: a@example.com\r\nTo: b@example.com\r\nSubject: Hi\r\nMessage-ID: <t1@example.com>\r\nContent-Type: text/html\r\n\r\n<p>Hi</p><img src=\"https://cdn.example.com/a.jpg\" width=600><img src=\"https://track.example.net/open.gif\" width=1 height=1>";
    let m = nightpost_core::parse::parse_message(raw, 1, 1, 1).unwrap();
    let page = reader::page(&m, false).unwrap();
    assert!(page.remote_hidden);
    assert_eq!(page.trackers, vec!["track.example.net".to_string()]);
    assert!(!page.html.contains("track.example.net"), "the pixel is gone");
    assert!(page.html.contains("img-src data: cid:;"), "nothing remote loads");
}

#[test]
fn replies_and_forwards_are_set_up() {
    scratch_cache();
    let (mail, _) = demo();
    let row = inbox(&mail).into_iter().find(|r| !message_ids(&mail, r).last().unwrap().from_email.is_empty()).unwrap();
    let last = message_ids(&mail, &row).pop().unwrap();
    let r = mail.start_compose(last.id, ComposeKind::Reply).unwrap();
    assert!(r.to.contains(&last.from_email), "{}", r.to);
    assert!(r.subject.to_lowercase().starts_with("re:"));
    assert!(r.in_reply_to.is_some() && !r.references.is_empty());
    assert!(r.html.starts_with("<div><br></div>") && r.html.contains("wrote:"), "an empty line above the quote");
    assert!(r.attachments.is_empty());

    let f = mail.start_compose(last.id, ComposeKind::Forward).unwrap();
    assert!(f.to.is_empty() && f.subject.starts_with("Fwd: ") && f.in_reply_to.is_none());
    assert!(f.html.contains("Forwarded message"));

    let with_files = inbox(&mail).into_iter().flat_map(|r| message_ids(&mail, &r)).find(|m| !m.attachments.is_empty()).expect("a demo email with attachments");
    let f = mail.start_compose(with_files.id, ComposeKind::Forward).unwrap();
    assert_eq!(f.attachments.len(), with_files.attachments.len(), "forwards keep the attachments");
    assert!(f.attachments.iter().all(|p| std::path::Path::new(p).is_file()));
}

#[test]
fn drafts_save_replace_and_send() {
    let (mail, _) = demo();
    let acc = mail.accounts().unwrap()[0].clone();
    let drafts = |mail: &Mail| mail.threads(Mailbox::Drafts, String::new(), 500).unwrap().into_iter().filter(|r| r.subject == "Phone test").count();
    let before = drafts(&mail);
    let mut d = draft(acc.id, "", "<div>First go</div>");
    let id = mail.save_draft(d.clone()).unwrap();
    assert_eq!(drafts(&mail), before + 1, "saved with no one to send to yet");
    d.replaces = Some(id);
    d.html = "<div>Second go, <b>bold</b></div>".into();
    let id2 = mail.save_draft(d.clone()).unwrap();
    assert_ne!(id, id2);
    assert_eq!(drafts(&mail), before + 1, "the new draft replaces the old");

    d.replaces = Some(id2);
    let MailError::Failed { message } = mail.send(d.clone()).unwrap_err();
    assert!(message.contains("add someone"), "{message}");
    d.to = "Ann <ann@example.com>".into();
    mail.send(d).unwrap();
    assert_eq!(drafts(&mail), before, "sending removes the draft");
    let sent = mail.threads(Mailbox::Sent, String::new(), 200).unwrap();
    let mine = sent.iter().find(|r| r.subject == "Phone test").expect("in Sent");
    let msg = message_ids(&mail, mine).pop().unwrap();
    assert!(msg.body_text.contains("Second go, bold"), "a plain-text version goes too: {}", msg.body_text);
    assert!(msg.has_html);
}

#[test]
fn bad_addresses_and_huge_messages_are_refused() {
    let d = draft(1, "Ann <ann@example.com>, not an address", "<p>x</p>");
    assert!(compose::outgoing(&d).unwrap_err().to_string().contains("isn't an email address"));
    let mut d = draft(1, "ann@example.com", "<p>Hello <i>there</i></p>");
    let out = compose::outgoing(&d).unwrap();
    assert!(out.body.contains("Hello there") && out.html.as_deref().unwrap().contains("<i>there</i>"));
    let big = std::env::temp_dir().join(format!("nightpost-ffi-big-{}.bin", std::process::id()));
    std::fs::write(&big, vec![0u8; 26 * 1024 * 1024]).unwrap();
    d.attachments = vec![big.to_string_lossy().into_owned()];
    assert!(compose::outgoing(&d).unwrap_err().to_string().contains("over 25 MB"));
    let _ = std::fs::remove_file(big);
    d.attachments = vec!["/no/such/file.pdf".into()];
    assert!(compose::outgoing(&d).is_err());
}

#[test]
fn addresses_are_suggested() {
    let (mail, _) = demo();
    let someone = inbox(&mail)[0].latest_email.clone();
    let found = mail.suggest_contacts(someone[..3].to_string()).unwrap();
    assert!(found.iter().any(|c| c.email == someone), "{someone} in {found:?}");
    assert!(mail.suggest_contacts("a".into()).unwrap().is_empty(), "one letter is too few");
}

#[test]
fn sync_progress_is_clear_between_syncs() {
    let (mail, _) = demo();
    assert_eq!(mail.sync_progress(), None);
    mail.sync(None).unwrap();
    assert_eq!(mail.sync_progress(), None, "a finished sync leaves no progress behind");
}

#[test]
fn moving_and_labelling() {
    let (mail, _) = demo();
    let row = inbox(&mail)[0].clone();
    let (a, k) = (row.account_id, row.thread_key.clone());
    let clients = mail.create_folder(a, "Clients".into()).unwrap();
    assert_eq!((clients.name.as_str(), clients.kind), ("Clients", FolderKind::Other));
    assert_eq!(mail.create_folder(a, "clients".into()).unwrap().id, clients.id, "an existing folder is reused");

    mail.set_label(a, k.clone(), clients.id, true).unwrap();
    assert!(mail.filing(a, k.clone()).unwrap().in_folders.contains(&clients.id));
    assert!(inbox(&mail).iter().any(|r| r.thread_key == k), "a label leaves it in the Inbox");
    mail.set_label(a, k.clone(), clients.id, false).unwrap();
    assert!(!mail.filing(a, k.clone()).unwrap().in_folders.contains(&clients.id));

    mail.move_to_folder(a, k.clone(), clients.id, None).unwrap();
    assert!(!inbox(&mail).iter().any(|r| r.thread_key == k), "a move takes it out of the Inbox");
    assert!(mail.folder_threads(clients.id, String::new(), 50).unwrap().iter().any(|r| r.thread_key == k));

    // Another account's folder, or one that's gone, is an error.
    let other = mail.accounts().unwrap().into_iter().find(|x| x.id != a).unwrap();
    let theirs = mail.folders(other.id).unwrap()[0].id;
    assert!(mail.move_to_folder(a, k.clone(), theirs, None).is_err());
    assert!(mail.set_label(a, k, 9999, true).is_err());
}

#[test]
fn inbox_tabs_and_sorting() {
    let (mail, _) = demo();
    let names = |m: &Mail| m.inbox_tabs(None).unwrap().into_iter().map(|t| t.name).collect::<Vec<_>>();
    assert_eq!(names(&mail), ["All", "Primary", "Promotions", "Social"]);
    let tabs = mail.inbox_tabs(None).unwrap();
    assert_eq!(tabs[0].unread as usize, mail.threads(Mailbox::Inbox, "is:unread".into(), 500).unwrap().len());

    let invoices = mail.new_tab("Invoices".into()).unwrap();
    assert_eq!(mail.new_tab("invoices".into()).unwrap(), invoices, "the tab already called that");
    let sixteen = mail.new_tab("16".into()).unwrap();
    assert_ne!(sixteen, invoices, "a tab named like a number is still its own tab");
    // Your own tabs come after the built-in ones (two made in the same
    // second may come in either order).
    let tabs = mail.inbox_tabs(None).unwrap();
    assert_eq!(names(&mail)[..4], ["All", "Primary", "Promotions", "Social"]);
    let mut own: Vec<&str> = tabs[4..].iter().map(|t| t.name.as_str()).collect();
    own.sort();
    assert_eq!(own, ["16", "Invoices"]);
    assert!(tabs[4..].iter().all(|t| t.own) && !tabs[..4].iter().any(|t| t.own));

    let row = inbox(&mail)[0].clone();
    let (a, k) = (row.account_id, row.thread_key.clone());
    mail.put_in_tab(a, k.clone(), Some(invoices)).unwrap();
    let f = mail.filing(a, k.clone()).unwrap();
    assert_eq!((f.tab, f.by_hand), (invoices, true));
    let listed = mail.threads(Mailbox::Inbox, format!("category:{invoices}"), 50).unwrap();
    assert_eq!(listed.iter().map(|r| &r.thread_key).collect::<Vec<_>>(), [&k]);
    mail.put_in_tab(a, k.clone(), None).unwrap();
    assert!(!mail.filing(a, k.clone()).unwrap().by_hand);

    let sender = f.sender.expect("someone else wrote in it");
    assert!(!mail.accounts().unwrap().iter().any(|x| x.email.eq_ignore_ascii_case(&sender)), "never one of your own addresses");
    mail.sort_sender(sender.clone(), Some(nightpost_core::category::SOCIAL)).unwrap();
    assert_eq!(mail.filing(a, k.clone()).unwrap().sender_tab, Some(nightpost_core::category::SOCIAL));
    assert!(mail.sort_sender("nobody".into(), None).is_err());

    mail.remove_tab(invoices).unwrap();
    assert_eq!(names(&mail), ["All", "Primary", "Promotions", "Social", "16"]);
    assert!(mail.remove_tab(nightpost_core::category::PROMOTIONS).is_err(), "built-in tabs stay");
}
