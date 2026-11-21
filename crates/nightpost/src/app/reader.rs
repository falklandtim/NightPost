//! The reading pane: the open conversation as message cards (or the HTML
//! web view), attachments, and Google sign-in prompts.

use super::*;

impl NightPostApp {
    pub(super) fn reader(&mut self, ctx: &egui::Context) {
        let col = self.colors.clone();
        // The conversation reads like a page: messages sit straight on the
        // background, separated by rules.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(col.bg))
            .show(ctx, |ui| {
                if self.multi() {
                    self.selection_summary(ui, &col);
                    return;
                }
                let Some(open) = &self.open else {
                    ui.centered_and_justified(|ui| {
                        let unread = self.unified_unread.get(&FolderRole::Inbox).copied().unwrap_or(0);
                        ui.label(
                            RichText::new(if unread > 0 { format!("{unread} unread in your inbox") } else { "No conversation selected".into() })
                                .color(col.very_subtle)
                                .size(15.0),
                        );
                    });
                    return;
                };
                let subject = open.messages.first().map(|m| style::drawable(&m.subject).into_owned()).unwrap_or_default();
                // "OLLAMA · SAT 7 NOV, 12:53 PM": who wrote last, and when.
                let kicker = open
                    .messages
                    .last()
                    .map(|m| {
                        let who = if m.from.name.is_empty() { m.from.email.clone() } else { m.from.name.clone() };
                        let when = m.date.with_timezone(&chrono::Local).format("%a %-d %b, %-I:%M %p");
                        format!("{} · {}", style::drawable(&who), when).to_uppercase()
                    })
                    .unwrap_or_default();
                let snoozed = open.snoozed_until;
                let pinned = self.store.is_pinned(open.account_id, &open.thread_key).unwrap_or(false);
                let in_folders = self.store.thread_folder_ids(open.account_id, &open.thread_key).unwrap_or_default();
                let labels: Vec<Folder> = self
                    .folders
                    .get(&open.account_id)
                    .map(|fs| fs.iter().filter(|f| f.role == FolderRole::Other && in_folders.contains(&f.id) && !snooze_sync::is_internal_folder(&f.path)).cloned().collect())
                    .unwrap_or_default();
                let thread_ref = (open.account_id, open.thread_key.clone());
                let mut remove_label: Option<Folder> = None;
                let mut attachment_req: Option<(Message, usize, AttachmentClick)> = None;
                // The newest draft of ours in the conversation, to finish.
                let me = self.accounts.iter().find(|a| a.id == open.account_id).map(|a| a.email.clone()).unwrap_or_default();
                let draft = open
                    .messages
                    .iter()
                    .rev()
                    .filter(|m| m.from.email.eq_ignore_ascii_case(&me))
                    .find_map(|m| self.store.copy_in_role(open.account_id, &m.message_id, FolderRole::Drafts).ok().flatten());
                let mut edit_draft: Option<Message> = None;
                let downloading = self.downloading.clone();
                // The account only needs naming when there's more than one.
                let account = self.accounts.iter().find(|a| a.id == open.account_id).filter(|_| self.accounts.len() > 1).cloned();

                // HTML email: the subject stays here and the web view shows
                // the conversation below it.
                let web = self.html_email && self.mail_view.is_some() && !self.blitz_engine();
                // Blitz (Linux): egui draws the message cards and Blitz each
                // HTML body inside them, so their text can be selected.
                #[cfg(target_os = "linux")]
                let mut blitz = self.blitz.as_mut().filter(|_| self.html_email);
                let remote = self.load_images || self.remote_images.contains(&thread_ref);
                let Some(open) = &mut self.open else { return };
                #[cfg(target_os = "linux")]
                if let Some(b) = blitz.as_deref_mut() {
                    b.keep_only(&open.messages);
                }
                // An HTML body drawn by Blitz, if that's the engine; false shows the text.
                let mut html = |ui: &mut egui::Ui, m: &Message| -> bool {
                    #[cfg(target_os = "linux")]
                    if let Some(b) = blitz.as_deref_mut() {
                        return b.show(ui, &col, m, remote);
                    }
                    let _ = (ui, m, remote);
                    false
                };
                let mut draw = |ui: &mut egui::Ui| {
                    // Generous space around the conversation, like Canary.
                    let margin = egui::Margin { left: 48, right: 48, top: 34, bottom: if web { 4 } else { 36 } };
                    egui::Frame::new().inner_margin(margin).show(ui, |ui| {
                        let max_w = ui.available_width().min(860.0);
                        ui.set_max_width(max_w);
                        let mut job = egui::text::LayoutJob::default();
                        job.append(&kicker, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(12.0), color: col.link, extra_letter_spacing: 1.4, ..Default::default() });
                        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
                        ui.label(job);
                        ui.add_space(6.0);
                        heavy_text(ui, &subject, 30.0, col.heading);
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            if let Some(a) = &account {
                                chip(ui, &a.email, style::hex(&a.color));
                            }
                            if pinned {
                                chip(ui, "📌 Pinned", col.accent);
                            }
                            for f in &labels {
                                // Theme-aware: the label's hue, pulled towards the text colour.
                                let c = style::blend(style::avatar_color(&f.path), col.text, 0.55);
                                if label_chip(ui, &folder_label(f), c).clicked() {
                                    remove_label = Some(f.clone());
                                }
                            }
                            if let Some(u) = snoozed {
                                chip(ui, &format!("⏰ Snoozed until {}", snooze::describe(u)), col.snooze);
                            }
                        });
                        if let Some(d) = &draft {
                            ui.add_space(8.0);
                            if icons::labeled(ui, Icon::Pencil, "Edit draft", col.text, col.bg).on_hover_text("Finish this draft in a new window").clicked() {
                                edit_draft = Some(d.clone());
                            }
                        }
                        if web {
                            return;
                        }
                        ui.add_space(16.0);
                        attachment_req = cards(ui, &col, open, &downloading, &self.faces, &mut html);
                    });
                };
                let mut web_rect = None;
                if web {
                    draw(ui);
                    web_rect = Some(ui.available_rect_before_wrap());
                } else {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| draw(ui));
                }
                self.web_rect = web_rect;
                self.blitz_clicks(ctx, &thread_ref);
                if let Some(f) = remove_label {
                    self.set_label(thread_ref.0, &thread_ref.1, &f, false);
                }
                if let Some((m, idx, what)) = attachment_req {
                    self.attachment(m, idx, what);
                }
                if let Some(d) = edit_draft {
                    self.edit_draft(d);
                }
                if let Some(mode) = ctx.memory_mut(|m| m.data.remove_temp::<u8>(egui::Id::new("reply_click"))) {
                    self.compose_reply(mode);
                }
            });
    }

    /// Whether Blitz is the chosen engine for HTML email (Linux only).
    pub(super) fn blitz_engine(&self) -> bool {
        #[cfg(target_os = "linux")]
        return self.blitz.is_some();
        #[cfg(not(target_os = "linux"))]
        false
    }

    /// Switch HTML email between Blitz and WebKitGTK (Linux only).
    pub(super) fn set_blitz_engine(&mut self, on: bool) {
        #[cfg(target_os = "linux")]
        {
            self.blitz = on.then(super::blitz_body::Bodies::new);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = on;
    }

    /// Act on clicks in emails drawn by Blitz (Linux): open links, and load
    /// web pictures in `thread` after "Load images".
    fn blitz_clicks(&mut self, ctx: &egui::Context, thread: &(AccountId, String)) {
        #[cfg(target_os = "linux")]
        for click in self.blitz.as_mut().map(|b| b.take_clicks()).unwrap_or_default() {
            match click {
                super::blitz_body::Click::Link(url) => Self::open_link(ctx, url),
                super::blitz_body::Click::LoadImages => {
                    self.remote_images.insert(thread.clone());
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (ctx, thread);
    }

    /// Open a link from an email in the browser. Only web and mail links:
    /// anything else (`file:`, `javascript:`, the app's own) is ignored.
    pub(super) fn open_link(ctx: &egui::Context, url: String) {
        if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }

    /// Act on clicks and keys from the HTML message view.
    pub(super) fn mail_view_events(&mut self, ctx: &egui::Context) {
        use crate::mail_view::ViewEvent;
        let events = self.mail_view.as_mut().map(|v| v.events()).unwrap_or_default();
        for e in events {
            match e {
                ViewEvent::OpenLink(url) => Self::open_link(ctx, url),
                ViewEvent::Attachment(id, i) | ViewEvent::SaveAttachment(id, i) | ViewEvent::SaveAttachmentAs(id, i) => {
                    let what = match e {
                        ViewEvent::SaveAttachment(..) => AttachmentClick::SaveToDownloads,
                        ViewEvent::SaveAttachmentAs(..) => AttachmentClick::SaveAs,
                        _ => AttachmentClick::Open,
                    };
                    if let Some(m) = self.open.as_ref().and_then(|o| o.messages.iter().find(|m| m.id == id)).cloned() {
                        self.attachment(m, i, what);
                    }
                }
                ViewEvent::LoadImages => {
                    if let Some(k) = self.current() {
                        self.remote_images.insert(k);
                    }
                }
                ViewEvent::Reply => self.compose_reply(0),
                ViewEvent::ReplyAll => self.compose_reply(1),
                ViewEvent::Forward => self.compose_reply(2),
                ViewEvent::Key(k, cmd) => {
                    self.pending_input.extend(crate::mail_view::key_events(&k, cmd));
                    ctx.request_repaint();
                }
            }
        }
    }

    /// Put the web view over the reading pane, or hide it while something
    /// egui draws (a menu, Preferences) would be underneath it.
    pub(super) fn place_mail_view(&mut self, ctx: &egui::Context) {
        // The macOS view is a native overlay, so it hides under menus and
        // windows; Linux draws the page inside egui, which layers correctly.
        let covered = cfg!(target_os = "macos") && (self.prefs.is_some() || self.setup.is_some() || self.new_tab.is_some() || self.show_keys || self.activity.is_some() || egui::Popup::is_any_open(ctx));
        // With Blitz, the reading pane draws the emails itself.
        let blitz = self.blitz_engine();
        let (Some(view), Some(rect), Some(open)) = (self.mail_view.as_mut(), self.web_rect, self.open.as_ref()) else {
            if let Some(v) = self.mail_view.as_mut() {
                v.hide();
            }
            return;
        };
        if covered || !self.html_email || blitz {
            view.hide();
            return;
        }
        let col = &self.colors;
        let key = (open.account_id, open.thread_key.clone());
        let remote = self.load_images || self.remote_images.contains(&key);
        let page_key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            let faces: Vec<bool> = open.messages.iter().map(|m| self.faces.texture(&m.from.email).is_some()).collect();
            (&key, remote, &self.theme_id, col.p.dark, self.dark_emails, open.messages.iter().map(|m| (m.id, m.starred, m.attachments.len())).collect::<Vec<_>>(), faces).hash(&mut h);
            h.finish()
        };
        let faces = &self.faces;
        let dark_emails = self.dark_emails;
        view.show(ctx, rect, ctx.zoom_factor(), page_key, || {
            let nonce = nightpost_core::mail_html::nonce();
            crate::mail_view::document(&crate::mail_view::Page { messages: &open.messages, expanded: &open.expanded, remote_images: remote, nonce: &nonce, avatar: &|e| faces.data_uri(e), dark_emails }, col)
        });
    }

    /// Start "Sign in with Google" (one at a time).
    pub(super) fn google_sign_in(&mut self, target: Option<AccountId>) {
        if self.google.is_some() {
            return;
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.worker.send(Cmd::GoogleSignIn(target, cancel.clone()));
        self.google = Some((cancel, None));
    }

    pub(super) fn cancel_google(&mut self) {
        if let Some((cancel, _)) = self.google.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Open or save a received attachment (downloaded on a worker thread).
    pub(super) fn attachment(&mut self, m: Message, index: usize, what: AttachmentClick) {
        let Some(a) = m.attachments.get(index) else { return };
        let what = match what {
            // A program, script or archive: ask first (see `risky_open_dialog`).
            AttachmentClick::Open if nightpost_core::file_safety::risky_extension(&a.filename) => {
                self.risky_open = Some((m, index));
                return;
            }
            AttachmentClick::Open => AttachmentDo::Open,
            AttachmentClick::SaveToDownloads => AttachmentDo::SaveToDownloads,
            AttachmentClick::SaveAs => {
                let mut dialog = rfd::FileDialog::new().set_title("Save Attachment").set_file_name(engine::safe_filename(&a.filename));
                if let Some(d) = directories::UserDirs::new().and_then(|u| u.download_dir().map(|d| d.to_path_buf())) {
                    dialog = dialog.set_directory(d);
                }
                let Some(path) = dialog.save_file() else { return };
                AttachmentDo::SaveTo(path)
            }
        };
        self.fetch_attachment(m, index, what);
    }

    /// Download (or reuse) an attachment on the worker, then do `what`.
    pub(super) fn fetch_attachment(&mut self, m: Message, index: usize, what: AttachmentDo) {
        if self.downloading.insert((m.id, index)) {
            self.worker.send(Cmd::Attachment(Box::new(m), index, what));
        }
    }
}

pub(super) fn expanded_default(msgs: &[Message]) -> Vec<bool> {
    let n = msgs.len();
    msgs.iter().enumerate().map(|(i, m)| i + 1 == n || m.unread).collect()
}

pub(super) fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color));
        });
}

