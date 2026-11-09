//! Preferences: appearance, accounts, MCP, shortcuts.

use crate::style::{self, Colors};
use eframe::egui::{self, RichText};
use owlpost_core::theme::Theme;
use crate::composer::SignatureBook;
use crate::worker::PushState;
use owlpost_core::{Account, AccountId, Auth, Signature};

/// What a swipe on a conversation in the list does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SwipeAction {
    Nothing,
    Archive,
    Trash,
    Snooze,
    Pin,
    ToggleRead,
}

impl SwipeAction {
    pub const ALL: [SwipeAction; 6] =
        [SwipeAction::Archive, SwipeAction::Trash, SwipeAction::Snooze, SwipeAction::Pin, SwipeAction::ToggleRead, SwipeAction::Nothing];
    pub fn label(self) -> &'static str {
        match self {
            SwipeAction::Nothing => "Nothing",
            SwipeAction::Archive => "Archive",
            SwipeAction::Trash => "Move to Trash",
            SwipeAction::Snooze => "Snooze",
            SwipeAction::Pin => "Pin / unpin",
            SwipeAction::ToggleRead => "Mark read / unread",
        }
    }
    fn key(self) -> &'static str {
        match self {
            SwipeAction::Nothing => "none",
            SwipeAction::Archive => "archive",
            SwipeAction::Trash => "trash",
            SwipeAction::Snooze => "snooze",
            SwipeAction::Pin => "pin",
            SwipeAction::ToggleRead => "read",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == s)
    }
}

/// Message-list behaviour, saved with the window state.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ListSettings {
    /// Row dragged to the left (reveals the right-hand side).
    pub swipe_left: SwipeAction,
    /// Row dragged to the right.
    pub swipe_right: SwipeAction,
    /// Show action buttons when hovering a conversation in a wide list.
    pub quick_actions: bool,
    /// The Pinned section is folded away ("Hide" in its header).
    pub hide_pinned: bool,
    /// Undo Send: seconds a sent message waits before it goes (0: off).
    pub undo_send_secs: u32,
}

/// The Undo Send delays on offer.
pub const UNDO_SEND_CHOICES: [u32; 6] = [0, 5, 10, 20, 30, 60];

impl Default for ListSettings {
    fn default() -> Self {
        Self { swipe_left: SwipeAction::Trash, swipe_right: SwipeAction::Archive, quick_actions: true, hide_pinned: false, undo_send_secs: 10 }
    }
}

impl ListSettings {
    pub fn encode(&self) -> String {
        format!("{},{},{},{},{}", self.swipe_left.key(), self.swipe_right.key(), self.quick_actions as u8, self.hide_pinned as u8, self.undo_send_secs)
    }
    pub fn decode(s: &str) -> Self {
        let mut d = Self::default();
        let parts: Vec<&str> = s.split(',').collect();
        if let [l, r, q, rest @ ..] = parts.as_slice() {
            d.swipe_left = SwipeAction::parse(l).unwrap_or(d.swipe_left);
            d.swipe_right = SwipeAction::parse(r).unwrap_or(d.swipe_right);
            d.quick_actions = *q != "0";
            // Added later: older settings have three fields.
            d.hide_pinned = rest.first().is_some_and(|h| *h == "1");
            if let Some(u) = rest.get(1).and_then(|u| u.parse().ok()) {
                d.undo_send_secs = u;
            }
        }
        d
    }
}

/// Minimum list width at which hover quick actions appear (as in Mailspring,
/// where they show once the list is wide enough).
pub const QUICK_ACTIONS_MIN_WIDTH: f32 = 420.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Appearance,
    List,
    Signatures,
    Accounts,
    Mcp,
    Shortcuts,
}

