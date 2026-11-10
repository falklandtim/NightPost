//! Offscreen UI tests (egui_kittest): render the app with the demo
//! accounts, click and type, and check what's drawn. `OWLPOST_SHOTS=<dir>`
//! saves screenshots.

use super::*;
use super::list::*;
use super::sidebar::FolderTree;
use egui_kittest::Harness;

fn shots_dir() -> std::path::PathBuf {
    let d = std::env::var("OWLPOST_SHOTS").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("owlpost-shots"));
    let _ = std::fs::create_dir_all(&d);
    d
}

#[test]
fn unread_rows_are_tinted() {
    for (name, th) in [("light", theme::MOONLIT_LIGHT), ("dark", theme::MOONLIT_DARK)] {
        let mut h = harness(th);
        h.run();
        let col = h.state().colors.clone();
        assert!(h.state().rows.iter().any(|r| r.t.unread), "unread mail in the demo inbox");
        let read = h.state().rows.iter().find(|r| !r.t.unread).map(|r| (r.t.account_id, r.t.thread_key.clone())).unwrap();
        h.state_mut().toggle_pin(read.0, &read.1);
        h.state_mut().reload();
        h.run();
        save(&mut h, &format!("unread-rows-{name}"));
        // Down the list's left edge, some rows wear the unread wash.
        if let Ok(img) = h.render() {
            let x = (h.state().sidebar_right * h.ctx.pixels_per_point() + 6.0) as u32;
            let want = unread_wash(&col, false);
            let near = |p: &image::Rgba<u8>| (0..3).all(|c| (p[c] as i32 - want.to_array()[c] as i32).abs() <= 2);
            assert!((0..img.height()).any(|y| near(img.get_pixel(x, y))), "unread rows use the sidebar's highlight ({name})");
            let want = pinned_wash(&col, false);
            let near = |p: &image::Rgba<u8>| (0..3).all(|c| (p[c] as i32 - want.to_array()[c] as i32).abs() <= 2);
            assert!((0..img.height()).any(|y| near(img.get_pixel(x, y))), "pinned rows are yellow ({name})");
        }
    }
}

#[test]
fn inbox_tabs_and_sender_rules() {
    use owlpost_core::category::{PRIMARY, PROMOTIONS, SOCIAL};
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run();
    assert!(h.state().tabs_shown());
    let subjects = |h: &Harness<'static, OwlPostApp>| h.state().rows.iter().map(|r| r.t.subject.clone()).collect::<Vec<_>>();
    assert_eq!(h.state().category, ALL_TAB, "the Inbox opens on All");
    assert!(subjects(&h).iter().any(|s| s.contains("November roast")), "All has everything");
    h.state_mut().category = PRIMARY;
    h.state_mut().reload();
    assert!(!subjects(&h).iter().any(|s| s.contains("November roast")), "the newsletter isn't in Primary");
    h.state_mut().category = PROMOTIONS;
    h.state_mut().reload();
    h.run();
    assert!(subjects(&h).iter().any(|s| s.contains("November roast")));
    save(&mut h, "tabs-promotions");
    // Always put the coffee shop in Social; ⌘Z puts it back.
    h.state_mut().sort_sender("hello@kowhai.demo", Some(SOCIAL));
    assert!(h.state().rows.is_empty());
    h.state_mut().category = SOCIAL;
    h.state_mut().reload();
    assert!(subjects(&h).iter().any(|s| s.contains("November roast")));
    h.state_mut().undo_last();
    h.state_mut().reload();
    assert!(!subjects(&h).iter().any(|s| s.contains("November roast")));
    // All mail from one sender, across folders and tabs.
    h.state_mut().category = PRIMARY;
    h.state_mut().show_sender("hello@kowhai.demo");
    assert!(!h.state().tabs_shown());
    assert_eq!(subjects(&h).len(), 1);
}

#[test]
fn own_tabs() {
    use owlpost_core::category::{PRIMARY, PROMOTIONS};
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run();
    let subjects = |h: &Harness<'static, OwlPostApp>| h.state().rows.iter().map(|r| r.t.subject.clone()).collect::<Vec<_>>();
    // Right-click → Always put their mail in → New tab… → "Invoices".
    h.state_mut().new_tab = Some(NewTab { name: String::new(), then: NewTabFor::Sender("hello@kowhai.demo".into()), error: None });
    h.run();
    save(&mut h, "new-tab-dialog");
    h.state_mut().create_tab("Invoices", &NewTabFor::Sender("hello@kowhai.demo".into())).unwrap();
    h.state_mut().new_tab = None;
    let inv = h.state().store.category_by_name("invoices").unwrap();
    assert!(owlpost_core::category::is_tab(inv));
    assert!(h.state().tabs.iter().any(|(c, n, _)| *c == inv && n == "Invoices"));
    assert!(h.state_mut().create_tab("promotions", &NewTabFor::Sender("x@y.z".into())).is_err(), "built-in names are taken");
    h.state_mut().category = inv;
    h.state_mut().reload();
    h.run();
    assert_eq!(subjects(&h).len(), 1);
    assert!(subjects(&h)[0].contains("November roast"));
    // One conversation by hand.
    h.state_mut().category = PRIMARY;
    h.state_mut().reload();
    let (a, k, subject) = h.state().rows.iter().map(|r| (r.t.account_id, r.t.thread_key.clone(), r.t.subject.clone())).next().unwrap();
    h.state_mut().sort_thread(a, &k, Some(inv));
    assert!(!subjects(&h).contains(&subject));
    h.state_mut().category = inv;
    h.state_mut().reload();
    h.run();
    assert!(subjects(&h).contains(&subject));
    save(&mut h, "tabs-invoices");
    h.state_mut().undo_last();
    h.state_mut().reload();
    assert!(!subjects(&h).contains(&subject));
    // Removing the tab sends its mail back where it came from; undo brings it back.
    h.state_mut().remove_tab(inv);
    assert_eq!(h.state().category, ALL_TAB);
    assert!(!h.state().tabs.iter().any(|(c, ..)| *c == inv));
    h.state_mut().category = PROMOTIONS;
    h.state_mut().reload();
    assert!(subjects(&h).iter().any(|s| s.contains("November roast")));
    h.state_mut().undo_last();
    assert!(h.state().tabs.iter().any(|(c, ..)| *c == inv));
    assert!(!subjects(&h).iter().any(|s| s.contains("November roast")));
}

fn harness(theme: &str) -> Harness<'static, OwlPostApp> {
    harness_with(theme, |_| {})
}