/// A label chip with a × to remove it.
pub(super) fn label_chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    let r = egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(10)
        .inner_margin(egui::Margin { left: 8, right: 4, top: 2, bottom: 2 })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new(text).small().color(color));
            ui.add(egui::Button::new(RichText::new("×").small().color(color)).frame(false).small())
        });
    r.inner.on_hover_text("Remove label")
}

/// The conversation as cards, newest first, then Reply, Reply all and
/// Forward. Folds a card whose header was clicked, and returns an
/// attachment that was clicked. `html` is as for [`message_card`].
fn cards(
    ui: &mut egui::Ui,
    col: &Colors,
    open: &mut OpenThread,
    downloading: &HashSet<(MessageId, usize)>,
    faces: &crate::avatars::Faces,
    html: &mut dyn FnMut(&mut egui::Ui, &Message) -> bool,
) -> Option<(Message, usize, AttachmentClick)> {
    let mut req = None;
    let n = open.messages.len();
    // Newest first, like the web view.
    for (i, m) in open.messages.iter().enumerate().rev() {
        let expanded = open.expanded.get(i).copied().unwrap_or(true);
        if i + 1 < n {
            ui.add_space(6.0);
        }
        let (r, clicked) = message_card(ui, col, m, expanded, i + 1 == n, downloading, faces, html);
        if let Some((idx, what)) = clicked {
            req = Some((m.clone(), idx, what));
        }
        if r && i + 1 != n {
            if let Some(e) = open.expanded.get_mut(i) {
                *e = !*e;
            }
        }
        ui.add_space(10.0);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (i, (icon, label)) in [(Icon::Reply, "Reply"), (Icon::ReplyAll, "Reply all"), (Icon::Forward, "Forward")].into_iter().enumerate() {
            if icons::labeled(ui, icon, label, col.text, col.bg).clicked() {
                ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new("reply_click"), i as u8));
            }
        }
    });
    req
}