pub struct Prefs {
    pub tab: Tab,
    pub editing: Option<Account>,
    pub message: Option<(bool, String)>,
    /// The signature open in the editor (an edited copy).
    pub sig_edit: Option<Signature>,
    /// Push status per account (kept up to date by the app).
    pub push: std::collections::HashMap<AccountId, PushState>,
    /// A Google OAuth client is saved (set by the app).
    pub google_ready: bool,
    /// A Google sign-in is under way: the sign-in URL once the browser opened.
    pub google_waiting: Option<Option<String>>,
    google_id: String,
    google_secret: String,
    google_edit: bool,
    /// Emails shown as designed (HTML in the web view); set by the app.
    pub html_email: bool,
    /// Load remote images without asking (trackers are always blocked).
    pub load_images: bool,
    /// Recolour emails to match a dark theme.
    pub dark_emails: bool,
    /// Show sender photos and logos.
    pub faces: bool,
}

pub enum PrefsAction {
    None,
    SetTheme(String),
    AddAccount,
    RemoveAccount(AccountId),
    SaveAccount(Account),
    AddDemo,
    RemoveDemo,
    SaveSignature(Signature),
    DeleteSignature(i64),
    SetDefaultSignature(AccountId, Option<i64>),
    /// Save the Google OAuth client (ID, secret).
    SaveGoogleClient(String, String),
    ClearGoogleClient,
    /// Sign in with Google: a new account (None) or switch/renew one.
    GoogleSignIn(Option<AccountId>),
    CancelGoogle,
    /// Show emails as designed (HTML) or as plain text.
    SetHtmlEmail(bool),
    SetLoadImages(bool),
    SetDarkEmails(bool),
    SetFaces(bool),
    Close,
}

impl Prefs {
    pub fn new(tab: Tab) -> Self {
        Self { tab, editing: None, message: None, sig_edit: None, push: Default::default(), google_ready: false, google_waiting: None, google_id: String::new(), google_secret: String::new(), google_edit: false, html_email: true, load_images: true, dark_emails: true, faces: true }
    }