fn harness_with(theme: &str, setup: impl FnOnce(&mut OwlPostApp)) -> Harness<'static, OwlPostApp> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let db = std::env::temp_dir().join(format!("sp-ui-{theme}-{}-{n}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Store::open(&db).unwrap();
    let theme = theme.to_string();
    let h = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_state(|ctx, app: &mut OwlPostApp| app.frame(ctx), {
            let ctx = egui::Context::default();
            let mut app = OwlPostApp::with_context(&ctx, Some(theme), store, true);
            setup(&mut app);
            app
        });
    // The app was built against a scratch context; set up the harness's own.
    let c = h.state().colors.clone();
    c.apply(&h.ctx);
    install_image_loaders(&h.ctx);
    h
}

fn save_ui(h: &mut Harness<'static>, name: &str) {
    if let Ok(img) = h.render() {
        img.save(shots_dir().join(format!("{name}.png"))).unwrap();
    }
}

fn save(h: &mut Harness<'static, OwlPostApp>, name: &str) {
    match h.render() {
        Ok(img) => {
            let p = shots_dir().join(format!("{name}.png"));
            img.save(&p).unwrap();
            eprintln!("wrote {}", p.display());
        }
        Err(e) => eprintln!("render unavailable: {e}"),
    }
}

#[test]
fn renders_every_builtin_theme_and_snoozes() {
    for theme in [theme::MOONLIT_LIGHT, theme::MOONLIT_DARK] {
        let mut h = harness(theme);
        let c = h.state().colors.clone();
    c.apply(&h.ctx);
        h.state_mut().titlebar_inset = 76.0;
        h.run_steps(3);
        // Pin an older conversation; it should jump to the top.
        let (a, k) = { let r = &h.state().rows[3]; (r.t.account_id, r.t.thread_key.clone()) };
        h.state_mut().toggle_pin(a, &k);
        assert_eq!(h.state().rows[0].t.thread_key, k);
        assert!(h.state().rows[0].t.pinned);
        // Open the first thread, like a click would.
        h.state_mut().select(0);
        h.run_steps(3);
        save(&mut h, theme);
        if theme == theme::MOONLIT_LIGHT {
            let before = h.state().rows.len();
            h.state_mut().act_snooze(Utc::now() + chrono::Duration::hours(4));
            h.run_steps(2);
            assert_eq!(h.state().rows.len(), before - 1);
            assert_eq!(h.state().snoozed_count, 1);
            h.state_mut().set_selection(Selection::Snoozed);
            h.state_mut().select(0);
            h.run_steps(3);
            save(&mut h, "snoozed-view");
            h.state_mut().compose_reply(0);
            h.run_steps(3);
            save(&mut h, "composer");
            h.state_mut().prefs = Some(Prefs::new(Tab::Appearance));
            h.run_steps(3);
            save(&mut h, "prefs-themes");
            h.state_mut().prefs = Some(Prefs::new(Tab::Mcp));
            h.run_steps(3);
            save(&mut h, "prefs-mcp");
        }
    }
}

fn row_center(h: &Harness<'static, OwlPostApp>, i: usize) -> egui::Pos2 {
    let s = h.state();
    let layout = list_layout(&s.rows, s.selection != Selection::Snoozed, s.hidden_pinned, Utc::now());
    let y = layout.iter().find(|(_, _, it)| *it == ListItem::Row(i)).map(|(y, _, _)| *y).unwrap();
    egui::pos2((s.sidebar_right + s.list_right) / 2.0, s.list_top + y + ROW_H / 2.0)
}

#[test]
fn swipe_quick_actions_and_composer_window() {
    let mut h = harness_with(theme::MOONLIT_LIGHT, |a| a.list_default_width = 500.0);
    h.run_steps(4);
    let before = h.state().rows.len();
    let first = h.state().rows[0].t.thread_key.clone();

    // Mouse drag to the right past the threshold: archive (the default).
    let start = row_center(&h, 0);
    h.drag_at(start);
    h.run_steps(1);
    for dx in [40.0, 90.0, 140.0, 190.0] {
        h.hover_at(start + egui::vec2(dx, 0.0));
        h.run_steps(1);
    }
    save(&mut h, "swipe-right-archive");
    h.drop_at(start + egui::vec2(190.0, 0.0));
    h.run_steps(3);
    assert_eq!(h.state().rows.len(), before - 1, "drag-swipe should archive");
    assert!(h.state().rows.iter().all(|r| r.t.thread_key != first));
    assert!(h.state().open.is_none(), "a swipe isn't a click");
    assert!(h.state().toasts.iter().any(|t| t.text == "Archived"));

    // A short drag springs back without doing anything.
    let start = row_center(&h, 0);
    h.drag_at(start);
    h.run_steps(1);
    h.hover_at(start - egui::vec2(30.0, 0.0));
    h.run_steps(1);
    h.drop_at(start - egui::vec2(30.0, 0.0));
    h.run_steps(8);
    assert_eq!(h.state().rows.len(), before - 1);

    // Two-finger trackpad swipe to the left: trash.
    let target = h.state().rows[1].t.thread_key.clone();
    h.hover_at(row_center(&h, 1));
    h.run_steps(1);
    for _ in 0..8 {
        h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(-25.0, 0.0), modifiers: Default::default() });
        h.run_steps(1);
    }
    save(&mut h, "swipe-left-trash");
    std::thread::sleep(Duration::from_millis(200));
    h.run_steps(3);
    assert!(h.state().rows.iter().all(|r| r.t.thread_key != target), "trackpad swipe should trash");
    assert_eq!(h.state().rows.len(), before - 2);

    // Hover quick actions in the wide list: click the pin (5th button).
    let c = row_center(&h, 2);
    h.hover_at(c);
    h.run_steps(2);
    save(&mut h, "quick-actions");
    let key = h.state().rows[2].t.thread_key.clone();
    let right = h.state().list_right - 1.0 - 16.0;
    let pin_btn = egui::pos2(right - 3.0 - 14.0, c.y);
    h.hover_at(pin_btn);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: pin_btn, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: pin_btn, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert_eq!(h.state().rows[0].t.thread_key, key, "quick-action pin should move it to the top");
    assert!(h.state().open.is_none(), "quick actions don't open the conversation");

    // No quick actions when the list is narrow.
    let mut n = harness(theme::MOONLIT_LIGHT);
    n.run_steps(3);
    n.hover_at(row_center(&n, 0));
    n.run_steps(2);
    save(&mut n, "narrow-no-quick-actions");

    // The composer is its own native window; the offscreen test renderer
    // can't open windows, so egui embeds it and we check that path draws.
    h.state_mut().compose_new();
    h.run_steps(3);
    assert_eq!(h.state().composers.len(), 1);
    save(&mut h, "composer-new");
}

