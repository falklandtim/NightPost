//! Preferences: themes (incl. Mailspring theme import), accounts, MCP, shortcuts.

use crate::style::{self, Colors};
use eframe::egui::{self, RichText};
use springpost_core::theme::{Theme, ThemeSource};
use crate::composer::SignatureBook;
use springpost_core::{Account, AccountId, Signature};

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
}

impl Default for ListSettings {
    fn default() -> Self {
        Self { swipe_left: SwipeAction::Trash, swipe_right: SwipeAction::Archive, quick_actions: true }
    }
}

impl ListSettings {
    pub fn encode(&self) -> String {
        format!("{},{},{}", self.swipe_left.key(), self.swipe_right.key(), self.quick_actions as u8)
    }
    pub fn decode(s: &str) -> Self {
        let mut d = Self::default();
        let parts: Vec<&str> = s.split(',').collect();
        if let [l, r, q] = parts.as_slice() {
            d.swipe_left = SwipeAction::parse(l).unwrap_or(d.swipe_left);
            d.swipe_right = SwipeAction::parse(r).unwrap_or(d.swipe_right);
            d.quick_actions = *q != "0";
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
    pub install_path: String,
    pub editing: Option<Account>,
    pub message: Option<(bool, String)>,
    /// The signature open in the editor (an edited copy).
    pub sig_edit: Option<Signature>,
}

pub enum PrefsAction {
    None,
    SetTheme(String),
    InstallTheme(String),
    ReloadThemes,
    AddAccount,
    RemoveAccount(AccountId),
    SaveAccount(Account),
    AddDemo,
    RemoveDemo,
    SaveSignature(Signature),
    DeleteSignature(i64),
    SetDefaultSignature(AccountId, Option<i64>),
    Close,
}

impl Prefs {
    pub fn new(tab: Tab) -> Self {
        Self { tab, install_path: String::new(), editing: None, message: None, sig_edit: None }
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
        ui.label(RichText::new("Theme").strong().size(16.0));
        ui.label(
            RichText::new("SpringPost reads Mailspring theme packages: their ui-variables.less colours are applied to the native interface.")
                .color(col.subtle),
        );
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            for t in themes {
                if theme_tile(ui, col, t, t.id == current) {
                    act = PrefsAction::SetTheme(t.id.clone());
                }
            }
        });
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new("Install a Mailspring theme").strong());
        ui.label(
            RichText::new(
                "Point at an unpacked theme folder (the one containing package.json), e.g. a clone of a theme from GitHub. \
                 Themes already installed in Mailspring are picked up automatically.",
            )
            .small()
            .color(col.subtle),
        );
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.install_path).hint_text("/path/to/mailspring-theme").desired_width(380.0));
            if ui.button("Install").clicked() && !self.install_path.trim().is_empty() {
                act = PrefsAction::InstallTheme(self.install_path.trim().to_string());
            }
            if ui.button("⟳ Rescan").clicked() {
                act = PrefsAction::ReloadThemes;
            }
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "Theme folders: {}  ·  {}",
                springpost_core::theme::user_themes_dir().display(),
                springpost_core::theme::mailspring_packages_dir().map(|p| p.display().to_string()).unwrap_or_default()
            ))
            .small()
            .color(col.very_subtle),
        );
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
                                    springpost_core::smtp::text_to_html(e.body.trim_end())
                                } else {
                                    springpost_core::parse::signature_text(&e.body)
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
                            RichText::new("This looks like a whole page copied from another app. On Save, SpringPost keeps just the signature and removes page styles (such as Canary's dark-mode colour inversion), which would otherwise affect the whole email.")
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
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Remove").clicked() {
                            act = PrefsAction::RemoveAccount(a.id);
                        }
                        if ui.button("Edit").clicked() {
                            self.editing = Some(a.clone());
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
    painter.rect_stroke(preview, 6.0, stroke, egui::StrokeKind::Outside);
    style::line(&painter, egui::pos2(rect.min.x, preview.max.y + 6.0), &t.display_name, egui::FontId::proportional(13.5), col.text, size.x);
    let src = match t.source {
        ThemeSource::BuiltIn => "Built-in",
        ThemeSource::User => "Installed",
        ThemeSource::Mailspring => "From Mailspring",
    };
    style::line(&painter, egui::pos2(rect.min.x, preview.max.y + 24.0), src, egui::FontId::proportional(11.0), col.subtle, size.x);
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    let r = if t.description.is_empty() { r } else { r.on_hover_text(&t.description) };
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
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "springpost".into());
    ui.label(RichText::new("Built-in MCP server").strong().size(16.0));
    ui.label(
        RichText::new(
            "SpringPost includes a Model Context Protocol server, so assistants like Claude can search, read, triage, \
             snooze and send your mail using the accounts configured here. It runs as `springpost mcp` over stdio and \
             shares this app's local database.",
        )
        .color(col.subtle),
    );
    ui.add_space(10.0);
    ui.label(RichText::new("Claude Code").strong());
    copy_block(ui, col, &format!("claude mcp add springpost -- \"{exe}\" mcp"));
    ui.add_space(8.0);
    ui.label(RichText::new("Claude Desktop (claude_desktop_config.json) or any MCP client").strong());
    copy_block(
        ui,
        col,
        &format!("{{\n  \"mcpServers\": {{\n    \"springpost\": {{\n      \"command\": \"{}\",\n      \"args\": [\"mcp\"]\n    }}\n  }}\n}}", exe.replace('\\', "\\\\")),
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
}