    #[allow(clippy::too_many_arguments)] // one window, one call site
    pub fn ui(&mut self, ctx: &egui::Context, col: &Colors, themes: &[Theme], current: &str, accounts: &[Account], list: &mut ListSettings, sigs: &SignatureBook) -> PrefsAction {
        let mut act = PrefsAction::None;
        let mut open = true;
        egui::Window::new("Preferences")
            .open(&mut open)
            .default_size([720.0, 520.0])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (t, label) in [
                        (Tab::Appearance, "🎨 Appearance"),
                        (Tab::List, "✋ Message list"),
                        (Tab::Signatures, "📝 Signatures"),
                        (Tab::Accounts, "👤 Accounts"),
                        (Tab::Mcp, "🔗 MCP Server"),
                        (Tab::Shortcuts, "⌨ Shortcuts"),
                    ] {
                        ui.selectable_value(&mut self.tab, t, label);
                    }
                });
                ui.separator();
                if let Some((ok, m)) = &self.message {
                    ui.label(RichText::new(m).color(if *ok { col.success } else { col.error }));
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                    Tab::Appearance => act = self.appearance(ui, col, themes, current),
                    Tab::List => list_settings(ui, col, list),
                    Tab::Signatures => act = self.signatures(ui, col, accounts, sigs),
                    Tab::Accounts => act = self.accounts(ui, col, accounts, sigs),
                    Tab::Mcp => mcp(ui, col),
                    Tab::Shortcuts => shortcuts(ui, col),
                });
            });
        if !open {
            act = PrefsAction::Close;
        }
        act
    }

    fn appearance(&mut self, ui: &mut egui::Ui, col: &Colors, themes: &[Theme], current: &str) -> PrefsAction {
        let mut act = PrefsAction::None;
        ui.label(RichText::new("Messages").strong().size(16.0));
        let mut html = self.html_email;
        let tip = if cfg!(any(target_os = "macos", target_os = "linux")) {
            "HTML emails are shown with their layout, colours and fonts (WebKit). Scripts never run, and tracking pixels are always blocked."
        } else {
            "Coming soon on this platform: messages are shown as text for now."
        };
        if ui.checkbox(&mut html, "Show emails as designed (HTML) — experimental").on_hover_text(tip).changed() {
            act = PrefsAction::SetHtmlEmail(html);
        }
        ui.add_enabled_ui(self.html_email, |ui| {
            let mut load = self.load_images;
            let tip = "On: pictures in emails load straight away, like Canary and Apple Mail. Off: they wait until you click Load images. \
                       Tracking pixels, which report when and where you open an email, are blocked either way.";
            if ui.checkbox(&mut load, "Load remote images (trackers are always blocked)").on_hover_text(tip).changed() {
                act = PrefsAction::SetLoadImages(load);
            }
            let mut dark = self.dark_emails;
            let tip = "With a dark theme, emails are recoloured to match: white backgrounds turn dark and black text turns light, \
                       while coloured banners, buttons and pictures keep their look. Off: emails are shown on white, as designed.";
            if ui.checkbox(&mut dark, "Match emails to a dark theme").on_hover_text(tip).changed() {
                act = PrefsAction::SetDarkEmails(dark);
            }
        });
        let mut faces = self.faces;
        let tip = "A person's photo from your Contacts, or a company's logo: the verified brand logo it publishes (BIMI), \
                   else its website's icon. Logos are fetched from the sender's own domain and cached for a month; no lookup service is used.";
        if ui.checkbox(&mut faces, "Show sender photos and logos").on_hover_text(tip).changed() {
            act = PrefsAction::SetFaces(faces);
        }
        ui.add_space(14.0);
        ui.label(RichText::new("Appearance").strong().size(16.0));
        ui.label(RichText::new("The Moonlit theme, light or dark.").color(col.subtle));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            for t in themes {
                if theme_tile(ui, col, t, t.id == current) {
                    act = PrefsAction::SetTheme(t.id.clone());
                }
            }
        });
        act
    }

    fn signatures(&mut self, ui: &mut egui::Ui, col: &Colors, accounts: &[Account], sigs: &SignatureBook) -> PrefsAction {
        let mut act = PrefsAction::None;
        ui.label(RichText::new("Signatures").strong().size(16.0));
        ui.label(RichText::new("Create signatures and choose which one each account uses by default. You can switch signature in any message you write.").color(col.subtle));
        ui.add_space(8.0);
        // Keep the editor pointed at something that exists.
        if self.sig_edit.as_ref().is_some_and(|e| sigs.get(e.id).is_none()) {
            self.sig_edit = None;
        }
        if self.sig_edit.is_none() {
            self.sig_edit = sigs.list.first().cloned();
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(180.0);
                for sg in &sigs.list {
                    let sel = self.sig_edit.as_ref().is_some_and(|e| e.id == sg.id);
                    let title = if sg.title.trim().is_empty() { "Untitled".to_string() } else { sg.title.clone() };
                    let title = if sg.html { format!("{title}  · HTML") } else { title };
                    if ui.add(egui::Button::selectable(sel, title.as_str()).min_size(egui::vec2(176.0, 26.0))).clicked() {
                        self.sig_edit = Some(sg.clone());
                    }
                }
                ui.add_space(6.0);
                if ui.button("+ New signature").clicked() {
                    let n = sigs.list.len() + 1;
                    act = PrefsAction::SaveSignature(Signature { id: 0, title: format!("Signature {n}"), body: String::new(), html: false });
                }
            });
            ui.separator();
            ui.vertical(|ui| {
                let Some(e) = self.sig_edit.as_mut() else {
                    ui.label(RichText::new("No signatures yet. Click “New signature” to make one.").color(col.subtle));
                    return;
                };
                ui.label(RichText::new("Name").small().color(col.subtle));
                ui.add(egui::TextEdit::singleline(&mut e.title).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Signature").small().color(col.subtle));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Plain text / HTML toggle. Switching keeps what you wrote readable.
                        let mut html = e.html;
                        ui.selectable_value(&mut html, true, "HTML");
                        ui.selectable_value(&mut html, false, "Plain text");
                        if html != e.html {
                            if !e.body.trim().is_empty() {
                                e.body = if html {
                                    owlpost_core::smtp::text_to_html(e.body.trim_end())
                                } else {
                                    owlpost_core::parse::signature_text(&e.body)
                                };
                            }
                            e.html = html;
                        }
                    });
                });
                if e.html {
                    ui.add(
                        egui::TextEdit::multiline(&mut e.body)
                            .code_editor()
                            .desired_rows(8)
                            .desired_width(f32::INFINITY)
                            .hint_text("<p><b>Alex Rivera</b><br>\n<a href=\"https://example.com\">example.com</a></p>"),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new("Preview").small().color(col.subtle));
                    egui::Frame::new().fill(col.bg).stroke(egui::Stroke::new(1.0_f32, col.border)).corner_radius(6).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if e.body.trim().is_empty() {
                            ui.label(RichText::new("(empty)").color(col.very_subtle));
                        } else {
                            crate::html_view::show(ui, col, &e.body);
                        }
                    });
                    egui::CollapsingHeader::new(RichText::new("Plain-text version").small().color(col.subtle))
                        .id_salt(("plain", e.id))
                        .show(ui, |ui| {
                            ui.label(RichText::new("Sent alongside the HTML for mail apps that don't show formatting.").small().color(col.very_subtle));
                            ui.label(e.text());
                        });
                    let lower = e.body.to_ascii_lowercase();
                    if lower.contains("<html") || lower.contains("<style") || lower.contains("<head") {
                        ui.label(
                            RichText::new("This looks like a whole page copied from another app. On Save, OwlPost keeps just the signature and removes page styles (such as Canary's dark-mode colour inversion), which would otherwise affect the whole email.")
                                .small()
                                .color(col.warning),
                        );
                    }
                } else {
                    ui.add(egui::TextEdit::multiline(&mut e.body).desired_rows(7).desired_width(f32::INFINITY).hint_text("Alex Rivera\nNorthwind · +64 21 000 000"));
                }
                ui.add_space(6.0);
                let dirty = sigs.get(e.id) != Some(&*e);
                ui.horizontal(|ui| {
                    if ui.add_enabled(dirty, egui::Button::new("Save")).clicked() {
                        act = PrefsAction::SaveSignature(e.clone());
                    }
                    if dirty && ui.button("Revert").clicked() {
                        if let Some(orig) = sigs.get(e.id) {
                            *e = orig.clone();
                        }
                    }
                    if ui.button("Delete").clicked() {
                        act = PrefsAction::DeleteSignature(e.id);
                    }
                });
                ui.add_space(10.0);
                ui.label(RichText::new("Use by default for").strong());
                for a in accounts {
                    let mut on = sigs.defaults.get(&a.id) == Some(&e.id);
                    if ui.checkbox(&mut on, &a.email).changed() {
                        act = PrefsAction::SetDefaultSignature(a.id, on.then_some(e.id));
                    }
                }
            });
        });
        act
    }

    fn accounts(&mut self, ui: &mut egui::Ui, col: &Colors, accounts: &[Account], sigs: &SignatureBook) -> PrefsAction {
        let mut act = PrefsAction::None;
        if let Some(a) = self.google_section(ui, col) {
            act = a;
        }
        ui.add_space(8.0);
        for a in accounts {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    style::avatar(ui, &a.display_name, &a.email, 30.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&a.email).strong());
                        ui.label(
                            RichText::new(if a.demo { "Demo account (offline)".to_string() } else { format!("{} · {}", a.imap.host, a.smtp.host) })
                                .small()
                                .color(col.subtle),
                        );
                        if !a.demo {
                            push_line(ui, col, self.push.get(&a.id));
                        }
                        if a.auth == Auth::Google {
                            ui.label(RichText::new("Signs in with Google").small().color(col.subtle));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Remove").clicked() {
                            act = PrefsAction::RemoveAccount(a.id);
                        }
                        if ui.button("Edit").clicked() {
                            self.editing = Some(a.clone());
                        }
                        if !a.demo && a.is_gmail() && self.google_waiting.is_none() {
                            let label = if a.auth == Auth::Google { "Sign in again" } else { "Use Google sign-in" };
                            let tip = if a.auth == Auth::Google {
                                "Sign in with Google again (if access was revoked or has expired)"
                            } else {
                                "Sign in with Google in your browser instead of an app password"
                            };
                            let r = ui.add_enabled(self.google_ready, egui::Button::new(label));
                            let r = if self.google_ready { r.on_hover_text(tip) } else { r.on_disabled_hover_text("Set up Google sign-in above first") };
                            if r.clicked() {
                                act = PrefsAction::GoogleSignIn(Some(a.id));
                            }
                        }
                    });
                });
                let mut cancel = false;
                if let Some(e) = self.editing.as_mut().filter(|e| e.id == a.id) {
                    ui.separator();
                    egui::Grid::new(("edit", a.id)).num_columns(2).show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut e.display_name);
                        ui.end_row();
                        ui.label("Signature");
                        ui.horizontal(|ui| {
                            ui.label(sigs.default_for(a.id).map(|s| s.title.clone()).unwrap_or_else(|| "None".into()));
                            if ui.link("Edit signatures…").clicked() {
                                self.tab = Tab::Signatures;
                            }
                        });
                        ui.end_row();
                        ui.label("Colour");
                        ui.text_edit_singleline(&mut e.color);
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if style::primary_button(ui, col, "Save").clicked() {
                            act = PrefsAction::SaveAccount(e.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                }
                if cancel {
                    self.editing = None;
                }
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if style::primary_button(ui, col, "+ Add Account").clicked() {
                act = PrefsAction::AddAccount;
            }
            if accounts.iter().any(|a| a.demo) {
                if ui.button("Remove demo accounts").clicked() {
                    act = PrefsAction::RemoveDemo;
                }
            } else if ui.button("Add demo accounts").clicked() {
                act = PrefsAction::AddDemo;
            }
        });
        act
    }
}

