//! The sidebar: All Accounts, then each account's folders as a tree.

use super::*;

impl NightPostApp {
    pub(super) fn sidebar(&mut self, ctx: &egui::Context) {
        if !self.show_sidebar {
            return;
        }
        let col = self.colors.clone();
        let panel = egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(244.0)
            .width_range(180.0..=320.0)
            .frame(egui::Frame::new().fill(col.sidebar).inner_margin(egui::Margin { left: 12, right: 12, top: 18, bottom: 10 }))
            .show(ctx, |ui| {
                // Footer: preferences and sync status.
                egui::TopBottomPanel::bottom("sidebar_footer")
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(col.sidebar))
                    .show_inside(ui, |ui| {
                        ui.horizontal(|ui| {
                            if icons::bare(ui, Icon::Gear, "Preferences (⌘,)", col.sidebar_text, col.sidebar).clicked() {
                                self.prefs = Some(Prefs::new(Tab::Appearance));
                            }
                            // Sync progress while syncing; otherwise how the accounts are doing.
                            let line = if !self.status.is_empty() {
                                Some((self.status.clone(), col.sidebar_text.gamma_multiply(0.8)))
                            } else {
                                self.health_line().map(|(t, h)| {
                                    let c = match h {
                                        activity::Health::Offline => col.error,
                                        activity::Health::Trouble => col.warning,
                                        _ => col.sidebar_text.gamma_multiply(0.65),
                                    };
                                    (t, c)
                                })
                            };
                            if let Some((text, c)) = line {
                                let r = ui.add(egui::Label::new(RichText::new(text).small().color(c)).truncate().sense(Sense::click()));
                                if r.on_hover_text(format!("Sync activity ({ACTIVITY_KEY})")).clicked() {
                                    self.toggle_activity(None);
                                }
                            }
                        });
                    });
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let multi = self.accounts.len() > 1;
                    if multi {
                        section(ui, &col, "ALL ACCOUNTS");
                    }
                    for role in FolderRole::UNIFIED {
                        let label = if role == FolderRole::Inbox && multi { "Unified Inbox" } else { role.label() };
                        let count = match role {
                            FolderRole::Inbox | FolderRole::Spam => self.unified_unread.get(&role).copied().unwrap_or(0),
                            // Drafts show how many there are, as Mailspring does.
                            FolderRole::Drafts => self.unified_drafts,
                            _ => 0,
                        };
                        let sel = self.selection == Selection::Unified(role) && self.search.is_empty();
                        // Dragged mail can land on Inbox, Archive, Spam and Trash.
                        let droppable = self.dragging.is_some() && matches!(role, FolderRole::Inbox | FolderRole::Archive | FolderRole::Spam | FolderRole::Trash);
                        let over = droppable && ui.rect_contains_pointer(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 31.0)));
                        if over {
                            self.drop_target = Some(DropTarget::Role(role));
                        }
                        if nav_item_r(ui, &col, role_icon(role), label, count, sel, over).clicked() {
                            self.set_selection(Selection::Unified(role));
                        }
                        if role == FolderRole::Starred {
                            let sel = self.selection == Selection::Snoozed && self.search.is_empty();
                            if nav_item(ui, &col, Icon::Snooze, "Snoozed", self.snoozed_count, sel) {
                                self.set_selection(Selection::Snoozed);
                            }
                        }
                    }
                    ui.add_space(22.0);
                    let accounts = self.accounts.clone();
                    for a in &accounts {
                        let collapsed = self.collapsed_accounts.contains(&a.id);
                        let header = ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add_space(4.0);
                            let (chev, chev_r) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), Sense::click());
                            let c = chev.center();
                            let sub = col.sidebar_text.gamma_multiply(0.75);
                            let pts = if collapsed { vec![c + egui::vec2(-1.5, -3.5), c + egui::vec2(2.0, 0.0), c + egui::vec2(-1.5, 3.5)] } else { vec![c + egui::vec2(-3.5, -1.5), c + egui::vec2(0.0, 2.0), c + egui::vec2(3.5, -1.5)] };
                            ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.4_f32, if chev_r.hovered() { col.sidebar_text } else { sub })));
                            chev_r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} {}", if collapsed { "Expand" } else { "Collapse" }, a.email)));
                            let chev_r = chev_r.on_hover_text(if collapsed { "Show folders" } else { "Hide folders" });
                            if multi {
                                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 4.0, style::hex(&a.color));
                            }
                            let mut job = egui::text::LayoutJob::default();
                            job.append(&a.email.to_uppercase(), 0.0, egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: sub, extra_letter_spacing: 1.1, ..Default::default() });
                            let room = ui.available_width() - if a.demo { 0.0 } else { 44.0 };
                            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                            let label = ui.scope(|ui| {
                                ui.set_max_width(room);
                                ui.add(egui::Label::new(job).truncate().sense(Sense::click()))
                            })
                            .inner;
                            // Is it connecting? Green, amber (a blip), red (can't), grey (not yet).
                            // (Next to the name: the panel's right edge belongs to the scroll bar.)
                            let dot = (!a.demo).then(|| {
                                let st = self.health_of(a.id);
                                let (rect, r) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::click());
                                let h = st.health();
                                paint_health(ui.painter(), rect.center(), h, self.health_color(h), r.hovered());
                                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} status: {}", a.email, h.as_str())));
                                r.on_hover_text(format!("{}\n\nClick for Sync activity", st.summary(Utc::now())))
                            });
                            (label, dot, chev_r.clicked())
                        });
                        let (label, dot, chev_clicked) = header.inner;
                        if dot.is_some_and(|d| d.clicked()) {
                            self.activity = Some(ActivityView::new(Some(a.id)));
                        }
                        ui.add_space(6.0);
                        if !a.demo {
                            label.context_menu(|ui| {
                                if ui.button("⟳ Sync now").clicked() {
                                    self.syncing = true;
                                    self.worker.send(Cmd::SyncAccount(a.id));
                                    ui.close();
                                }
                                if ui.button("Sync activity").clicked() {
                                    self.activity = Some(ActivityView::new(Some(a.id)));
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button("New folder…").clicked() {
                                    self.folder_dialog = Some(FolderDialog::New { account: a.id, parent: None, name: String::new(), error: None });
                                    ui.close();
                                }
                            });
                        }
                        if label.clicked() || chev_clicked {
                            if collapsed {
                                self.collapsed_accounts.retain(|x| *x != a.id);
                            } else {
                                self.collapsed_accounts.push(a.id);
                            }
                        }
                        if collapsed {
                            ui.add_space(4.0);
                            continue;
                        }
                        let folders = self.folders.get(&a.id).cloned().unwrap_or_default();
                        // With one account the list above already is that
                        // account's Inbox, Sent and so on.
                        for f in folders.iter().filter(|f| f.role != FolderRole::Other && (multi || !FolderRole::UNIFIED.contains(&f.role))) {
                            let count = if matches!(f.role, FolderRole::Inbox | FolderRole::Spam | FolderRole::Other) { f.unread } else { 0 };
                            let sel = self.selection == Selection::Folder(f.id) && self.search.is_empty();
                            let label = if f.role == FolderRole::All { "All Mail".to_string() } else { f.role.label().to_string() };
                            let droppable = self.dragging.is_some() && matches!(f.role, FolderRole::Inbox | FolderRole::Archive | FolderRole::Spam | FolderRole::Trash | FolderRole::All);
                            let over = droppable && ui.rect_contains_pointer(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 31.0)));
                            if over {
                                self.drop_target = Some(DropTarget::Folder(f.clone()));
                            }
                            if nav_item_r(ui, &col, role_icon(f.role), &label, count, sel, over).clicked() {
                                self.set_selection(Selection::Folder(f.id));
                            }
                        }
                        // Custom folders as a tree: parents fold away their subfolders.
                        let others: Vec<&Folder> = folders.iter().filter(|f| f.role == FolderRole::Other && !snooze_sync::is_internal_folder(&f.path)).collect();
                        let tree = FolderTree::new(&others);
                        let mut toggle: Option<String> = None;
                        for (i, f) in others.iter().enumerate() {
                            if tree.ancestors[i].iter().any(|&p| !self.open_folders.contains(&folder_key(a.id, &others[p].path))) {
                                continue;
                            }
                            let key = folder_key(a.id, &f.path);
                            let parent = tree.has_children(i);
                            let expanded = self.open_folders.contains(&key);
                            // A folded parent counts the unread mail inside it too.
                            let count = if parent && !expanded { f.unread + tree.descendants(i).map(|d| others[d].unread).sum::<i64>() } else { f.unread };
                            let sel = self.selection == Selection::Folder(f.id) && self.search.is_empty();
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.add_space(tree.ancestors[i].len() as f32 * 14.0);
                                let (slot, r) = ui.allocate_exact_size(egui::vec2(16.0, 31.0), if parent { Sense::click() } else { Sense::hover() });
                                if parent {
                                    let c = slot.center() + egui::vec2(2.0, 0.0);
                                    let fg = if r.hovered() { col.sidebar_text } else { col.sidebar_text.gamma_multiply(0.6) };
                                    let pts = if expanded {
                                        vec![c + egui::vec2(-3.5, -1.5), c + egui::vec2(0.0, 2.0), c + egui::vec2(3.5, -1.5)]
                                    } else {
                                        vec![c + egui::vec2(-1.5, -3.5), c + egui::vec2(2.0, 0.0), c + egui::vec2(-1.5, 3.5)]
                                    };
                                    ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.4_f32, fg)));
                                    let r = r.on_hover_text(if expanded { "Hide subfolders" } else { "Show subfolders" });
                                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} {}", if expanded { "Collapse" } else { "Expand" }, f.name)));
                                    if r.clicked() {
                                        toggle = Some(key.clone());
                                    }
                                }
                                let over = self.dragging.is_some() && ui.rect_contains_pointer(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 31.0)));
                                if over {
                                    self.drop_target = Some(DropTarget::Folder((*f).clone()));
                                }
                                let r = nav_item_r(ui, &col, Icon::Folder, &f.name, count, sel, over);
                                if r.clicked() {
                                    self.set_selection(Selection::Folder(f.id));
                                }
                                r.context_menu(|ui| {
                                    if ui.button("New folder inside…").clicked() {
                                        self.folder_dialog = Some(FolderDialog::New { account: a.id, parent: Some((*f).clone()), name: String::new(), error: None });
                                        ui.close();
                                    }
                                    let why = engine::cant_delete_folder(&self.store, f).unwrap_or(None);
                                    let del = ui.add_enabled(why.is_none(), egui::Button::new("Delete folder…"));
                                    if let Some(why) = &why {
                                        del.clone().on_disabled_hover_text(why);
                                    }
                                    if del.clicked() {
                                        self.folder_dialog = Some(FolderDialog::Delete { folder: (*f).clone(), gmail: a.is_gmail() });
                                        ui.close();
                                    }
                                });
                            });
                        }
                        if let Some(k) = toggle {
                            if !self.open_folders.remove(&k) {
                                self.open_folders.insert(k);
                            }
                        }
                        ui.add_space(8.0);
                    }
                    ui.add_space(6.0);
                    if ui.add(egui::Button::new(RichText::new("+ Add account").color(col.sidebar_text)).frame(false)).clicked() {
                        self.setup = Some(AccountSetup::default());
                    }
                });
            });
        self.sidebar_right = panel.response.rect.right();
    }
}