/// One message in the reader. Returns whether its header was clicked
/// (to fold it) and any attachment that was clicked. `html` draws the
/// message's HTML body and returns true, or returns false for the text.
#[allow(clippy::too_many_arguments)] // one call site, each a different part of the card
pub(super) fn message_card(
    ui: &mut egui::Ui,
    col: &Colors,
    m: &Message,
    expanded: bool,
    is_last: bool,
    downloading: &HashSet<(MessageId, usize)>,
    faces: &crate::avatars::Faces,
    html: &mut dyn FnMut(&mut egui::Ui, &Message) -> bool,
) -> (bool, Option<(usize, AttachmentClick)>) {
    let mut toggled = false;
    let mut clicked = None;
    // Flat, on the page: a rule above each message instead of a card.
    hairline(ui, col);
    egui::Frame::new()
        .inner_margin(egui::Margin { left: 0, right: 0, top: 18, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui.horizontal(|ui| {
                faces.show(ui, &m.from.name, &m.from.email, 36.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(m.from.display()).strong().color(col.text));
                        if !m.from.name.is_empty() {
                            ui.label(RichText::new(format!("<{}>", m.from.email)).small().color(col.very_subtle));
                        }
                    });
                    if expanded {
                        let mut to = format!("to {}", m.to.iter().map(|c| c.display()).collect::<Vec<_>>().join(", "));
                        if !m.cc.is_empty() {
                            to.push_str(&format!("  ·  cc {}", m.cc.iter().map(|c| c.display()).collect::<Vec<_>>().join(", ")));
                        }
                        ui.add(egui::Label::new(RichText::new(to).small().color(col.subtle)).truncate());
                    } else {
                        ui.add(egui::Label::new(RichText::new(&m.snippet).small().color(col.subtle)).truncate());
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.label(RichText::new(style::long_date(m.date)).small().color(col.very_subtle));
                    if m.starred {
                        ui.label(RichText::new("★").color(col.star));
                    }
                });
            });
            if !is_last && ui.interact(header.response.rect, ui.id().with(("hdr", m.id)), Sense::click()).clicked() {
                toggled = true;
            }
            if expanded {
                // Header, a hairline, then (like Canary) attachments above the text.
                ui.add_space(12.0);
                hairline(ui, col);
                if !m.attachments.is_empty() {
                    ui.add_space(12.0);
                    ui.horizontal_wrapped(|ui| {
                        for (i, a) in m.attachments.iter().enumerate() {
                            if let Some(what) = attachment_chip(ui, col, a, downloading.contains(&(m.id, i)), (m.id, i)) {
                                clicked = Some((i, what));
                            }
                        }
                    });
                    ui.add_space(12.0);
                    hairline(ui, col);
                }
                ui.add_space(20.0);
                if !html(ui, m) {
                    body(ui, col, &m.body_text);
                }
            }
        });
    (toggled, clicked)
}

