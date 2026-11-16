// Filing one conversation, as the desktop's right-click menu does: move it
// to a folder, add or take off labels, put it in an Inbox tab, and choose
// where its sender's mail always goes. Folders can be searched (a Gmail
// account has many labels), and new folders, labels and tabs made here.

import SwiftUI

struct MoveSheet: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let row: ThreadRow
    /// Called after a move, which takes the conversation out of the list.
    var moved: () -> Void = {}
    @State private var filing: Filing?
    @State private var search = ""
    @State private var naming = false
    @State private var newName = ""

    var body: some View {
        NavigationStack {
            Group {
                if let f = filing {
                    list(f)
                } else {
                    ProgressView()
                }
            }
            .navigationTitle("Move")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .task { await refresh() }
    }

    private func refresh() async {
        filing = await model.filing(row)
    }

    private func list(_ f: Filing) -> some View {
        List {
            if search.isEmpty {
                Section {
                    NavigationLink {
                        LabelPicker(row: row, filing: f) { await refresh() }
                    } label: {
                        LabeledContent {
                            Text(labelSummary(f))
                        } label: {
                            Label("Labels", systemImage: "tag")
                        }
                    }
                }
                Section("Inbox tab") {
                    NavigationLink {
                        TabPicker(title: "This Conversation", footer: "Beats the sender’s tab. Replies stay with it.", tabs: f.tabs, current: f.byHand ? f.tab : nil) { c in
                            await model.putInTab(row, c)
                            await refresh()
                        }
                    } label: {
                        LabeledContent("This conversation", value: tabName(f.tab, in: f) + (f.byHand ? "" : " (automatic)"))
                    }
                    if let sender = f.sender {
                        NavigationLink {
                            TabPicker(title: "Mail from \(sender)", footer: "All their mail, including what’s already here.", tabs: f.tabs, current: f.senderTab) { c in
                                await model.sortSender(sender, c)
                                await refresh()
                            }
                        } label: {
                            LabeledContent {
                                Text(f.senderTab.map { tabName($0, in: f) } ?? "Automatic")
                            } label: {
                                Text("Mail from \(sender)").lineLimit(1).truncationMode(.middle)
                            }
                        }
                    }
                }
            }
            Section("Move to") {
                // First, not last: the last row sits behind the search field.
                Button { naming = true } label: { Label("New Folder…", systemImage: "folder.badge.plus") }
                ForEach(moveTargets(f), id: \.id) { folder in
                    Button {
                        Task { await moveTo(folder) }
                    } label: {
                        FolderLabel(folder: folder, indent: search.isEmpty)
                    }
                    .disabled(folder.id == model.folder?.id)
                }
            }
        }
        .searchable(text: $search, placement: .navigationBarDrawer(displayMode: .always), prompt: "Find a folder")
        .alert("New Folder", isPresented: $naming) {
            TextField("Name", text: $newName)
            Button("Cancel", role: .cancel) { newName = "" }
            Button("Make and Move") {
                let name = newName
                newName = ""
                Task {
                    if let folder = await model.createFolder(row.accountId, name) { await moveTo(folder) }
                }
            }
        } message: {
            Text("On Gmail this makes a label.")
        }
    }

    private func moveTo(_ folder: FolderInfo) async {
        dismiss()
        moved()
        await model.move(row, to: folder)
    }

    /// Where a conversation can go: not Drafts, Sent, Starred or All Mail,
    /// which a server fills itself, nor a standard folder it's already in
    /// (the Inbox, from the Inbox). Labels it has stay: on Gmail, moving to
    /// one still takes it out of the Inbox.
    private func moveTargets(_ f: Filing) -> [FolderInfo] {
        f.folders.filter {
            ![.drafts, .sent, .starred, .allMail].contains($0.kind)
                && ($0.kind == .other || !f.inFolders.contains($0.id))
                && matches($0)
        }
    }

    private func matches(_ folder: FolderInfo) -> Bool {
        search.isEmpty || folder.path.localizedCaseInsensitiveContains(search)
    }

    private func labelSummary(_ f: Filing) -> String {
        let on = f.folders.filter { $0.kind == .other && f.inFolders.contains($0.id) }
        return switch on.count {
        case 0: "None"
        case 1: on[0].name
        default: "\(on.count)"
        }
    }

    private func tabName(_ c: UInt8, in f: Filing) -> String {
        f.tabs.first { $0.category == c }?.name ?? "Primary"
    }
}

