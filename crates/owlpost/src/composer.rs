//! The compose / reply / forward window (a separate native window).

use crate::attachments::{human_size, Attached, ImageSize};
use crate::style::{self, Colors};
use eframe::egui::{self, Key, RichText};
use owlpost_core::smtp::OutgoingMessage;
use owlpost_core::{Account, AccountId, Contact, Message, Signature, Store};
use std::collections::HashMap;

/// Every signature plus each account's default, loaded by the app.
#[derive(Default, Clone)]
pub struct SignatureBook {
    pub list: Vec<Signature>,
    pub defaults: HashMap<AccountId, i64>,
}

impl SignatureBook {
    pub fn load(store: &Store) -> Self {
        Self { list: store.signatures().unwrap_or_default(), defaults: store.signature_defaults().unwrap_or_default() }
    }
    pub fn get(&self, id: i64) -> Option<&Signature> {
        self.list.iter().find(|s| s.id == id)
    }
    pub fn default_for(&self, account: AccountId) -> Option<&Signature> {
        self.defaults.get(&account).and_then(|id| self.get(*id))
    }
}

pub struct Composer {
    pub id: u64,
    pub account_id: AccountId,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub show_cc: bool,
    pub subject: String,
    /// What you type. The signature and any quoted text are kept separately
    /// and joined on send.
    pub body: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub title: String,
    /// The signature shown under the message (rendered, HTML included).
    signature: Option<Signature>,
    sig_resolved: bool,
    /// The user picked a signature by hand, so changing From won't swap it.
    sig_touched: bool,
    last_account: AccountId,
    /// Quoted / forwarded text, sent after the signature.
    pub tail: String,
    /// Quoted text is folded away until asked for (forwards start open).
    show_tail: bool,
    focus_first: bool,
    focus_body: bool,
    /// The window's close button was pressed on a non-empty draft.
    confirm_close: bool,
    pub attachments: Vec<Attached>,
    /// How big attached photos are sent.
    pub image_size: ImageSize,
    /// Height of everything under the message, measured each frame.
    bottom_h: f32,
}

pub enum ComposerAction {
    None,
    Send(AccountId, Box<OutgoingMessage>),
    Discard,
}

impl Composer {
    pub fn new(id: u64, account: &Account) -> Self {
        Self {
            id,
            account_id: account.id,
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            show_cc: false,
            subject: String::new(),
            body: String::new(),
            in_reply_to: None,
            references: vec![],
            title: "New Message".into(),
            signature: None,
            sig_resolved: false,
            sig_touched: false,
            last_account: account.id,
            tail: String::new(),
            show_tail: false,
            focus_first: true,
            focus_body: false,
            confirm_close: false,
            attachments: vec![],
            image_size: ImageSize::Actual,
            bottom_h: 48.0,
        }
    }

    pub fn reply(id: u64, account: &Account, m: &Message, all: bool) -> Self {
        let d = owlpost_core::mcp::reply_draft(account, m, all);
        let mut c = Self::new(id, account);
        c.to = Contact::join(&d.to);
        c.cc = Contact::join(&d.cc);
        c.show_cc = !c.cc.is_empty();
        c.subject = d.subject;
        c.in_reply_to = d.in_reply_to;
        c.references = d.references;
        c.tail = owlpost_core::smtp::quote(m);
        c.title = if all { "Reply All".into() } else { "Reply".into() };
        // Cursor goes in the body when the recipients are already set.
        c.focus_first = false;
        c.focus_body = true;
        c
    }

    pub fn forward(id: u64, account: &Account, m: &Message) -> Self {
        let mut c = Self::new(id, account);
        c.subject = if m.subject.to_ascii_lowercase().starts_with("fwd:") { m.subject.clone() } else { format!("Fwd: {}", m.subject) };
        c.tail = format!(
            "\n\n---------- Forwarded message ---------\nFrom: {}\nDate: {}\nSubject: {}\nTo: {}\n\n{}",
            m.from.to_header(),
            style::long_date(m.date),
            m.subject,
            Contact::join(&m.to),
            m.body_text
        );
        c.show_tail = true;
        c.title = "Forward".into();
        c
    }

