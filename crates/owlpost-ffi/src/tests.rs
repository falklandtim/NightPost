//! The phone's calls against the demo accounts (never the network).

use super::*;

fn demo() -> (Arc<Mail>, PathBuf) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("owlpost-ffi-{}-{n}.db", std::process::id()));
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
    assert!(url.starts_with(owlpost_core::oauth::AUTH_URL), "{url}");
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
        // parent and its child by raw path, and OwlPost's hidden folders.
        for path in ["Work/Receipts/2026", "Work", "Work B", "Work/Clients", "Archive Old", "OwlPost Sync", "OwlPost Snoozed"] {
            store.upsert_folder(acc.id, path, owlpost_core::FolderRole::Other).unwrap();
        }
    }
    let folders = mail.folders(acc.id).unwrap();
    let names: Vec<(&str, u32)> = folders.iter().map(|f| (f.path.as_str(), f.depth)).collect();
    let first_other = folders.iter().position(|f| f.kind == FolderKind::Other).unwrap();
    assert!(folders[..first_other].iter().all(|f| f.kind != FolderKind::Other && f.depth == 0), "standard folders first: {names:?}");
    assert_eq!(folders[0].kind, FolderKind::Inbox);
    assert_eq!(folders[0].name, "Inbox");
    // The demo's own folders sit among them, so only look at ours.
    let ours: Vec<(&str, u32)> = names[first_other..].iter().copied().filter(|(p, _)| p.starts_with("Work") || p.starts_with("Archive Old") || p.starts_with("OwlPost")).collect();
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