fn theme_tile(ui: &mut egui::Ui, col: &Colors, t: &Theme, selected: bool) -> bool {
    // "Match system" shows light and dark side by side.
    let system = t.id == owlpost_core::theme::MOONLIT;
    let p = t.palette();
    let size = egui::vec2(150.0, 118.0);
    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter_at(rect.expand(2.0));
    let preview = egui::Rect::from_min_size(rect.min, egui::vec2(size.x, 78.0));
    painter.rect_filled(preview, 6.0, style::c(p.list));
    // Mini three-pane preview drawn from the theme's own variables.
    let side = egui::Rect::from_min_size(preview.min, egui::vec2(38.0, preview.height()));
    painter.rect_filled(side, egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, style::c(p.sidebar));
    painter.rect_filled(
        egui::Rect::from_min_size(side.min + egui::vec2(4.0, 10.0), egui::vec2(30.0, 8.0)),
        2.0,
        style::c(p.sidebar_active),
    );
    for i in 0..3 {
        let y = preview.min.y + 10.0 + i as f32 * 20.0;
        let row = egui::Rect::from_min_size(egui::pos2(side.max.x + 4.0, y - 4.0), egui::vec2(52.0, 16.0));
        if i == 0 {
            painter.rect_filled(row, 2.0, style::c(p.list_focused));
        }
        let txt = if i == 0 { p.list_focused_text } else { p.text };
        painter.rect_filled(egui::Rect::from_min_size(row.min + egui::vec2(4.0, 4.0), egui::vec2(36.0, 3.0)), 1.0, style::c(txt));
        painter.rect_filled(egui::Rect::from_min_size(row.min + egui::vec2(4.0, 10.0), egui::vec2(26.0, 2.0)), 1.0, style::c(p.text_subtle));
    }
    let reader = egui::Rect::from_min_max(egui::pos2(side.max.x + 60.0, preview.min.y), preview.max);
    painter.rect_filled(reader, egui::CornerRadius { ne: 6, se: 6, nw: 0, sw: 0 }, style::c(p.background_off));
    painter.rect_filled(egui::Rect::from_min_size(reader.min + egui::vec2(6.0, 8.0), egui::vec2(36.0, 4.0)), 1.0, style::c(p.heading));
    painter.rect_filled(egui::Rect::from_min_size(reader.min + egui::vec2(6.0, 60.0), egui::vec2(20.0, 8.0)), 2.0, style::c(p.accent));
    let stroke = if selected {
        egui::Stroke::new(2.5_f32, col.accent)
    } else if r.hovered() {
        egui::Stroke::new(1.5_f32, col.accent.gamma_multiply(0.6))
    } else {
        egui::Stroke::new(1.0_f32, col.border)
    };
    if system {
        let dark = owlpost_core::theme::discover().into_iter().find(|d| d.id == owlpost_core::theme::MOONLIT_DARK).map(|d| d.palette());
        if let Some(d) = dark {
            let half = egui::Rect::from_min_max(egui::pos2(preview.center().x, preview.min.y), preview.max);
            painter.rect_filled(half, egui::CornerRadius { ne: 6, se: 6, nw: 0, sw: 0 }, style::c(d.background_off));
            painter.rect_filled(egui::Rect::from_min_size(half.min + egui::vec2(12.0, 8.0), egui::vec2(36.0, 4.0)), 1.0, style::c(d.heading));
            painter.rect_filled(egui::Rect::from_min_size(half.min + egui::vec2(12.0, 20.0), egui::vec2(46.0, 2.0)), 1.0, style::c(d.text_subtle));
            painter.rect_filled(egui::Rect::from_min_size(half.min + egui::vec2(12.0, 60.0), egui::vec2(20.0, 8.0)), 2.0, style::c(d.accent));
        }
    }
    painter.rect_stroke(preview, 6.0, stroke, egui::StrokeKind::Outside);
    style::line(&painter, egui::pos2(rect.min.x, preview.max.y + 6.0), &t.display_name, egui::FontId::proportional(13.5), col.text, size.x);
    let sub = t.description.split(':').next().unwrap_or_default();
    style::line(&painter, egui::pos2(rect.min.x, preview.max.y + 24.0), sub, egui::FontId::proportional(11.0), col.subtle, size.x);
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    r.clicked()
}

