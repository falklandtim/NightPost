//! The compose / reply / forward window (a separate native window).

use crate::style::{self, Colors};
use eframe::egui::{self, Key, RichText};
use springpost_core::smtp::OutgoingMessage;
use springpost_core::{Account, AccountId, Contact, Message, Signature, Store};
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
        }
    }

    pub fn reply(id: u64, account: &Account, m: &Message, all: bool) -> Self {
        let d = springpost_core::mcp::reply_draft(account, m, all);
        let mut c = Self::new(id, account);
        c.to = Contact::join(&d.to);
        c.cc = Contact::join(&d.cc);
        c.show_cc = !c.cc.is_empty();
        c.subject = d.subject;
        c.in_reply_to = d.in_reply_to;
        c.references = d.references;
        c.tail = springpost_core::smtp::quote(m);
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
                    html_signature = Some(springpost_core::smtp::HtmlSignature { text, html: sig.body.clone() });
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

    /// Nothing typed yet (the signature and quoted text don't count).
    fn is_blank(&self) -> bool {
        self.to.trim().is_empty() && self.subject.trim().is_empty() && self.body.trim().is_empty()
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
                let r = ui.add(egui::TextEdit::singleline(&mut self.to).desired_width(w).hint_text("Recipients"));
                if self.focus_first {
                    r.request_focus();
                    self.focus_first = false;
                }
                any_focus |= r.has_focus();
                if r.has_focus() {
                    suggest(ui, &mut self.to, store, col);
                }
                if !self.show_cc && ui.small_button("Cc Bcc").clicked() {
                    self.show_cc = true;
                }
            });
            ui.end_row();
            if self.show_cc {
                ui.label(RichText::new("Cc").color(col.subtle));
                let r = ui.add(egui::TextEdit::singleline(&mut self.cc).desired_width(f32::INFINITY));
                any_focus |= r.has_focus();
                ui.end_row();
                ui.label(RichText::new("Bcc").color(col.subtle));
                let r = ui.add(egui::TextEdit::singleline(&mut self.bcc).desired_width(f32::INFINITY));
                any_focus |= r.has_focus();
                ui.end_row();
            }
            ui.label(RichText::new("Subject").color(col.subtle));
            let r = ui.add(egui::TextEdit::singleline(&mut self.subject).desired_width(f32::INFINITY));
            any_focus |= r.has_focus();
            ui.end_row();
        });
        ui.separator();
        let footer_h = 40.0;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - footer_h).max(120.0))
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
        ui.separator();
        ui.horizontal(|ui| {
            // Discard on the left, Send on the far right (Mac convention).
            if crate::icons::labeled(ui, crate::icons::Icon::Trash, "Discard", col.text, col.bg).clicked() {
                action = ComposerAction::Discard;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let send = style::primary_button(ui, col, "Send").on_hover_text("⌘↩");
                let shortcut = any_focus && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
                if send.clicked() || shortcut {
                    action = ComposerAction::Send(self.account_id, Box::new(self.draft()));
                }
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
        action
    }
}

/// Address autocomplete for the token currently being typed.
fn suggest(ui: &mut egui::Ui, field: &mut String, store: &Store, col: &Colors) {
    let (head, tail) = match field.rfind(',') {
        Some(i) => (field[..=i].to_string(), field[i + 1..].trim().to_string()),
        None => (String::new(), field.trim().to_string()),
    };
    if tail.len() < 2 || tail.contains('<') {
        return;
    }
    let Ok(list) = store.suggest_contacts(&tail, 6) else { return };
    let list: Vec<Contact> = list.into_iter().filter(|c| !c.email.eq_ignore_ascii_case(&tail)).collect();
    if list.is_empty() {
        return;
    }
    let pos = ui.min_rect().left_bottom() + egui::vec2(40.0, 4.0);
    egui::Area::new(ui.id().with("suggest"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                for c in list {
                    let text = RichText::new(format!("{}  ", c.display())).color(col.text);
                    let r = ui.horizontal(|ui| {
                        let r = ui.selectable_label(false, text);
                        ui.label(RichText::new(&c.email).color(col.subtle).small());
                        r
                    });
                    if r.inner.clicked() {
                        let sep = if head.is_empty() { "" } else { " " };
                        *field = format!("{head}{sep}{}, ", c.to_header());
                    }
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct() -> Account {
        use springpost_core::{Security, ServerConfig};
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
        }
    }

    fn plain(body: &str) -> Signature {
        Signature { id: 2, title: "Work".into(), body: body.into(), html: false }
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
        let raw = include_str!("../../springpost-core/tests/fixtures/canary-signature.html");
        let html = Signature { id: 3, title: "Canary".into(), body: raw.into(), html: true };
        let mut c = Composer::new(1, &acct());
        c.body = "Hello".into();
        c.set_signature(Some(&html));
        let out = c.draft();
        assert!(out.body.starts_with("Hello\n\nTim Morrison\n"), "{}", out.body);
        let full = springpost_core::smtp::html_body(&out).unwrap();
        assert!(full.contains("Hello<br>") && full.contains("<img src=\"https://www.dropbox.com/"));
        assert!(!full.contains("invert("), "Canary's dark-mode CSS must not be sent");
        // A plain signature: no HTML part.
        c.set_signature(Some(&plain("— A")));
        assert!(c.draft().html_signature.is_none());
    }
}