#[test]
fn move_and_label() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(3);
    let idx = h.state().rows.iter().position(|r| r.t.subject.contains("IMAP IDLE")).unwrap();
    h.state_mut().select(idx);
    h.run_steps(2);
    // Open the Label picker with `l`, like the keyboard shortcut.
    h.event(egui::Event::Text("l".into()));
    h.run_steps(3);
    save(&mut h, "label-picker");
    let (a, k) = h.state().current().unwrap();
    let clients = h.state().folders[&a].iter().find(|f| f.path == "Clients").cloned().unwrap();
    h.state_mut().set_label(a, &k, &clients, true);
    // The worker applies it in the background.
    for _ in 0..50 {
        h.run_steps(1);
        if h.state().store.thread_folder_ids(a, &k).unwrap().contains(&clients.id) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(h.state().store.thread_folder_ids(a, &k).unwrap().contains(&clients.id));
    h.run_steps(2);
    save(&mut h, "labels-in-reader");

    // Move to a folder: leaves the inbox.
    let before = h.state().rows.len();
    let proj = h.state().folders[&a].iter().find(|f| f.path == "Projects/OwlPost").cloned().unwrap();
    h.state_mut().move_to(a, &k, &proj);
    assert_eq!(h.state().rows.len(), before - 1);
    for _ in 0..50 {
        h.run_steps(1);
        let inbox = h.state().store.folder_by_role(a, FolderRole::Inbox).unwrap().unwrap().id;
        if !h.state().store.thread_folder_ids(a, &k).unwrap().contains(&inbox) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    h.state_mut().reload();
    assert!(h.state().rows.iter().all(|r| r.t.thread_key != k), "moved thread should stay out of the inbox");
    h.state_mut().set_selection(Selection::Folder(proj.id));
    assert!(h.state().rows.iter().any(|r| r.t.thread_key == k));
}

#[test]
fn signatures_tab_and_composer_picker() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    let work = h.state().store.save_signature(&Signature { id: 0, title: "Work".into(), body: "Alex Rivera\nNorthwind · Product".into(), html: false }).unwrap();
    let gmail = h.state().accounts.iter().find(|a| a.is_gmail()).unwrap().id;
    h.state().store.set_default_signature(gmail, Some(work)).unwrap();
    h.state_mut().reload();
    h.state_mut().prefs = Some(Prefs::new(Tab::Signatures));
    h.run_steps(3);
    save(&mut h, "prefs-signatures");
    h.state_mut().prefs = None;
    // A new message from the Gmail account gets its default signature.
    let acc = h.state().accounts.iter().find(|a| a.id == gmail).cloned().unwrap();
    h.state_mut().composers.push(Composer::new(99, &acc));
    h.run_steps(3);
    assert!(h.state().composers[0].draft().body.ends_with("Alex Rivera\nNorthwind · Product"));
    save(&mut h, "composer-signature");
    h.state_mut().composers.clear();

    // An HTML signature: edited with the toggle on, shown as text while
    // composing, sent as HTML.
    let html = Signature {
        id: 0,
        title: "Canary".into(),
        body: include_str!("../../../owlpost-core/tests/fixtures/canary-signature.html").into(),
        html: true,
    };
    let id = h.state().store.save_signature(&html).unwrap();
    h.state().store.set_default_signature(gmail, Some(id)).unwrap();
    h.state_mut().reload();
    let mut p = Prefs::new(Tab::Signatures);
    p.sig_edit = Some(Signature { id, ..html.clone() });
    h.state_mut().prefs = Some(p);
    // Give the signature image a moment to download (it renders a spinner offline).
    for _ in 0..40 {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(100));
    }
    let url = "https://www.dropbox.com/scl/fi/4p1cn5i52pwncw4ef7qer/Timbo-headshot-2025-Sig-250x250.png?rlkey=tf2nqzy247vscirjk8x923fet&st=56dp1mmc&dl=1";
    match h.ctx.try_load_image(url, egui::SizeHint::default()) {
        Ok(egui::load::ImagePoll::Ready { image }) => eprintln!("signature image loaded: {:?}", image.size),
        Ok(egui::load::ImagePoll::Pending { .. }) => eprintln!("signature image still loading (slow or no network)"),
        Err(e) => eprintln!("signature image failed: {e}"),
    }
    save(&mut h, "prefs-signature-html");
    h.state_mut().prefs = None;
    let acc = h.state().accounts.iter().find(|a| a.id == gmail).cloned().unwrap();
    h.state_mut().composers.push(Composer::new(100, &acc));
    h.run_steps(3);
    let out = h.state().composers[0].draft();
    assert!(out.body.contains("Tim Morrison") && !out.body.contains("<b>"), "{}", out.body);
    assert!(out.html_signature.is_some());
    h.state_mut().composers.clear();
    let mut c2 = Composer::new(101, &acc);
    c2.to = "ann@example.com".into();
    c2.body = "Hi Ann,\n\nThanks for today — photos coming tomorrow.".into();
    h.state_mut().composers.push(c2);
    for _ in 0..10 {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(50));
    }
    save(&mut h, "composer-signature-html");
}

