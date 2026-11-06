//! Welcome screen and the add-account flow (Gmail, iCloud, and any IMAP host).

use crate::style::{self, Colors};
use eframe::egui::{self, RichText};
use owlpost_core::presets::Provider;
use owlpost_core::{Account, Security, ServerConfig};

pub struct AccountSetup {
    pub provider: Option<Provider>,
    pub name: String,
    pub email: String,
    pub password: String,
    pub username: String,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    pub advanced: bool,
    pub busy: bool,
    pub error: Option<String>,
    /// A Google OAuth client is set up (set by the app).
    pub google_ready: bool,
    /// A Google sign-in is waiting for the browser (set by the app).
    pub google_waiting: bool,
}

pub enum SetupAction {
    None,
    Submit(Box<Account>, String),
    Demo,
    Cancel,
    /// Start "Sign in with Google" in the browser.
    Google,
    CancelGoogle,
    /// Open Preferences → Accounts to set up the Google client.
    GoogleSettings,
}

impl Default for AccountSetup {
    fn default() -> Self {
        Self {
            provider: None,
            name: String::new(),
            email: String::new(),
            password: String::new(),
            username: String::new(),
            imap: ServerConfig { host: String::new(), port: 993, security: Security::Tls },
            smtp: ServerConfig { host: String::new(), port: 587, security: Security::StartTls },
            advanced: false,
            busy: false,
            error: None,
            google_ready: false,
            google_waiting: false,
        }
    }
}