fn copy_block(ui: &mut egui::Ui, col: &Colors, text: &str) {
    egui::Frame::new()
        .fill(col.bg_secondary)
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(text.to_string());
                    }
                });
            });
        });
}

fn mcp(ui: &mut egui::Ui, col: &Colors) {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "owlpost".into());
    ui.label(RichText::new("Built-in MCP server").strong().size(16.0));
    ui.label(
        RichText::new(
            "OwlPost includes a Model Context Protocol server, so assistants like Claude can search, read, triage, \
             snooze and send your mail using the accounts configured here. It runs as `owlpost mcp` over stdio and \
             shares this app's local database.",
        )
        .color(col.subtle),
    );
    ui.add_space(10.0);
    ui.label(RichText::new("Claude Code").strong());
    copy_block(ui, col, &format!("claude mcp add owlpost -- \"{exe}\" mcp"));
    ui.add_space(8.0);
    ui.label(RichText::new("Claude Desktop (claude_desktop_config.json) or any MCP client").strong());
    copy_block(
        ui,
        col,
        &format!("{{\n  \"mcpServers\": {{\n    \"owlpost\": {{\n      \"command\": \"{}\",\n      \"args\": [\"mcp\"]\n    }}\n  }}\n}}", exe.replace('\\', "\\\\")),
    );
    ui.add_space(10.0);
    ui.label(RichText::new("Tools").strong());
    for (name, desc) in [
        ("list_accounts / list_folders", "See configured accounts and mailboxes"),
        ("list_threads / search", "Browse or search conversations (from:, subject:, is:unread …)"),
        ("read_thread", "Read a whole conversation"),
        ("snooze_thread / unsnooze_thread / list_snoozed", "Defer mail until later"),
        ("mark_read / star_thread / move_thread", "Triage: read state, stars, archive, trash, spam"),
        ("send_email / reply", "Compose and send (clients ask you to confirm)"),
        ("sync", "Fetch new mail now"),
    ] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).monospace().color(col.accent));
            ui.label(RichText::new(desc).color(col.subtle));
        });
    }
}

