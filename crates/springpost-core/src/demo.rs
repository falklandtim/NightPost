//! Sample accounts and mail so SpringPost can be tried without a server.
//! Demo accounts are flagged `demo = true` and never touch the network.

use crate::models::*;
use crate::store::Store;
use anyhow::Result;
use chrono::{Duration, Utc};

struct Seed {
    folder: FolderRole,
    from: (&'static str, &'static str),
    to: (&'static str, &'static str),
    subject: &'static str,
    body: &'static str,
    hours_ago: i64,
    id: &'static str,
    reply_to: Option<&'static str>,
    unread: bool,
    starred: bool,
    attachment: Option<&'static str>,
}

const ME_PERSONAL: (&str, &str) = ("Alex Rivera", "alex@icloud.demo");
const ME_WORK: (&str, &str) = ("Alex Rivera", "alex.rivera@gmail.demo");

fn personal() -> Vec<Seed> {
    vec![
        Seed {
            folder: FolderRole::Inbox,
            from: ("Maya Chen", "maya@chen.family"),
            to: ME_PERSONAL,
            subject: "Dinner on Saturday?",
            body: "Hey Alex!\n\nAre you and Sam free for dinner this Saturday? We're thinking of trying the new Basque place on Elm St around 7.\n\nLet me know!\nMaya",
            hours_ago: 30,
            id: "dinner-1@chen.family",
            reply_to: None,
            unread: false,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Sent,
            from: ME_PERSONAL,
            to: ("Maya Chen", "maya@chen.family"),
            subject: "Re: Dinner on Saturday?",
            body: "We'd love to! 7 works. Should I book?\n\nAlex",
            hours_ago: 28,
            id: "dinner-2@icloud.demo",
            reply_to: Some("dinner-1@chen.family"),
            unread: false,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Inbox,
            from: ("Maya Chen", "maya@chen.family"),
            to: ME_PERSONAL,
            subject: "Re: Dinner on Saturday?",
            body: "Already done 😄 Table for four under Chen. See you then!\n\nM",
            hours_ago: 2,
            id: "dinner-3@chen.family",
            reply_to: Some("dinner-2@icloud.demo"),
            unread: true,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Inbox,
            from: ("Apple", "no_reply@email.apple.demo"),
            to: ME_PERSONAL,
            subject: "Your receipt from Apple",
            body: "Receipt\n\niCloud+ with 200 GB storage — Monthly\nTotal: $2.99\n\nThis subscription renews automatically.",
            hours_ago: 50,
            id: "receipt-1@apple.demo",
            reply_to: None,
            unread: false,
            starred: false,
            attachment: Some("receipt.pdf"),
        },
        Seed {
            folder: FolderRole::Inbox,
            from: ("Northside Running Club", "hello@northside.run"),
            to: ME_PERSONAL,
            subject: "Sunday long run: new route 🏃",
            body: "Hi runners,\n\nThis Sunday we're heading along the river path for 14 km. Meet at the boathouse at 8:00. Coffee afterwards at Dock Street.\n\nSee you there,\nThe NRC team",
            hours_ago: 9,
            id: "run-1@northside.run",
            reply_to: None,
            unread: true,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Archive,
            from: ("Jordan Blake", "jordan@blake.studio"),
            to: ME_PERSONAL,
            subject: "Photos from the wedding",
            body: "Here's the full gallery link — the ones by the lake came out especially well.\n\nJordan",
            hours_ago: 24 * 12,
            id: "photos-1@blake.studio",
            reply_to: None,
            unread: false,
            starred: true,
            attachment: None,
        },
    ]
}