/// A thin full-width divider inside a message card.
pub(super) fn hairline(ui: &mut egui::Ui, col: &Colors) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, egui::Stroke::new(1.0_f32, style::blend(col.border, col.bg, 0.7)));
}

/// A received attachment: click to open, right-click for Save options.
pub(super) fn attachment_chip(ui: &mut egui::Ui, col: &Colors, a: &Attachment, busy: bool, salt: (MessageId, usize)) -> Option<AttachmentClick> {
    let id = ui.id().with(("attachment", salt));
    let hovered = ui.ctx().memory(|mem| mem.data.get_temp::<bool>(id)).unwrap_or(false);
    let fill = if hovered { style::blend(col.text, col.bg_secondary, 0.07) } else { col.bg_secondary };
    let inner = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0_f32, col.border))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if busy {
                    ui.add(egui::Spinner::new().size(14.0).color(col.subtle));
                } else {
                    ui.label(RichText::new("📎").color(col.subtle));
                }
                ui.label(RichText::new(&a.filename).color(col.text));
                ui.label(RichText::new(human_size(a.size)).small().color(col.subtle));
            });
        });
    let r = ui
        .interact(inner.response.rect, id.with("click"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if busy { "Downloading…" } else { "Click to open · right-click to save" });
    ui.ctx().memory_mut(|mem| mem.data.insert_temp(id, r.hovered()));
    let mut what = None;
    if r.clicked() && !busy {
        what = Some(AttachmentClick::Open);
    }
    r.context_menu(|ui| {
        if ui.button("Open").clicked() {
            what = Some(AttachmentClick::Open);
            ui.close();
        }
        if ui.button("Save to Downloads").clicked() {
            what = Some(AttachmentClick::SaveToDownloads);
            ui.close();
        }
        if ui.button("Save As…").clicked() {
            what = Some(AttachmentClick::SaveAs);
            ui.close();
        }
    });
    what
}