fn shortcuts(ui: &mut egui::Ui, col: &Colors) {
    egui::Grid::new("keys").num_columns(2).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
        for (k, d) in crate::app::SHORTCUTS {
            ui.label(RichText::new(*k).monospace().strong());
            ui.label(RichText::new(*d).color(col.subtle));
            ui.end_row();
        }
    });
}

fn list_settings(ui: &mut egui::Ui, col: &Colors, list: &mut ListSettings) {
    ui.label(RichText::new("Swipe actions").strong().size(16.0));
    ui.label(RichText::new("Swipe sideways with two fingers on a trackpad, or drag a conversation with the mouse.").color(col.subtle));
    ui.add_space(6.0);
    egui::Grid::new("swipes").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
        for (label, value, salt) in [("Swipe right", &mut list.swipe_right, "sr"), ("Swipe left", &mut list.swipe_left, "sl")] {
            ui.label(label);
            egui::ComboBox::from_id_salt(salt).selected_text(value.label()).width(200.0).show_ui(ui, |ui| {
                for a in SwipeAction::ALL {
                    ui.selectable_value(value, a, a.label());
                }
            });
            ui.end_row();
        }
    });
    ui.add_space(14.0);
    ui.label(RichText::new("Quick actions").strong().size(16.0));
    ui.checkbox(&mut list.quick_actions, "Show action buttons when hovering over a conversation");
    ui.label(
        RichText::new(format!(
            "Like Mailspring, they appear once the message list is at least {} points wide. Drag the list's edge to widen it.",
            QUICK_ACTIONS_MIN_WIDTH as i32
        ))
        .small()
        .color(col.subtle),
    );
    ui.add_space(14.0);
    ui.label(RichText::new("Undo Send").strong().size(16.0));
    ui.horizontal(|ui| {
        ui.label("Wait before sending");
        let label = |s: u32| if s == 0 { "Off — send straight away".to_string() } else { format!("{s} seconds") };
        egui::ComboBox::from_id_salt("undo_send").selected_text(label(list.undo_send_secs)).width(200.0).show_ui(ui, |ui| {
            for s in UNDO_SEND_CHOICES {
                ui.selectable_value(&mut list.undo_send_secs, s, label(s));
            }
        });
    });
    ui.label(
        RichText::new("After you press Send, the message waits this long. Click Undo (or press ⌘Z) to get it back and keep editing.")
            .small()
            .color(col.subtle),
    );
}

