// Folders: each account's own folders and Gmail labels, as on the desktop
// sidebar. Standard folders first (Inbox, Sent, Trash …), then the
// account's own as a tree; a folder with subfolders folds them away, and a
// folded one counts the unread mail inside it too. Tapping a folder shows
// its conversations in the main list.

import SwiftUI

struct FoldersView: View {
    @Environment(MailModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var folders: [Int64: [FolderInfo]] = [:]
    /// Parents showing their subfolders. Per phone, kept while the app runs.
    @State private var expanded: Set<Int64> = []

    var body: some View {
        NavigationStack {
            List {
                ForEach(model.accounts, id: \.id) { a in
                    Section {
                        ForEach(visible(folders[a.id] ?? []), id: \.id) { f in
                            row(f, in: folders[a.id] ?? [])
                        }
                    } header: {
                        HStack(spacing: 6) {
                            Circle().fill(Color(hex: a.color)).frame(width: 8, height: 8)
                            Text(a.email)
                        }
                    }
                }
            }
            .navigationTitle("Folders")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .task {
                for a in model.accounts {
                    folders[a.id] = await model.folders(a.id)
                }
            }
        }
    }

    private func row(_ f: FolderInfo, in all: [FolderInfo]) -> some View {
        let parent = all.contains { $0.parentId == f.id }
        let open = expanded.contains(f.id)
        // A folded parent counts the unread mail in its subfolders too.
        let unread = parent && !open ? f.unread + descendants(of: f.id, in: all).reduce(0) { $0 + $1.unread } : f.unread
        return HStack(spacing: 8) {
            Button {
                dismiss()
                Task { await model.open(f) }
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: f.kind.icon)
                        .foregroundStyle(.tint)
                        .frame(width: 24)
                    Text(f.name)
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                    Spacer()
                    if unread > 0 {
                        Text("\(unread)").foregroundStyle(.secondary).monospacedDigit()
                    }
                    if model.folder?.id == f.id {
                        Image(systemName: "checkmark").foregroundStyle(.tint)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if parent {
                Button {
                    withAnimation {
                        if open { expanded.remove(f.id) } else { expanded.insert(f.id) }
                    }
                } label: {
                    Image(systemName: open ? "chevron.down" : "chevron.right")
                        .foregroundStyle(.secondary)
                        .frame(width: 28, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(open ? "Hide subfolders of \(f.name)" : "Show subfolders of \(f.name)")
            }
        }
        .padding(.leading, CGFloat(f.depth) * 18)
    }

    /// The folders not folded away under a closed parent.
    private func visible(_ all: [FolderInfo]) -> [FolderInfo] {
        let byId = Dictionary(all.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        return all.filter { f in
            var p = f.parentId
            while let id = p {
                if !expanded.contains(id) { return false }
                p = byId[id]?.parentId
            }
            return true
        }
    }

    private func descendants(of id: Int64, in all: [FolderInfo]) -> [FolderInfo] {
        let children = all.filter { $0.parentId == id }
        return children + children.flatMap { descendants(of: $0.id, in: all) }
    }
}

extension FolderKind {
    var icon: String {
        switch self {
        case .inbox: "tray"
        case .starred: "star"
        case .drafts: "doc"
        case .sent: "paperplane"
        case .archive, .allMail: "archivebox"
        case .spam: "xmark.bin"
        case .trash: "trash"
        case .other: "folder"
        }
    }
}