    pub fn set_signature(&mut self, sig: Option<&Signature>) {
        self.signature = sig.cloned();
    }

    /// The message as sent: your text, the signature, then any quoted text.
    /// An HTML signature also goes in as real HTML (see `html_signature`).
    pub(crate) fn draft(&self) -> OutgoingMessage {
        let mut body = self.body.trim_end().to_string();
        let mut html_signature = None;
        if let Some(sig) = &self.signature {
            let text = sig.text();
            if !text.trim().is_empty() {
                body = if body.is_empty() { text.clone() } else { format!("{body}\n\n{text}") };
                if sig.html {
                    html_signature = Some(owlpost_core::smtp::HtmlSignature { text, html: sig.body.clone() });
                }
            }
        }
        body.push_str(&self.tail);
        OutgoingMessage {
            to: Contact::parse_list(&self.to),
            cc: Contact::parse_list(&self.cc),
            bcc: Contact::parse_list(&self.bcc),
            subject: self.subject.clone(),
            body,
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            html_signature,
            attachments: self.attachments.iter().map(|a| a.outgoing(self.image_size)).collect(),
        }
    }

    /// Attach files from disk; returns the names of any that couldn't be read.
    pub fn attach_paths(&mut self, paths: impl IntoIterator<Item = std::path::PathBuf>) -> Vec<String> {
        let mut failed = vec![];
        for p in paths {
            if p.is_dir() {
                failed.push(format!("{} (folders can't be attached)", p.display()));
                continue;
            }
            match Attached::from_path(&p) {
                Ok(a) => self.attachments.push(a),
                Err(e) => failed.push(format!("{}: {e}", p.display())),
            }
        }
        failed
    }

    /// Resizing for the chosen size is still under way.
    pub(crate) fn resizing(&self) -> bool {
        self.attachments.iter().any(|a| a.pending(self.image_size))
    }

    /// Roughly how big the message is once encoded for sending.
    fn message_size(&self) -> usize {
        let files: usize = self.attachments.iter().map(|a| a.bytes(self.image_size).len() * 4 / 3).sum();
        self.body.len() + self.tail.len() + files
    }

    /// Kick off background resizes: thumbnails, and the chosen size.
    fn prepare_images(&self, ctx: &egui::Context) {
        for a in &self.attachments {
            for size in [ImageSize::Small, self.image_size] {
                let ctx = ctx.clone();
                a.prepare(size, move || ctx.request_repaint());
            }
        }
    }