impl Prefs {
    /// Google sign-in: the OAuth client (set up once), and the sign-in button.
    fn google_section(&mut self, ui: &mut egui::Ui, col: &Colors) -> Option<PrefsAction> {
        let mut act = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Sign in with Google").strong());
            if let Some(url) = &self.google_waiting {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Finish signing in in your browser…");
                    if ui.button("Cancel").clicked() {
                        act = Some(PrefsAction::CancelGoogle);
                    }
                    if let Some(url) = url {
                        if ui.small_button("Copy link").on_hover_text("If the browser didn't open, paste this into it").clicked() {
                            ui.ctx().copy_text(url.clone());
                        }
                    }
                });
                return;
            }
            if self.google_ready && !self.google_edit {
                ui.label(
                    RichText::new("Gmail and Google Workspace accounts can sign in through Google in your browser — no app passwords.")
                        .small()
                        .color(col.subtle),
                );
                ui.horizontal(|ui| {
                    if style::primary_button(ui, col, "Sign in with Google…").on_hover_text("Add a Gmail or Google Workspace account").clicked() {
                        act = Some(PrefsAction::GoogleSignIn(None));
                    }
                    if ui.button("Change client…").clicked() {
                        self.google_edit = true;
                    }
                });
                return;
            }
            ui.label(
                RichText::new(
                    "Google needs each app to have its own sign-in client. Create one (free, about 10 minutes) and paste its details here. \
                     They're kept in the keychain.",
                )
                .small()
                .color(col.subtle),
            );
            egui::CollapsingHeader::new("How to create a Google client").default_open(!self.google_ready).show(ui, |ui| {
                for (i, step) in [
                    "Open console.cloud.google.com and create a project (e.g. \"OwlPost\").",
                    "Go to Google Auth Platform > Branding and fill in the app name and your email.",
                    "Audience: choose Internal for a Google Workspace domain (no review, no warnings). For @gmail.com choose External, add yourself as a test user, then Publish app so sign-in doesn't expire after 7 days (Google will show an \"unverified app\" warning — click Advanced > Continue).",
                    "Data Access > Add or remove scopes > add https://mail.google.com/ by hand.",
                    "Clients > Create client > Application type: Desktop app.",
                    "Copy the Client ID and Client secret below.",
                ]
                .iter()
                .enumerate()
                {
                    ui.label(RichText::new(format!("{}. {step}", i + 1)).small());
                }
                ui.hyperlink_to("Open Google Cloud Console ↗", "https://console.cloud.google.com/auth/clients");
            });
            egui::Grid::new("google_client").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Client ID");
                ui.add(egui::TextEdit::singleline(&mut self.google_id).hint_text("….apps.googleusercontent.com").desired_width(360.0));
                ui.end_row();
                ui.label("Client secret");
                ui.add(egui::TextEdit::singleline(&mut self.google_secret).password(true).hint_text("GOCSPX-…").desired_width(360.0));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                let ok = !self.google_id.trim().is_empty() && !self.google_secret.trim().is_empty();
                if ui.add_enabled(ok, egui::Button::new("Save")).clicked() {
                    act = Some(PrefsAction::SaveGoogleClient(self.google_id.trim().to_string(), self.google_secret.trim().to_string()));
                    self.google_edit = false;
                    self.google_secret.clear();
                }
                if self.google_ready {
                    if ui.button("Cancel").clicked() {
                        self.google_edit = false;
                    }
                    if ui.button("Remove client").on_hover_text("Accounts using Google sign-in will stop connecting").clicked() {
                        act = Some(PrefsAction::ClearGoogleClient);
                        self.google_edit = false;
                    }
                }
            });
        });
        act
    }
}

/// "● New mail: instant (push)" and friends, under each account.
pub(crate) fn push_line(ui: &mut egui::Ui, col: &Colors, st: Option<&PushState>) {
    let (dot, text, tip) = match st {
        Some(PushState::Live) => (col.success, "New mail: instant (push)".to_string(), "Connected with IMAP IDLE: the server tells OwlPost about new mail as it arrives.".to_string()),
        Some(PushState::Retrying(e)) => (col.warning, "New mail: reconnecting push…".to_string(), format!("{e}\n\nMail is still checked every 5 minutes meanwhile.")),
        Some(PushState::Unavailable(why)) => (col.very_subtle, "New mail: every 5 minutes".to_string(), format!("Push isn't available: {why}. OwlPost checks for new mail every 5 minutes instead.")),
        Some(PushState::Connecting) | None => (col.very_subtle, "New mail: connecting push…".to_string(), "Setting up push (IMAP IDLE).".to_string()),
    };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(r.center(), 3.5, dot);
        ui.label(RichText::new(text).small().color(col.subtle));
    })
    .response
    .on_hover_text(tip);
}