#[test]
fn command_w_closes_windows() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    // Preferences.
    h.state_mut().prefs = Some(Prefs::new(Tab::Appearance));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    h.run_steps(2);
    assert!(h.state().prefs.is_none(), "⌘W closes Preferences");
    // A blank composer closes straight away.
    h.state_mut().compose_new();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    h.run_steps(2);
    assert!(h.state().composers.is_empty(), "⌘W closes an empty composer");
    // With Preferences open over a composer, ⌘W closes Preferences only.
    h.state_mut().compose_new();
    h.run_steps(2);
    h.state_mut().prefs = Some(Prefs::new(Tab::Appearance));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    h.run_steps(2);
    assert!(h.state().prefs.is_none());
    assert_eq!(h.state().composers.len(), 1);
    // One with text closes too, and is kept as a draft.
    h.state_mut().composers[0].subject = "Half-written thought".into();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    h.run_steps(2);
    assert!(h.state().composers.is_empty());
    let mut saved = false;
    for _ in 0..100 {
        h.run_steps(1);
        if !h.state().store.threads(&MailboxView::Unified(FolderRole::Drafts), "subject:Half-written", 5).unwrap().is_empty() {
            saved = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(saved, "⌘W never drops a message: it's saved to Drafts");
}

#[test]
fn drafts_reopen_and_save() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    let gmail = h.state().accounts.iter().find(|a| a.is_gmail()).cloned().unwrap();
    let sig = h.state().store.save_signature(&Signature { id: 0, title: "Work".into(), body: "Alex Rivera\nNorthwind".into(), html: false }).unwrap();
    h.state().store.set_default_signature(gmail.id, Some(sig)).unwrap();
    h.state_mut().reload();
    // A reply draft, as the MCP server's save_draft makes it.
    let t = h.state().store.threads(&MailboxView::Everything, "subject:Dinner", 1).unwrap().remove(0);
    let acc = h.state().accounts.iter().find(|a| a.id == t.account_id).cloned().unwrap();
    let last = h.state().store.thread_messages(acc.id, &t.thread_key).unwrap().pop().unwrap();
    let mut out = owlpost_core::mcp::reply_draft(&acc, &last, false);
    out.body = format!("Sounds lovely!\n\nAlex Rivera\nNorthwind{}", owlpost_core::smtp::quote(&last));
    out.bcc = vec![Contact::new("", "sam@example.com")];
    owlpost_core::engine::save_draft(&h.state().store, &acc, &out, None).unwrap();
    h.state_mut().set_selection(Selection::Unified(FolderRole::Drafts));
    let i = h.state().rows.iter().position(|r| r.t.thread_key == t.thread_key).expect("draft listed in Drafts");
    h.state_mut().select(i);
    h.run_steps(3);
    assert!(h.query_by_label("Edit draft").is_some(), "a conversation with a draft offers Edit draft");
    save(&mut h, "thread-with-draft");

    // Reopened: the signature and quote are split off again.
    let m = h.state().store.thread_messages(acc.id, &t.thread_key).unwrap().into_iter().find(|m| m.body_text.starts_with("Sounds lovely")).unwrap();
    let parts = owlpost_core::engine::DraftParts { bcc: vec![Contact::new("", "sam@example.com")], attachments: vec![] };
    let c = Composer::from_draft(7, &acc, &m, parts, &h.state().sigs);
    assert_eq!(c.body, "Sounds lovely!");
    assert!(c.tail.contains("wrote:"), "{:?}", c.tail);
    assert_eq!(c.bcc, "sam@example.com");
    let again = c.draft();
    assert_eq!(again.body.trim_end(), m.body_text.replace("\r\n", "\n").trim_end(), "saving again keeps the text as it was");
    h.state_mut().composers.push(c);
    h.run_steps(3);
    // Unchanged: ⌘W closes without asking.
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    h.run_steps(2);
    assert!(h.state().composers.is_empty(), "an unchanged draft closes straight away");
    // Changed: ⌘W saves the change, replacing the old copy.
    let mut c = Composer::from_draft(8, &acc, &m, Default::default(), &h.state().sigs);
    c.body.push_str(" See you at 7.");
    h.state_mut().composers.push(c);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::W);
    h.run_steps(2);
    assert!(h.state().composers.is_empty());
    let drafts = |h: &Harness<'static, OwlPostApp>| {
        h.state().store.thread_messages(acc.id, &t.thread_key).unwrap().into_iter().filter(|m| m.body_text.starts_with("Sounds lovely")).map(|m| m.body_text).collect::<Vec<_>>()
    };
    let mut ok = false;
    for _ in 0..100 {
        h.run_steps(1);
        let d = drafts(&h);
        if d.len() == 1 && d[0].starts_with("Sounds lovely! See you at 7.") {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ok, "one draft, with the change: {:?}", drafts(&h));
}

#[test]
fn drafts_save_themselves() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    let acc = h.state().accounts[0].clone();
    let mut c = Composer::new(70, &acc);
    c.to = "ann@example.com".into();
    h.state_mut().composers.push(c);
    h.run_steps(2);
    // Typing, then a pause: saved without asking.
    h.state_mut().composers[0].subject = "Autosave check".into();
    h.run_steps(1);
    let found = |h: &Harness<'static, OwlPostApp>| h.state().store.threads(&MailboxView::Unified(FolderRole::Drafts), "subject:Autosave", 5).unwrap().len();
    assert_eq!(found(&h), 0, "not before the pause");
    let mut n = 0;
    for _ in 0..80 {
        h.run_steps(1);
        n = found(&h);
        if n > 0 && h.state().composers[0].draft.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(60));
    }
    assert_eq!(n, 1, "saved after the pause");
    assert!(h.state().composers[0].status.as_deref().is_some_and(|s| s.starts_with("Saved")));
    save(&mut h, "composer-autosaved");
    // More typing replaces it rather than adding another copy.
    h.state_mut().composers[0].body = "Second version".into();
    for _ in 0..80 {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(60));
        let d = h.state().composers[0].draft.as_ref().map(|d| d.body_text.clone()).unwrap_or_default();
        if d.starts_with("Second version") {
            break;
        }
    }
    assert_eq!(found(&h), 1, "still one draft");
    // Sending removes it.
    h.state_mut().list_settings.undo_send_secs = 0;
    let c = h.state_mut().composers.remove(0);
    let out = c.draft();
    h.state_mut().queue_send(acc.id, out, c);
    let mut gone = false;
    for _ in 0..100 {
        h.run_steps(1);
        if found(&h) == 0 {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(gone, "sent, so no draft left");
}

#[test]
fn sidebar_folder_dialogs() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    let acc = h.state().accounts.iter().find(|a| !a.is_gmail()).unwrap().clone();
    let has = |h: &Harness<'static, OwlPostApp>, path: &str| h.state().folders.get(&acc.id).into_iter().flatten().find(|f| f.path == path).cloned();
    let wait = |h: &mut Harness<'static, OwlPostApp>, path: &str, want: bool| {
        for _ in 0..100 {
            h.run_steps(1);
            if has(h, path).is_some() == want {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };
    h.state_mut().folder_dialog = Some(FolderDialog::New { account: acc.id, parent: None, name: "Receipts".into(), error: None });
    h.run_steps(2);
    h.get_by_label("Create").click();
    assert!(wait(&mut h, "Receipts", true), "the folder appears in the sidebar");
    // The same name again is refused in the dialog.
    h.state_mut().folder_dialog = Some(FolderDialog::New { account: acc.id, parent: None, name: "receipts".into(), error: None });
    h.run_steps(1);
    h.get_by_label("Create").click();
    h.run_steps(2);
    assert!(matches!(&h.state().folder_dialog, Some(FolderDialog::New { error: Some(_), .. })));
    h.state_mut().folder_dialog = None;
    // Delete it again.
    let f = has(&h, "Receipts").unwrap();
    h.state_mut().folder_dialog = Some(FolderDialog::Delete { folder: f, gmail: false });
    h.run_steps(2);
    h.get_by_label("Delete folder").click();
    assert!(wait(&mut h, "Receipts", false), "and goes when deleted");
}

#[test]
fn drag_conversations_onto_folders() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    let before = h.state().rows.len();
    // Two selected, dropped on the All Accounts Archive.
    h.state_mut().toggle_mark(0);
    h.state_mut().toggle_mark(1);
    let keys = h.state().marked.clone();
    h.state_mut().dragging = Some(keys.clone());
    h.state_mut().drop_on(keys, DropTarget::Role(FolderRole::Archive));
    h.run_steps(2);
    assert_eq!(h.state().rows.len(), before - 2);
    assert!(h.state().toasts.iter().any(|t| t.text == "Archived 2 conversations"));
    // One conversation onto a folder of another account moves across.
    let row = h.state().rows[0].t.clone();
    let other = h.state().folders.iter().find(|(a, _)| **a != row.account_id).and_then(|(_, fs)| fs.iter().find(|f| f.role == FolderRole::Other).cloned()).unwrap();
    h.state_mut().drop_on(vec![(row.account_id, row.thread_key.clone())], DropTarget::Folder(other.clone()));
    assert!(!h.state().rows.iter().any(|r| r.t.account_id == row.account_id && r.t.thread_key == row.thread_key), "it leaves the list");
    assert!(h.state().toasts.iter().any(|t| !t.error && t.text.contains("Trash")), "and says the original goes to Trash");
    let mut arrived = false;
    for _ in 0..100 {
        h.run_steps(1);
        if h.state().store.threads(&MailboxView::Folder(other.id), "", 100).unwrap().iter().any(|t| t.account_id == other.account_id && t.subject == row.subject) {
            arrived = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(arrived, "a copy lands in the other account's folder");
    // A real drag: press on the first row, move down (picks it up), then
    // left onto All Accounts → Archive, and let go.
    let rows_before = h.state().rows.len();
    let first = h.state().rows[0].t.thread_key.clone();
    let x = (h.state().sidebar_right + h.state().list_right) / 2.0;
    let start = egui::pos2(x, 280.0);
    let pointer = |h: &mut Harness<'static, OwlPostApp>, p: egui::Pos2| {
        h.event(egui::Event::PointerMoved(p));
        h.run_steps(1);
    };
    let button = |h: &mut Harness<'static, OwlPostApp>, p: egui::Pos2, pressed: bool| {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
    };
    pointer(&mut h, start);
    button(&mut h, start, true);
    for dy in [4.0, 10.0, 20.0, 30.0] {
        pointer(&mut h, start + egui::vec2(0.0, dy));
    }
    assert!(h.state().dragging.is_some(), "picked up");
    let archive = egui::pos2(80.0, 301.0);
    for t in [0.3, 0.6, 1.0] {
        pointer(&mut h, start + (archive - start) * t);
    }
    assert!(matches!(h.state().drop_target, Some(DropTarget::Role(FolderRole::Archive))), "over Archive");
    save(&mut h, "dragging");
    button(&mut h, archive, false);
    h.run_steps(2);
    assert!(h.state().dragging.is_none());
    assert_eq!(h.state().rows.len(), rows_before - 1, "dropped on Archive");
    assert!(!h.state().rows.iter().any(|r| r.t.thread_key == first));
}