    /// Files dragged onto the window are attached.
    fn take_dropped_files(&mut self, ctx: &egui::Context, col: &Colors) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(path) = f.path {
                self.attach_paths([path]);
            } else if let Some(bytes) = f.bytes {
                self.attachments.push(Attached::new(if f.name.is_empty() { "attachment".to_string() } else { f.name }, bytes.to_vec()));
            }
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let rect = ctx.content_rect();
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("drop", self.id))));
            p.rect_filled(rect, 0.0, col.bg.gamma_multiply(0.85));
            p.rect_stroke(rect.shrink(10.0), 10.0, egui::Stroke::new(2.0_f32, col.accent), egui::StrokeKind::Inside);
            p.text(rect.center(), egui::Align2::CENTER_CENTER, "Drop to attach", egui::FontId::proportional(20.0), col.accent);
        }
    }

    /// Show the composer in its own native window (falls back to an
    /// in-app window where the platform can't open more windows).
    pub fn ui(&mut self, ctx: &egui::Context, accounts: &[Account], store: &Store, col: &Colors, sigs: &SignatureBook) -> ComposerAction {
        if !self.sig_resolved {
            self.sig_resolved = true;
            self.signature = sigs.default_for(self.account_id).cloned();
        }
        let title = if self.subject.trim().is_empty() { self.title.clone() } else { self.subject.clone() };
        let id = egui::ViewportId::from_hash_of(("composer", self.id));
        let builder = egui::ViewportBuilder::default()
            .with_title(&title)
            .with_inner_size([680.0, 580.0])
            .with_min_inner_size([460.0, 340.0]);
        ctx.show_viewport_immediate(id, builder, |ctx, class| {
            if class == egui::ViewportClass::Embedded {
                let mut open = true;
                let mut action = ComposerAction::None;
                egui::Window::new(&title)
                    .id(egui::Id::new(("composer", self.id)))
                    .open(&mut open)
                    .default_size([640.0, 520.0])
                    .min_width(420.0)
                    .resizable(true)
                    .collapsible(true)
                    .show(ctx, |ui| action = self.contents(ui, accounts, store, col, sigs));
                self.take_dropped_files(ctx, col);
                if !open {
                    action = ComposerAction::Discard;
                }
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::W)) {
                    self.request_close(&mut action);
                }
                return action;
            }
            let mut action = ComposerAction::None;
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(col.bg).inner_margin(egui::Margin::same(16)))
                .show(ctx, |ui| action = self.contents(ui, accounts, store, col, sigs));
            self.take_dropped_files(ctx, col);
            // Drafts aren't saved yet, so ask before throwing away any text.
            if ctx.input(|i| i.viewport().close_requested()) {
                if !self.is_blank() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                self.request_close(&mut action);
            }
            // ⌘W closes this window, like the close button.
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::W)) {
                self.request_close(&mut action);
            }
            if ctx.input(|i| i.key_pressed(Key::Escape)) && self.is_blank() {
                action = ComposerAction::Discard;
            }
            action
        })
    }

    /// Close (⌘W or the close button): straight away when nothing's been
    /// typed, otherwise ask first.
    fn request_close(&mut self, action: &mut ComposerAction) {
        if self.is_blank() {
            *action = ComposerAction::Discard;
        } else {
            self.confirm_close = true;
        }
    }

    /// Nothing typed or attached yet (the signature and quoted text don't count).
    pub(crate) fn is_blank(&self) -> bool {
        self.to.trim().is_empty() && self.subject.trim().is_empty() && self.body.trim().is_empty() && self.attachments.is_empty()
    }

    fn contents(&mut self, ui: &mut egui::Ui, accounts: &[Account], store: &Store, col: &Colors, sigs: &SignatureBook) -> ComposerAction {
        let mut action = ComposerAction::None;
        let mut any_focus = false;
        if self.confirm_close {
            egui::Frame::new().fill(col.bg_secondary).corner_radius(6).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Discard this draft?").strong());
                    // No keyboard shortcut for Discard: typing on shouldn't lose the draft.
                    if ui.button("Discard").clicked() {
                        action = ComposerAction::Discard;
                    }
                    if ui.button("Keep editing").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        self.confirm_close = false;
                    }
                });
            });
            ui.add_space(6.0);
            if matches!(action, ComposerAction::Discard) {
                return action;
            }
        }
        egui::Grid::new(("hdr", self.id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("From").color(col.subtle));
            let current = accounts.iter().find(|a| a.id == self.account_id);
            egui::ComboBox::from_id_salt(("from", self.id))
                .width(ui.available_width())
                .selected_text(current.map(|a| format!("{} <{}>", a.display_name, a.email)).unwrap_or_default())
                .show_ui(ui, |ui| {
                    for a in accounts {
                        ui.selectable_value(&mut self.account_id, a.id, format!("{} <{}>", a.display_name, a.email));
                    }
                });
            ui.end_row();
            // Switching From picks up that account's default signature,
            // unless one was chosen by hand.
            if self.account_id != self.last_account {
                self.last_account = self.account_id;
                if !self.sig_touched {
                    self.signature = sigs.default_for(self.account_id).cloned();
                }
            }

            ui.label(RichText::new("To").color(col.subtle));
            ui.horizontal(|ui| {
                let w = ui.available_width() - if self.show_cc { 0.0 } else { 70.0 };
                let r = recipients(ui, "to", &mut self.to, w, "Recipients", store, col);
                if self.focus_first {
                    r.request_focus();
                    self.focus_first = false;
                }
                any_focus |= r.has_focus();
                if !self.show_cc && ui.small_button("Cc Bcc").clicked() {
                    self.show_cc = true;
                }
            });
            ui.end_row();
            if self.show_cc {
                ui.label(RichText::new("Cc").color(col.subtle));
                let r = recipients(ui, "cc", &mut self.cc, f32::INFINITY, "", store, col);
                any_focus |= r.has_focus();
                ui.end_row();
                ui.label(RichText::new("Bcc").color(col.subtle));
                let r = recipients(ui, "bcc", &mut self.bcc, f32::INFINITY, "", store, col);
                any_focus |= r.has_focus();
                ui.end_row();
            }
            ui.label(RichText::new("Subject").color(col.subtle));
            let r = ui.add(egui::TextEdit::singleline(&mut self.subject).desired_width(f32::INFINITY));
            any_focus |= r.has_focus();
            ui.end_row();
        });
        ui.separator();
        // The message takes the height left over by the attachments and
        // footer, as measured last frame.
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - self.bottom_h).max(120.0))
            .show(ui, |ui| {
                let r = ui.add(
                    egui::TextEdit::multiline(&mut self.body)
                        .desired_width(f32::INFINITY)
                        .desired_rows(if self.signature.is_some() || !self.tail.is_empty() { 8 } else { 14 })
                        .frame(false)
                        .lock_focus(true),
                );
                if self.focus_body {
                    r.request_focus();
                    self.focus_body = false;
                }
                any_focus |= r.has_focus();
                // The signature, rendered as recipients will see it.
                if let Some(sig) = &self.signature {
                    ui.add_space(6.0);
                    let r = egui::Frame::new().inner_margin(egui::Margin { left: 4, right: 4, top: 2, bottom: 2 }).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if sig.html {
                            crate::html_view::show(ui, col, &sig.body);
                        } else {
                            ui.label(RichText::new(sig.body.trim_end()).color(col.text));
                        }
                    });
                    r.response.on_hover_text(format!("Signature: {} — change it with the Signature menu below", sig.title));
                }
                // Quoted / forwarded text, folded like Gmail's "•••".
                if !self.tail.is_empty() {
                    ui.add_space(8.0);
                    if self.show_tail {
                        let r = ui.add(
                            egui::TextEdit::multiline(&mut self.tail)
                                .desired_width(f32::INFINITY)
                                .frame(false)
                                .text_color(col.subtle),
                        );
                        any_focus |= r.has_focus();
                    } else if ui.button("•••").on_hover_text("Show quoted text").clicked() {
                        self.show_tail = true;
                    }
                }
            });
        let top = ui.cursor().top();
        if !self.attachments.is_empty() {
            ui.separator();
            self.attachment_strip(ui, col);
        }
        ui.separator();
        let (send_clicked, discard) = self.footer(ui, col, sigs);
        let used = ui.cursor().top() - top + ui.spacing().item_spacing.y;
        if (used - self.bottom_h).abs() > 0.5 {
            self.bottom_h = used;
            ui.ctx().request_repaint();
        }
        if discard {
            return ComposerAction::Discard;
        }
        let shortcut = any_focus && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
        if (send_clicked || shortcut) && self.can_send().is_ok() {
            action = ComposerAction::Send(self.account_id, Box::new(self.draft()));
        }
        action
    }
}