pub(super) fn section(ui: &mut egui::Ui, col: &Colors, text: &str) {
    let mut job = egui::text::LayoutJob::default();
    let sub = col.sidebar_text.gamma_multiply(0.75);
    job.append(text, 4.0, egui::TextFormat { font_id: egui::FontId::proportional(11.0), color: sub, extra_letter_spacing: 1.1, ..Default::default() });
    ui.label(job);
    ui.add_space(4.0);
}

/// How a folder is named in menus: the role for system folders, else its path.
pub(super) fn folder_label(f: &Folder) -> String {
    match f.role {
        FolderRole::Other => f.path.clone(),
        FolderRole::All => "Archive (All Mail)".into(),
        r => r.label().into(),
    }
}

/// How a folder is remembered as open in the sidebar.
pub(super) fn folder_key(account: AccountId, path: &str) -> String {
    format!("{account}\t{path}")
}

/// Custom folders as a tree, by path. A folder's ancestors are the folders
/// above it that exist (a server may list `A/B/C` without `A/B`).
pub(super) struct FolderTree {
    /// For each folder, the indexes of its ancestors, nearest first.
    pub(super) ancestors: Vec<Vec<usize>>,
}

impl FolderTree {
    pub(super) fn new(folders: &[&Folder]) -> Self {
        let index: HashMap<&str, usize> = folders.iter().enumerate().map(|(i, f)| (f.path.as_str(), i)).collect();
        let ancestors = folders
            .iter()
            .map(|f| {
                let mut out = vec![];
                let mut p = nightpost_core::folder_parent(&f.path);
                while let Some(path) = p {
                    if let Some(&i) = index.get(path.as_str()) {
                        out.push(i);
                    }
                    p = nightpost_core::folder_parent(&path);
                }
                out
            })
            .collect();
        Self { ancestors }
    }