#[test]
fn show_original_window() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    h.state_mut().originals.push(Original { id: 900, subject: "Rates".into(), text: "From: rates@example.com\r\nSubject: Rates\r\n\r\nKia ora".into() });
    h.run_steps(3);
    assert!(h.query_by_label("Save as .eml…").is_some());
    save(&mut h, "show-original");
}

#[test]
fn multi_select_and_batch_actions() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    let before = h.state().rows.len();
    // ⌘-click two conversations (the first one opens, the second joins it).
    h.state_mut().toggle_mark(0);
    assert!(!h.state().multi() && h.state().open.is_some(), "one marked: it just opens");
    h.state_mut().toggle_mark(1);
    assert_eq!(h.state().marked.len(), 2);
    assert!(h.state().open.is_none());
    // ⇧-click extends from the anchor.
    h.state_mut().mark_range(3);
    assert_eq!(h.state().marked.len(), 3, "rows 1 to 3");
    h.run_steps(3);
    save(&mut h, "multi-select");
    let keys: Vec<String> = h.state().marked.iter().map(|m| m.1.clone()).collect();
    h.state_mut().act_move(FolderRole::Archive);
    h.run_steps(2);
    assert_eq!(h.state().rows.len(), before - 3, "all three archived");
    assert!(h.state().toasts.iter().any(|t| t.undo && t.text == "Archived 3 conversations"), "one toast for the batch");
    assert!(h.state().marked.is_empty());
    // Let the moves finish, then one ⌘Z brings back all three.
    let back = |h: &mut Harness<'static, OwlPostApp>, n: usize| {
        for _ in 0..150 {
            h.run_steps(1);
            h.state_mut().reload();
            if h.state().rows.len() == n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };
    assert!(back(&mut h, before - 3));
    h.state_mut().undo_last();
    assert!(back(&mut h, before), "⌘Z restores the whole batch");
    assert!(keys.iter().all(|k| h.state().rows.iter().any(|r| &r.t.thread_key == k)));
    // ⌘A, then Esc.
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    assert_eq!(h.state().marked.len(), before);
    h.key_press(Key::Escape);
    h.run_steps(1);
    assert!(h.state().marked.is_empty());
}

#[test]
fn undo_send_holds_the_message() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    let acc = h.state().accounts[0].clone();
    let sent = |h: &Harness<'static, OwlPostApp>| h.state().store.threads(&MailboxView::Unified(FolderRole::Sent), "subject:Lunch", 50).unwrap().len();
    let compose = |id| {
        let mut c = Composer::new(id, &acc);
        c.to = "ann@example.com".into();
        c.subject = "Lunch?".into();
        c.body = "Free on Friday?".into();
        c
    };
    h.state_mut().list_settings.undo_send_secs = 10;
    let c = compose(50);
    let out = c.draft();
    h.state_mut().queue_send(acc.id, out.clone(), c);
    h.run_steps(2);
    assert_eq!(h.state().pending_sends.len(), 1, "held, not sent");
    save(&mut h, "undo-send");
    // ⌘Z brings the composer back.
    h.state_mut().undo_last();
    assert!(h.state().pending_sends.is_empty());
    assert_eq!(h.state().composers.len(), 1, "the message is back in its window");
    assert_eq!(h.state().composers[0].body, "Free on Friday?");
    // Send now goes straight away.
    let c = h.state_mut().composers.remove(0);
    h.state_mut().queue_send(acc.id, out, c);
    let id = h.state().pending_sends[0].id;
    h.state_mut().send_now(id);
    assert!(h.state().pending_sends.is_empty());
    let mut ok = false;
    for _ in 0..100 {
        h.run_steps(1);
        if sent(&h) > 0 {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ok, "sent");
}

#[test]
fn sidebar_folders_fold() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    let gmail = h.state().accounts.iter().find(|a| a.is_gmail()).unwrap().id;
    for path in ["Projects", "Projects/Website", "Projects/Website/2026", "timson.co"] {
        h.state().store.upsert_folder(gmail, path, FolderRole::Other).unwrap();
    }
    // A tall window and the other account folded, so the folders are on screen.
    h.set_size(egui::vec2(1280.0, 1100.0));
    let others_acc: Vec<AccountId> = h.state().accounts.iter().filter(|a| a.id != gmail).map(|a| a.id).collect();
    h.state_mut().collapsed_accounts = others_acc;
    h.state_mut().reload();
    h.run_steps(2);
    // Parents start folded; dotted names stay whole.
    let f = h.state().folders[&gmail].clone();
    assert_eq!(f.iter().find(|x| x.path == "timson.co").unwrap().name, "timson.co");
    let others: Vec<&Folder> = f.iter().filter(|x| x.role == FolderRole::Other).collect();
    let tree = FolderTree::new(&others);
    let at = |p: &str| others.iter().position(|x| x.path == p).unwrap();
    assert!(tree.has_children(at("Projects")) && !tree.has_children(at("timson.co")));
    assert_eq!(tree.ancestors[at("Projects/Website/2026")], vec![at("Projects/Website"), at("Projects")]);
    assert_eq!(tree.descendants(at("Projects")).count(), 3, "Website, 2026 and OwlPost");
    assert!(h.query_by_label("Expand Projects").is_some());
    assert!(h.query_by_label("Expand Website").is_none(), "hidden inside Projects");
    save(&mut h, "sidebar-folded");
    h.get_by_label("Expand Projects").click();
    h.run_steps(2);
    assert!(h.query_by_label("Collapse Projects").is_some());
    assert!(h.query_by_label("Expand Website").is_some(), "now showing");
    save(&mut h, "sidebar-unfolded");
}