impl Composer {
    /// Why Send is unavailable right now, if it is.
    fn can_send(&self) -> Result<(), String> {
        if self.resizing() {
            return Err("Resizing images…".into());
        }
        if self.message_size() > owlpost_core::smtp::SIZE_LIMIT {
            return Err("Too large to send: most mail servers accept up to 25 MB. Try a smaller image size or fewer files.".into());
        }
        Ok(())
    }

    /// Discard · Attach … Signature · Send. Returns (Send, Discard) clicks.
    fn footer(&mut self, ui: &mut egui::Ui, col: &Colors, sigs: &SignatureBook) -> (bool, bool) {
        let (mut send_clicked, mut discard) = (false, false);
        ui.horizontal(|ui| {
            // Discard on the left, Send on the far right (Mac convention).
            discard = crate::icons::labeled(ui, crate::icons::Icon::Trash, "Discard", col.text, col.bg).clicked();
            if crate::icons::labeled(ui, crate::icons::Icon::Paperclip, "Attach", col.text, col.bg)
                .on_hover_text("Attach files — or drag them onto this window")
                .clicked()
            {
                if let Some(paths) = rfd::FileDialog::new().set_title("Attach Files").pick_files() {
                    self.attach_paths(paths);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let blocked = self.can_send().err();
                let send = ui.add_enabled_ui(blocked.is_none(), |ui| style::primary_button(ui, col, "Send")).inner;
                let send = match &blocked {
                    Some(why) => send.on_disabled_hover_text(why),
                    None => send.on_hover_text("⌘↩"),
                };
                send_clicked = send.clicked();
                ui.add_space(6.0);
                // Signature picker.
                let current = self.signature.as_ref().map(|s| s.id);
                let mut chosen = current;
                let label = chosen.and_then(|id| sigs.get(id)).map(|s| s.title.clone()).unwrap_or_else(|| "No signature".into());
                egui::ComboBox::from_id_salt(("sig", self.id)).selected_text(label).width(170.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "No signature");
                    for sg in &sigs.list {
                        let title = if sg.html { format!("{}  (HTML)", sg.title) } else { sg.title.clone() };
                        ui.selectable_value(&mut chosen, Some(sg.id), title);
                    }
                });
                ui.label(RichText::new("Signature").color(col.subtle));
                if chosen != current {
                    self.sig_touched = true;
                    self.set_signature(chosen.and_then(|id| sigs.get(id)));
                }
            });
        });
        (send_clicked, discard)
    }

    /// The attached files, then the total size and image size menu.
    fn attachment_strip(&mut self, ui: &mut egui::Ui, col: &Colors) {
        self.prepare_images(ui.ctx());
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            for (i, a) in self.attachments.iter().enumerate() {
                if chip(ui, col, a, self.image_size, (self.id, i)) {
                    remove = Some(i);
                }
            }
        });
        ui.horizontal(|ui| {
            let size = self.message_size();
            let n = self.attachments.len();
            let label = format!("{n} attachment{}", if n == 1 { "" } else { "s" });
            ui.label(RichText::new(label).color(col.subtle).small());
            let text = if self.resizing() { "Message size: resizing…".to_string() } else { format!("Message size: {}", human_size(size)) };
            let over = size > owlpost_core::smtp::SIZE_LIMIT;
            ui.label(RichText::new(text).small().color(if over { col.error } else { col.subtle }));
            if self.attachments.iter().any(Attached::resizable) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt(("imgsize", self.id)).selected_text(self.image_size.label()).width(130.0).show_ui(ui, |ui| {
                        for sz in ImageSize::ALL {
                            let text = match sz.max_edge() {
                                Some(px) => format!("{}  ({px} px)", sz.label()),
                                None => sz.label().to_string(),
                            };
                            ui.selectable_value(&mut self.image_size, sz, text);
                        }
                    });
                    ui.label(RichText::new("Image size").color(col.subtle).small());
                });
            }
        });
        if let Some(i) = remove {
            self.attachments.remove(i);
        }
    }
}