/// Render a plain-text body: quoted lines dimmed, links clickable.
pub(super) fn body(ui: &mut egui::Ui, col: &Colors, text: &str) {
    let mut quoted = String::new();
    let flush_quote = |ui: &mut egui::Ui, q: &mut String| {
        if q.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(RichText::new("••• show quoted text").small().color(col.very_subtle))
            .id_salt(ui.next_auto_id())
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new(q.trim_end()).color(col.subtle));
            });
        q.clear();
    };
    let mut para = String::new();
    let flush_para = |ui: &mut egui::Ui, p: &mut String| {
        if p.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (i, line) in p.trim_end_matches('\n').split('\n').enumerate() {
                if i > 0 {
                    ui.end_row();
                }
                for (j, word) in line.split(' ').enumerate() {
                    if j > 0 {
                        ui.label(" ");
                    }
                    let w = word.trim_end_matches(['.', ',', ')', ';']);
                    if w.starts_with("http://") || w.starts_with("https://") {
                        ui.hyperlink_to(RichText::new(w).color(col.link), w);
                        ui.label(&word[w.len()..]);
                    } else {
                        ui.label(RichText::new(word).color(col.text));
                    }
                }
            }
        });
        p.clear();
    };
    for line in text.lines() {
        let is_quote = line.trim_start().starts_with('>') || (line.starts_with("On ") && line.trim_end().ends_with("wrote:"));
        if is_quote || !quoted.is_empty() && line.trim().is_empty() {
            flush_para(ui, &mut para);
            quoted.push_str(line);
            quoted.push('\n');
        } else {
            flush_quote(ui, &mut quoted);
            para.push_str(line);
            para.push('\n');
        }
    }
    flush_para(ui, &mut para);
    flush_quote(ui, &mut quoted);
}

pub(super) fn human_size(b: usize) -> String {
    match b {
        0..=1023 => format!("{b} B"),
        1024..=1_048_575 => format!("{:.0} KB", b as f64 / 1024.0),
        _ => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}
