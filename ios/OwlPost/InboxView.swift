// The message list: every account together (or one account's folder,
// picked in Folders), with swipes as on the desktop (right to archive,
// left to snooze), pull to refresh and search.

import SwiftUI

struct InboxView: View {
    @Environment(MailModel.self) private var model
    @State private var snoozing: ThreadRow?
    @State private var settingsOpen = false
    @State private var foldersOpen = false
    @State private var debugAdd = false

    var body: some View {
        @Bindable var model = model
        List {
            ForEach(model.rows, id: \.self) { t in
                Group {
                    if isDrafts {
                        // A draft opens in the composer, to carry on writing.
                        Button { Task { await model.openDraft(t) } } label: {
                            ThreadRowView(row: t, account: model.account(t.accountId), showAccount: model.accounts.count > 1)
                        }
                        .buttonStyle(.plain)
                    } else {
                        NavigationLink(value: t) {
                            ThreadRowView(row: t, account: model.account(t.accountId), showAccount: model.accounts.count > 1)
                        }
                    }
                }
                .swipeActions(edge: .leading, allowsFullSwipe: true) {
                    if model.mailbox == .snoozed {
                        Button { Task { await model.unsnooze(t) } } label: { Label("Unsnooze", systemImage: "tray.and.arrow.down") }
                            .tint(.blue)
                    } else {
                        Button { Task { await model.archive(t) } } label: { Label("Archive", systemImage: "archivebox") }
                            .tint(.green)
                    }
                    Button { Task { await model.setRead(t, t.unread) } } label: {
                        Label(t.unread ? "Read" : "Unread", systemImage: t.unread ? "envelope.open" : "envelope.badge")
                    }
                    .tint(.blue)
                }
                .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                    Button { snoozing = t } label: { Label("Snooze", systemImage: "clock") }
                        .tint(.orange)
                    Button(role: .destructive) { Task { await model.trash(t) } } label: { Label("Trash", systemImage: "trash") }
                    Button { Task { await model.setPinned(t, !t.pinned) } } label: { Label(t.pinned ? "Unpin" : "Pin", systemImage: t.pinned ? "pin.slash" : "pin") }
                        .tint(.yellow)
                }
                .listRowBackground(t.pinned ? Color.yellow.opacity(0.10) : nil)
            }
        }
        .listStyle(.plain)
        .overlay {
            if model.rows.isEmpty && model.started {
                EmptyListView(mailbox: model.mailbox, inFolder: model.folder != nil, searching: !model.search.isEmpty)
            }
        }
        .navigationTitle(model.title)
        .navigationDestination(for: ThreadRow.self) { t in
            ThreadView(row: t)
        }
        .searchable(text: $model.search, prompt: model.folder == nil ? "Search all mail" : "Search \(model.title)")
        .onSubmit(of: .search) { Task { await model.reload() } }
        .onChange(of: model.search) { _, new in
            if new.isEmpty { Task { await model.reload() } }
        }
        .refreshable { await model.sync() }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Menu {
                    // Picking a list leaves any folder; the selection shows
                    // only while no folder is open.
                    Picker("Mailbox", selection: Binding(get: { model.folder == nil ? model.mailbox : nil }, set: { m in
                        guard let m else { return }
                        model.mailbox = m
                        if model.folder != nil { Task { await model.open(nil) } }
                    })) {
                        ForEach(Mailbox.shown, id: \.self) { m in
                            Label(m.title, systemImage: m.icon).tag(Optional(m))
                        }
                    }
                    Divider()
                    Button { foldersOpen = true } label: { Label("Folders", systemImage: "folder") }
                    Button { settingsOpen = true } label: { Label("Settings", systemImage: "gearshape") }
                } label: {
                    Image(systemName: "line.3.horizontal")
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                HStack {
                    if model.syncing {
                        ProgressView()
                        if let s = model.syncStatus, s.total > 1 {
                            // Folder 84 of 110: a long first sync shows it's moving.
                            Text("\(min(s.done + 1, s.total))/\(s.total)")
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                                .accessibilityLabel("Syncing folder \(min(s.done + 1, s.total)) of \(s.total)")
                        }
                    }
                    Button { model.newMessage() } label: { Image(systemName: "square.and.pencil") }
                        .accessibilityLabel("New Message")
                }
            }
        }
        .onChange(of: model.mailbox) { Task { await model.reload() } }
        .sheet(item: $snoozing) { t in
            SnoozeSheet { date in
                snoozing = nil
                Task { await model.snooze(t, until: date) }
            }
            .presentationDetents([.medium])
        }
        .sheet(isPresented: $settingsOpen) {
            SettingsView()
        }
        .sheet(isPresented: $foldersOpen) {
            FoldersView()
        }
        .navigationDestination(item: $debugOpen) { ThreadView(row: $0) }
        .sheet(isPresented: $debugAdd) { AddAccountView() }
        .onChange(of: model.rows.count) { debugShow() }
        .task { debugShow() }
    }

    /// Drafts (unified, or an account's Drafts folder) open in the composer.
    private var isDrafts: Bool {
        model.folder.map { $0.kind == .drafts } ?? (model.mailbox == .drafts)
    }

    @State private var debugOpen: ThreadRow?
    @State private var debugShown = false

    /// Debug builds only: `-show thread` opens the longest conversation and
    /// `-show snooze` the snooze sheet (`-show settings`, Settings; `-show add`, Add Account; `-show folders`, Folders; `-show compose`, a reply; `-show new`, a new message; `-show html`, an HTML email) once the list loads, for screenshots
    /// from the simulator (where nothing can tap).
    private func debugShow() {
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        guard !debugShown, let i = args.firstIndex(of: "-show"), i + 1 < args.count,
              let t = model.rows.max(by: { $0.count < $1.count }) else { return }
        debugShown = true
        switch args[i + 1] {
        case "thread": debugOpen = t
        case "snooze": snoozing = t
        case "settings": settingsOpen = true
        case "add": debugAdd = true
        case "folders": foldersOpen = true
        case "compose": Task { await model.startCompose(model.messages(t).last?.id ?? 0, .reply) }
        case "new": model.newMessage()
        case "html":
            Task {
                for r in model.rows where await model.messages(r).contains(where: { $0.hasHtml }) {
                    debugOpen = r
                    return
                }
            }
        default: break
        }
        #endif
    }
}