    pub(super) fn has_children(&self, i: usize) -> bool {
        self.ancestors.iter().any(|a| a.first() == Some(&i))
    }

    pub(super) fn descendants(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.ancestors.iter().enumerate().filter(move |(_, a)| a.contains(&i)).map(|(j, _)| j)
    }
}

/// The sidebar's line icon for a folder role.
pub(super) fn role_icon(role: FolderRole) -> Icon {
    match role {
        FolderRole::Inbox => Icon::Inbox,
        FolderRole::Starred => Icon::Star,
        FolderRole::Drafts => Icon::Pencil,
        FolderRole::Sent => Icon::Send,
        FolderRole::Archive | FolderRole::All => Icon::Archive,
        FolderRole::Spam => Icon::Warning,
        FolderRole::Trash => Icon::Trash,
        FolderRole::Other => Icon::Folder,
    }
}

/// A sidebar row: line icon, name, and a quiet count on the right.
pub(super) fn nav_item(ui: &mut egui::Ui, col: &Colors, icon: Icon, label: &str, count: i64, selected: bool) -> bool {
    nav_item_r(ui, col, icon, label, count, selected, false).clicked()
}

/// A sidebar entry. `drop` highlights it as where dragged mail will land.
pub(super) fn nav_item_r(ui: &mut egui::Ui, col: &Colors, icon: Icon, label: &str, count: i64, selected: bool, drop: bool) -> egui::Response {
    let h = 31.0;
    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::click());
    let p = ui.painter_at(rect);
    if drop {
        p.rect_filled(rect, 8.0, style::blend(col.accent, col.sidebar, 0.25));
        p.rect_stroke(rect.shrink(0.5), 8.0, egui::Stroke::new(1.5_f32, col.accent), egui::StrokeKind::Inside);
    } else if selected {
        p.rect_filled(rect, 8.0, col.sidebar_active);
    } else if r.hovered() {
        p.rect_filled(rect, 8.0, style::blend(col.sidebar_active, col.sidebar, 0.5));
    }
    let fg = if selected { col.sidebar_active_text } else { col.sidebar_text };
    icons::paint(&p, egui::pos2(rect.left() + 19.0, rect.center().y), 14.5, icon, fg);
    let mut count_w = 0.0;
    if count > 0 {
        let sub = if selected { fg } else { style::blend(col.sidebar_text, col.sidebar, 0.7) };
        let g = p.layout_no_wrap(count.to_string(), egui::FontId::proportional(12.0), sub);
        count_w = g.size().x + 8.0;
        let pos = egui::pos2(rect.right() - 12.0 - g.size().x, rect.center().y - g.size().y / 2.0);
        if selected {
            // Drawn twice for weight, like the label.
            p.galley(pos + egui::vec2(0.5, 0.0), g.clone(), sub);
        }
        p.galley(pos, g, sub);
    }
    let pos = egui::pos2(rect.left() + 38.0, rect.center().y - 8.5);
    let font = egui::FontId::proportional(14.0);
    let max = rect.width() - 50.0 - count_w;
    if selected {
        style::bold_line(&p, pos, label, font, fg, max);
    } else {
        style::line(&p, pos, label, font, fg, max);
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}