/// One attachment: a thumbnail or file badge, name, size and ×.
/// Returns true when × is clicked.
fn chip(ui: &mut egui::Ui, col: &Colors, a: &Attached, size: ImageSize, salt: (u64, usize)) -> bool {
    let mut remove = false;
    egui::Frame::new().fill(col.bg_secondary).corner_radius(8).inner_margin(egui::Margin::symmetric(6, 5)).show(ui, |ui| {
        ui.set_max_width(260.0);
        ui.horizontal(|ui| {
            let thumb = egui::vec2(34.0, 34.0);
            // Thumbnails come from the small copy, made off the UI thread.
            let small = (a.resizable() && !a.pending(ImageSize::Small)).then(|| a.bytes(ImageSize::Small));
            let small = small.or_else(|| (a.is_image() && !a.resizable() && a.data.len() < 2 << 20).then(|| a.data.clone()));
            match small {
                Some(bytes) => {
                    let uri = format!("bytes://attachment/{}/{}/{}", salt.0, salt.1, a.name);
                    ui.add(egui::Image::from_bytes(uri, egui::load::Bytes::from(bytes.to_vec())).fit_to_exact_size(thumb).corner_radius(4));
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(thumb, egui::Sense::hover());
                    ui.painter().rect_filled(rect, 4.0, col.bg);
                    let ext = a.name.rsplit_once('.').map(|(_, e)| e.to_ascii_uppercase()).unwrap_or_else(|| "FILE".into());
                    let ext: String = ext.chars().take(4).collect();
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, ext, egui::FontId::proportional(10.0), col.subtle);
                }
            }
            ui.vertical(|ui| {
                ui.add(egui::Label::new(RichText::new(&a.name).color(col.text)).truncate());
                let bytes = a.bytes(size).len();
                let note = if bytes < a.data.len() { format!("{} (was {})", human_size(bytes), human_size(a.data.len())) } else { human_size(bytes) };
                ui.label(RichText::new(note).color(col.subtle).small());
            });
            if ui.add(egui::Button::new(RichText::new("×").color(col.subtle)).frame(false)).on_hover_text("Remove").clicked() {
                remove = true;
            }
        });
    });
    remove
}