fn work() -> Vec<Seed> {
    vec![
        Seed {
            folder: FolderRole::Inbox,
            from: ("Priya Natarajan", "priya@northwind.demo"),
            to: ME_WORK,
            subject: "Q4 roadmap review — slides attached",
            body: "Hi Alex,\n\nAttached are the draft slides for Thursday's roadmap review. Could you take a look at the sync-engine section and flag anything that looks off?\n\nIn particular:\n  • Is the offline-first milestone realistic for December?\n  • Should we move search indexing ahead of snooze?\n\nThanks!\nPriya",
            hours_ago: 5,
            id: "roadmap-1@northwind.demo",
            reply_to: None,
            unread: true,
            starred: true,
            attachment: Some("Q4-roadmap.key"),
        },
        Seed {
            folder: FolderRole::Inbox,
            from: ("GitHub", "notifications@github.demo"),
            to: ME_WORK,
            subject: "[springpost] Fix IMAP IDLE reconnect (#142)",
            body: "@alex requested your review on this pull request.\n\nThis change reconnects IDLE sessions after a NAT timeout and backs off exponentially.\n\n3 files changed, 87 insertions(+), 12 deletions(-)",
            hours_ago: 1,
            id: "gh-142@github.demo",
            reply_to: None,
            unread: true,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Inbox,
            from: ("Sam Okafor", "sam@northwind.demo"),
            to: ME_WORK,
            subject: "Offsite logistics",
            body: "Team — the offsite is confirmed for the 18th–19th. Train tickets are booked; I'll send calendar invites today.\n\nSam",
            hours_ago: 26,
            id: "offsite-1@northwind.demo",
            reply_to: None,
            unread: false,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Sent,
            from: ME_WORK,
            to: ("Sam Okafor", "sam@northwind.demo"),
            subject: "Re: Offsite logistics",
            body: "Thanks Sam! Any dietary form we should fill in?",
            hours_ago: 25,
            id: "offsite-2@gmail.demo",
            reply_to: Some("offsite-1@northwind.demo"),
            unread: false,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Inbox,
            from: ("Sam Okafor", "sam@northwind.demo"),
            to: ME_WORK,
            subject: "Re: Offsite logistics",
            body: "Yes — link coming in the invite. Vegetarian is the default this time.",
            hours_ago: 20,
            id: "offsite-3@northwind.demo",
            reply_to: Some("offsite-2@gmail.demo"),
            unread: false,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Inbox,
            from: ("Linear", "updates@linear.demo"),
            to: ME_WORK,
            subject: "Weekly digest: 12 issues completed",
            body: "Your team completed 12 issues this week. Top contributors: Priya (5), Alex (4), Sam (3).",
            hours_ago: 72,
            id: "linear-1@linear.demo",
            reply_to: None,
            unread: false,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Drafts,
            from: ME_WORK,
            to: ("Priya Natarajan", "priya@northwind.demo"),
            subject: "Hiring plan notes",
            body: "Draft — two backend roles, one design. Need budget sign-off before posting.",
            hours_ago: 40,
            id: "draft-1@gmail.demo",
            reply_to: None,
            unread: false,
            starred: false,
            attachment: None,
        },
        Seed {
            folder: FolderRole::Spam,
            from: ("Prize Center", "winner@totally-legit.demo"),
            to: ME_WORK,
            subject: "You've WON a free cruise!!!",
            body: "Click here to claim your prize.",
            hours_ago: 15,
            id: "spam-1@totally-legit.demo",
            reply_to: None,
            unread: true,
            starred: false,
            attachment: None,
        },
    ]
}