#[test]
fn snooze_calendar_picks_a_time() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    h.state_mut().select(0);
    h.run_steps(2);
    h.ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("snooze_open"), true));
    h.run_steps(2);
    h.state_mut().snooze_pick.open = true;
    let day = chrono::Local::now().date_naive() + chrono::Duration::days(3);
    h.state_mut().snooze_pick.day = Some(day);
    h.state_mut().snooze_pick.month = day.with_day(1).unwrap();
    h.state_mut().snooze_pick.hour = 13;
    h.run_steps(3);
    assert!(h.query_by_label(&day.format("%B %Y").to_string()).is_some(), "the month is shown");
    save(&mut h, "snooze-calendar");
    let before = h.state().rows.len();
    h.get_by_label("Snooze").click();
    h.run_steps(3);
    assert_eq!(h.state().rows.len(), before - 1, "snoozed from the calendar");
    let snoozed = h.state().store.snoozed_threads().unwrap();
    let until = snoozed[0].1.with_timezone(&chrono::Local);
    assert_eq!(until.format("%Y-%m-%d %H:%M").to_string(), format!("{} 13:00", day.format("%Y-%m-%d")));
}

#[test]
fn attachments_and_image_sizes() {
    use crate::attachments::ImageSize;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    h.state_mut().compose_new();
    h.run_steps(2);
    // Dragging files onto the window attaches them.
    let dir = std::env::temp_dir().join(format!("sp-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let photo = dir.join("Beach day.jpg");
    std::fs::write(&photo, crate::attachments::tests::jpeg(3000, 2000)).unwrap();
    let pdf = dir.join("Quote 2026.pdf");
    std::fs::write(&pdf, b"%PDF-1.4 a quote").unwrap();
    h.input_mut().dropped_files = vec![
        egui::DroppedFile { path: Some(photo.clone()), ..Default::default() },
        egui::DroppedFile { path: Some(pdf), ..Default::default() },
    ];
    h.run_steps(2);
    let names: Vec<_> = h.state().composers[0].attachments.iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, ["Beach day.jpg", "Quote 2026.pdf"]);
    assert!(!h.state().composers[0].is_blank(), "attachments count as content");

    // Pick Medium: the photo is shrunk in the background.
    {
        let c = &mut h.state_mut().composers[0];
        c.to = "Maya <maya@example.com>".into();
        c.subject = "Photos from Saturday".into();
        c.body = "Here's the one I mentioned.".into();
        c.image_size = ImageSize::Medium;
    }
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().composers[0].resizing() && !h.state().composers[0].attachments[0].pending(ImageSize::Small) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    h.run_steps(3);
    save(&mut h, "composer-attachments");
    let out = h.state().composers[0].draft();
    assert_eq!(out.attachments.len(), 2);
    let img = image::load_from_memory(&out.attachments[0].data).unwrap();
    assert_eq!((img.width(), img.height()), (1280, 853));
    assert_eq!(out.attachments[1].data, b"%PDF-1.4 a quote");
    assert_eq!(out.attachments[1].content_type, "application/pdf");
    // Removing one.
    h.state_mut().composers[0].attachments.pop();
    h.run_steps(2);
    assert_eq!(h.state().composers[0].draft().attachments.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn open_received_attachment() {
    use egui_kittest::kittest::Queryable;
    let cache = std::env::temp_dir().join(format!("sp-ui-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    std::env::set_var("OWLPOST_CACHE", &cache);
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(3);
    let idx = h.state().rows.iter().position(|r| r.t.subject.contains("receipt")).unwrap();
    h.state_mut().select(idx);
    h.run_steps(3);
    let at = h.get_by_label("receipt.pdf").rect().center();
    h.hover_at(at);
    h.run_steps(2);
    save(&mut h, "attachment-hover");
    // Click: downloaded (a stand-in for demo mail), cached, then opened.
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(1);
    assert_eq!(h.state().downloading.len(), 1, "shows as downloading");
    for _ in 0..100 {
        h.run_steps(1);
        if h.state().downloading.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(h.state().downloading.is_empty());
    assert!(h.state().toasts.iter().all(|t| !t.error), "{:?}", h.state().toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
    let cached: Vec<_> = std::fs::read_dir(cache.join("attachments")).unwrap().flatten().flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten()).collect();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].file_name(), "receipt.pdf");
    assert!(std::fs::read(cached[0].path()).unwrap().starts_with(b"%PDF"));
    // Right-click offers the save options.
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert!(h.query_by_label("Save to Downloads").is_some() && h.query_by_label("Save As…").is_some());
    save(&mut h, "attachment-menu");
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn push_status_lines() {
    use crate::worker::PushState;
    let palette = theme::discover().into_iter().find(|t| t.id == theme::MOONLIT_LIGHT).unwrap().palette();
    let col = Colors::new(palette);
    let states = [
        Some(PushState::Live),
        Some(PushState::Connecting),
        Some(PushState::Retrying("connecting to imap.mail.me.com:993: timed out".into())),
        Some(PushState::Unavailable("the server doesn't support it".into())),
    ];
    let mut h = Harness::builder().with_size(egui::vec2(360.0, 120.0)).build_ui(move |ui| {
        col.apply(ui.ctx());
        for st in &states {
            crate::prefs::push_line(ui, &col, st.as_ref());
        }
    });
    h.run_steps(2);
    save_ui(&mut h, "push-status");
    use egui_kittest::kittest::Queryable;
    for text in ["New mail: instant (push)", "New mail: connecting push…", "New mail: reconnecting push…", "New mail: every 5 minutes"] {
        assert!(h.query_by_label(text).is_some(), "{text}");
    }
}

#[test]
fn google_sign_in_screens() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(2);
    assert!(!h.state().google_ready, "tests never read the real keychain");
    h.state_mut().prefs = Some(Prefs::new(Tab::Accounts));
    h.run_steps(3);
    assert!(h.query_by_label("Client ID").is_some());
    assert!(h.query_by_label("How to create a Google client").is_some());
    save(&mut h, "prefs-google-setup");
    // Set up: the sign-in button appears.
    h.state_mut().google_ready = true;
    h.run_steps(3);
    assert!(h.query_by_label("Sign in with Google…").is_some());
    save(&mut h, "prefs-google-ready");
    // Signing in: waiting for the browser.
    h.state_mut().google = Some((Default::default(), Some("https://accounts.google.com/o/oauth2/v2/auth?x".into())));
    h.run_steps(3);
    assert!(h.query_by_label("Finish signing in in your browser…").is_some());
    save(&mut h, "prefs-google-waiting");
    h.state_mut().cancel_google();
    h.state_mut().prefs = None;
    // The add-account window for Gmail.
    h.state_mut().setup = Some(AccountSetup { provider: Some(owlpost_core::presets::Provider::Gmail), ..Default::default() });
    h.run_steps(3);
    assert!(h.query_by_label("Sign in with Google").is_some());
    assert!(h.query_by_label("…or connect with an app password:").is_some());
    save(&mut h, "setup-gmail-google");
}

#[test]
fn sync_health_and_activity() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(3);
    // Treat the demo accounts as real ones (in the window only, so the
    // worker never connects anywhere): one fine, one that can't connect.
    let ids: Vec<AccountId> = h.state().accounts.iter().map(|a| a.id).collect();
    {
        let st = h.state_mut();
        for a in st.accounts.iter_mut() {
            a.demo = false;
        }
        st.store.sync_ok(ids[0]);
        st.store.set_push(ids[0], activity::Push::Live, None);
        st.store.note(Some(ids[0]), activity::Level::Info, "Checked 12 folders · 3 new");
        st.store.sync_failed(ids[1], "connecting to imap.mail.me.com:993: failed to lookup address information: nodename nor servname provided");
        st.store.sync_failed(ids[1], "connecting to imap.mail.me.com:993: failed to lookup address information: nodename nor servname provided");
        st.health_read = None;
    }
    h.run_steps(3);
    let line = h.state().health_line().unwrap();
    assert_eq!(line.1, activity::Health::Offline);
    assert!(line.0.starts_with("Can't connect: "), "{}", line.0);
    h.get_by_label_contains("status: offline");
    h.get_by_label_contains("status: ok");
    save(&mut h, "sync-health-sidebar");
    // Clicking the red mark opens Sync activity for that account.
    h.get_by_label_contains("status: offline").click();
    h.run_steps(3);
    assert_eq!(h.state().activity.as_ref().map(|v| v.account), Some(Some(ids[1])));
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().activity.is_none());
    // ⌘⇧L opens Sync activity; Esc closes it.
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::L);
    h.run_steps(4);
    assert!(h.state().activity.is_some());
    h.get_by_label_contains("Can't connect: can't find the server");
    save(&mut h, "sync-activity");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().activity.is_none());
}