/// The address being typed: everything before it (up to and including the
/// last comma) and the partial address itself.
fn split_tail(field: &str) -> (&str, &str) {
    match field.rfind([',', ';']) {
        Some(i) => (&field[..=i], field[i + 1..].trim()),
        None => ("", field.trim()),
    }
}

/// Replace the partial address with `c`, ready for the next one.
fn accept(field: &str, c: &Contact) -> String {
    let (head, _) = split_tail(field);
    let sep = if head.is_empty() { "" } else { " " };
    format!("{head}{sep}{}, ", c.to_header())
}

/// Suggestions for the partial address: people from your mail (those you
/// write to first), then the system address book. Skips anyone already in
/// the field.
fn suggestions(field: &str, store: &Store) -> Vec<Contact> {
    let (head, tail) = split_tail(field);
    if tail.chars().count() < 2 || tail.contains('<') {
        return Vec::new();
    }
    let taken: Vec<String> = Contact::parse_list(head).into_iter().map(|c| c.email.to_lowercase()).collect();
    let mut out: Vec<Contact> = Vec::new();
    let found = store.suggest_contacts(tail, 6).unwrap_or_default().into_iter().chain(crate::contacts::search(tail, 6));
    for c in found {
        let e = c.email.to_lowercase();
        if e == tail.to_lowercase() || taken.contains(&e) || out.iter().any(|o| o.email.eq_ignore_ascii_case(&e)) {
            continue;
        }
        out.push(c);
        if out.len() == 8 {
            break;
        }
    }
    out
}