fn provider_tile(ui: &mut egui::Ui, col: &Colors, p: Provider) -> bool {
    let (icon, name) = match p {
        Provider::Gmail => ("✉", "Gmail"),
        Provider::ICloud => ("☁", "iCloud"),
        Provider::Outlook => ("📨", "Outlook"),
        Provider::Yahoo => ("📧", "Yahoo"),
        Provider::Fastmail => ("⚡", "Fastmail"),
        Provider::Custom => ("⚙", "IMAP / SMTP"),
    };
    let size = egui::vec2(132.0, 92.0);
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = if r.hovered() { col.list_hover } else { col.bg };
    let p_ = ui.painter();
    p_.rect(rect, 10.0, fill, egui::Stroke::new(1.0_f32, if r.hovered() { style::hex(p.color()) } else { col.border }), egui::StrokeKind::Inside);
    p_.circle_filled(rect.center_top() + egui::vec2(0.0, 34.0), 20.0, style::hex(p.color()));
    p_.text(rect.center_top() + egui::vec2(0.0, 34.0), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(20.0), egui::Color32::WHITE);
    p_.text(rect.center_bottom() - egui::vec2(0.0, 16.0), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(13.5), col.text);
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

impl AccountSetup {
    fn choose(&mut self, p: Provider) {
        self.provider = Some(p);
        self.error = None;
        if let (Some(i), Some(s)) = (p.imap(), p.smtp()) {
            self.imap = i;
            self.smtp = s;
        }
        self.advanced = p == Provider::Custom;
    }

    fn email_changed(&mut self) {
        // Autodetect provider from the domain and keep username in sync.
        self.username = self.email.trim().to_string();
        if matches!(self.provider, Some(Provider::Custom) | None) {
            let d = Provider::detect(&self.email);
            if d != Provider::Custom {
                self.choose(d);
            } else if self.imap.host.is_empty() {
                if let Some(domain) = self.email.split('@').nth(1).filter(|d| d.contains('.')) {
                    self.imap.host = format!("imap.{domain}");
                    self.smtp.host = format!("smtp.{domain}");
                }
            }
        }
    }

    fn account(&self) -> Account {
        let p = self.provider.unwrap_or(Provider::Custom);
        Account {
            id: 0,
            display_name: if self.name.trim().is_empty() {
                self.email.split('@').next().unwrap_or("").to_string()
            } else {
                self.name.trim().to_string()
            },
            email: self.email.trim().to_string(),
            username: if self.username.trim().is_empty() { self.email.trim().to_string() } else { self.username.trim().to_string() },
            imap: self.imap.clone(),
            smtp: self.smtp.clone(),
            color: p.color().into(),
            signature: String::new(),
            demo: false,
            auth: owlpost_core::Auth::Password,
        }
    }

    /// Full-page welcome shown when no accounts exist.
    pub fn welcome(&mut self, ui: &mut egui::Ui, col: &Colors) -> SetupAction {
        let mut act = SetupAction::None;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.08).max(20.0));
            ui.label(RichText::new("🌱").size(48.0));
            ui.label(RichText::new("Welcome to OwlPost").size(28.0).strong().color(col.heading));
            ui.label(RichText::new("A fast, native mail client. Connect an account to get started.").color(col.subtle));
            ui.add_space(24.0);
            act = self.body(ui, col, true);
        });
        act
    }

    /// Modal window used from Preferences → Accounts.
    pub fn window(&mut self, ctx: &egui::Context, col: &Colors) -> SetupAction {
        let mut act = SetupAction::None;
        let mut open = true;
        egui::Window::new("Add Account")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| act = self.body(ui, col, false));
            });
        if !open {
            act = SetupAction::Cancel;
        }
        act
    }

    fn body(&mut self, ui: &mut egui::Ui, col: &Colors, welcome: bool) -> SetupAction {
        let mut act = SetupAction::None;
        let Some(provider) = self.provider else {
            ui.label(RichText::new("Choose your provider").strong());
            ui.add_space(8.0);
            let width = 3.0 * 132.0 + 2.0 * 12.0;
            for row in Provider::ALL.chunks(3) {
                ui.allocate_ui_with_layout(egui::vec2(width, 96.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    for p in row {
                        if provider_tile(ui, col, *p) {
                            self.choose(*p);
                        }
                    }
                });
                ui.add_space(6.0);
            }
            ui.add_space(14.0);
            if welcome
                && ui.link("Just looking? Explore OwlPost with a demo mailbox").clicked() {
                    act = SetupAction::Demo;
                }
            return act;
        };

        ui.horizontal(|ui| {
            if ui.button("Back").clicked() {
                self.provider = None;
                self.error = None;
            }
            ui.label(RichText::new(provider.label()).strong().size(16.0));
        });
        ui.add_space(8.0);
        if provider == Provider::Gmail {
            ui.horizontal(|ui| {
                if self.google_waiting {
                    ui.spinner();
                    ui.label("Finish signing in in your browser…");
                    if ui.button("Cancel").clicked() {
                        act = SetupAction::CancelGoogle;
                    }
                } else if self.google_ready {
                    if style::primary_button(ui, col, "Sign in with Google").on_hover_text("Sign in in your browser — no app password needed").clicked() {
                        self.error = None;
                        act = SetupAction::Google;
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new("Sign in with Google"));
                    if ui.link("Set up Google sign-in first").on_hover_text("Preferences > Accounts").clicked() {
                        act = SetupAction::GoogleSettings;
                    }
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("…or connect with an app password:").small().color(col.subtle));
            ui.add_space(4.0);
        }
        let form_w = 420.0;
        ui.allocate_ui(egui::vec2(form_w, 0.0), |ui| {
            egui::Grid::new("acct").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("Your name").desired_width(300.0));
                ui.end_row();
                ui.label("Email");
                let hint = match provider {
                    Provider::ICloud => "you@icloud.com",
                    Provider::Gmail => "you@gmail.com",
                    _ => "you@example.com",
                };
                if ui.add(egui::TextEdit::singleline(&mut self.email).hint_text(hint).desired_width(300.0)).changed() {
                    self.email_changed();
                }
                ui.end_row();
                ui.label(match provider {
                    Provider::Gmail | Provider::ICloud | Provider::Yahoo | Provider::Fastmail => "App password",
                    _ => "Password",
                });
                ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(300.0));
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.label(RichText::new(provider.password_help()).small().color(col.subtle));
            match provider {
                Provider::ICloud => {
                    ui.hyperlink_to("Create an app-specific password at account.apple.com ↗", "https://account.apple.com/account/manage");
                }
                Provider::Gmail => {
                    ui.hyperlink_to("Create an app password at myaccount.google.com ↗", "https://myaccount.google.com/apppasswords");
                }
                _ => {}
            }
            ui.add_space(6.0);
            ui.checkbox(&mut self.advanced, "Show server settings");
            if self.advanced {
                egui::Grid::new("servers").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Username");
                    ui.add(egui::TextEdit::singleline(&mut self.username).desired_width(200.0));
                    ui.end_row();
                    server_row(ui, "IMAP", &mut self.imap, "imap");
                    server_row(ui, "SMTP", &mut self.smtp, "smtp");
                });
            }
        });
        if let Some(e) = &self.error {
            ui.add_space(6.0);
            ui.label(RichText::new(e).color(col.error));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let ok = self.email.contains('@') && !self.password.is_empty() && !self.imap.host.is_empty() && !self.smtp.host.is_empty();
            ui.add_enabled_ui(ok && !self.busy, |ui| {
                if style::primary_button(ui, col, if self.busy { "Connecting…" } else { "Connect Account" }).clicked() {
                    self.busy = true;
                    self.error = None;
                    act = SetupAction::Submit(Box::new(self.account()), self.password.clone());
                }
            });
            if self.busy {
                ui.spinner();
            }
        });
        act
    }
}

fn server_row(ui: &mut egui::Ui, label: &str, s: &mut ServerConfig, id: &str) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(&mut s.host).desired_width(200.0).hint_text("host"));
    let mut port = s.port.to_string();
    if ui.add(egui::TextEdit::singleline(&mut port).desired_width(48.0)).changed() {
        if let Ok(p) = port.parse() {
            s.port = p;
        }
    }
    egui::ComboBox::from_id_salt(id).selected_text(s.security.label()).show_ui(ui, |ui| {
        for sec in [Security::Tls, Security::StartTls, Security::None] {
            ui.selectable_value(&mut s.security, sec, sec.label());
        }
    });
    ui.end_row();
}