fn seed_account(store: &Store, account: &Account, seeds: Vec<Seed>, roles: &[(&str, FolderRole)]) -> Result<()> {
    let mut folder_ids = std::collections::HashMap::new();
    for (path, role) in roles {
        folder_ids.insert(*role, store.upsert_folder(account.id, path, *role)?);
    }
    // Map our ids to full reference chains as we go.
    let mut chains: std::collections::HashMap<&str, Vec<String>> = Default::default();
    for (i, s) in seeds.into_iter().enumerate() {
        let folder_id = folder_ids[&s.folder];
        let references = match s.reply_to {
            Some(parent) => {
                let mut c = chains.get(parent).cloned().unwrap_or_default();
                c.push(parent.to_string());
                c
            }
            None => vec![],
        };
        chains.insert(s.id, references.clone());
        let mut m = Message {
            id: 0,
            account_id: account.id,
            folder_id,
            uid: i as u32 + 1,
            message_id: s.id.to_string(),
            thread_key: String::new(),
            in_reply_to: s.reply_to.unwrap_or_default().to_string(),
            references,
            subject: s.subject.to_string(),
            from: Contact::new(s.from.0, s.from.1),
            to: vec![Contact::new(s.to.0, s.to.1)],
            cc: vec![],
            date: Utc::now() - Duration::hours(s.hours_ago),
            snippet: crate::parse::make_snippet(s.body),
            body_text: s.body.to_string(),
            body_html: None,
            attachments: s
                .attachment
                .map(|f| vec![Attachment { filename: f.into(), mime_type: "application/octet-stream".into(), size: 482_113 }])
                .unwrap_or_default(),
            unread: s.unread,
            starred: s.starred,
        };
        m.thread_key = store.resolve_thread_key(account.id, &m.message_id, &m.in_reply_to, &m.references)?;
        store.insert_message(&m)?;
    }
    Ok(())
}

/// Add the demo accounts (idempotent).
pub fn seed(store: &Store) -> Result<()> {
    if store.accounts()?.iter().any(|a| a.demo) {
        return Ok(());
    }
    let mk = |name: &str, (_, email): (&str, &str), color: &str, imap: &str, smtp: &str| Account {
        id: 0,
        display_name: name.into(),
        email: email.into(),
        username: email.into(),
        imap: ServerConfig { host: imap.into(), port: 993, security: Security::Tls },
        smtp: ServerConfig { host: smtp.into(), port: 587, security: Security::StartTls },
        color: color.into(),
        signature: "— Alex".into(),
        demo: true,
    };
    let mut personal_acc = mk("Alex Rivera", ME_PERSONAL, "#3693f3", "imap.mail.me.com", "smtp.mail.me.com");
    personal_acc.id = store.add_account(&personal_acc)?;
    seed_account(
        store,
        &personal_acc,
        personal(),
        &[
            ("INBOX", FolderRole::Inbox),
            ("Sent Messages", FolderRole::Sent),
            ("Drafts", FolderRole::Drafts),
            ("Archive", FolderRole::Archive),
            ("Junk", FolderRole::Spam),
            ("Deleted Messages", FolderRole::Trash),
            ("Receipts", FolderRole::Other),
        ],
    )?;
    let mut work_acc = mk("Alex Rivera", ME_WORK, "#d93025", "imap.gmail.com", "smtp.gmail.com");
    work_acc.id = store.add_account(&work_acc)?;
    seed_account(
        store,
        &work_acc,
        work(),
        &[
            ("INBOX", FolderRole::Inbox),
            ("[Gmail]/Sent Mail", FolderRole::Sent),
            ("[Gmail]/Drafts", FolderRole::Drafts),
            ("[Gmail]/All Mail", FolderRole::Archive),
            ("[Gmail]/Spam", FolderRole::Spam),
            ("[Gmail]/Trash", FolderRole::Trash),
            ("Projects/SpringPost", FolderRole::Other),
            ("Clients", FolderRole::Other),
        ],
    )?;
    // Gmail-style labels: the PR notification is also labelled with its project.
    let project = store.upsert_folder(work_acc.id, "Projects/SpringPost", FolderRole::Other)?;
    let key = store.resolve_thread_key(work_acc.id, "gh-142@github.demo", "", &[])?;
    store.copy_thread_local(work_acc.id, &key, project)?;
    Ok(())
}

pub fn remove(store: &Store) -> Result<()> {
    for a in store.accounts()?.into_iter().filter(|a| a.demo) {
        store.remove_account(a.id)?;
    }
    Ok(())
}