#[test]
fn keyboard_find_and_move_picker() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(3);
    // ⌘F puts the cursor in the search box.
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::F);
    h.run_steps(3);
    assert!(h.ctx.wants_keyboard_input(), "search has the keyboard");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.ctx.wants_keyboard_input());
    // ? shows the shortcuts.
    h.event(egui::Event::Text("?".into()));
    h.run_steps(3);
    assert!(h.state().show_keys);
    save(&mut h, "keyboard-shortcuts");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().show_keys);
    // v, type "re", ↓ ↑, Return: moved to Receipts.
    let idx = h.state().rows.iter().position(|r| r.t.subject.contains("Dinner")).unwrap();
    let (acct, key) = (h.state().rows[idx].t.account_id, h.state().rows[idx].t.thread_key.clone());
    h.state_mut().select(idx);
    h.run_steps(2);
    h.event(egui::Event::Text("v".into()));
    h.run_steps(3);
    h.event(egui::Event::Text("re".into()));
    h.run_steps(2);
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    assert_eq!(h.state().picker_index, 1);
    save(&mut h, "move-picker");
    h.key_press(Key::ArrowUp);
    h.run_steps(2);
    assert_eq!(h.state().picker_index, 0);
    h.key_press(Key::Enter);
    h.run_steps(2);
    let receipts = h.state().store.folders(acct).unwrap().into_iter().find(|f| f.path == "Receipts").unwrap();
    let mut moved = false;
    for _ in 0..100 {
        h.run_steps(1);
        if h.state().store.thread_folder_ids(acct, &key).unwrap().contains(&receipts.id) {
            moved = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(moved, "moved to Receipts");
}

#[test]
fn right_click_move_with_keys() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(3);
    let idx = h.state().rows.iter().position(|r| r.t.subject.contains("Dinner")).unwrap();
    let (acct, key) = (h.state().rows[idx].t.account_id, h.state().rows[idx].t.thread_key.clone());
    let at = row_center(&h, idx);
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    save(&mut h, "right-click-menu");
    h.get_by_label_contains("Move to").hover();
    h.run_steps(4);
    assert!(h.ctx.wants_keyboard_input(), "the folder list is ready for typing");
    h.event(egui::Event::Text("re".into()));
    h.run_steps(2);
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    assert_eq!(h.state().picker_index, 1);
    save(&mut h, "right-click-move");
    h.key_press(Key::ArrowUp);
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(!egui::Popup::is_any_open(&h.ctx), "the menu closes");
    let receipts = h.state().store.folders(acct).unwrap().into_iter().find(|f| f.path == "Receipts").unwrap();
    let mut moved = false;
    for _ in 0..100 {
        h.run_steps(1);
        if h.state().store.thread_folder_ids(acct, &key).unwrap().contains(&receipts.id) {
            moved = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(moved, "moved to Receipts");
}

#[test]
fn arrow_keys_in_context_menus() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(3);
    let right_click = |h: &mut Harness<'static, OwlPostApp>, idx: usize| {
        let at = row_center(h, idx);
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
        h.run_steps(3);
    };
    let idx = h.state().rows.iter().position(|r| r.t.subject.contains("Dinner")).unwrap();
    let (acct, key) = (h.state().rows[idx].t.account_id, h.state().rows[idx].t.thread_key.clone());
    let selected = h.state().selected;
    assert!(h.state().rows[idx].t.unread);
    // ↓ ↓ Return: the second item, Mark as read. The list behind doesn't move.
    right_click(&mut h, idx);
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    save(&mut h, "context-menu-keys");
    assert_eq!(h.state().selected, selected, "the arrows don't move the list");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(!h.state().store.threads(&MailboxView::Everything, "subject:Dinner is:unread", 5).unwrap().iter().any(|t| t.account_id == acct && t.thread_key == key));
    assert!(!egui::Popup::is_any_open(&h.ctx));
    // ↑ from the top wraps to the last item, Move to; Return opens it, ready to type.
    let idx = h.state().rows.iter().position(|r| r.t.thread_key == key).unwrap();
    right_click(&mut h, idx);
    h.key_press(Key::ArrowUp);
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(4);
    h.event(egui::Event::Text("rec".into()));
    h.run_steps(2);
    assert_eq!(h.state().picker_query, "rec");
    // Esc closes it all.
    h.key_press(Key::Escape);
    h.run_steps(3);
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!egui::Popup::is_any_open(&h.ctx), "Esc closes the menu");
}