/// A folder's icon and name, indented under its parent.
struct FolderLabel: View {
    let folder: FolderInfo
    var indent = true

    var body: some View {
        Label {
            Text(indent ? folder.name : folder.path).lineLimit(1)
        } icon: {
            Image(systemName: folder.kind.icon)
        }
        .padding(.leading, indent ? CGFloat(folder.depth) * 16 : 0)
    }
}

/// Labels on and off, each a tap; a Gmail account's labels, or copies in
/// other servers' folders.
private struct LabelPicker: View {
    @Environment(MailModel.self) private var model
    let row: ThreadRow
    let filing: Filing
    let changed: () async -> Void
    @State private var on: Set<Int64>
    @State private var search = ""
    @State private var naming = false
    @State private var newName = ""

    init(row: ThreadRow, filing: Filing, changed: @escaping () async -> Void) {
        self.row = row
        self.filing = filing
        self.changed = changed
        _on = State(initialValue: Set(filing.inFolders))
    }

    var body: some View {
        List {
            Button { naming = true } label: { Label("New Label…", systemImage: "tag") }
            ForEach(filing.folders.filter { $0.kind == .other && (search.isEmpty || $0.path.localizedCaseInsensitiveContains(search)) }, id: \.id) { f in
                Button {
                    toggle(f)
                } label: {
                    HStack {
                        FolderLabel(folder: f, indent: search.isEmpty)
                        Spacer()
                        if on.contains(f.id) { Image(systemName: "checkmark").foregroundStyle(Color.accentColor) }
                    }
                }
                .accessibilityAddTraits(on.contains(f.id) ? .isSelected : [])
            }
        }
        .navigationTitle("Labels")
        .searchable(text: $search, placement: .navigationBarDrawer(displayMode: .always), prompt: "Find a label")
        .alert("New Label", isPresented: $naming) {
            TextField("Name", text: $newName)
            Button("Cancel", role: .cancel) { newName = "" }
            Button("Make and Add") {
                let name = newName
                newName = ""
                Task {
                    if let f = await model.createFolder(row.accountId, name) {
                        on.insert(f.id)
                        await model.setLabel(row, f, on: true)
                        await changed()
                    }
                }
            }
        }
    }

    private func toggle(_ f: FolderInfo) {
        let add = !on.contains(f.id)
        if add { on.insert(f.id) } else { on.remove(f.id) }
        Task {
            await model.setLabel(row, f, on: add)
            await changed()
        }
    }
}

/// Pick a tab, or Automatic (`nil`), for a conversation or a sender.
private struct TabPicker: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let title: String
    let footer: String
    let tabs: [InboxTab]
    let current: UInt8?
    let pick: (UInt8?) async -> Void
    @State private var naming = false
    @State private var newName = ""

    var body: some View {
        List {
            Section {
                choice("Automatic", nil)
            } footer: {
                Text("NightPost sorts from each email (Gmail’s own tabs on Gmail).")
            }
            Section {
                ForEach(tabs, id: \.name) { t in choice(t.name, t.category) }
                Button { naming = true } label: { Label("New Tab…", systemImage: "plus") }
            } footer: {
                Text(footer)
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .alert("New Tab", isPresented: $naming) {
            TextField("Name, like Invoices", text: $newName)
            Button("Cancel", role: .cancel) { newName = "" }
            Button("Make") {
                let name = newName
                newName = ""
                Task {
                    if let c = await model.newTab(name) { await choose(c) }
                }
            }
        }
    }

    private func choice(_ name: String, _ c: UInt8?) -> some View {
        Button {
            Task { await choose(c) }
        } label: {
            HStack {
                Text(name).foregroundStyle(.primary)
                Spacer()
                if c == current { Image(systemName: "checkmark").foregroundStyle(Color.accentColor) }
            }
        }
        .accessibilityAddTraits(c == current ? .isSelected : [])
    }

    private func choose(_ c: UInt8?) async {
        dismiss()
        if c != current { await pick(c) }
    }
}