/// One conversation in the list: picture, people and date, subject with
/// badges, and a two-line preview, as on the desktop.
struct ThreadRowView: View {
    let row: ThreadRow
    let account: AccountInfo?
    let showAccount: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Avatar(name: row.people, email: row.latestEmail)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    if row.unread {
                        Circle().fill(Color.accentColor).frame(width: 8, height: 8)
                    }
                    Text(row.people)
                        .font(.subheadline.weight(row.unread ? .bold : .semibold))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if let until = row.snoozedUntil {
                        Label(shortDate(until), systemImage: "clock")
                            .labelStyle(.titleAndIcon)
                            .font(.caption)
                            .foregroundStyle(.orange)
                    } else {
                        Text(shortDate(row.date))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    if showAccount, let account {
                        RoundedRectangle(cornerRadius: 1.5)
                            .fill(Color(hex: account.color))
                            .frame(width: 3, height: 13)
                    }
                }
                HStack(spacing: 4) {
                    Text(row.subject.isEmpty ? "(no subject)" : row.subject)
                        .font(.subheadline.weight(row.unread ? .semibold : .regular))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if row.pinned { Image(systemName: "pin.fill").foregroundStyle(.yellow) }
                    if row.hasAttachments { Image(systemName: "paperclip").foregroundStyle(.secondary) }
                    if row.starred { Image(systemName: "star.fill").foregroundStyle(.yellow) }
                    if row.count > 1 {
                        Text("\(row.count)")
                            .font(.caption2.weight(.medium))
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(.quaternary, in: RoundedRectangle(cornerRadius: 3))
                    }
                }
                .font(.caption)
                Text(row.snippet)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
        }
        .padding(.vertical, 4)
    }
}

struct EmptyListView: View {
    let mailbox: Mailbox
    var inFolder = false
    let searching: Bool

    var body: some View {
        if searching {
            ContentUnavailableView.search
        } else if inFolder {
            ContentUnavailableView("No conversations here", systemImage: "folder")
        } else {
            switch mailbox {
            case .inbox:
                ContentUnavailableView("Inbox zero", systemImage: "leaf", description: Text("Nice work!"))
            case .snoozed:
                ContentUnavailableView("Nothing snoozed", systemImage: "clock", description: Text("Swipe left on a conversation to snooze it."))
            default:
                ContentUnavailableView("No conversations here", systemImage: "tray")
            }
        }
    }
}

extension Mailbox {
    /// The lists in the mailbox menu, in order.
    static let shown: [Mailbox] = [.inbox, .snoozed, .starred, .drafts, .sent, .archive, .trash]

    var title: String {
        switch self {
        case .inbox: "Inbox"
        case .starred: "Starred"
        case .drafts: "Drafts"
        case .sent: "Sent"
        case .archive: "Archive"
        case .trash: "Trash"
        case .snoozed: "Snoozed"
        }
    }

    var icon: String {
        switch self {
        case .inbox: "tray"
        case .starred: "star"
        case .drafts: "doc"
        case .sent: "paperplane"
        case .archive: "archivebox"
        case .trash: "trash"
        case .snoozed: "clock"
        }
    }
}

extension ThreadRow: Identifiable {
    public var id: String { "\(accountId)/\(threadKey)" }
}