#[test]
fn command_z_undoes() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(3);
    let in_list = |h: &Harness<'static, OwlPostApp>, k: &str| h.state().rows.iter().any(|r| r.t.thread_key == k);
    let wait_for = |h: &mut Harness<'static, OwlPostApp>, k: &str, present: bool| {
        for _ in 0..100 {
            h.run_steps(1);
            h.state_mut().reload();
            if h.state().rows.iter().any(|r| r.t.thread_key == k) == present {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };
    // Archive with `e`, then ⌘Z.
    let idx = h.state().rows.iter().position(|r| r.t.subject.contains("Dinner")).unwrap();
    let key = h.state().rows[idx].t.thread_key.clone();
    h.state_mut().select(idx);
    h.run_steps(2);
    h.event(egui::Event::Text("e".into()));
    h.run_steps(2);
    assert!(!in_list(&h, &key), "archived");
    assert!(h.state().toasts.iter().any(|t| t.undo && t.text == "Archived"));
    save(&mut h, "undo-toast");
    assert!(wait_for(&mut h, &key, false), "the archive finishes");
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert!(wait_for(&mut h, &key, true), "⌘Z brings it back to the Inbox");
    assert!(h.state().toasts.iter().any(|t| t.text == "Undid archive"));

    // Trash straight after, then click Undo on the toast at once: the undo
    // waits for the move to finish rather than racing it.
    let idx = h.state().rows.iter().position(|r| r.t.thread_key == key).unwrap();
    h.state_mut().select(idx);
    h.run_steps(1);
    h.state_mut().act_move(FolderRole::Trash);
    h.state_mut().undo_last();
    assert!(wait_for(&mut h, &key, true), "trash undone");
    let acct = h.state().rows.iter().find(|r| r.t.thread_key == key).unwrap().t.account_id;
    let trash = h.state().store.folder_by_role(acct, FolderRole::Trash).unwrap().unwrap();
    for _ in 0..50 {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!h.state().store.thread_folder_ids(acct, &key).unwrap().contains(&trash.id), "and out of the Trash");

    // Snooze, pin and read state.
    let idx = h.state().rows.iter().position(|r| r.t.thread_key == key).unwrap();
    h.state_mut().select(idx);
    h.run_steps(1);
    h.state_mut().act_snooze(Utc::now() + chrono::Duration::hours(3));
    assert!(!in_list(&h, &key));
    h.state_mut().undo_last();
    // The snooze may still be finishing in the background; the undo follows it.
    assert!(wait_for(&mut h, &key, true), "unsnoozed");
    h.state_mut().toggle_pin(acct, &key);
    assert!(h.state().store.is_pinned(acct, &key).unwrap());
    h.state_mut().undo_last();
    assert!(!h.state().store.is_pinned(acct, &key).unwrap(), "unpinned again");
    // Nothing left to undo.
    while !h.state().undo.is_empty() {
        h.state_mut().undo.pop();
    }
    h.state_mut().undo_last();
    assert!(h.state().toasts.iter().any(|t| t.text == "Nothing to undo"));
}

#[test]
fn pinned_section_hides_and_shows() {
    let mut h = harness(theme::MOONLIT_LIGHT);
    h.run_steps(3);
    let (a, k) = (h.state().rows[2].t.account_id, h.state().rows[2].t.thread_key.clone());
    h.state_mut().toggle_pin(a, &k);
    h.run_steps(3);
    let layout = list_layout(&h.state().rows, true, 0, Utc::now());
    assert_eq!(layout[0].2, ListItem::Header("Pinned".into()));
    assert_eq!(layout[1].2, ListItem::Row(0));
    assert!(h.state().rows[0].t.pinned);
    save(&mut h, "list-pinned");
    // Click "Hide" in the Pinned header.
    let s = h.state();
    let at = egui::pos2(s.list_right - 30.0, s.list_top + LIST_TOP + HEADER_H - 14.0);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert!(h.state().list_settings.hide_pinned);
    assert_eq!(h.state().hidden_pinned, 1);
    assert!(h.state().rows.iter().all(|r| !r.t.pinned), "pinned conversations are folded away");
    save(&mut h, "list-pinned-hidden");
    assert_eq!(ListSettings::decode(&h.state().list_settings.encode()), h.state().list_settings, "remembered");
    assert!(!ListSettings::decode("trash,archive,1").hide_pinned, "older settings still load");
}

#[test]
fn date_groups() {
    use chrono::{Local, TimeZone};
    let now = Local.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap().with_timezone(&Utc); // a Saturday
    let at = |d: u32, m: u32, y: i32| Local.with_ymd_and_hms(y, m, d, 9, 0, 0).unwrap().with_timezone(&Utc);
    assert_eq!(date_group(at(7, 11, 2026), now), "Today");
    assert_eq!(date_group(at(6, 11, 2026), now), "Yesterday");
    assert_eq!(date_group(at(3, 11, 2026), now), "Earlier this week");
    assert_eq!(date_group(at(28, 10, 2026), now), "Last week");
    assert_eq!(date_group(at(2, 10, 2026), now), "October");
    assert_eq!(date_group(at(2, 10, 2025), now), "October 2025");
}

#[test]
fn unused_download_names() {
    let dir = std::env::temp_dir().join(format!("sp-dl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(crate::worker::unused_path(&dir, "a.pdf"), dir.join("a.pdf"));
    std::fs::write(dir.join("a.pdf"), b"x").unwrap();
    assert_eq!(crate::worker::unused_path(&dir, "a.pdf"), dir.join("a (2).pdf"));
    std::fs::write(dir.join("a (2).pdf"), b"x").unwrap();
    assert_eq!(crate::worker::unused_path(&dir, "a.pdf"), dir.join("a (3).pdf"));
    std::fs::write(dir.join("README"), b"x").unwrap();
    assert_eq!(crate::worker::unused_path(&dir, "README"), dir.join("README (2)"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn welcome_screen() {
    let db = std::env::temp_dir().join(format!("sp-ui-welcome-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let store = Store::open(&db).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .build_state(|ctx, app: &mut OwlPostApp| app.frame(ctx), {
            let ctx = egui::Context::default();
            OwlPostApp::with_context(&ctx, Some(theme::MOONLIT_LIGHT.into()), store, false)
        });
    let c = h.state().colors.clone();
    c.apply(&h.ctx);
    h.run_steps(3);
    save(&mut h, "welcome");
}