/// A To/Cc/Bcc field with address autocomplete. Up/Down pick a
/// suggestion, Enter or Tab takes it, Esc hides the list.
fn recipients(ui: &mut egui::Ui, key: &str, field: &mut String, width: f32, hint: &str, store: &Store, col: &Colors) -> egui::Response {
    let id = ui.make_persistent_id(("recipients", key));
    let sel_id = id.with("sel");
    let hover_id = id.with("hover");
    let hidden_id = id.with("hidden");
    let ctx = ui.ctx().clone();
    let focused = ctx.memory(|m| m.has_focus(id));
    let popup_hovered = ctx.data(|d| d.get_temp::<bool>(hover_id)).unwrap_or(false);
    let hidden_for = ctx.data(|d| d.get_temp::<String>(hidden_id));

    let list = if (focused || popup_hovered) && hidden_for.as_deref() != Some(field.as_str()) { suggestions(field, store) } else { Vec::new() };
    let mut sel = ctx.data(|d| d.get_temp::<usize>(sel_id)).unwrap_or(0).min(list.len().saturating_sub(1));
    let mut chosen: Option<Contact> = None;

    // Keys go to the list before the text field sees them.
    if focused && !list.is_empty() {
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                sel = (sel + 1) % list.len();
            }
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                sel = (sel + list.len() - 1) % list.len();
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Enter) || i.consume_key(egui::Modifiers::NONE, Key::Tab) {
                chosen = Some(list[sel].clone());
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                ctx.data_mut(|d| d.insert_temp(hidden_id, field.clone()));
            }
        });
    }

    let r = ui.add(egui::TextEdit::singleline(field).id(id).desired_width(width).hint_text(hint));

    let mut hovered = false;
    let hidden_now = ctx.data(|d| d.get_temp::<String>(hidden_id));
    if chosen.is_none() && !list.is_empty() && hidden_now.as_deref() != Some(field.as_str()) && (r.has_focus() || popup_hovered) {
        let area = egui::Area::new(id.with("popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(r.rect.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(r.rect.width().min(420.0));
                    for (i, c) in list.iter().enumerate() {
                        let item = ui.horizontal(|ui| {
                            let row = ui.selectable_label(i == sel, RichText::new(format!("{}  ", c.display())).color(col.text));
                            if !c.name.trim().is_empty() {
                                ui.label(RichText::new(&c.email).color(col.subtle).small());
                            }
                            row
                        });
                        let row = item.inner.union(item.response.interact(egui::Sense::click()));
                        if row.hovered() {
                            sel = i;
                        }
                        if row.clicked() {
                            chosen = Some(c.clone());
                        }
                    }
                });
            });
        hovered = area.response.contains_pointer();
    }
    ctx.data_mut(|d| {
        d.insert_temp(hover_id, hovered);
        d.insert_temp(sel_id, sel);
    });

    if let Some(c) = chosen {
        *field = accept(field, &c);
        ctx.data_mut(|d| {
            d.insert_temp(hover_id, false);
            d.insert_temp(sel_id, 0usize);
        });
        // Back to the field with the cursor after the new address.
        ctx.memory_mut(|m| m.request_focus(id));
        if let Some(mut state) = egui::TextEdit::load_state(&ctx, id) {
            let end = egui::text::CCursor::new(field.chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(&ctx, id);
        }
        ctx.request_repaint();
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct() -> Account {
        use owlpost_core::{Security, ServerConfig};
        let server = ServerConfig { host: "imap.example.com".into(), port: 993, security: Security::Tls };
        Account {
            id: 1,
            display_name: "Alex".into(),
            email: "alex@example.com".into(),
            username: "alex@example.com".into(),
            imap: server.clone(),
            smtp: server,
            color: "#3693f3".into(),
            signature: String::new(),
            demo: true,
            auth: owlpost_core::Auth::Password,
        }
    }

    fn plain(body: &str) -> Signature {
        Signature { id: 2, title: "Work".into(), body: body.into(), html: false }
    }

    #[test]
    fn picking_a_suggestion_inserts_the_full_address() {
        let maya = Contact::new("Maya Chen", "maya@chen.family");
        assert_eq!(accept("May", &maya), "\"Maya Chen\" <maya@chen.family>, ");
        let both = accept("\"Sam\" <sam@x.com>, ma", &maya);
        assert_eq!(both, "\"Sam\" <sam@x.com>, \"Maya Chen\" <maya@chen.family>, ");
        let parsed = Contact::parse_list(&both);
        assert_eq!(parsed.iter().map(|c| c.email.as_str()).collect::<Vec<_>>(), ["sam@x.com", "maya@chen.family"]);
        assert_eq!(split_tail("a@b.com; jo"), ("a@b.com;", "jo"));
    }

    #[test]
    fn suggestions_skip_people_already_added() {
        let p = std::env::temp_dir().join(format!("sp-suggest-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let store = Store::open(&p).unwrap();
        owlpost_core::demo::seed(&store).unwrap();
        assert!(suggestions("ma", &store).iter().any(|c| c.email == "maya@chen.family"));
        assert!(!suggestions("maya@chen.family, ma", &store).iter().any(|c| c.email == "maya@chen.family"));
        assert!(suggestions("m", &store).is_empty());
    }

    /// A real click: hover, press, release, each on its own frame.
    #[test]
    fn clicking_a_suggestion_with_the_mouse() {
        use egui_kittest::kittest::Queryable;
        let p = std::env::temp_dir().join(format!("sp-suggest-click-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let store = Store::open(&p).unwrap();
        owlpost_core::demo::seed(&store).unwrap();
        let col = Colors::new(owlpost_core::theme::Palette::from_vars(&Default::default()));
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(600.0, 300.0)).build_ui_state(
            move |ui, field: &mut String| {
                recipients(ui, "to", field, 400.0, "Recipients", &store, &col);
            },
            String::new(),
        );
        h.get_by_role(egui::accesskit::Role::TextInput).focus();
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput).type_text("May");
        h.run();
        let pos = h.get_by_label_contains("Maya Chen").rect().center();
        h.hover_at(pos);
        h.run();
        h.drag_at(pos);
        h.run();
        h.drop_at(pos);
        h.run();
        assert_eq!(h.state(), "\"Maya Chen\" <maya@chen.family>, ");

        // Keyboard: Down then Enter takes the second suggestion.
        h.get_by_role(egui::accesskit::Role::TextInput).type_text("sa");
        h.run();
        h.key_press(Key::Enter);
        h.run();
        assert!(h.state().ends_with(">, ") && h.state().matches('@').count() == 2, "{}", h.state());
    }

    #[test]
    fn signature_goes_between_text_and_quote() {
        let mut c = Composer::new(1, &acct());
        c.body = "Sounds good\n\n".into();
        c.tail = "\n\nOn Monday, Maya wrote:\n> hello".into();
        c.set_signature(Some(&plain("AR")));
        assert_eq!(c.draft().body, "Sounds good\n\nAR\n\nOn Monday, Maya wrote:\n> hello");
        c.set_signature(None);
        assert_eq!(c.draft().body, "Sounds good\n\nOn Monday, Maya wrote:\n> hello");
        // Only typed text counts as content.
        let mut blank = Composer::new(2, &acct());
        blank.set_signature(Some(&plain("— Alex")));
        blank.tail = c.tail.clone();
        assert!(blank.is_blank());
    }

    #[test]
    fn html_signature_reaches_the_outgoing_message() {
        let raw = include_str!("../../owlpost-core/tests/fixtures/canary-signature.html");
        let html = Signature { id: 3, title: "Canary".into(), body: raw.into(), html: true };
        let mut c = Composer::new(1, &acct());
        c.body = "Hello".into();
        c.set_signature(Some(&html));
        let out = c.draft();
        assert!(out.body.starts_with("Hello\n\nTim Morrison\n"), "{}", out.body);
        let full = owlpost_core::smtp::html_body(&out).unwrap();
        assert!(full.contains("Hello<br>") && full.contains("<img src=\"https://www.dropbox.com/"));
        assert!(!full.contains("invert("), "Canary's dark-mode CSS must not be sent");
        // A plain signature: no HTML part.
        c.set_signature(Some(&plain("— A")));
        assert!(c.draft().html_signature.is_none());
    }
}
